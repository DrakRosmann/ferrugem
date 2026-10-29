//! mkfifo — cria um FIFO (pipe com nome): um processo escreve nele, outro lê.
//! Ex.: `mkfifo /tmp/canal; cat /tmp/canal &  echo oi > /tmp/canal`

#![no_std]
#![no_main]

use ulib::sys;
use ulib::*;

programa!(principal);

fn principal(args: &[&str]) -> i32 {
    let mut status = 0;
    for &caminho in &args[1..] {
        if let Err(e) = sys::criar_fifo(caminho, 0o666) {
            eprintln!("mkfifo: {}: {}", caminho, e);
            status = 1;
        }
    }
    status
}
