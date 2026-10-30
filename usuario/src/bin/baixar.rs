//! baixar — baixa um endereço HTTP (como o wget).
//!
//! `baixar URL` grava no arquivo com o nome do fim do endereço (ou
//! index.html); `baixar URL -O arquivo` escolhe o nome; `-O -` escreve na
//! saída. `-i` mostra também os cabeçalhos da resposta.

#![no_std]
#![no_main]

use ulib::http::{self, Url};
use ulib::sys;
use ulib::*;

programa!(principal);

fn principal(args: &[&str]) -> i32 {
    let (mut url, mut saida, mut cabecalhos) = (None, None, false);
    let mut i = 1;
    while i < args.len() {
        match args[i] {
            "-O" => {
                saida = args.get(i + 1).copied();
                i += 1;
            }
            "-i" => cabecalhos = true,
            u => url = Some(u),
        }
        i += 1;
    }
    let Some(texto) = url else {
        eprintln!("uso: baixar [-i] URL [-O arquivo]");
        return 2;
    };
    let texto = if texto.contains("://") { String::from(texto) } else { format!("http://{}", texto) };
    let Some(url) = Url::ler(&texto) else {
        eprintln!("baixar: endereço inválido: {}", texto);
        return 2;
    };
    let para_saida = saida == Some("-");
    if !para_saida {
        eprintln!("baixando {}", url.texto());
    }
    let inicio = sys::tempo_ms();
    let resposta = match http::obter(&url, http::abrir_simples, &mut |_| {}) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("baixar: {}", e);
            return 1;
        }
    };
    if cabecalhos {
        eprintln!("HTTP {}", resposta.status);
        for (n, v) in &resposta.cabecalhos {
            eprintln!("{}: {}", n, v);
        }
    }
    if resposta.status != 200 {
        eprintln!("baixar: o servidor respondeu {}", resposta.status);
        return 1;
    }
    if para_saida {
        let _ = sys::escrever_tudo(1, &resposta.corpo);
        return 0;
    }
    let nome = match saida {
        Some(n) => String::from(n),
        None => {
            let caminho = resposta.url.caminho.split('?').next().unwrap_or("");
            match caminho.rsplit('/').next() {
                Some(n) if !n.is_empty() => String::from(n),
                _ => String::from("index.html"),
            }
        }
    };
    if let Err(e) = sys::escrever_arquivo(&nome, &resposta.corpo) {
        eprintln!("baixar: {}: {}", nome, e);
        return 1;
    }
    let ms = (sys::tempo_ms() - inicio).max(1);
    eprintln!("{} bytes gravados em {} ({} ms, {} KiB/s)", resposta.corpo.len(), nome, ms, resposta.corpo.len() as u64 * 1000 / 1024 / ms);
    0
}
