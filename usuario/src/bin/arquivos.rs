//! arquivos — o gerenciador de arquivos (rode dentro do `janelas`).
//!
//! ```text
//!   arquivos [pasta]
//! ```
//!
//! Mostra o conteúdo de uma pasta numa tabela (nome, tamanho, data, dono),
//! com as pastas primeiro. Dois cliques (ou Enter) abrem: uma pasta entra
//! nela; uma imagem vai para o `imagens`; um programa é executado; o resto
//! vai para o `editor`. Os menus criam pastas, renomeiam, apagam, copiam e
//! colam arquivos.

#![no_std]
#![no_main]

use ulib::janela::Janela;
use ulib::ui::{self, CampoTexto, Dialogo, Icone, Lista, Ui};
use ulib::sys;
use ulib::usuarios::nome_do_usuario;
use ulib::*;

programa!(principal);

struct Entrada {
    nome: String,
    pasta: bool,
    modo: u32,
    tamanho: u64,
    modificado: u64,
    uid: u32,
}

/// O que a caixa de diálogo aberta está perguntando.
enum Pergunta {
    NovaPasta,
    Renomear(String),
    Apagar(String),
}

fn juntar(pasta: &str, nome: &str) -> String {
    if pasta.ends_with('/') { format!("{}{}", pasta, nome) } else { format!("{}/{}", pasta, nome) }
}

fn acima(pasta: &str) -> String {
    match pasta.trim_end_matches('/').rsplit_once('/') {
        Some(("", _)) | None => String::from("/"),
        Some((pai, _)) => String::from(pai),
    }
}

/// O conteúdo da pasta: as pastas primeiro, depois os arquivos, por nome.
fn ler(pasta: &str) -> Result<Vec<Entrada>, sys::Erro> {
    let mut entradas: Vec<Entrada> = sys::listar_diretorio(pasta)?
        .into_iter()
        .map(|e| {
            let st = sys::stat(&juntar(pasta, &e.nome)).ok();
            Entrada {
                pasta: st.as_ref().is_some_and(|s| s.eh_diretorio()),
                modo: st.as_ref().map_or(0, |s| s.modo),
                tamanho: st.as_ref().map_or(0, |s| s.tamanho),
                modificado: st.as_ref().map_or(0, |s| s.modificado),
                uid: st.as_ref().map_or(0, |s| s.uid),
                nome: e.nome,
            }
        })
        .collect();
    entradas.sort_by(|a, b| b.pasta.cmp(&a.pasta).then_with(|| a.nome.to_lowercase().cmp(&b.nome.to_lowercase())));
    Ok(entradas)
}

fn extensao(nome: &str) -> &str {
    nome.rsplit_once('.').map_or("", |(_, e)| e)
}

fn icone(e: &Entrada) -> Icone {
    if e.pasta {
        return Icone::Pasta;
    }
    match extensao(&e.nome).to_lowercase().as_str() {
        "png" => Icone::Imagem,
        "txt" | "md" | "rs" | "c" | "h" | "lua" | "sh" | "conf" | "log" | "csv" => Icone::Texto,
        _ if e.modo & 0o111 != 0 => Icone::Programa,
        "" if e.tamanho < 64 * 1024 => Icone::Texto,
        _ => Icone::Arquivo,
    }
}

fn tamanho_legivel(n: u64) -> String {
    match n {
        0..=1023 => format!("{} B", n),
        1024..=1048575 => format!("{},{} KiB", n / 1024, n % 1024 * 10 / 1024),
        _ => format!("{},{} MiB", n >> 20, (n & 0xFFFFF) * 10 >> 20),
    }
}

/// Roda um programa com argumentos, sem esperar por ele.
fn lancar(programa: &str, args: &[&str]) {
    if sys::fork() == Ok(0) {
        let mut todos = Vec::from([programa]);
        todos.extend_from_slice(args);
        let erro = sys::executar(programa, &todos);
        eprintln!("arquivos: {}: {}", programa, erro);
        sys::sair(127);
    }
}

