//! fontes — mostra as fontes TrueType do sistema em vários tamanhos (rode dentro do `janelas`).
//!
//! Exemplo de programa com janela que escreve com `ulib::truetype`: as
//! letras são desenhadas a partir dos contornos da fonte, com as bordas
//! suavizadas pela cobertura de cada pixel.

#![no_std]
#![no_main]

use ulib::janela::{Evento, Janela};
use ulib::truetype::FonteTT;
use ulib::*;

programa!(principal);

const FRASE: &str = "Ação, coração, pão — à vista, ÓRGÃO, PÊSSEGO";

fn principal(_args: &[&str]) -> i32 {
    let arquivos = ["DejaVuSans.ttf", "DejaVuSans-Bold.ttf", "LiberationMono-Regular.ttf"];
    let fontes: Vec<(&str, FonteTT)> = arquivos
        .iter()
        .filter_map(|n| FonteTT::carregar(&format!("/usr/share/fonts/{}", n)).map(|f| (*n, f)))
        .collect();
    if fontes.is_empty() {
        eprintln!("fontes: nenhuma fonte em /usr/share/fonts");
        return 1;
    }
    let mut j = match Janela::abrir(620, 440, "Fontes") {
        Ok(j) => j,
        Err(e) => {
            eprintln!("fontes: {}", e);
            return 1;
        }
    };
    let mut fontes = fontes;
    desenhar(&mut j, &mut fontes);
    while let Some(evento) = j.evento() {
        match evento {
            Evento::Fechar => break,
            Evento::Tamanho { .. } => desenhar(&mut j, &mut fontes),
            _ => {}
        }
    }
    0
}

fn desenhar(j: &mut Janela, fontes: &mut [(&str, FonteTT)]) {
    {
        let mut q = j.quadro();
        let (l, a) = (q.largura, q.altura);
        q.retangulo(0, 0, l, a, 0xFFFFFF);
        let mut y = 10;
        for (nome, f) in fontes.iter_mut() {
            q.texto_tt(f, 12, y, nome, 11, 0x888888);
            y += 16;
            for tamanho in [10, 14, 20] {
                let (_, linha) = f.metricas(tamanho);
                q.texto_tt(f, 12, y, FRASE, tamanho, 0x202020);
                y += linha + 2;
            }
            y += 8;
        }
        let (nome, f) = &mut fontes[0];
        let _ = nome;
        q.texto_tt(f, 12, y, "FerrugemOS", 48, 0xB7410E);
    }
    j.mostrar();
}
