//! whoami — o nome do usuário efetivo.

#![no_std]
#![no_main]

use ulib::sys;
use ulib::usuarios::nome_do_usuario;
use ulib::*;

programa!(principal);

fn principal(_args: &[&str]) -> i32 {
    println!("{}", nome_do_usuario(sys::euid()));
    0
}
