//! df — espaço no disco montado em /disco.

#![no_std]
#![no_main]

use ulib::sys;
use ulib::*;

programa!(principal);

fn principal(_args: &[&str]) -> i32 {
    let d = sys::disco();
    if d.total_blocos == 0 {
        eprintln!("df: nenhum disco montado");
        return 1;
    }
    let kib = |blocos: u64| blocos * d.tamanho_bloco / 1024;
    let usados = d.total_blocos - d.blocos_livres;
    println!("Sistema   Tamanho     Usado     Livre  Uso%  Montado em");
    println!(
        "ext2    {:>7} KiB {:>5} KiB {:>5} KiB  {:>3}%  /disco",
        kib(d.total_blocos),
        kib(usados),
        kib(d.blocos_livres),
        usados * 100 / d.total_blocos
    );
    0
}
