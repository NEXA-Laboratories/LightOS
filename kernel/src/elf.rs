//! ELF64 loader + ring-3 launcher.
//!
//! Static x86_64 ET_EXEC binaries are loaded into the user region (4 TiB, `APP_BASE`) with the
//! USER flag set on their pages only, then entered in ring 3 with `iretq`. Apps reach the kernel
//! through `int 0x80`; kernel memory is supervisor-only, so a stray or hostile access is a page
//! fault that kills just the app. Ctrl+C / Esc also kills a running app (timer tick check).
use crate::{gdt, keyboard, memory, process};
use alloc::{format, string::String, vec::Vec};
use core::arch::global_asm;
use core::sync::atomic::{AtomicBool, Ordering};
use spin::Mutex;

pub const APP_BASE: u64 = 0x0000_0400_0000_0000;
pub const APP_CODE_MAX: u64 = 16 * 1024 * 1024;
const STACK_SIZE: u64 = 64 * 1024;
pub const STACK_TOP: u64 = APP_BASE + APP_CODE_MAX + STACK_SIZE;

static RUNNING: AtomicBool = AtomicBool::new(false);
/// Memory the running app owns (page-aligned [start, end) ranges); used to validate syscall pointers.
static RANGES: Mutex<Vec<(u64, u64)>> = Mutex::new(Vec::new());

pub fn is_running() -> bool {
    RUNNING.load(Ordering::SeqCst)
}

/// May a syscall of the running app touch [ptr, ptr+len)?
pub fn user_range_ok(ptr: u64, len: u64) -> bool {
    let end = match ptr.checked_add(len) {
        Some(e) => e,
        None => return false,
    };
    RANGES.lock().iter().any(|&(s, e)| ptr >= s && end <= e)
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

/// Validate and load an ELF64 executable into the user region.
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
            "entry point {:#x} is outside the app region ({:#x}..{:#x}); relink with --image-base={:#x}",
            entry,
            APP_BASE,
            APP_BASE + APP_CODE_MAX,
            APP_BASE
        ));
    }

    let mut segs: Vec<Seg> = Vec::new();
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

    // Pass 1: map as user memory + zero whole pages (so .bss and the previous app's data are gone).
    let mut ranges: Vec<(u64, u64)> = Vec::new();
    for s in &segs {
        let start = s.vaddr & !0xFFF;
        let end = (s.vaddr + s.memsz + 0xFFF) & !0xFFF;
        memory::map_user_region(start, (end - start) as usize)?;
        unsafe { core::ptr::write_bytes(start as *mut u8, 0, (end - start) as usize) };
        ranges.push((start, end));
    }
    let stack_start = STACK_TOP - STACK_SIZE;
    memory::map_user_region(stack_start, STACK_SIZE as usize)?;
    unsafe { core::ptr::write_bytes(stack_start as *mut u8, 0, STACK_SIZE as usize) };
    ranges.push((stack_start, STACK_TOP));

    // Pass 2: copy file contents.
    let mut bytes = 0;
    for s in &segs {
        unsafe {
            core::ptr::copy_nonoverlapping(d.as_ptr().add(s.off as usize), s.vaddr as *mut u8, s.filesz as usize);
        }
        bytes += s.memsz as usize;
    }
    *RANGES.lock() = ranges;
    Ok(Image { entry, segments: segs.len(), bytes })
}

// ------------------------------------------------------------------ ending an app early

/// A ring-3 app faulted: report it and return to the shell. Never returns.
pub fn user_fault(what: &str, addr: u64, rip: u64) -> ! {
    println!("\n[app] crashed: {} (address {:#x}, instruction {:#x}) - process killed, the system is fine", what, addr, rip);
    process::terminate_current(-11);
    unsafe { exec_leave(-11) }
}

/// Kill the running app on request (Ctrl+C). Never returns.
pub fn user_abort(reason: &str) -> ! {
    println!("\n[app] {}", reason);
    process::terminate_current(-9);
    unsafe { exec_leave(-9) }
}

/// Checked from the timer interrupt while ring 3 code is running.
pub fn user_should_stop() -> bool {
    is_running() && keyboard::cancel_requested()
}

// exec_enter_user(entry, stack, cs, ss) -> exit code
//   saves the callee-saved registers and the kernel stack pointer, then iretq's into ring 3.
// exec_leave(code) -> never returns; resumes after the exec_enter_user call with eax = code.
//   Used by sys_exit and by the fault/abort paths (they run on the kernel stack from the TSS).
global_asm!(
    r#"
    .global exec_enter_user
exec_enter_user:
    push rbx
    push rbp
    push r12
    push r13
    push r14
    push r15
    mov [rip + EXEC_SP], rsp
    push rcx
    push rsi
    push 514
    push rdx
    push rdi
    xor eax, eax
    xor ebx, ebx
    xor ecx, ecx
    xor edx, edx
    xor esi, esi
    xor edi, edi
    xor ebp, ebp
    xor r8d, r8d
    xor r9d, r9d
    xor r10d, r10d
    xor r11d, r11d
    xor r12d, r12d
    xor r13d, r13d
    xor r14d, r14d
    xor r15d, r15d
    iretq

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
    fn exec_enter_user(entry: u64, stack: u64, cs: u64, ss: u64) -> i32;
    pub fn exec_leave(code: i32) -> !;
}

/// Load `data` as an ELF app named `name`, run it in ring 3 to completion, return its exit code
/// (negative if it was killed: -11 crashed, -9 stopped).
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
    serial_println!("elf: {} loaded ({} segment(s), {} bytes), entry {:#x}, ring 3", name, img.segments, img.bytes, img.entry);
    let (cs, ss) = gdt::user_selectors();
    let pid = process::begin_external(name);
    keyboard::clear_cancel();
    let code = unsafe { exec_enter_user(img.entry, STACK_TOP - 8, cs as u64, ss as u64) };
    process::end_external(pid, code);
    RUNNING.store(false, Ordering::SeqCst);
    Ok(code)
}
