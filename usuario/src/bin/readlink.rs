//! readlink — mostra para onde um link simbólico aponta.

#![no_std]
#![no_main]

use ulib::sys;
use ulib::*;

programa!(principal);

fn principal(args: &[&str]) -> i32 {
    let mut status = 0;
    for &caminho in &args[1..] {
        match sys::ler_link(caminho) {
            Ok(alvo) => println!("{}", alvo),
            Err(e) => {
                eprintln!("readlink: {}: {}", caminho, e);
                status = 1;
            }
        }
    }
    status
}
