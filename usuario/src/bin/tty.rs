//! tty — mostra o nome do terminal ligado à entrada padrão.
//!
//! ```text
//!   $ tty
//!   /dev/tty1
//! ```

#![no_std]
#![no_main]

use ulib::io;
use ulib::sys;
use ulib::*;

programa!(principal);

fn principal(_args: &[&str]) -> i32 {
    match sys::nome_do_terminal(io::ENTRADA) {
        Some(nome) => {
            println!("{}", nome);
            0
        }
        None => {
            println!("não é um terminal");
            1
        }
    }
}
