//! configuracoes — as configurações da interface gráfica (rode dentro do `janelas`).
//!
//! Papel de parede, estilo claro ou escuro, cor de destaque, brilho, luz
//! noturna, relógio com segundos e cursor grande. Cada mudança vai para
//! ~/.janelas (os mesmos ajustes do painel de controle) e vale na hora: o
//! servidor relê o arquivo e avisa todos os programas.

#![no_std]
#![no_main]

use ulib::janela::{self, Janela};
use ulib::png;
use ulib::sys;
use ulib::ui::{self, Ui};
use ulib::grafico::Quadro;
use ulib::*;

programa!(principal);

const CORES: [(&str, u32); 6] = [
    ("Ferrugem", 0xB7410E),
    ("Vermelho", 0xD7263D),
    ("Azul", 0x2E6FD8),
    ("Verde", 0x2E9E5B),
    ("Roxo", 0x8E44AD),
    ("Laranja", 0xE67E22),
];
const MINIATURA: (i32, i32) = (128, 80);

/// Os ajustes atuais (lidos de ~/.janelas).
fn ler_ajustes() -> Vec<(String, String)> {
    let texto = sys::ler_arquivo(&format!("{}/.janelas", variavel("HOME").unwrap_or("/"))).unwrap_or_default();
    String::from_utf8_lossy(&texto)
        .lines()
        .filter_map(|l| l.split_once('=').map(|(c, v)| (String::from(c.trim()), String::from(v.trim()))))
        .collect()
}

/// Uma miniatura do papel de parede (cobrindo o quadrinho, sem deformar).
fn miniatura(caminho: &str) -> Option<Vec<u32>> {
    let img = png::decodificar(&sys::ler_arquivo(caminho).ok()?)?;
    let (l, a) = MINIATURA;
    let (iw, ih) = (img.largura as i32, img.altura as i32);
    let escala = (l as f32 / iw as f32).max(a as f32 / ih as f32);
    let (w, h) = ((iw as f32 * escala) as i32 + 1, (ih as f32 * escala) as i32 + 1);
    let mut pixels = vec![0u32; (l * a) as usize];
    Quadro::novo(&mut pixels, l, a).desenhar_imagem(&img.pixels, iw, ih, (l - w) / 2, (a - h) / 2, w, h);
    Some(pixels)
}

