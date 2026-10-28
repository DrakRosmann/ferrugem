//! mkdir diretório...: cria diretórios.

#![no_std]
#![no_main]

use ferrugem_user::{entry, eprintln, rt, sys};

entry!(main);

fn main() -> i32 {
    let mut status = 0;
    for path in rt::args().skip(1) {
        if let Err(errno) = sys::mkdir(path) {
            eprintln!("mkdir: {path}: {}", sys::strerror(errno));
            status = 1;
        }
    }
    status
}
