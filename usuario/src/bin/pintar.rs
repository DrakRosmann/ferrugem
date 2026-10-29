//! pintar — um programa de desenho com janela própria (rode dentro do `janelas`).
//!
//! Arraste com o botão esquerdo para desenhar; clique numa cor da paleta
//! para trocar; o botão direito apaga (desenha com o branco, mais grosso).
//! A tecla `c` limpa tudo.
//!
//! Serve de exemplo de cliente do servidor de janelas (veja `ulib::janela`):
//! ele desenha nos pixels da memória compartilhada e só avisa "atualizar".

#![no_std]
#![no_main]

use ulib::janela::{Evento, Janela};
use ulib::*;

programa!(principal);

const LARGURA: i32 = 480;
const ALTURA: i32 = 320;
const PALETA: i32 = 24; // a faixa de cores no alto
const CORES: [u32; 8] = [0x000000, 0xCC3333, 0x33AA33, 0x3366CC, 0xE0A020, 0xAA44AA, 0x2AA1B3, 0xB7410E];

fn limpar(j: &mut Janela, cor: u32) {
    let mut q = j.quadro();
    let (largura_total, altura_total) = (q.largura, q.altura);
    q.retangulo(0, 0, largura_total, altura_total, 0xFFFFFF);
    let largura = largura_total / CORES.len() as i32;
    for (i, &c) in CORES.iter().enumerate() {
        let x = i as i32 * largura;
        q.retangulo(x, 0, largura, PALETA, c);
        if c == cor {
            q.moldura(x + 2, 2, largura - 4, PALETA - 4, 0xFFFFFF);
        }
    }
    q.retangulo(0, PALETA, largura_total, 1, 0x888888);
}

fn principal(_args: &[&str]) -> i32 {
    let mut j = match Janela::abrir(LARGURA, ALTURA, "Pintar") {
        Ok(j) => j,
        Err(e) => {
            eprintln!("pintar: {}", e);
            return 1;
        }
    };
    let mut cor = CORES[0];
    limpar(&mut j, cor);
    j.mostrar();
    // (x, y, cor, espessura) do último ponto, enquanto um botão está apertado
    let mut tracando: Option<(i32, i32, u32, i32)> = None;
    while let Some(evento) = j.evento() {
        match evento {
            Evento::Botao { x, y, botao, apertado: true } => {
                if y < PALETA {
                    let i = (x / (j.largura / CORES.len() as i32)) as usize;
                    cor = CORES[i.min(CORES.len() - 1)];
                    // redesenha só a paleta, com a cor nova marcada
                    let largura = j.largura / CORES.len() as i32;
                    let mut q = j.quadro();
                    for (k, &c) in CORES.iter().enumerate() {
                        let x = k as i32 * largura;
                        q.retangulo(x, 0, largura, PALETA, c);
                        if c == cor {
                            q.moldura(x + 2, 2, largura - 4, PALETA - 4, 0xFFFFFF);
                        }
                    }
                } else {
                    let (c, espessura) = if botao == 2 { (0xFFFFFF, 12) } else { (cor, 3) };
                    j.quadro().linha(x, y, x, y, espessura, c);
                    tracando = Some((x, y, c, espessura));
                }
                j.mostrar();
            }
            Evento::Botao { apertado: false, .. } => tracando = None,
            Evento::Mover { x, y } => {
                if let Some((x0, y0, c, espessura)) = tracando {
                    let y = y.max(PALETA + espessura / 2 + 1);
                    j.quadro().linha(x0, y0, x, y, espessura, c);
                    tracando = Some((x, y, c, espessura));
                    j.mostrar();
                }
            }
            Evento::Tecla('c') => {
                limpar(&mut j, cor);
                j.mostrar();
            }
            Evento::Tamanho { .. } => {
                limpar(&mut j, cor); // o desenho não é guardado: a tela nova começa limpa
                j.mostrar();
            }
            Evento::Tecla(_) | Evento::Roda { .. } | Evento::Seta(_) | Evento::Colar(_) | Evento::Tema { .. } => {}
            Evento::Fechar => break,
        }
    }
    0
}
