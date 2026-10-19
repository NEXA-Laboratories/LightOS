//! `edit <file>`: a small full-screen text editor (part of the los-edit package).
//! Keys: arrows/Home/End/PgUp/PgDn move, Backspace/Delete/Enter/Tab edit, F2 saves, Esc quits.
use crate::keyboard::{self, KEY_DOWN, KEY_END, KEY_F2, KEY_HOME, KEY_LEFT, KEY_PGDN, KEY_PGUP, KEY_RIGHT, KEY_UP};
use crate::vga_buffer::{self, put_at, Color, SCREEN_H, SCREEN_W};
use crate::fat32;
use alloc::{format, string::{String, ToString}, vec::Vec};

const ROWS: usize = SCREEN_H - 1;
const MAX_LINE: usize = 400;

struct Ed {
    lines: Vec<String>,
    row: usize,
    col: usize,
    top: usize,
    left: usize,
    dirty: bool,
    armed: bool,
    path: String,
    msg: String,
}

fn clean(s: &str) -> String {
    s.chars().map(|c| if c.is_ascii() && (!c.is_control() || c == '\t') { c } else { '?' }).collect()
}

impl Ed {
    fn len(&self) -> usize {
        self.lines[self.row].len()
    }

    fn clamp(&mut self) {
        if self.col > self.len() {
            self.col = self.len();
        }
        if self.row < self.top {
            self.top = self.row;
        }
        if self.row >= self.top + ROWS {
            self.top = self.row + 1 - ROWS;
        }
        if self.col < self.left {
            self.left = self.col;
        }
        if self.col >= self.left + SCREEN_W {
            self.left = self.col + 1 - SCREEN_W;
        }
    }

    fn draw(&self) {
        for r in 0..ROWS {
            let line = self.lines.get(self.top + r).map(|l| l.as_bytes()).unwrap_or(&[]);
            for c in 0..SCREEN_W {
                let ch = line.get(self.left + c).copied().unwrap_or(b' ');
                put_at(r, c, if ch == b'\t' { ' ' } else { ch as char }, Color::LightGray, Color::Black);
            }
        }
        // cursor: inverted cell
        let cur = self.lines[self.row].as_bytes().get(self.col).copied().unwrap_or(b' ');
        put_at(self.row - self.top, self.col - self.left, cur as char, Color::Black, Color::LightGray);
        // status bar
        let status = format!(
            " {}{}   Ln {} Col {}   F2 Save   Esc Quit   {}",
            self.path,
            if self.dirty { " *" } else { "" },
            self.row + 1,
            self.col + 1,
            self.msg
        );
        let b = status.as_bytes();
        for c in 0..SCREEN_W {
            put_at(ROWS, c, b.get(c).map(|&x| x as char).unwrap_or(' '), Color::White, Color::Blue);
        }
    }

    fn save(&mut self) {
        let mut text = self.lines.join("\n");
        text.push('\n');
        match fat32::with(|fs| fs.write_file(&self.path, text.as_bytes())) {
            Ok(()) => {
                self.dirty = false;
                self.msg = format!("saved {} bytes", text.len());
            }
            Err(e) => self.msg = format!("SAVE FAILED: {}", e),
        }
    }

    fn insert(&mut self, c: char) {
        if self.len() < MAX_LINE {
            let col = self.col;
            self.lines[self.row].insert(col, c);
            self.col += 1;
            self.dirty = true;
        }
    }
}

pub fn edit(path: &str) -> Result<(), String> {
    let lines: Vec<String> = match fat32::with(|fs| fs.read_file(path)) {
        Ok(d) => {
            let text = String::from_utf8_lossy(&d).to_string();
            let mut v: Vec<String> = text.split('\n').map(|l| clean(l.trim_end_matches('\r'))).collect();
            if v.len() > 1 && v.last().map_or(false, |l| l.is_empty()) {
                v.pop();
            }
            v
        }
        Err(e) if e.contains("not found") => alloc::vec![String::new()],
        Err(e) => return Err(e),
    };
    keyboard::flush();
    let mut ed = Ed { lines, row: 0, col: 0, top: 0, left: 0, dirty: false, armed: false, path: String::from(path), msg: String::new() };
    ed.msg = String::from(if ed.lines.len() == 1 && ed.lines[0].is_empty() { "new file" } else { "" });

    loop {
        ed.clamp();
        ed.draw();
        let key = keyboard::read_char() as u32 as u8;
        if key != 0x1B {
            ed.armed = false;
        }
        ed.msg.clear();
        match key {
            0x1B => {
                if ed.dirty && !ed.armed {
                    ed.armed = true;
                    ed.msg = String::from("UNSAVED CHANGES - F2 saves, Esc again discards");
                } else {
                    break;
                }
            }
            KEY_F2 => ed.save(),
            b'\n' => {
                let col = ed.col;
                let rest = ed.lines[ed.row].split_off(col);
                ed.lines.insert(ed.row + 1, rest);
                ed.row += 1;
                ed.col = 0;
                ed.dirty = true;
            }
            0x08 => {
                if ed.col > 0 {
                    let col = ed.col - 1;
                    ed.lines[ed.row].remove(col);
                    ed.col = col;
                    ed.dirty = true;
                } else if ed.row > 0 {
                    let cur = ed.lines.remove(ed.row);
                    ed.row -= 1;
                    ed.col = ed.lines[ed.row].len();
                    ed.lines[ed.row].push_str(&cur);
                    ed.dirty = true;
                }
            }
            0x7F => {
                if ed.col < ed.len() {
                    let col = ed.col;
                    ed.lines[ed.row].remove(col);
                    ed.dirty = true;
                } else if ed.row + 1 < ed.lines.len() {
                    let next = ed.lines.remove(ed.row + 1);
                    ed.lines[ed.row].push_str(&next);
                    ed.dirty = true;
                }
            }
            b'\t' => {
                for _ in 0..4 {
                    ed.insert(' ');
                }
            }
            0x20..=0x7E => ed.insert(key as char),
            KEY_LEFT => {
                if ed.col > 0 {
                    ed.col -= 1;
                } else if ed.row > 0 {
                    ed.row -= 1;
                    ed.col = ed.len();
                }
            }
            KEY_RIGHT => {
                if ed.col < ed.len() {
                    ed.col += 1;
                } else if ed.row + 1 < ed.lines.len() {
                    ed.row += 1;
                    ed.col = 0;
                }
            }
            KEY_UP => ed.row = ed.row.saturating_sub(1),
            KEY_DOWN => {
                if ed.row + 1 < ed.lines.len() {
                    ed.row += 1;
                }
            }
            KEY_HOME => ed.col = 0,
            KEY_END => ed.col = ed.len(),
            KEY_PGUP => ed.row = ed.row.saturating_sub(ROWS),
            KEY_PGDN => ed.row = (ed.row + ROWS).min(ed.lines.len() - 1),
            _ => {}
        }
    }
    vga_buffer::clear();
    Ok(())
}
