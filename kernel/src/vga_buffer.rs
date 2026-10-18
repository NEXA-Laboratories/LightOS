//! VGA text-mode driver (80x25, 16 colours, CP437 mapping for box drawing).
use core::fmt;
use lazy_static::lazy_static;
use spin::Mutex;
use volatile::Volatile;

#[allow(dead_code)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum Color {
    Black = 0,
    Blue = 1,
    Green = 2,
    Cyan = 3,
    Red = 4,
    Magenta = 5,
    Brown = 6,
    LightGray = 7,
    DarkGray = 8,
    LightBlue = 9,
    LightGreen = 10,
    LightCyan = 11,
    LightRed = 12,
    Pink = 13,
    Yellow = 14,
    White = 15,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(transparent)]
struct ColorCode(u8);

impl ColorCode {
    fn new(fg: Color, bg: Color) -> ColorCode {
        ColorCode((bg as u8) << 4 | (fg as u8))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(C)]
struct ScreenChar {
    ascii_character: u8,
    color_code: ColorCode,
}

const BUFFER_HEIGHT: usize = 25;
const BUFFER_WIDTH: usize = 80;

#[repr(transparent)]
struct Buffer {
    chars: [[Volatile<ScreenChar>; BUFFER_WIDTH]; BUFFER_HEIGHT],
}

pub struct Writer {
    column_position: usize,
    color_code: ColorCode,
    buffer: &'static mut Buffer,
}

/// Map a Unicode char to its CP437 glyph.
fn cp437(c: char) -> u8 {
    match c {
        ' '..='~' => c as u8,
        '\u{2713}' => 0xFB, // check mark
        '\u{2717}' => b'x',
        '\u{26A0}' => b'!',
        '\u{2554}' => 0xC9,
        '\u{2557}' => 0xBB,
        '\u{255A}' => 0xC8,
        '\u{255D}' => 0xBC,
        '\u{2550}' => 0xCD,
        '\u{2551}' => 0xBA,
        '\u{2560}' => 0xCC,
        '\u{2563}' => 0xB9,
        '\u{2500}' => 0xC4,
        '\u{2502}' => 0xB3,
        '\u{2588}' => 0xDB,
        '\u{2580}' => 0xDF, // upper half block
        '\u{2584}' => 0xDC, // lower half block
        '\u{2591}' => 0xB0, // light shade
        '\u{2592}' => 0xB1,
        '\u{2593}' => 0xB2,
        _ => 0xFE,
    }
}

impl Writer {
    fn put_char(&mut self, c: char) {
        match c {
            '\n' => self.new_line(),
            '\u{8}' => self.backspace(),
            '\t' => {
                for _ in 0..4 {
                    self.write_cell(b' ');
                }
            }
            c => self.write_cell(cp437(c)),
        }
    }

    fn write_cell(&mut self, byte: u8) {
        if self.column_position >= BUFFER_WIDTH {
            self.new_line();
        }
        let row = BUFFER_HEIGHT - 1;
        let col = self.column_position;
        let color_code = self.color_code;
        self.buffer.chars[row][col].write(ScreenChar { ascii_character: byte, color_code });
        self.column_position += 1;
    }

    fn backspace(&mut self) {
        if self.column_position > 0 {
            self.column_position -= 1;
            let blank = ScreenChar { ascii_character: b' ', color_code: self.color_code };
            self.buffer.chars[BUFFER_HEIGHT - 1][self.column_position].write(blank);
        }
    }

    fn new_line(&mut self) {
        for row in 1..BUFFER_HEIGHT {
            for col in 0..BUFFER_WIDTH {
                let ch = self.buffer.chars[row][col].read();
                self.buffer.chars[row - 1][col].write(ch);
            }
        }
        self.clear_row(BUFFER_HEIGHT - 1);
        self.column_position = 0;
    }

    fn clear_row(&mut self, row: usize) {
        let blank = ScreenChar { ascii_character: b' ', color_code: self.color_code };
        for col in 0..BUFFER_WIDTH {
            self.buffer.chars[row][col].write(blank);
        }
    }

    fn clear(&mut self) {
        for row in 0..BUFFER_HEIGHT {
            self.clear_row(row);
        }
        self.column_position = 0;
    }
}

impl fmt::Write for Writer {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        for c in s.chars() {
            self.put_char(c);
        }
        Ok(())
    }
}

lazy_static! {
    pub static ref WRITER: Mutex<Writer> = Mutex::new(Writer {
        column_position: 0,
        color_code: ColorCode::new(Color::LightGray, Color::Black),
        buffer: unsafe { &mut *(0xb8000 as *mut Buffer) },
    });
}

pub fn set_color(fg: Color, bg: Color) {
    x86_64::instructions::interrupts::without_interrupts(|| {
        WRITER.lock().color_code = ColorCode::new(fg, bg);
    });
}

pub fn clear() {
    x86_64::instructions::interrupts::without_interrupts(|| {
        WRITER.lock().clear();
    });
}

#[macro_export]
macro_rules! print {
    ($($arg:tt)*) => ($crate::vga_buffer::_print(format_args!($($arg)*)));
}

#[macro_export]
macro_rules! println {
    () => ($crate::print!("\n"));
    ($($arg:tt)*) => ($crate::print!("{}\n", format_args!($($arg)*)));
}

#[doc(hidden)]
pub fn _print(args: fmt::Arguments) {
    use core::fmt::Write;
    x86_64::instructions::interrupts::without_interrupts(|| {
        WRITER.lock().write_fmt(args).unwrap();
    });
}

/// Draw one character at an absolute screen position (used by the boot animation).
pub fn put_at(row: usize, col: usize, c: char, fg: Color, bg: Color) {
    if row >= BUFFER_HEIGHT || col >= BUFFER_WIDTH {
        return;
    }
    x86_64::instructions::interrupts::without_interrupts(|| {
        let mut w = WRITER.lock();
        w.buffer.chars[row][col].write(ScreenChar { ascii_character: cp437(c), color_code: ColorCode::new(fg, bg) });
    });
}

pub fn put_str_at(row: usize, col: usize, s: &str, fg: Color) {
    for (i, c) in s.chars().enumerate() {
        put_at(row, col + i, c, fg, Color::Black);
    }
}

pub const SCREEN_W: usize = BUFFER_WIDTH;
pub const SCREEN_H: usize = BUFFER_HEIGHT;
