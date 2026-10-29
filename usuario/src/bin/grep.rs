//! grep — mostra as linhas que contêm um texto.
//! `-i` ignora maiúsculas, `-v` inverte, `-n` numera, `-c` só conta.

#![no_std]
#![no_main]

use ulib::sys;
use ulib::*;

programa!(principal);

fn principal(args: &[&str]) -> i32 {
    let opcoes: String = args[1..].iter().filter(|a| a.starts_with('-')).map(|a| &a[1..]).collect();
    let resto: Vec<&str> = args[1..].iter().copied().filter(|a| !a.starts_with('-')).collect();
    let Some((&padrao, arquivos)) = resto.split_first() else {
        eprintln!("uso: grep [-ivnc] <texto> [arquivos...]");
        return 2;
    };
    let ignorar_caixa = opcoes.contains('i');
    let inverter = opcoes.contains('v');
    let numerar = opcoes.contains('n');
    let so_contar = opcoes.contains('c');
    let padrao = if ignorar_caixa { padrao.to_lowercase() } else { String::from(padrao) };

    let mut achou = 0;
    let mut buscar = |leitor: &mut Leitor, prefixo: &str| {
        let mut numero = 0;
        while let Ok(Some(linha)) = leitor.ler_linha() {
            numero += 1;
            let alvo = if ignorar_caixa { linha.to_lowercase() } else { linha.clone() };
            if alvo.contains(padrao.as_str()) != inverter {
                achou += 1;
                if !so_contar {
                    let num = if numerar { format!("{}:", numero) } else { String::new() };
                    println!("{}{}{}", prefixo, num, linha);
                }
            }
        }
    };

    if arquivos.is_empty() {
        buscar(&mut Leitor::new(0), "");
    } else {
        for &arquivo in arquivos {
            match sys::abrir(arquivo, sys::O_RDONLY) {
                Ok(fd) => {
                    let prefixo = if arquivos.len() > 1 { format!("{}:", arquivo) } else { String::new() };
                    buscar(&mut Leitor::new(fd), &prefixo);
                    let _ = sys::fechar(fd);
                }
                Err(e) => eprintln!("grep: {}: {}", arquivo, e),
            }
        }
    }
    if so_contar {
        println!("{}", achou);
    }
    if achou > 0 { 0 } else { 1 }
}
