//! wc — conta linhas, palavras e bytes. `-l`, `-w`, `-c` mostram só um deles.

#![no_std]
#![no_main]

use ulib::sys;
use ulib::*;

programa!(principal);

fn contar(dados: &[u8]) -> [usize; 3] {
    let linhas = dados.iter().filter(|&&b| b == b'\n').count();
    let palavras = dados.split(|b| b.is_ascii_whitespace()).filter(|p| !p.is_empty()).count();
    [linhas, palavras, dados.len()]
}

fn ler_entrada() -> Vec<u8> {
    let mut dados = Vec::new();
    let mut buf = [0u8; 4096];
    while let Ok(n) = sys::ler(0, &mut buf) {
        if n == 0 {
            break;
        }
        dados.extend_from_slice(&buf[..n]);
    }
    dados
}

fn principal(args: &[&str]) -> i32 {
    let opcoes: Vec<&str> = args[1..].iter().copied().filter(|a| a.starts_with('-')).collect();
    let arquivos: Vec<&str> = args[1..].iter().copied().filter(|a| !a.starts_with('-')).collect();
    let mostrar = [opcoes.contains(&"-l"), opcoes.contains(&"-w"), opcoes.contains(&"-c")];
    let todos = !mostrar.iter().any(|&m| m);

    let imprimir = |contagem: [usize; 3], nome: &str| {
        let mut linha = String::new();
        for i in 0..3 {
            if todos || mostrar[i] {
                linha.push_str(&format!("{:>7}", contagem[i]));
            }
        }
        if nome.is_empty() {
            println!("{}", linha);
        } else {
            println!("{} {}", linha, nome);
        }
    };

    if arquivos.is_empty() {
        imprimir(contar(&ler_entrada()), "");
        return 0;
    }
    let mut status = 0;
    for arquivo in arquivos {
        match sys::ler_arquivo(arquivo) {
            Ok(dados) => imprimir(contar(&dados), arquivo),
            Err(e) => {
                eprintln!("wc: {}: {}", arquivo, e);
                status = 1;
            }
        }
    }
    status
}
