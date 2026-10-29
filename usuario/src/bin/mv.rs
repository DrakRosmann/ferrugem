//! mv — move ou renomeia. Se o destino for um diretório, move para dentro dele.

#![no_std]
#![no_main]

use ulib::sys;
use ulib::*;

programa!(principal);

fn principal(args: &[&str]) -> i32 {
    if args.len() != 3 {
        eprintln!("uso: mv <origem> <destino>");
        return 2;
    }
    let destino = match sys::stat(args[2]) {
        Ok(info) if info.eh_diretorio() => {
            let nome = args[1].rsplit('/').next().unwrap_or(args[1]);
            format!("{}/{}", args[2].trim_end_matches('/'), nome)
        }
        _ => String::from(args[2]),
    };
    let resultado = match sys::renomear(args[1], &destino) {
        // Entre a memória e o disco não dá para só trocar o nome: copia e apaga,
        // como o mv do Unix faz entre partições (só para arquivos, aqui).
        Err(e) if e == sys::EXDEV => sys::ler_arquivo(args[1]).and_then(|dados| {
            let fd = sys::abrir(&destino, sys::O_WRONLY | sys::O_CREAT | sys::O_TRUNC)?;
            let r = sys::escrever_tudo(fd, &dados);
            let _ = sys::fechar(fd);
            r.and_then(|_| sys::remover(args[1]))
        }),
        outro => outro,
    };
    match resultado {
        Ok(()) => 0,
        Err(e) => {
            eprintln!("mv: {}", e);
            1
        }
    }
}
