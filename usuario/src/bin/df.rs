//! df — espaço em cada sistema de arquivos montado.

#![no_std]
#![no_main]

use ulib::sys;
use ulib::*;

programa!(principal);

fn principal(_args: &[&str]) -> i32 {
    let Ok(dados) = sys::ler_arquivo("/proc/mounts") else {
        eprintln!("df: não consegui ler /proc/mounts");
        return 1;
    };
    println!("Sistema       Tipo     Tamanho       Usado       Livre  Uso%  Montado em");
    for linha in String::from_utf8_lossy(&dados).lines() {
        let c: Vec<&str> = linha.split(' ').collect();
        if c.len() < 3 {
            continue;
        }
        let Ok(info) = sys::statfs(c[1]) else { continue };
        if info.blocos == 0 {
            continue; // /proc e companhia: não ocupam espaço
        }
        let kib = |b: u64| b * info.tamanho_bloco / 1024;
        let usados = info.blocos - info.livres;
        println!(
            "{:<12}  {:<6} {:>8} KiB {:>7} KiB {:>7} KiB  {:>3}%  {}",
            c[0],
            c[2],
            kib(info.blocos),
            kib(usados),
            kib(info.livres),
            usados * 100 / info.blocos,
            c[1]
        );
    }
    0
}
