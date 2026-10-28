//! cat [arquivo...]: copia os arquivos (ou a entrada padrão) para a saída.

#![no_std]
#![no_main]

use ferrugem_user::{entry, eprintln, rt, sys};

entry!(main);

fn copy(fd: u64) -> sys::Result<()> {
    let mut buf = [0u8; 4096];
    loop {
        let n = sys::read(fd, &mut buf);
        if n < 0 {
            return Err(-n as sys::Errno);
        }
        if n == 0 {
            return Ok(());
        }
        sys::write_all(1, &buf[..n as usize])?;
    }
}

fn main() -> i32 {
    let mut status = 0;
    let mut files = rt::args().skip(1).peekable();
    if files.peek().is_none() {
        return if copy(0).is_ok() { 0 } else { 1 };
    }
    for path in files {
        let result = if path == "-" {
            copy(0)
        } else {
            sys::open(path, sys::O_RDONLY, 0).and_then(|fd| {
                let result = copy(fd);
                let _ = sys::close(fd);
                result
            })
        };
        if let Err(errno) = result {
            eprintln!("cat: {path}: {}", sys::strerror(errno));
            status = 1;
        }
    }
    status
}
