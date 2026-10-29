//! umount — desmonta (gravando antes o que estiver pendente no cache).

#![no_std]
#![no_main]

use ulib::sys;
use ulib::*;

programa!(principal);

fn principal(args: &[&str]) -> i32 {
    let mut status = 0;
    for &alvo in &args[1..] {
        if let Err(e) = sys::desmontar(alvo) {
            eprintln!("umount: {}: {}", alvo, e);
            status = 1;
        }
    }
    status
}
