//! Saída formatada (`print!`, `println!`, `eprintln!`) e leitura de linhas.

use core::fmt;

use crate::sys;

pub struct Fd(pub u64);

impl fmt::Write for Fd {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        if sys::write(self.0, s.as_bytes()) < 0 { Err(fmt::Error) } else { Ok(()) }
    }
}

#[doc(hidden)]
pub fn _print(fd: u64, args: fmt::Arguments) {
    let _ = fmt::Write::write_fmt(&mut Fd(fd), args);
}

#[macro_export]
macro_rules! print {
    ($($arg:tt)*) => ($crate::io::_print(1, format_args!($($arg)*)));
}

#[macro_export]
macro_rules! println {
    () => ($crate::print!("\n"));
    ($($arg:tt)*) => ($crate::print!("{}\n", format_args!($($arg)*)));
}

#[macro_export]
macro_rules! eprintln {
    ($($arg:tt)*) => ($crate::io::_print(2, format_args!("{}\n", format_args!($($arg)*))));
}

/// Lê uma linha da entrada padrão (sem o '\n'). `None` no fim do arquivo.
pub fn read_line(buf: &mut [u8]) -> Option<&str> {
    let n = sys::read(0, buf);
    if n <= 0 {
        return None;
    }
    let line = core::str::from_utf8(&buf[..n as usize]).unwrap_or("");
    Some(line.trim_end_matches('\n'))
}
