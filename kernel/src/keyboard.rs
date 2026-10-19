//! PS/2 keyboard: scancodes are decoded in the IRQ handler into a lock-free-ish ring buffer.
use pc_keyboard::{layouts, DecodedKey, HandleControl, KeyCode, Keyboard, ScancodeSet1};
use core::sync::atomic::{AtomicBool, Ordering};
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
static CANCEL: AtomicBool = AtomicBool::new(false);
static BUSY: AtomicBool = AtomicBool::new(false);

/// True once Ctrl+C (or Esc) has been pressed while a command runs.
pub fn cancel_requested() -> bool {
    CANCEL.load(Ordering::SeqCst)
}

pub fn clear_cancel() {
    CANCEL.store(false, Ordering::SeqCst);
}

/// The shell marks the time a command is running: Ctrl+C then cancels it instead of editing the prompt.
pub fn set_busy(b: bool) {
    BUSY.store(b, Ordering::SeqCst);
}

/// Drop everything typed so far.
pub fn flush() {
    interrupts::without_interrupts(|| {
        let mut r = RING.lock();
        r.head = 0;
        r.tail = 0;
    });
}

// ---- debug trace for the `keytest` command: (scancode, what it decoded to)
const DBG: usize = 64;
struct Trace {
    buf: [(u8, u8); DBG],
    head: usize,
    tail: usize,
}
static TRACE: Mutex<Trace> = Mutex::new(Trace { buf: [(0, 0); DBG], head: 0, tail: 0 });

fn trace(sc: u8, code: u8) {
    let mut t = TRACE.lock();
    let next = (t.head + 1) % DBG;
    if next != t.tail {
        let h = t.head;
        t.buf[h] = (sc, code);
        t.head = next;
    }
}

/// (scancode, decoded): decoded is an ASCII/special code, 0 = no character (modifier/release), 0xFF = other key.
pub fn trace_pop() -> Option<(u8, u8)> {
    interrupts::without_interrupts(|| {
        let mut t = TRACE.lock();
        if t.head == t.tail {
            None
        } else {
            let v = t.buf[t.tail];
            t.tail = (t.tail + 1) % DBG;
            Some(v)
        }
    })
}

lazy_static! {
    static ref KEYBOARD: Mutex<Keyboard<layouts::Us104Key, ScancodeSet1>> =
        Mutex::new(Keyboard::new(ScancodeSet1::new(), layouts::Us104Key, HandleControl::MapLettersToUnicode));
}

/// Called from the keyboard IRQ handler. Must not allocate.
pub fn handle_scancode(scancode: u8) {
    let mut kb = KEYBOARD.lock();
    let mut seen: u8 = 0;
    if let Ok(Some(event)) = kb.add_byte(scancode) {
        match kb.process_keyevent(event) {
            Some(DecodedKey::Unicode('\u{3}')) => {
                // Ctrl+C: cancel the running command, or abort the line being typed
                seen = 3;
                if BUSY.load(Ordering::SeqCst) {
                    CANCEL.store(true, Ordering::SeqCst);
                } else {
                    RING.lock().push(3);
                }
            }
            Some(DecodedKey::Unicode(c)) if c.is_ascii() => {
                seen = c as u8;
                if c == '\u{1b}' {
                    CANCEL.store(true, Ordering::SeqCst); // Esc cancels too (the editor still receives it)
                }
                RING.lock().push(c as u8);
            }
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
                seen = if code != 0 { code } else { 0xFF };
                if code != 0 {
                    RING.lock().push(code);
                }
            }
            _ => {}
        }
    }
    trace(scancode, seen);
}

/// Non-blocking read.
pub fn poll_char() -> Option<char> {
    interrupts::without_interrupts(|| RING.lock().pop()).map(|b| b as char)
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
