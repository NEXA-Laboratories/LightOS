//! liblightos - system call wrappers and print macros for LightOS applications.
#![no_std]

use core::arch::asm;
use core::fmt;

pub const SYS_WRITE: u64 = 1;
pub const SYS_GETPID: u64 = 2;
pub const SYS_EXIT: u64 = 3;
pub const SYS_YIELD: u64 = 4;
pub const SYS_UPTIME: u64 = 5;
pub const SYS_GETCHAR: u64 = 6;
pub const SYS_SLEEP: u64 = 7;
pub const SYS_LOS_INSTALL: u64 = 10;
pub const SYS_LOS_REMOVE: u64 = 11;
pub const SYS_LOS_LIST: u64 = 12;

/// Raw system call through the `int 0x80` gate.
#[inline(always)]
pub fn syscall(num: u64, a1: u64, a2: u64, a3: u64) -> u64 {
    let ret: u64;
    unsafe {
        asm!("int 0x80", inlateout("rax") num => ret, in("rdi") a1, in("rsi") a2, in("rdx") a3, options(nostack));
    }
    ret
}

pub fn sys_write(fd: u64, buf: *const u8, len: usize) -> u64 {
    syscall(SYS_WRITE, fd, buf as u64, len as u64)
}
pub fn sys_getpid() -> u32 {
    syscall(SYS_GETPID, 0, 0, 0) as u32
}
pub fn sys_exit(code: i32) -> ! {
    syscall(SYS_EXIT, code as u64, 0, 0);
    loop {
        unsafe { asm!("hlt") }
    }
}
pub fn sys_yield() {
    syscall(SYS_YIELD, 0, 0, 0);
}
/// Timer ticks since boot (100 per second).
pub fn sys_uptime() -> u64 {
    syscall(SYS_UPTIME, 0, 0, 0)
}
/// Block until a key is typed; returns its ASCII code (3 = Ctrl+C).
pub fn sys_getchar() -> u8 {
    syscall(SYS_GETCHAR, 0, 0, 0) as u8
}
/// Sleep for `ms` milliseconds (max 10 s per call).
pub fn sys_sleep_ms(ms: u64) {
    syscall(SYS_SLEEP, ms, 0, 0);
}
pub fn sys_los_install(name: *const u8, len: usize) -> u64 {
    syscall(SYS_LOS_INSTALL, name as u64, len as u64, 0)
}
pub fn sys_los_remove(name: *const u8, len: usize) -> u64 {
    syscall(SYS_LOS_REMOVE, name as u64, len as u64, 0)
}
/// Prints the installed packages and returns how many there are.
pub fn sys_los_list() -> u64 {
    syscall(SYS_LOS_LIST, 0, 0, 0)
}

pub struct Stdout;

impl fmt::Write for Stdout {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        sys_write(1, s.as_ptr(), s.len());
        Ok(())
    }
}

#[doc(hidden)]
pub fn _print(args: fmt::Arguments) {
    use core::fmt::Write;
    let _ = Stdout.write_fmt(args);
}

#[macro_export]
macro_rules! print {
    ($($arg:tt)*) => ($crate::_print(format_args!($($arg)*)));
}

#[macro_export]
macro_rules! println {
    () => ($crate::print!("\n"));
    ($($arg:tt)*) => ($crate::print!("{}\n", format_args!($($arg)*)));
}

#[cfg(feature = "panic-handler")]
#[panic_handler]
fn panic(info: &core::panic::PanicInfo) -> ! {
    println!("panic: {}", info);
    sys_exit(101)
}
