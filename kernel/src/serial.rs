//! COM1 serial driver. Used for debug logging and as the transport to los-server.
use alloc::string::String;
use core::fmt;
use lazy_static::lazy_static;
use spin::Mutex;
use uart_16550::SerialPort;
use x86_64::instructions::port::Port;

const COM1: u16 = 0x3F8;

lazy_static! {
    pub static ref SERIAL1: Mutex<SerialPort> = {
        let mut port = unsafe { SerialPort::new(COM1) };
        port.init();
        Mutex::new(port)
    };
}

#[doc(hidden)]
pub fn _print(args: fmt::Arguments) {
    use core::fmt::Write;
    x86_64::instructions::interrupts::without_interrupts(|| {
        SERIAL1.lock().write_fmt(args).expect("Printing to serial failed");
    });
}

#[macro_export]
macro_rules! serial_print {
    ($($arg:tt)*) => ($crate::serial::_print(format_args!($($arg)*)));
}

#[macro_export]
macro_rules! serial_println {
    () => ($crate::serial_print!("\n"));
    ($($arg:tt)*) => ($crate::serial_print!("{}\n", format_args!($($arg)*)));
}

/// True if a byte is waiting in the receive register.
pub fn data_ready() -> bool {
    unsafe { Port::<u8>::new(COM1 + 5).read() & 1 != 0 }
}

/// Poll for one byte; `spins` bounds the wait.
pub fn read_byte(spins: u32) -> Option<u8> {
    for _ in 0..spins {
        if data_ready() {
            return Some(unsafe { Port::<u8>::new(COM1).read() });
        }
        core::hint::spin_loop();
    }
    None
}

/// Read one '\n'-terminated line. Returns None if the peer goes silent.
pub fn read_line(spins: u32) -> Option<String> {
    let mut s = String::new();
    loop {
        match read_byte(spins)? {
            b'\n' => return Some(s),
            b'\r' => {}
            b => {
                if s.len() < 100_000 {
                    s.push(b as char);
                }
            }
        }
    }
}

/// Discard any pending input.
pub fn drain() {
    while data_ready() {
        unsafe {
            Port::<u8>::new(COM1).read();
        }
    }
}

/// Write a protocol line (terminated with a single '\n').
pub fn write_line(s: &str) {
    _print(format_args!("{}\n", s));
}
