//! cp — copia um arquivo. Se o destino for um diretório, copia para dentro dele.

#![no_std]
#![no_main]

use ulib::sys;
use ulib::*;

programa!(principal);

pub fn destino_final(origem: &str, destino: &str) -> String {
    match sys::stat(destino) {
        Ok(info) if info.eh_diretorio() => {
            let nome = origem.rsplit('/').next().unwrap_or(origem);
            format!("{}/{}", destino.trim_end_matches('/'), nome)
        }
        _ => String::from(destino),
    }
}

fn principal(args: &[&str]) -> i32 {
    if args.len() != 3 {
        eprintln!("uso: cp <origem> <destino>");
        return 2;
    }
    let destino = destino_final(args[1], args[2]);
    let resultado = sys::ler_arquivo(args[1]).and_then(|dados| {
        let fd = sys::abrir(&destino, sys::O_WRONLY | sys::O_CREAT | sys::O_TRUNC)?;
        let r = sys::escrever_tudo(fd, &dados);
        let _ = sys::fechar(fd);
        r
    });
    match resultado {
        Ok(()) => 0,
        Err(e) => {
            eprintln!("cp: {}", e);
            1
        }
    }
}
