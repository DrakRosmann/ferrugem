//! editor — o editor de texto (rode dentro do `janelas`).
//!
//! ```text
//!   editor [arquivo]
//! ```
//!
//! Abre, edita e salva arquivos de texto. Setas, Home/End, Page Up/Down,
//! Delete; seleção com o mouse; Ctrl+C, Ctrl+X e Ctrl+V usam a área de
//! transferência (a mesma dos outros programas e terminais). Atalhos:
//! Ctrl+S salva, Ctrl+O abre, Ctrl+N começa um novo, Ctrl+A seleciona tudo.

#![no_std]
#![no_main]

use ulib::janela::Janela;
use ulib::ui::{self, AreaTexto, Dialogo, Ui};
use ulib::sys;
use ulib::*;

programa!(principal);

enum Pergunta {
    Abrir,
    SalvarComo,
}

fn salvar(caminho: &str, texto: &str) -> Result<(), sys::Erro> {
    let fd = sys::abrir_com_permissoes(caminho, sys::O_WRONLY | sys::O_CREAT | sys::O_TRUNC, 0o644)?;
    let r = sys::escrever_tudo(fd, texto.as_bytes());
    let _ = sys::fechar(fd);
    r
}

fn abrir(caminho: &str) -> Result<AreaTexto, sys::Erro> {
    let dados = sys::ler_arquivo(caminho)?;
    Ok(AreaTexto::novo(&String::from_utf8_lossy(&dados)))
}

fn principal(args: &[&str]) -> i32 {
    let mut janela = match Janela::abrir(720, 500, "Editor de texto") {
        Ok(j) => j,
        Err(e) => {
            eprintln!("editor: {}", e);
            return 1;
        }
    };
    let mut ui = Ui::novo();
    let mut arquivo: Option<String> = args.get(1).map(|s| String::from(*s));
    let mut mensagem = String::new();
    let mut area = match &arquivo {
        Some(c) => match abrir(c) {
            Ok(a) => a,
            Err(e) if e == sys::ENOENT => {
                mensagem = String::from("Arquivo novo");
                AreaTexto::novo("")
            }
            Err(e) => {
                mensagem = format!("{}: {}", c, e);
                AreaTexto::novo("")
            }
        },
        None => AreaTexto::novo(""),
    };
    let mut dialogo: Option<Dialogo> = None;
    let mut pergunta: Option<Pergunta> = None;
    let mut titulo_mostrado = String::new();

    ui::laco(&mut janela, &mut ui, |t| {
        let mut acao = t.barra_de_menus(&[
            ("Arquivo", &["Novo", "Abrir…", "Salvar", "Salvar como…", "Sair"]),
            ("Editar", &["Recortar", "Copiar", "Colar", "Selecionar tudo"]),
        ]);
        // Os atalhos viram as mesmas ações dos menus.
        for (letra, menu) in [('n', (0, 0)), ('o', (0, 1)), ('s', (0, 2)), ('a', (1, 3))] {
            if t.atalho(letra) {
                acao = Some(menu);
            }
        }
        match acao {
            Some((0, 0)) => {
                area = AreaTexto::novo("");
                arquivo = None;
                mensagem.clear();
            }
            Some((0, 1)) => {
                dialogo = Some(Dialogo::com_campo("Abrir", "Caminho do arquivo:", arquivo.as_deref().unwrap_or("/"), &["Abrir", "Cancelar"]));
                pergunta = Some(Pergunta::Abrir);
            }
            Some((0, 2)) if arquivo.is_some() => {
                let caminho = arquivo.clone().unwrap();
                mensagem = match salvar(&caminho, &area.texto()) {
                    Ok(()) => {
                        area.modificado = false;
                        format!("Salvo em {}", caminho)
                    }
                    Err(e) => format!("Não consegui salvar: {}", e),
                };
            }
            Some((0, 2)) | Some((0, 3)) => {
                let sugestao = arquivo.clone().unwrap_or_else(|| format!("{}/texto.txt", variavel("HOME").unwrap_or("")));
                dialogo = Some(Dialogo::com_campo("Salvar como", "Salvar em:", &sugestao, &["Salvar", "Cancelar"]));
                pergunta = Some(Pergunta::SalvarComo);
            }
            Some((0, 4)) => t.fechar_janela(),
            Some((1, 0)) => {
                if area.selecao().is_some() {
                    t.copiar(&area.texto_selecionado());
                    area.apagar_selecao();
                }
            }
            Some((1, 1)) => {
                if area.selecao().is_some() {
                    t.copiar(&area.texto_selecionado());
                }
            }
            Some((1, 2)) => t.pedir_colagem(),
            Some((1, 3)) => area.selecionar_tudo(),
            _ => {}
        }
        let (largura, altura) = (t.largura_livre(), t.altura_livre() - 34);
        t.area_texto(&mut area, largura, altura);
        if area.modificado {
            mensagem.clear(); // voltou a editar: a mensagem anterior ("salvo") já não vale
        }
        let (l, c) = area.cursor;
        let estado = if area.modificado { " · modificado" } else { "" };
        let status = if mensagem.is_empty() {
            format!("Linha {}, coluna {} · {} linhas{}", l + 1, c + 1, area.linhas.len(), estado)
        } else {
            format!("{}{}", mensagem, estado)
        };
        t.rotulo(&status);

        if let Some(b) = t.dialogo(&mut dialogo) {
            let caminho = t.texto_do_dialogo();
            if b == 0 {
                match pergunta.take() {
                    Some(Pergunta::Abrir) => match abrir(&caminho) {
                        Ok(a) => {
                            area = a;
                            arquivo = Some(caminho);
                            mensagem.clear();
                        }
                        Err(e) => mensagem = format!("{}: {}", caminho, e),
                    },
                    Some(Pergunta::SalvarComo) => {
                        mensagem = match salvar(&caminho, &area.texto()) {
                            Ok(()) => {
                                area.modificado = false;
                                let m = format!("Salvo em {}", caminho);
                                arquivo = Some(caminho);
                                m
                            }
                            Err(e) => format!("Não consegui salvar: {}", e),
                        };
                    }
                    None => {}
                }
            }
            pergunta = None;
        }
        // O título: o nome do arquivo e um "•" se houver alterações.
        let nome = arquivo.as_deref().map_or("Sem título", |c| c.rsplit('/').next().unwrap_or(c));
        let titulo = format!("{}{} — Editor de texto", if area.modificado { "• " } else { "" }, nome);
        if titulo != titulo_mostrado {
            t.mudar_titulo(&titulo);
            titulo_mostrado = titulo;
        }
    });
    0
}
