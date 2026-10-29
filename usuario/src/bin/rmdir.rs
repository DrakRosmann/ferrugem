//! rmdir — apaga diretórios vazios.

#![no_std]
#![no_main]

use ulib::sys;
use ulib::*;

programa!(principal);

fn principal(args: &[&str]) -> i32 {
    let mut status = 0;
    for &caminho in &args[1..] {
        if let Err(e) = sys::remover_diretorio(caminho) {
            eprintln!("rmdir: {}: {}", caminho, e);
            status = 1;
        }
    }
    status
}
