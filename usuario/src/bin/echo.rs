//! echo — escreve os argumentos na saída. `-n`: sem quebra de linha no fim.

#![no_std]
#![no_main]

use ulib::*;

programa!(principal);

fn principal(args: &[&str]) -> i32 {
    let (sem_quebra, palavras) = match args.get(1) {
        Some(&"-n") => (true, &args[2..]),
        _ => (false, args.get(1..).unwrap_or(&[])),
    };
    let mut texto = palavras.join(" ");
    if !sem_quebra {
        texto.push('\n');
    }
    print!("{}", texto);
    0
}
