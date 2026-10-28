//! `print!`/`println!` do kernel: escrevem na tela VGA e na porta serial.

use core::fmt::{self, Write};

use crate::serial::SERIAL;
use crate::vga::{self, Color, WRITER};

pub fn _print(args: fmt::Arguments) {
    let _ = WRITER.lock().write_fmt(args);
    let _ = SERIAL.lock().write_fmt(args);
}

/// Muda a cor do texto seguinte (na VGA e, via ANSI, no terminal serial).
pub fn set_color(fg: Color) {
    WRITER.lock().set_attr(vga::attr(fg, Color::Black));
    let _ = write!(SERIAL.lock(), "\x1b[{}m", fg.ansi());
}

pub fn reset_color() {
    WRITER.lock().set_attr(vga::DEFAULT_ATTR);
    let _ = SERIAL.lock().write_str("\x1b[0m");
}

pub fn clear() {
    WRITER.lock().clear();
    let _ = SERIAL.lock().write_str("\x1b[2J\x1b[H");
}

#[macro_export]
macro_rules! print {
    ($($arg:tt)*) => ($crate::console::_print(format_args!($($arg)*)));
}

#[macro_export]
macro_rules! println {
    () => ($crate::print!("\n"));
    ($($arg:tt)*) => ($crate::print!("{}\n", format_args!($($arg)*)));
}
