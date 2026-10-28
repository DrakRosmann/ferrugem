//! ln [-s] alvo nome: cria um link físico (ou simbólico, com -s).

#![no_std]
#![no_main]

extern crate alloc;

use alloc::vec::Vec;
use ferrugem_user::{entry, eprintln, rt, sys};

entry!(main);

fn main() -> i32 {
    let mut args: Vec<&str> = rt::args().skip(1).collect();
    let symbolic = args.first() == Some(&"-s");
    if symbolic {
        args.remove(0);
    }
    let [target, name] = args[..] else {
        eprintln!("uso: ln [-s] alvo nome");
        return 2;
    };
    let result = if symbolic { sys::symlink(target, name) } else { sys::link(target, name) };
    match result {
        Ok(()) => 0,
        Err(errno) => {
            eprintln!("ln: {name}: {}", sys::strerror(errno));
            1
        }
    }
}
