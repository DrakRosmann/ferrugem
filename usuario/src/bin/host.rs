//! host — o endereço IP de um nome (pelo /etc/hosts ou pelo DNS).

#![no_std]
#![no_main]

use ulib::rede;
use ulib::*;

programa!(principal);

fn principal(args: &[&str]) -> i32 {
    if args.len() < 2 {
        eprintln!("uso: host nome...");
        return 2;
    }
    let mut status = 0;
    for nome in &args[1..] {
        match rede::resolver(nome) {
            Ok(ip) => println!("{} tem o endereço {}", nome, rede::texto_ip(ip)),
            Err(e) => {
                eprintln!("host: {}", e);
                status = 1;
            }
        }
    }
    status
}
