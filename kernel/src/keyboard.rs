//! PS/2 keyboard: scancodes are decoded in the IRQ handler into a lock-free-ish ring buffer.
use pc_keyboard::{layouts, DecodedKey, HandleControl, KeyCode, Keyboard, ScancodeSet1};
use spin::Mutex;
use lazy_static::lazy_static;
use x86_64::instructions::interrupts;

pub const KEY_UP: u8 = 0x81;
pub const KEY_DOWN: u8 = 0x82;
pub const KEY_LEFT: u8 = 0x83;
pub const KEY_RIGHT: u8 = 0x84;
pub const KEY_HOME: u8 = 0x85;
pub const KEY_END: u8 = 0x86;
pub const KEY_PGUP: u8 = 0x88;
pub const KEY_PGDN: u8 = 0x89;
pub const KEY_F2: u8 = 0x90;

const SIZE: usize = 256;

struct Ring {
    buf: [u8; SIZE],
    head: usize,
    tail: usize,
}

impl Ring {
    const fn new() -> Self {
        Ring { buf: [0; SIZE], head: 0, tail: 0 }
    }
    fn push(&mut self, b: u8) {
        let next = (self.head + 1) % SIZE;
        if next != self.tail {
            self.buf[self.head] = b;
            self.head = next;
        }
    }
    fn pop(&mut self) -> Option<u8> {
        if self.head == self.tail {
            None
        } else {
            let b = self.buf[self.tail];
            self.tail = (self.tail + 1) % SIZE;
            Some(b)
        }
    }
}

static RING: Mutex<Ring> = Mutex::new(Ring::new());

lazy_static! {
    static ref KEYBOARD: Mutex<Keyboard<layouts::Us104Key, ScancodeSet1>> =
        Mutex::new(Keyboard::new(ScancodeSet1::new(), layouts::Us104Key, HandleControl::Ignore));
}

/// Called from the keyboard IRQ handler. Must not allocate.
pub fn handle_scancode(scancode: u8) {
    let mut kb = KEYBOARD.lock();
    if let Ok(Some(event)) = kb.add_byte(scancode) {
        match kb.process_keyevent(event) {
            Some(DecodedKey::Unicode(c)) if c.is_ascii() => RING.lock().push(c as u8),
            Some(DecodedKey::RawKey(k)) => {
                let code = match k {
                    KeyCode::ArrowUp => KEY_UP,
                    KeyCode::ArrowDown => KEY_DOWN,
                    KeyCode::ArrowLeft => KEY_LEFT,
                    KeyCode::ArrowRight => KEY_RIGHT,
                    KeyCode::Home => KEY_HOME,
                    KeyCode::End => KEY_END,
                    KeyCode::PageUp => KEY_PGUP,
                    KeyCode::PageDown => KEY_PGDN,
                    KeyCode::F2 => KEY_F2,
                    _ => 0,
                };
                if code != 0 {
                    RING.lock().push(code);
                }
            }
            _ => {}
        }
    }
}

/// Block (halting the CPU) until a character is typed.
pub fn read_char() -> char {
    loop {
        if let Some(b) = interrupts::without_interrupts(|| RING.lock().pop()) {
            return b as char;
        }
        interrupts::enable_and_hlt();
    }
}
