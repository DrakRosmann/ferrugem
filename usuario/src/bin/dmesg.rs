//! dmesg — mostra as mensagens do kernel (as do boot, inclusive).
//!
//! O kernel guarda o que imprime num buffer circular; esta é a chamada
//! `syslog(SYSLOG_ACTION_READ_ALL)` do Linux, a mesma que o dmesg de lá usa.

#![no_std]
#![no_main]

use ulib::sys;
use ulib::*;

programa!(principal);

fn principal(_args: &[&str]) -> i32 {
    match sys::registro_do_kernel() {
        Ok(texto) => {
            let _ = sys::escrever_tudo(io::SAIDA, &texto);
            0
        }
        Err(e) => {
            eprintln!("dmesg: {}", e);
            1
        }
    }
}
