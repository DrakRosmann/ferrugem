//! imagens — o visualizador de imagens (rode dentro do `janelas`).
//!
//! ```text
//!   imagens [arquivo.png]
//! ```
//!
//! Mostra um PNG (decodificado por `ulib::png`: todos os tipos de cor,
//! entrelaçamento e transparência, sobre um fundo xadrez). As setas (ou os
//! botões) passam para a anterior e a próxima da mesma pasta; "Usar como
//! papel de parede" põe a imagem no fundo da área de trabalho.

#![no_std]
#![no_main]

use ulib::janela::{self, Janela, Seta};
use ulib::png::{self, Imagem};
use ulib::sys;
use ulib::ui::{self, Ui};
use ulib::*;

programa!(principal);

/// Os PNGs da pasta, em ordem alfabética.
fn irmas(caminho: &str) -> Vec<String> {
    let pasta = caminho.rsplit_once('/').map_or(".", |(p, _)| if p.is_empty() { "/" } else { p });
    let mut nomes: Vec<String> = sys::listar_diretorio(pasta)
        .unwrap_or_default()
        .into_iter()
        .filter(|e| e.nome.to_lowercase().ends_with(".png"))
        .map(|e| if pasta == "/" { format!("/{}", e.nome) } else { format!("{}/{}", pasta, e.nome) })
        .collect();
    nomes.sort();
    nomes
}

fn carregar(caminho: &str) -> Result<Imagem, String> {
    let dados = sys::ler_arquivo(caminho).map_err(|e| format!("{}: {}", caminho, e))?;
    png::decodificar(&dados).ok_or_else(|| format!("{}: não é um PNG que eu saiba ler", caminho))
}

fn principal(args: &[&str]) -> i32 {
    let mut caminho = String::from(args.get(1).copied().unwrap_or("/usr/share/imagens/mandelbrot.png"));
    let mut imagem = carregar(&caminho);
    // A janela do tamanho da imagem (sem passar de 900 × 600).
    let (iw, ih) = imagem.as_ref().map_or((480, 320), |i| (i.largura as i32, i.altura as i32));
    let escala = (900.0f32 / iw as f32).min(600.0 / ih as f32).min(1.0);
    let (jw, jh) = (((iw as f32 * escala) as i32 + 24).max(460), (ih as f32 * escala) as i32 + 24 + 34 + 34 + 16);
    let mut janela = match Janela::abrir(jw, jh, "Imagens") {
        Ok(j) => j,
        Err(e) => {
            eprintln!("imagens: {}", e);
            return 1;
        }
    };
    let mut ui = Ui::novo();
    let mut tamanho_real = 0usize; // 0 = ajustar à janela, 1 = tamanho real
    let mut mensagem = String::new();
    let mut titulo_mostrado = String::new();

    ui::laco(&mut janela, &mut ui, |t| {
        let lista = irmas(&caminho);
        let posicao = lista.iter().position(|c| *c == caminho);
        let mut ir: i32 = 0;
        t.linha(|t| {
            if t.botao("◀ Anterior") {
                ir = -1;
            }
            if t.botao("Próxima ▶") {
                ir = 1;
            }
            t.opcao("Ajustar", &mut tamanho_real, 0);
            t.opcao("Tamanho real", &mut tamanho_real, 1);
            if t.botao("Usar como papel de parede") && imagem.is_ok() {
                mensagem = match janela::mudar_ajuste("fundo", &caminho) {
                    Ok(()) => {
                        t.pedir_recarga();
                        String::from("Papel de parede trocado")
                    }
                    Err(e) => format!("Não consegui: {}", e),
                };
            }
        });
        if t.seta(Seta::Esquerda) {
            ir = -1;
        }
        if t.seta(Seta::Direita) {
            ir = 1;
        }
        if let (Some(p), true) = (posicao, ir != 0 && !lista.is_empty()) {
            let n = lista.len() as i32;
            caminho = lista[((p as i32 + ir + n) % n) as usize].clone();
            imagem = carregar(&caminho);
            mensagem.clear();
        }
        let posicao = lista.iter().position(|c| *c == caminho);

        // A imagem, centrada, sobre um xadrez (que aparece onde ela é transparente).
        let (w, h) = (t.largura_livre(), t.altura_livre() - 34);
        let (x, y) = t.reservar(w, h);
        let escuro = t.tema().fundo < 0x808080;
        let (c1, c2) = if escuro { (0x2A2A2A, 0x333333) } else { (0xDDDDDD, 0xF4F4F4) };
        let status = match &imagem {
            Ok(img) => {
                let (iw, ih) = (img.largura as i32, img.altura as i32);
                let escala = if tamanho_real == 1 { 1.0 } else { (w as f32 / iw as f32).min(h as f32 / ih as f32).min(1.0) };
                let (dw, dh) = (((iw as f32 * escala) as i32).max(1), ((ih as f32 * escala) as i32).max(1));
                let (dx, dy) = (x + (w - dw) / 2, y + (h - dh) / 2);
                let antes = t.q.recorte;
                t.q.recortar(x, y, w, h);
                for cy in (dy.max(y)..(dy + dh).min(y + h)).step_by(8) {
                    for cx in (dx.max(x)..(dx + dw).min(x + w)).step_by(8) {
                        let cor = if ((cx - dx) / 8 + (cy - dy) / 8) % 2 == 0 { c1 } else { c2 };
                        t.q.retangulo(cx, cy, 8.min(dx + dw - cx), 8.min(dy + dh - cy), cor);
                    }
                }
                t.q.desenhar_imagem(&img.pixels, iw, ih, dx, dy, dw, dh);
                t.q.recorte = antes;
                let nome = caminho.rsplit('/').next().unwrap_or(&caminho);
                let lugar = posicao.map_or(String::new(), |p| format!(" · {} de {}", p + 1, lista.len()));
                format!("{} — {} × {} — {}%{}", nome, iw, ih, (escala * 100.0) as i32, lugar)
            }
            Err(e) => e.clone(),
        };
        t.rotulo(if mensagem.is_empty() { &status } else { &mensagem });
        let nome = caminho.rsplit('/').next().unwrap_or(&caminho);
        let titulo = format!("{} — Imagens", nome);
        if titulo != titulo_mostrado {
            t.mudar_titulo(&titulo);
            titulo_mostrado = titulo;
        }
    });
    0
}
