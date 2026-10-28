//! head [-n N] [arquivo]: as primeiras N linhas (10 por padrão).

#![no_std]
#![no_main]

use ferrugem_user::{entry, eprintln, rt, sys};

entry!(main);

fn main() -> i32 {
    let args: alloc::vec::Vec<&str> = rt::args().skip(1).collect();
    let mut lines = 10usize;
    let mut path = None;
    let mut i = 0;
    while i < args.len() {
        match args[i] {
            "-n" if i + 1 < args.len() => {
                lines = args[i + 1].parse().unwrap_or(10);
                i += 1;
            }
            arg => path = Some(arg),
        }
        i += 1;
    }
    let fd = match path {
        None | Some("-") => 0,
        Some(path) => match sys::open(path, sys::O_RDONLY, 0) {
            Ok(fd) => fd,
            Err(errno) => {
                eprintln!("head: {path}: {}", sys::strerror(errno));
                return 1;
            }
        },
    };
    let mut buf = [0u8; 4096];
    while lines > 0 {
        let n = sys::read(fd, &mut buf);
        if n <= 0 {
            break;
        }
        let chunk = &buf[..n as usize];
        // Escreve até a N-ésima quebra de linha (ou o bloco todo).
        let mut end = chunk.len();
        for (index, &byte) in chunk.iter().enumerate() {
            if byte == b'\n' {
                lines -= 1;
                if lines == 0 {
                    end = index + 1;
                    break;
                }
            }
        }
        if sys::write_all(1, &chunk[..end]).is_err() {
            return 1;
        }
    }
    0
}

extern crate alloc;
