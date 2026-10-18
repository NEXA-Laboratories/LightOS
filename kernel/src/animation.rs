//! Text-mode boot animation: the LightOS logo (tools/gen_logo.py) sweeps in,
//! shines, then a progress bar fills while the title appears.
use crate::logo::{LOGO, LOGO_H, LOGO_W};
use crate::process::sleep_ms;
use crate::vga_buffer::{self, put_at, put_str_at, Color, SCREEN_H, SCREEN_W};

fn draw_logo(reveal: usize, shine: bool, base: Color) {
    let top = 1;
    let left = (SCREEN_W - LOGO_W) / 2;
    for (r, line) in LOGO.iter().enumerate() {
        for (c, ch) in line.chars().enumerate() {
            if ch == ' ' {
                continue;
            }
            let (shown, color) = if c < reveal {
                // bright leading edge while sweeping
                let edge = shine && reveal < LOGO_W && c + 4 >= reveal;
                (true, if edge { Color::LightCyan } else { base })
            } else {
                (false, Color::Black)
            };
            if shown {
                put_at(top + r, left + c, ch, color, Color::Black);
            }
        }
    }
}

fn progress(pct: usize) {
    let width = 40;
    let left = (SCREEN_W - width - 2) / 2;
    let row = SCREEN_H - 3;
    put_at(row, left, '[', Color::DarkGray, Color::Black);
    put_at(row, left + width + 1, ']', Color::DarkGray, Color::Black);
    let filled = width * pct / 100;
    for i in 0..width {
        if i < filled {
            put_at(row, left + 1 + i, '\u{2588}', Color::LightCyan, Color::Black);
        } else {
            put_at(row, left + 1 + i, '\u{2591}', Color::DarkGray, Color::Black);
        }
    }
}

/// Needs the timer running (interrupts enabled, PIT programmed).
pub fn play() {
    vga_buffer::clear();

    // 1) sweep reveal, left to right
    let step = 2;
    let mut reveal = 0;
    while reveal <= LOGO_W + 4 {
        draw_logo(reveal, true, Color::LightGray);
        progress(reveal.min(LOGO_W) * 60 / LOGO_W);
        sleep_ms(18);
        reveal += step;
    }

    // 2) fade up: gray -> white
    draw_logo(LOGO_W, false, Color::White);
    sleep_ms(120);

    // 3) title + the rest of the progress bar
    let title = "LightOS";
    let version = crate::VERSION;
    let row = 1 + LOGO_H + 1;
    let total = title.len() + 1 + version.len() + 1;
    let col = (SCREEN_W - total) / 2;
    for (i, c) in title.chars().enumerate() {
        put_at(row, col + i, c, Color::White, Color::Black);
        sleep_ms(40);
    }
    put_str_at(row, col + title.len() + 1, "v", Color::DarkGray);
    put_str_at(row, col + title.len() + 2, version, Color::LightCyan);
    let tag = "NEXA Laboratories";
    put_str_at(row + 1, (SCREEN_W - tag.len()) / 2, tag, Color::DarkGray);

    for p in (60..=100).step_by(4) {
        progress(p);
        sleep_ms(30);
    }
    sleep_ms(350);
    vga_buffer::clear();
}
