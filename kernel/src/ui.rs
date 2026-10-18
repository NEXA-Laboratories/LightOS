//! Tiny box-drawing table helper used by the shell and the package manager.
use alloc::string::String;

const W: usize = 72;

fn rule(l: char, m: char, r: char) {
    let mut s = String::new();
    s.push(l);
    for _ in 0..W + 2 {
        s.push(m);
    }
    s.push(r);
    println!("{}", s);
}

fn row(text: &str) {
    let t: String = text.chars().take(W).collect();
    println!("║ {:<w$} ║", t, w = W);
}

pub fn table(title: &str, header: Option<&str>, rows: &[String]) {
    rule('╔', '═', '╗');
    let pad = W.saturating_sub(title.chars().count()) / 2;
    let mut t = String::new();
    for _ in 0..pad {
        t.push(' ');
    }
    t.push_str(title);
    row(&t);
    rule('╠', '═', '╣');
    if let Some(h) = header {
        row(h);
        rule('╠', '═', '╣');
    }
    for r in rows {
        row(r);
    }
    rule('╚', '═', '╝');
}
