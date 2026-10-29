//! uname — nome do sistema. `-a`: tudo.

#![no_std]
#![no_main]

use ulib::sys;
use ulib::*;

programa!(principal);

fn principal(args: &[&str]) -> i32 {
    let u = sys::uname();
    if args.contains(&"-a") {
        println!("{} {} {} {} {}", u.sistema, u.maquina, u.versao, u.compilacao, u.arquitetura);
    } else {
        println!("{}", u.sistema);
    }
    0
}
