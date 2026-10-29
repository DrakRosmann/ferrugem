//! ldd — mostra as bibliotecas compartilhadas de que um programa precisa.
//!
//! ```text
//!   $ ldd /bin/dinamico
//!   	carregador: /lib/ld-ferrugem.so
//!   	libsaudacao.so => /lib/libsaudacao.so (0x...)
//! ```
//!
//! Lê o PT_INTERP do executável (o carregador dinâmico) e roda o programa
//! com LD_TRACE_LOADED_OBJECTS=1: o carregador então só lista o que
//! carregou e sai, sem executar o programa.

#![no_std]
#![no_main]

use ulib::sys;
use ulib::*;

programa!(principal);

/// O caminho do PT_INTERP, se houver.
fn interpretador(dados: &[u8]) -> Option<String> {
    if dados.get(0..4) != Some(b"\x7fELF") || dados.len() < 64 {
        return None;
    }
    let u16_em = |o: usize| u16::from_le_bytes([dados[o], dados[o + 1]]) as usize;
    let u64_em = |o: usize| -> Option<u64> { Some(u64::from_le_bytes(dados.get(o..o + 8)?.try_into().ok()?)) };
    let tabela = u64_em(32)? as usize;
    for i in 0..u16_em(56) {
        let o = tabela + i * u16_em(54);
        if dados.get(o..o + 4)? == [3, 0, 0, 0] {
            let inicio = u64_em(o + 8)? as usize;
            let tamanho = u64_em(o + 32)? as usize;
            let texto = dados.get(inicio..inicio + tamanho)?;
            return Some(String::from_utf8_lossy(texto.split(|&b| b == 0).next()?).into_owned());
        }
    }
    None
}

fn principal(args: &[&str]) -> i32 {
    let Some(&caminho) = args.get(1) else {
        eprintln!("uso: ldd programa");
        return 1;
    };
    let caminho = if caminho.contains('/') { String::from(caminho) } else { format!("/bin/{}", caminho) };
    let dados = match sys::ler_arquivo(&caminho) {
        Ok(d) => d,
        Err(e) => {
            eprintln!("ldd: {}: {}", caminho, e);
            return 1;
        }
    };
    let Some(carregador) = interpretador(&dados) else {
        println!("\tnão é dinâmico (ligado estaticamente)");
        return 0;
    };
    println!("\tcarregador: {}", carregador);
    let mut ambiente: Vec<&str> = ambiente().to_vec();
    ambiente.push("LD_TRACE_LOADED_OBJECTS=1");
    match sys::fork() {
        Ok(0) => {
            let erro = sys::executar_com_ambiente(&caminho, &[&caminho], &ambiente);
            eprintln!("ldd: {}: {}", caminho, erro);
            sys::sair(127);
        }
        Ok(pid) => match sys::esperar(pid) {
            Ok((_, status)) => sys::codigo_de_saida(status).unwrap_or(1),
            Err(_) => 1,
        },
        Err(e) => {
            eprintln!("ldd: {}", e);
            1
        }
    }
}
