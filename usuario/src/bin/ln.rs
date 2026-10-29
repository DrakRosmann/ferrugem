//! ln — cria links. `ln -s alvo nome`: link simbólico (guarda o caminho);
//! `ln existente nome`: link físico (mais um nome para o mesmo arquivo).
//! Se `nome` for um diretório, o link é criado dentro dele.

#![no_std]
#![no_main]

use ulib::sys;
use ulib::*;

programa!(principal);

fn principal(args: &[&str]) -> i32 {
    let simbolico = args.contains(&"-s");
    let resto: Vec<&str> = args[1..].iter().copied().filter(|a| *a != "-s").collect();
    let [alvo, nome] = resto[..] else {
        eprintln!("uso: ln [-s] <alvo> <nome>");
        return 2;
    };
    let nome = match sys::stat(nome) {
        Ok(info) if info.eh_diretorio() => {
            format!("{}/{}", nome.trim_end_matches('/'), alvo.trim_end_matches('/').rsplit('/').next().unwrap_or(alvo))
        }
        _ => String::from(nome),
    };
    let resultado = if simbolico { sys::criar_link_simbolico(alvo, &nome) } else { sys::criar_link(alvo, &nome) };
    match resultado {
        Ok(()) => 0,
        Err(e) => {
            eprintln!("ln: {}: {}", nome, e);
            1
        }
    }
}