fn copiar_arquivo(de: &str, para: &str) -> Result<(), sys::Erro> {
    let dados = sys::ler_arquivo(de)?;
    let modo = sys::stat(de).map_or(0o644, |s| s.modo & 0o777);
    let fd = sys::abrir_com_permissoes(para, sys::O_WRONLY | sys::O_CREAT | sys::O_TRUNC, modo)?;
    let r = sys::escrever_tudo(fd, &dados);
    let _ = sys::fechar(fd);
    r
}

fn principal(args: &[&str]) -> i32 {
    let inicial = args.get(1).map(|s| String::from(*s)).unwrap_or_else(|| String::from(variavel("HOME").unwrap_or("/")));
    let mut janela = match Janela::abrir(720, 480, "Arquivos") {
        Ok(j) => j,
        Err(e) => {
            eprintln!("arquivos: {}", e);
            return 1;
        }
    };
    let mut ui = Ui::novo();
    let mut pasta = String::new();
    let mut entradas: Vec<Entrada> = Vec::new();
    let mut lista = Lista::default();
    let mut caminho = CampoTexto::novo("");
    let mut dialogo: Option<Dialogo> = None;
    let mut pergunta: Option<Pergunta> = None;
    let mut copiado: Option<String> = None;
    let mut mensagem = String::new();
    let mut ir_para = Some(inicial);

    ui::laco(&mut janela, &mut ui, |t| {
        while let Ok(Some(_)) = sys::recolher_terminado() {}
        // Trocar de pasta (pedido no quadro anterior).
        if let Some(nova) = ir_para.take() {
            match ler(&nova) {
                Ok(e) => {
                    entradas = e;
                    if nova != pasta {
                        lista = Lista::default();
                        mensagem.clear();
                    }
                    pasta = nova;
                    caminho = CampoTexto::novo(&pasta);
                }
                Err(e) => mensagem = format!("{}: {}", nova, e),
            }
        }
        let selecionada = lista.selecionado.and_then(|i| entradas.get(i)).map(|e| e.nome.clone());
        let caminho_selecionado = selecionada.as_ref().map(|n| juntar(&pasta, n));

        match t.barra_de_menus(&[
            ("Arquivo", &["Nova pasta", "Abrir", "Sair"]),
            ("Editar", &["Copiar", "Colar", "Renomear", "Apagar"]),
            ("Ir", &["Pasta pessoal", "Raiz (/)", "Pasta acima"]),
        ]) {
            Some((0, 0)) => {
                dialogo = Some(Dialogo::com_campo("Nova pasta", "Nome da pasta:", "nova pasta", &["Criar", "Cancelar"]));
                pergunta = Some(Pergunta::NovaPasta);
            }
            Some((0, 1)) => lista.ativado = lista.selecionado,
            Some((0, 2)) => t.fechar_janela(),
            Some((1, 0)) => {
                if let Some(c) = &caminho_selecionado {
                    copiado = Some(c.clone());
                    mensagem = format!("Copiado: {}", c);
                }
            }
            Some((1, 1)) => match &copiado {
                Some(origem) => {
                    let nome = origem.rsplit('/').next().unwrap_or("copia");
                    let mut destino = juntar(&pasta, nome);
                    if sys::stat(&destino).is_ok() {
                        destino = juntar(&pasta, &format!("cópia de {}", nome));
                    }
                    mensagem = match copiar_arquivo(origem, &destino) {
                        Ok(()) => format!("Colado: {}", destino),
                        Err(e) => format!("Não consegui copiar: {}", e),
                    };
                    ir_para = Some(pasta.clone());
                }
                None => mensagem = String::from("Nada copiado ainda (Editar → Copiar)"),
            },
            Some((1, 2)) => {
                if let Some(n) = &selecionada {
                    dialogo = Some(Dialogo::com_campo("Renomear", "Novo nome:", n, &["Renomear", "Cancelar"]));
                    pergunta = Some(Pergunta::Renomear(n.clone()));
                }
            }
            Some((1, 3)) => {
                if let Some(n) = &selecionada {
                    let texto = format!("Apagar \"{}\"? Isso não pode ser desfeito.", n);
                    dialogo = Some(Dialogo::novo("Apagar", &texto, &["Apagar", "Cancelar"]));
                    pergunta = Some(Pergunta::Apagar(n.clone()));
                }
            }
            Some((2, 0)) => ir_para = Some(String::from(variavel("HOME").unwrap_or("/"))),
            Some((2, 1)) => ir_para = Some(String::from("/")),
            Some((2, 2)) => ir_para = Some(acima(&pasta)),
            _ => {}
        }

        let largura = t.largura_livre();
        t.linha(|t| {
            if t.botao("↑") {
                ir_para = Some(acima(&pasta));
            }
            if t.botao("Pessoal") {
                ir_para = Some(String::from(variavel("HOME").unwrap_or("/")));
            }
            let w = largura - t.posicao().0 + 12;
            if t.campo(&mut caminho, w) {
                ir_para = Some(caminho.texto.clone());
            }
        });

        let linhas: Vec<(Icone, Vec<String>)> = entradas
            .iter()
            .map(|e| {
                let tamanho = if e.pasta { String::from("—") } else { tamanho_legivel(e.tamanho) };
                let data = String::from(sys::formatar_data(e.modificado).get(..16).unwrap_or(""));
                (icone(e), Vec::from([e.nome.clone(), tamanho, data, nome_do_usuario(e.uid)]))
            })
            .collect();
        let colunas = [("Nome", largura - 360), ("Tamanho", 90), ("Modificado", 150), ("Dono", 90)];
        let altura = t.altura_livre() - 34;
        t.tabela(&mut lista, &colunas, &linhas, largura, altura);

        // Abrir (clique duplo ou Enter).
        if let Some(i) = lista.ativado.take() {
            if let Some(e) = entradas.get(i) {
                let alvo = juntar(&pasta, &e.nome);
                match icone(e) {
                    Icone::Pasta => ir_para = Some(alvo),
                    Icone::Imagem => lancar("/bin/imagens", &[&alvo]),
                    Icone::Programa => lancar(&alvo, &[]),
                    _ => lancar("/bin/editor", &[&alvo]),
                }
            }
        }

        let status = if !mensagem.is_empty() {
            mensagem.clone()
        } else {
            match lista.selecionado.and_then(|i| entradas.get(i)) {
                Some(e) if !e.pasta => format!("{} itens · {} ({})", entradas.len(), e.nome, tamanho_legivel(e.tamanho)),
                Some(e) => format!("{} itens · pasta {}", entradas.len(), e.nome),
                None => format!("{} itens", entradas.len()),
            }
        };
        t.rotulo(&status);

        if let Some(b) = t.dialogo(&mut dialogo) {
            let texto = t.texto_do_dialogo();
            if b == 0 {
                let resultado = match pergunta.take() {
                    Some(Pergunta::NovaPasta) => sys::criar_diretorio(&juntar(&pasta, &texto)),
                    Some(Pergunta::Renomear(antigo)) => sys::renomear(&juntar(&pasta, &antigo), &juntar(&pasta, &texto)),
                    Some(Pergunta::Apagar(nome)) => {
                        let alvo = juntar(&pasta, &nome);
                        if sys::stat(&alvo).is_ok_and(|s| s.eh_diretorio()) {
                            sys::remover_diretorio(&alvo)
                        } else {
                            sys::remover(&alvo)
                        }
                    }
                    None => Ok(()),
                };
                mensagem = match resultado {
                    Ok(()) => String::new(),
                    Err(e) => format!("Não deu: {}", e),
                };
                ir_para = Some(pasta.clone());
            }
            pergunta = None;
        }
    });
    0
}
