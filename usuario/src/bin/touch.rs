//! touch — cria arquivos vazios (se ainda não existirem).

#![no_std]
#![no_main]

use ulib::sys;
use ulib::*;

programa!(principal);

fn principal(args: &[&str]) -> i32 {
    let mut status = 0;
    for &caminho in &args[1..] {
        match sys::abrir(caminho, sys::O_WRONLY | sys::O_CREAT) {
            Ok(fd) => {
                let _ = sys::fechar(fd);
            }
            Err(e) => {
                eprintln!("touch: {}: {}", caminho, e);
                status = 1;
            }
        }
    }
    status
}
