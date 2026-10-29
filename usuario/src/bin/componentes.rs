//! componentes — uma vitrine da biblioteca `ulib::ui` (rode dentro do `janelas`).
//!
//! Menus, campo de texto, botões, caixas de seleção, opções, deslizante,
//! barra de progresso, uma lista com rolagem e caixas de diálogo. Também
//! serve de exemplo: veja como pouco código monta uma janela.

#![no_std]
#![no_main]

use ulib::janela::Janela;
use ulib::ui::{self, CampoTexto, Dialogo, Lista, Ui};
use ulib::*;

programa!(principal);

const PLANETAS: [&str; 12] = [
    "Mercúrio", "Vênus", "Terra", "Marte", "Júpiter", "Saturno", "Urano", "Netuno", "Plutão (anão)", "Ceres (anão)",
    "Éris (anão)", "Haumea (anão)",
];

fn principal(_args: &[&str]) -> i32 {
    let mut janela = match Janela::abrir(560, 470, "Componentes") {
        Ok(j) => j,
        Err(e) => {
            eprintln!("componentes: {}", e);
            return 1;
        }
    };
    let mut ui = Ui::novo();
    let mut nome = CampoTexto::novo("");
    let mut saudacao = String::from("Digite seu nome e aperte Enter.");
    let (mut negrito, mut avisos) = (false, true);
    let mut tamanho = 1usize;
    let mut volume = 40;
    let mut lista = Lista::default();
    let mut dialogo: Option<Dialogo> = None;

    ui::laco(&mut janela, &mut ui, |t| {
        match t.barra_de_menus(&[("Arquivo", &["Limpar", "Sair"]), ("Ajuda", &["Sobre"])]) {
            Some((0, 0)) => {
                nome = CampoTexto::novo("");
                saudacao = String::from("Tudo limpo.");
            }
            Some((0, 1)) => t.fechar_janela(),
            Some((1, 0)) => {
                dialogo = Some(Dialogo::novo(
                    "Sobre",
                    "Componentes do FerrugemOS: uma biblioteca de interface em modo imediato, escrita do zero em Rust.",
                    &["Fechar"],
                ))
            }
            _ => {}
        }
        t.titulo("Olá!");
        let mut cumprimentar = false;
        t.linha(|t| {
            t.rotulo("Nome:");
            cumprimentar |= t.campo(&mut nome, 220);
            cumprimentar |= t.botao("Cumprimentar");
        });
        if cumprimentar {
            saudacao = if nome.texto.is_empty() {
                String::from("Sem nome? Tudo bem.")
            } else {
                format!("Olá, {}! Seja bem-vindo ao FerrugemOS.", nome.texto)
            };
        }
        t.rotulo(&saudacao);
        t.separador();
        t.linha(|t| {
            t.caixa("Negrito", &mut negrito);
            t.caixa("Avisos", &mut avisos);
            t.espaco(20);
            t.opcao("Pequeno", &mut tamanho, 0);
            t.opcao("Médio", &mut tamanho, 1);
            t.opcao("Grande", &mut tamanho, 2);
        });
        t.linha(|t| {
            t.rotulo("Volume:");
            t.deslizante(&mut volume, 0, 100, 200);
            t.rotulo(&format!("{}%", volume));
        });
        t.progresso(volume as f32 / 100.0, 300);
        t.separador();
        t.linha(|t| {
            if let Some(i) = t.lista(&mut lista, &PLANETAS, 220, 150) {
                if avisos {
                    dialogo = Some(Dialogo::novo("Planeta", &format!("Você escolheu {}.", PLANETAS[i]), &["OK"]));
                }
            }
            let escolhido = lista.selecionado.map_or("nenhum", |i| PLANETAS[i]);
            t.rotulo(&format!("Escolhido: {}", escolhido));
        });
        t.linha(|t| {
            if t.botao("Perguntar") {
                dialogo = Some(Dialogo::novo("Pergunta", "Gostou dos componentes?", &["Sim", "Muito", "Não"]));
            }
            if t.botao("Sair") {
                t.fechar_janela();
            }
        });
        if let Some(b) = t.dialogo(&mut dialogo) {
            saudacao = format!("Na caixa de diálogo, você apertou o botão {}.", b + 1);
        }
    });
    0
}
