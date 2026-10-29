//! sleep — espera N segundos (aceita frações: `sleep 0.5`).

#![no_std]
#![no_main]

use ulib::sys;
use ulib::*;

programa!(principal);

fn principal(args: &[&str]) -> i32 {
    let Some(texto) = args.get(1) else {
        eprintln!("uso: sleep <segundos>");
        return 2;
    };
    let (inteira, fracao) = texto.split_once('.').unwrap_or((texto, ""));
    let segundos: u64 = inteira.parse().unwrap_or(0);
    let mut milis: u64 = format!("{:0<3}", &fracao[..fracao.len().min(3)]).parse().unwrap_or(0);
    milis += segundos * 1000;
    match sys::dormir_ms(milis) {
        Ok(()) => 0,
        Err(_) => 1,
    }
}
