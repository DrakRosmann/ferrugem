//! cat — concatena arquivos na saída. Sem argumentos (ou com `-`), lê a entrada.

#![no_std]
#![no_main]

use ulib::sys;
use ulib::*;

programa!(principal);

fn copiar(fd: i32) -> Result<(), sys::Erro> {
    let mut buf = [0u8; 4096];
    loop {
        match sys::ler(fd, &mut buf)? {
            0 => return Ok(()),
            n => sys::escrever_tudo(1, &buf[..n])?,
        }
    }
}

fn principal(args: &[&str]) -> i32 {
    let arquivos = if args.len() > 1 { &args[1..] } else { &["-"][..] };
    let mut status = 0;
    for &arquivo in arquivos {
        let resultado = if arquivo == "-" {
            copiar(0)
        } else {
            sys::abrir(arquivo, sys::O_RDONLY).and_then(|fd| {
                let r = copiar(fd);
                let _ = sys::fechar(fd);
                r
            })
        };
        if let Err(e) = resultado {
            eprintln!("cat: {}: {}", arquivo, e);
            status = 1;
        }
    }
    status
}
