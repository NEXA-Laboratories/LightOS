//! System calls via `int 0x80`.
//!
//! ABI: rax = number, rdi/rsi/rdx = arguments, result in rax (u64::MAX = error).
use crate::{los, process};
use core::arch::{asm, global_asm};

pub const SYS_WRITE: u64 = 1; // (fd, ptr, len)    fd 1 = screen, 2 = serial
pub const SYS_GETPID: u64 = 2;
pub const SYS_EXIT: u64 = 3; // (code)
pub const SYS_YIELD: u64 = 4;
pub const SYS_UPTIME: u64 = 5; // timer ticks
pub const SYS_LOS_INSTALL: u64 = 10; // (name_ptr, name_len)
pub const SYS_LOS_REMOVE: u64 = 11; // (name_ptr, name_len)
pub const SYS_LOS_LIST: u64 = 12; // () -> number of installed packages
pub const ERR: u64 = u64::MAX;

global_asm!(
    r#"
    .global syscall_entry
syscall_entry:
    push rcx
    push rdx
    push rsi
    push rdi
    push r8
    push r9
    push r10
    push r11
    sub rsp, 8
    mov rcx, rdx
    mov rdx, rsi
    mov rsi, rdi
    mov rdi, rax
    cld
    call syscall_dispatch
    add rsp, 8
    pop r11
    pop r10
    pop r9
    pop r8
    pop rdi
    pop rsi
    pop rdx
    pop rcx
    iretq
"#
);

extern "C" {
    fn syscall_entry();
}

pub fn entry_addr() -> u64 {
    syscall_entry as usize as u64
}

pub fn init() {
    serial_println!("syscall: int 0x80 gate installed");
}

fn str_arg(ptr: u64, len: u64) -> Option<&'static str> {
    if ptr == 0 || len == 0 || len > 4096 {
        return None;
    }
    let bytes = unsafe { core::slice::from_raw_parts(ptr as *const u8, len as usize) };
    core::str::from_utf8(bytes).ok()
}

#[no_mangle]
pub extern "C" fn syscall_dispatch(num: u64, a1: u64, a2: u64, a3: u64) -> u64 {
    match num {
        SYS_WRITE => match str_arg(a2, a3) {
            Some(s) => {
                match a1 {
                    1 => print!("{}", s),
                    2 => serial_print!("{}", s),
                    _ => return ERR,
                }
                s.len() as u64
            }
            None => ERR,
        },
        SYS_GETPID => process::current_pid() as u64,
        SYS_EXIT => {
            process::terminate_current(a1 as i32);
            if crate::elf::is_running() {
                // Unwind straight back to the shell that started the app (never returns).
                unsafe { crate::elf::exec_leave(a1 as i32) }
            }
            0
        }
        SYS_YIELD => 0,
        SYS_UPTIME => process::ticks(),
        SYS_LOS_INSTALL => match str_arg(a1, a2) {
            Some(name) => match los::install(name) {
                Ok(()) => 0,
                Err(e) => {
                    println!("los: {}", e);
                    ERR
                }
            },
            None => ERR,
        },
        SYS_LOS_REMOVE => match str_arg(a1, a2) {
            Some(name) => match los::remove(name, false) {
                Ok(()) => 0,
                Err(e) => {
                    println!("los: {}", e);
                    ERR
                }
            },
            None => ERR,
        },
        SYS_LOS_LIST => los::list() as u64,
        _ => ERR,
    }
}

/// Issue a system call from kernel code (goes through the real int 0x80 gate).
pub fn invoke(num: u64, a1: u64, a2: u64, a3: u64) -> u64 {
    let ret: u64;
    unsafe {
        asm!(
            "int 0x80",
            inlateout("rax") num => ret,
            in("rdi") a1,
            in("rsi") a2,
            in("rdx") a3,
            options(nostack)
        );
    }
    ret
}

pub fn write(fd: u64, s: &str) -> u64 {
    invoke(SYS_WRITE, fd, s.as_ptr() as u64, s.len() as u64)
}
pub fn getpid() -> u32 {
    invoke(SYS_GETPID, 0, 0, 0) as u32
}
pub fn uptime() -> u64 {
    invoke(SYS_UPTIME, 0, 0, 0)
}
pub fn yield_now() {
    invoke(SYS_YIELD, 0, 0, 0);
}
