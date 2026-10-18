//! ELF64 loader + launcher.
//!
//! 0.2.0 loads static x86_64 ET_EXEC binaries into a dedicated region of the kernel address
//! space and runs them in ring 0 on their own stack. Apps talk to the kernel through the same
//! `int 0x80` ABI as everything else; sys_exit unwinds back to the shell.
//! (Ring-3 isolation is planned for a later release.)
use crate::{memory, process};
use alloc::{format, string::String};
use core::arch::global_asm;
use core::sync::atomic::{AtomicBool, Ordering};

pub const APP_BASE: u64 = 0x0000_0100_0000_0000;
pub const APP_CODE_MAX: u64 = 16 * 1024 * 1024;
const STACK_SIZE: u64 = 64 * 1024;
pub const STACK_TOP: u64 = APP_BASE + APP_CODE_MAX + STACK_SIZE;

static RUNNING: AtomicBool = AtomicBool::new(false);

pub fn is_running() -> bool {
    RUNNING.load(Ordering::SeqCst)
}

pub struct Image {
    pub entry: u64,
    pub segments: usize,
    pub bytes: usize,
}

fn u16_at(d: &[u8], o: usize) -> u64 {
    u16::from_le_bytes([d[o], d[o + 1]]) as u64
}
fn u32_at(d: &[u8], o: usize) -> u64 {
    u32::from_le_bytes([d[o], d[o + 1], d[o + 2], d[o + 3]]) as u64
}
fn u64_at(d: &[u8], o: usize) -> u64 {
    let mut b = [0u8; 8];
    b.copy_from_slice(&d[o..o + 8]);
    u64::from_le_bytes(b)
}

struct Seg {
    vaddr: u64,
    memsz: u64,
    off: u64,
    filesz: u64,
}

/// Validate and load an ELF64 executable.
pub fn load(d: &[u8]) -> Result<Image, String> {
    if d.len() < 64 || &d[0..4] != b"\x7fELF" {
        return Err(String::from("not an ELF file"));
    }
    if d[4] != 2 || d[5] != 1 {
        return Err(String::from("only 64-bit little-endian ELF is supported"));
    }
    if u16_at(d, 16) != 2 {
        return Err(String::from("only static executables (ET_EXEC) are supported"));
    }
    if u16_at(d, 18) != 0x3E {
        return Err(String::from("not an x86_64 binary"));
    }
    let entry = u64_at(d, 24);
    let phoff = u64_at(d, 32) as usize;
    let phentsize = u16_at(d, 54) as usize;
    let phnum = u16_at(d, 56) as usize;
    if phentsize != 56 || phnum == 0 || phnum > 16 || phoff.checked_add(phnum * 56).map_or(true, |e| e > d.len()) {
        return Err(String::from("bad program header table"));
    }
    if entry < APP_BASE || entry >= APP_BASE + APP_CODE_MAX {
        return Err(format!(
            "entry point {:#x} is outside the app region ({:#x}..{:#x}); link with --image-base={:#x}",
            entry,
            APP_BASE,
            APP_BASE + APP_CODE_MAX,
            APP_BASE
        ));
    }

    let mut segs: alloc::vec::Vec<Seg> = alloc::vec::Vec::new();
    for i in 0..phnum {
        let o = phoff + i * 56;
        if u32_at(d, o) != 1 {
            continue; // not PT_LOAD
        }
        let s = Seg { off: u64_at(d, o + 8), vaddr: u64_at(d, o + 16), filesz: u64_at(d, o + 32), memsz: u64_at(d, o + 40) };
        let vend = s.vaddr.checked_add(s.memsz).ok_or("segment overflows")?;
        let fend = s.off.checked_add(s.filesz).ok_or("segment overflows")?;
        if s.memsz < s.filesz || s.vaddr < APP_BASE || vend > APP_BASE + APP_CODE_MAX || fend as usize > d.len() {
            return Err(format!("segment {} is outside the app region or the file", i));
        }
        segs.push(s);
    }
    if segs.is_empty() {
        return Err(String::from("no loadable segments"));
    }

    // Pass 1: map + zero whole pages (so .bss and stale data are clean).
    for s in &segs {
        let start = s.vaddr & !0xFFF;
        let end = (s.vaddr + s.memsz + 0xFFF) & !0xFFF;
        memory::map_region(start, (end - start) as usize)?;
        unsafe { core::ptr::write_bytes(start as *mut u8, 0, (end - start) as usize) };
    }
    memory::map_region(STACK_TOP - STACK_SIZE, STACK_SIZE as usize)?;

    // Pass 2: copy file contents.
    let mut bytes = 0;
    for s in &segs {
        unsafe {
            core::ptr::copy_nonoverlapping(d.as_ptr().add(s.off as usize), s.vaddr as *mut u8, s.filesz as usize);
        }
        bytes += s.memsz as usize;
    }
    Ok(Image { entry, segments: segs.len(), bytes })
}

// exec_enter(entry, stack_top) -> exit code
// exec_leave(code)             -> never returns; resumes after exec_enter's caller
global_asm!(
    r#"
    .global exec_enter
exec_enter:
    push rbx
    push rbp
    push r12
    push r13
    push r14
    push r15
    mov [rip + EXEC_SP], rsp
    mov rsp, rsi
    xor ebp, ebp
    call rdi
    xor edi, edi
    jmp exec_leave

    .global exec_leave
exec_leave:
    mov rsp, [rip + EXEC_SP]
    pop r15
    pop r14
    pop r13
    pop r12
    pop rbp
    pop rbx
    mov eax, edi
    sti
    ret
"#
);

#[no_mangle]
static mut EXEC_SP: u64 = 0;

extern "C" {
    fn exec_enter(entry: u64, stack_top: u64) -> i32;
    pub fn exec_leave(code: i32) -> !;
}

/// Load `data` as an ELF app named `name`, run it to completion, return its exit code.
pub fn exec(name: &str, data: &[u8]) -> Result<i32, String> {
    if RUNNING.swap(true, Ordering::SeqCst) {
        return Err(String::from("an application is already running"));
    }
    let img = match load(data) {
        Ok(i) => i,
        Err(e) => {
            RUNNING.store(false, Ordering::SeqCst);
            return Err(e);
        }
    };
    serial_println!("elf: {} loaded ({} segment(s), {} bytes), entry {:#x}", name, img.segments, img.bytes, img.entry);
    let pid = process::begin_external(name);
    let code = unsafe { exec_enter(img.entry, STACK_TOP) };
    process::end_external(pid, code);
    RUNNING.store(false, Ordering::SeqCst);
    Ok(code)
}