fn principal(_args: &[&str]) -> i32 {
    let mut janela = match Janela::abrir(640, 600, "Configurações") {
        Ok(j) => j,
        Err(e) => {
            eprintln!("configuracoes: {}", e);
            return 1;
        }
    };
    let mut ui = Ui::novo();
    let ajustes = ler_ajustes();
    let valor = |chave: &str| ajustes.iter().find(|(c, _)| c == chave).map(|(_, v)| v.clone());
    let mut fundo = valor("fundo").unwrap_or_default();
    // 0 = claro, 1 = escuro (o índice das opções)
    let mut escuro: usize = (valor("escuro").as_deref() == Some("1")) as usize;
    let mut destaque = valor("destaque").and_then(|c| u32::from_str_radix(c.trim_start_matches('#'), 16).ok()).unwrap_or(0xB7410E);
    let mut brilho = valor("brilho").and_then(|b| b.parse().ok()).unwrap_or(100);
    let (mut noturna, mut segundos, mut cursor_grande) =
        (valor("noturna").as_deref() == Some("1"), valor("segundos").as_deref() == Some("1"), valor("cursor_grande").as_deref() == Some("1"));
    let papeis: Vec<(String, Option<Vec<u32>>)> = {
        let mut nomes: Vec<String> = sys::listar_diretorio("/usr/share/imagens")
            .unwrap_or_default()
            .into_iter()
            .filter(|e| e.nome.starts_with("papel-") && e.nome.ends_with(".png"))
            .map(|e| format!("/usr/share/imagens/{}", e.nome))
            .collect();
        nomes.sort();
        nomes.into_iter().map(|c| { let m = miniatura(&c); (c, m) }).collect()
    };
    let uname = sys::uname();
    let cpus = String::from_utf8_lossy(&sys::ler_arquivo("/proc/cpuinfo").unwrap_or_default()).matches("processor").count();
    let memoria = String::from_utf8_lossy(&sys::ler_arquivo("/proc/meminfo").unwrap_or_default())
        .lines()
        .find(|l| l.starts_with("MemTotal:"))
        .and_then(|l| l.split_whitespace().nth(1)?.parse::<u64>().ok())
        .unwrap_or(0);

    ui::laco(&mut janela, &mut ui, |t| {
        // Muda um ajuste e avisa o servidor (que avisa os outros programas).
        let mudar = |t: &mut ui::Tela, chave: &str, valor: String| {
            if janela::mudar_ajuste(chave, &valor).is_ok() {
                t.pedir_recarga();
            }
        };
        let tema = t.tema();
        t.titulo("Aparência");
        t.rotulo("Papel de parede");
        t.linha(|t| {
            // A cor sólida (sem papel) e as miniaturas.
            let (l, a) = MINIATURA;
            let ((x, y), sobre, clicado) = t.clicavel(l, a);
            t.q.retangulo(x, y, l, a, 0x1F4E5F);
            t.escrever(x + 18, y + a / 2 - 8, "Cor sólida", 0xECEFF4, true, 12);
            let escolhido = fundo.is_empty();
            if escolhido || sobre {
                let cor = if escolhido { tema.destaque } else { tema.borda };
                t.q.moldura(x - 2, y - 2, l + 4, a + 4, cor);
                t.q.moldura(x - 3, y - 3, l + 6, a + 6, cor);
            }
            if clicado {
                fundo.clear();
                mudar(t, "fundo", String::new());
            }
            for (caminho, pixels) in &papeis {
                let ((x, y), sobre, clicado) = t.clicavel(l, a);
                match pixels {
                    Some(p) => t.q.copiar(p, l, a, x, y),
                    None => t.q.retangulo(x, y, l, a, tema.painel),
                }
                let escolhido = fundo == *caminho;
                if escolhido || sobre {
                    let cor = if escolhido { tema.destaque } else { tema.borda };
                    t.q.moldura(x - 2, y - 2, l + 4, a + 4, cor);
                    t.q.moldura(x - 3, y - 3, l + 6, a + 6, cor);
                }
                if clicado {
                    fundo = caminho.clone();
                    mudar(t, "fundo", caminho.clone());
                }
            }
        });
        t.linha(|t| {
            t.rotulo("Estilo:");
            let mudou = t.opcao("Claro", &mut escuro, 0) | t.opcao("Escuro", &mut escuro, 1);
            if mudou {
                mudar(t, "escuro", format!("{}", escuro));
            }
        });
        t.linha(|t| {
            t.rotulo("Cor de destaque:");
            for (nome, cor) in CORES {
                let ((x, y), sobre, clicado) = t.clicavel(28, 28);
                if destaque == cor {
                    t.q.circulo_suave(x * 2 + 28, y * 2 + 28, 28, tema.texto);
                }
                t.q.circulo_suave(x * 2 + 28, y * 2 + 28, if sobre || destaque == cor { 24 } else { 22 }, cor);
                if clicado {
                    destaque = cor;
                    mudar(t, "destaque", format!("#{:06X}", cor));
                }
                let _ = nome;
            }
            let nome = CORES.iter().find(|(_, c)| *c == destaque).map_or("personalizada", |(n, _)| *n);
            t.rotulo(nome);
        });
        t.separador();
        t.titulo("Tela");
        t.linha(|t| {
            t.rotulo("Brilho:");
            if t.deslizante(&mut brilho, 30, 100, 260) {
                mudar(t, "brilho", format!("{}", brilho));
            }
            t.rotulo(&format!("{}%", brilho));
        });
        t.linha(|t| {
            if t.caixa("Luz noturna", &mut noturna) {
                mudar(t, "noturna", format!("{}", noturna as u8));
            }
            if t.caixa("Segundos no relógio", &mut segundos) {
                mudar(t, "segundos", format!("{}", segundos as u8));
            }
            if t.caixa("Cursor grande", &mut cursor_grande) {
                mudar(t, "cursor_grande", format!("{}", cursor_grande as u8));
            }
        });
        t.separador();
        t.titulo("Sobre este sistema");
        t.paragrafo(
            &format!(
                "{} {} ({}) — um sistema tipo Unix escrito do zero em Rust. {} CPUs, {} MiB de memória. Os ajustes desta janela ficam em ~/.janelas.",
                uname.sistema,
                uname.versao,
                uname.arquitetura,
                cpus,
                memoria / 1024
            ),
            t.largura_livre(),
        );
    });
    0
}
