//! rm — apaga arquivos. `-r`: apaga diretórios com tudo dentro.

#![no_std]
#![no_main]

use ulib::sys;
use ulib::*;

programa!(principal);

fn apagar(caminho: &str, recursivo: bool) -> Result<(), sys::Erro> {
    let info = sys::stat(caminho)?;
    if !info.eh_diretorio() {
        return sys::remover(caminho);
    }
    if !recursivo {
        return Err(sys::Erro(21)); // é um diretório
    }
    for entrada in sys::listar_diretorio(caminho)? {
        apagar(&format!("{}/{}", caminho, entrada.nome), true)?;
    }
    sys::remover_diretorio(caminho)
}

fn principal(args: &[&str]) -> i32 {
    let recursivo = args.iter().any(|a| *a == "-r" || *a == "-rf");
    let mut status = 0;
    for &caminho in args[1..].iter().filter(|a| !a.starts_with('-')) {
        if let Err(e) = apagar(caminho, recursivo) {
            eprintln!("rm: {}: {}", caminho, e);
            status = 1;
        }
    }
    status
}
