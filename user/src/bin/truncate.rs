//! truncate -s TAMANHO arquivo...: corta ou estende arquivos (criando os
//! que não existem). O tamanho aceita os sufixos K e M.

#![no_std]
#![no_main]

extern crate alloc;

use alloc::vec::Vec;
use ferrugem_user::{entry, eprintln, rt, sys};

entry!(main);

fn parse_size(text: &str) -> Option<u64> {
    let (number, unit) = match text.as_bytes().last()? {
        b'K' | b'k' => (&text[..text.len() - 1], 1024),
        b'M' | b'm' => (&text[..text.len() - 1], 1024 * 1024),
        _ => (text, 1),
    };
    number.parse::<u64>().ok()?.checked_mul(unit)
}

fn main() -> i32 {
    let args: Vec<&str> = rt::args().skip(1).collect();
    let (Some(&"-s"), Some(size)) = (args.first(), args.get(1).and_then(|s| parse_size(s))) else {
        eprintln!("uso: truncate -s TAMANHO arquivo...");
        return 2;
    };
    let mut status = 0;
    for path in &args[2..] {
        let result = sys::open(path, sys::O_WRONLY | sys::O_CREAT, 0o644)
            .and_then(sys::close)
            .and_then(|()| sys::truncate(path, size));
        if let Err(errno) = result {
            eprintln!("truncate: {path}: {}", sys::strerror(errno));
            status = 1;
        }
    }
    status
}
