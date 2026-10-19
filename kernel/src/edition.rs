//! System edition: Console (text shell only) or Desktop (text-mode desktop environment).
//! Stored in /LOS/EDITION.TXT; the installer asks which one to set up.
use crate::fat32;
use alloc::string::String;

#[derive(Clone, Copy, PartialEq)]
pub enum Edition {
    Console,
    Desktop,
}

const FILE: &str = "/LOS/EDITION.TXT";

pub fn name(e: Edition) -> &'static str {
    match e {
        Edition::Console => "console",
        Edition::Desktop => "desktop",
    }
}

pub fn current() -> Edition {
    match fat32::with(|fs| fs.read_file(FILE)) {
        Ok(d) if String::from_utf8_lossy(&d).trim() == "desktop" => Edition::Desktop,
        _ => Edition::Console,
    }
}

pub fn set(e: Edition) -> Result<(), String> {
    fat32::with(|fs| {
        if !fs.is_dir("/LOS") {
            fs.mkdir("/LOS")?;
        }
        fs.write_file(FILE, alloc::format!("{}\n", name(e)).as_bytes())
    })
}
