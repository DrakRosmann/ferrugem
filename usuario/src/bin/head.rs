//! head — mostra as primeiras linhas (10, ou `-n N`).

#![no_std]
#![no_main]

use ulib::sys;
use ulib::*;

programa!(principal);

fn principal(args: &[&str]) -> i32 {
    let mut quantas = 10;
    let mut arquivo = None;
    let mut i = 1;
    while i < args.len() {
        match args[i] {
            "-n" => {
                quantas = args.get(i + 1).and_then(|n| n.parse().ok()).unwrap_or(10);
                i += 1;
            }
            a => arquivo = Some(a),
        }
        i += 1;
    }
    let fd = match arquivo {
        Some(a) => match sys::abrir(a, sys::O_RDONLY) {
            Ok(fd) => fd,
            Err(e) => {
                eprintln!("head: {}: {}", a, e);
                return 1;
            }
        },
        None => 0,
    };
    let mut leitor = Leitor::new(fd);
    for _ in 0..quantas {
        match leitor.ler_linha() {
            Ok(Some(linha)) => println!("{}", linha),
            _ => break,
        }
    }
    0
}
