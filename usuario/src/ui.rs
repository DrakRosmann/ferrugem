//! ui — componentes para programas com janela: botões, campos de texto,
//! listas, menus, caixas de diálogo...
//!
//! É uma biblioteca de "modo imediato" (como a Dear ImGui e a egui): não há
//! uma árvore de objetos guardada. A cada evento, o programa redesenha a
//! janela inteira chamando os componentes, e cada um, ao ser desenhado,
//! confere o mouse e o teclado e diz se foi usado:
//!
//! ```text
//!   ui::laco(&mut janela, &mut ui, |t| {
//!       t.rotulo("Seu nome:");
//!       t.campo(&mut nome, 200);
//!       if t.botao("Cumprimentar") {
//!           mensagem = format!("Olá, {}!", nome.texto);
//!       }
//!       t.rotulo(&mensagem);
//!   });
//! ```
//!
//! O estado que precisa durar (o texto de um campo, o item escolhido numa
//! lista) fica com o programa (`CampoTexto`, `Lista`...); o que é só da
//! interação (qual botão está apertado, quem tem o foco do teclado, qual
//! menu está aberto) fica no `Ui`. Os componentes são posicionados um
//! embaixo do outro, ou lado a lado dentro de `t.linha(...)`.

use crate::grafico::{Fonte, Quadro, ALTURA_LETRA, LARGURA_LETRA};
use crate::janela::{Evento, Janela, Seta};
use crate::truetype::FonteTT;
use alloc::string::String;
use alloc::vec::Vec;

/// As cores da interface.
#[derive(Clone, Copy)]
pub struct Tema {
    pub fundo: u32,
    pub painel: u32,
    pub texto: u32,
    pub texto_fraco: u32,
    pub borda: u32,
    pub botao: u32,
    pub botao_sobre: u32,
    pub botao_apertado: u32,
    pub campo: u32,
    pub realce: u32,
    pub destaque: u32,
    pub texto_destaque: u32,
}

pub const TEMA_CLARO: Tema = Tema {
    fundo: 0xEEEEEC,
    painel: 0xE2E2DF,
    texto: 0x2E3436,
    texto_fraco: 0x888A85,
    borda: 0xA9ABA6,
    botao: 0xF8F8F7,
    botao_sobre: 0xFFFFFF,
    botao_apertado: 0xD6D6D2,
    campo: 0xFFFFFF,
    realce: 0xF5E1D6,
    destaque: 0xB7410E, // a cor de ferrugem
    texto_destaque: 0xFFFFFF,
};

/// O estilo escuro (ligado no painel de controle do `janelas`).
pub const TEMA_ESCURO: Tema = Tema {
    fundo: 0x242424,
    painel: 0x303030,
    texto: 0xEEEEEC,
    texto_fraco: 0x9A9A96,
    borda: 0x4C4C4C,
    botao: 0x383838,
    botao_sobre: 0x444444,
    botao_apertado: 0x2C2C2C,
    campo: 0x1C1C1C,
    realce: 0x4A3027,
    destaque: 0xC64A1C,
    texto_destaque: 0xFFFFFF,
};

const MARGEM: i32 = 12;
const ESPACO: i32 = 8;
const ALTURA: i32 = 26;
const ALTURA_MENU: i32 = 24;

#[derive(Clone, PartialEq)]
pub enum Tecla {
    Letra(char),
    Apagar,
    Enter,
    Tab,
    Esc,
    Seta(Seta),
    /// Ctrl+C, Ctrl+V e Ctrl+X.
    Copiar,
    Colar,
    Recortar,
    /// Texto colado (pode ter várias linhas).
    Texto(String),
    /// Outro Ctrl+letra (um atalho: veja `Tela::atalho`).
    Controle(char),
}

/// Os ícones das tabelas (16 × 16 pixels).
#[derive(Clone, Copy, PartialEq)]
pub enum Icone {
    Pasta,
    Arquivo,
    Texto,
    Imagem,
    Programa,
    Acima,
}

/// Desenha um ícone com o canto de cima à esquerda em (x, y).
pub fn desenhar_icone(q: &mut Quadro, icone: Icone, x: i32, y: i32) {
    match icone {
        Icone::Pasta => {
            q.retangulo_arredondado(x, y + 2, 8, 6, 0xC98F2A);
            q.retangulo_arredondado(x, y + 4, 16, 11, 0xE8A83A);
            q.retangulo(x + 1, y + 6, 14, 1, 0xF5C76E);
        }
        Icone::Acima => {
            q.retangulo_arredondado(x, y + 4, 16, 11, 0xE8A83A);
            q.linha(x + 8, y + 6, x + 8, y + 13, 2, 0xFFFFFF);
            q.linha(x + 5, y + 9, x + 8, y + 6, 2, 0xFFFFFF);
            q.linha(x + 11, y + 9, x + 8, y + 6, 2, 0xFFFFFF);
        }
        Icone::Programa => {
            q.retangulo_arredondado(x, y + 1, 16, 14, 0x2E3436);
            q.linha(x + 3, y + 5, x + 6, y + 8, 1, 0x8AE234);
            q.linha(x + 6, y + 8, x + 3, y + 11, 1, 0x8AE234);
            q.retangulo(x + 8, y + 11, 5, 1, 0x8AE234);
        }
        _ => {
            // Uma folha com o canto dobrado, e o desenho do tipo dentro.
            q.retangulo(x + 2, y, 9, 16, 0x8A8A86);
            q.retangulo(x + 2, y, 12, 16, 0x8A8A86);
            q.retangulo(x + 3, y + 1, 10, 14, 0xFFFFFF);
            q.retangulo(x + 10, y, 4, 4, 0xD0D0CC);
            match icone {
                Icone::Texto => {
                    for k in 0..4 {
                        q.retangulo(x + 5, y + 5 + k * 2, 6, 1, 0x7A7A76);
                    }
                }
                Icone::Imagem => {
                    q.retangulo(x + 4, y + 5, 8, 8, 0x87CEEB);
                    q.linha(x + 4, y + 12, x + 7, y + 8, 1, 0x4E9A06);
                    q.linha(x + 7, y + 8, x + 11, y + 12, 1, 0x4E9A06);
                    q.retangulo(x + 9, y + 6, 2, 2, 0xF0C020);
                }
                _ => {}
            }
        }
    }
}

/// O que os componentes pedem à janela (a área de transferência).
pub enum Pedido {
    Copiar(String),
    Colar,
    Titulo(String),
    /// Reler ~/.janelas (depois de `janela::mudar_ajuste`).
    Recarregar,
}

/// O estado de um campo de texto: o texto e onde está o cursor (em caracteres).
pub struct CampoTexto {
    pub texto: String,
    pub cursor: usize,
    rolagem: i32,
    /// Ponha true para o campo pegar o foco no próximo desenho (um atalho
    /// como o Ctrl+L do navegador).
    pub pedir_foco: bool,
}

impl CampoTexto {
    pub fn novo(texto: &str) -> CampoTexto {
        CampoTexto { texto: String::from(texto), cursor: texto.chars().count(), rolagem: 0, pedir_foco: false }
    }

    /// Troca o texto (o cursor vai para o fim).
    pub fn mudar(&mut self, texto: &str) {
        self.texto = String::from(texto);
        self.cursor = texto.chars().count();
        self.rolagem = 0;
    }

    fn byte(&self, caractere: usize) -> usize {
        self.texto.char_indices().nth(caractere).map_or(self.texto.len(), |(i, _)| i)
    }
}

/// Uma posição no texto: (linha, coluna em caracteres).
pub type Posicao = (usize, usize);

/// O que `Tela::area` devolve: onde ela ficou e o que o mouse fez nela.
pub struct Area {
    pub x: i32,
    pub y: i32,
    pub largura: i32,
    pub altura: i32,
    /// O mouse, relativo ao canto da área (se estiver em cima dela).
    pub mouse: Option<(i32, i32)>,
    /// O botão foi apertado e solto em cima da área.
    pub clique: bool,
    /// O botão está apertado (arrastando) com o mouse em cima.
    pub apertado: bool,
    /// A roda girou em cima da área: positivo = para baixo.
    pub roda: i32,
    pub focada: bool,
}

/// O estado de uma área de texto de várias linhas (o editor).
pub struct AreaTexto {
    pub linhas: Vec<String>,
    pub cursor: Posicao,
    /// Onde a seleção começou (ela vai daqui até o cursor).
    ancora: Option<Posicao>,
    /// A primeira linha e a primeira coluna visíveis.
    rolagem: (usize, usize),
    /// Mudou desde a última vez que foi salvo?
    pub modificado: bool,
}

impl AreaTexto {
    pub fn novo(texto: &str) -> AreaTexto {
        let linhas: Vec<String> = texto.split('\n').map(String::from).collect();
        AreaTexto { linhas, cursor: (0, 0), ancora: None, rolagem: (0, 0), modificado: false }
    }

    pub fn texto(&self) -> String {
        self.linhas.join("\n")
    }

    fn byte(&self, (l, c): Posicao) -> usize {
        self.linhas[l].char_indices().nth(c).map_or(self.linhas[l].len(), |(i, _)| i)
    }

    fn colunas(&self, l: usize) -> usize {
        self.linhas[l].chars().count()
    }

    /// A seleção, em ordem (início, fim), se houver uma.
    pub fn selecao(&self) -> Option<(Posicao, Posicao)> {
        let a = self.ancora?;
        (a != self.cursor).then(|| (a.min(self.cursor), a.max(self.cursor)))
    }

    pub fn texto_selecionado(&self) -> String {
        let Some((a, b)) = self.selecao() else { return String::new() };
        if a.0 == b.0 {
            return String::from(&self.linhas[a.0][self.byte(a)..self.byte(b)]);
        }
        let mut s = String::from(&self.linhas[a.0][self.byte(a)..]);
        for l in a.0 + 1..b.0 {
            s.push('\n');
            s.push_str(&self.linhas[l]);
        }
        s.push('\n');
        s.push_str(&self.linhas[b.0][..self.byte(b)]);
        s
    }

    pub fn selecionar_tudo(&mut self) {
        self.ancora = Some((0, 0));
        let ultima = self.linhas.len() - 1;
        self.cursor = (ultima, self.colunas(ultima));
    }

    pub fn apagar_selecao(&mut self) -> bool {
        let Some((a, b)) = self.selecao() else {
            self.ancora = None;
            return false;
        };
        let resto = String::from(&self.linhas[b.0][self.byte(b)..]);
        let inicio = self.byte(a);
        self.linhas[a.0].truncate(inicio);
        self.linhas[a.0].push_str(&resto);
        self.linhas.drain(a.0 + 1..=b.0);
        self.cursor = a;
        self.ancora = None;
        self.modificado = true;
        true
    }

    /// Insere texto no cursor (no lugar da seleção, se houver).
    pub fn inserir(&mut self, texto: &str) {
        self.apagar_selecao();
        for c in texto.chars() {
            let b = self.byte(self.cursor);
            if c == '\n' {
                let resto = self.linhas[self.cursor.0].split_off(b);
                self.linhas.insert(self.cursor.0 + 1, resto);
                self.cursor = (self.cursor.0 + 1, 0);
            } else if !c.is_control() || c == '\t' {
                self.linhas[self.cursor.0].insert(b, c);
                self.cursor.1 += 1;
            }
        }
        self.modificado = true;
    }

    /// Backspace (`para_tras`) ou Delete.
    fn apagar(&mut self, para_tras: bool) {
        if self.apagar_selecao() {
            return;
        }
        let (l, c) = self.cursor;
        if para_tras {
            if c > 0 {
                self.cursor.1 -= 1;
                let b = self.byte(self.cursor);
                self.linhas[l].remove(b);
            } else if l > 0 {
                let linha = self.linhas.remove(l);
                self.cursor = (l - 1, self.colunas(l - 1));
                self.linhas[l - 1].push_str(&linha);
            } else {
                return;
            }
        } else if c < self.colunas(l) {
            let b = self.byte(self.cursor);
            self.linhas[l].remove(b);
        } else if l + 1 < self.linhas.len() {
            let linha = self.linhas.remove(l + 1);
            self.linhas[l].push_str(&linha);
        } else {
            return;
        }
        self.modificado = true;
    }
}

/// O estado de uma lista: o item escolhido e quanto ela foi rolada (em pixels).
#[derive(Default)]
pub struct Lista {
    pub selecionado: Option<usize>,
    pub rolagem: i32,
    /// Clique duplo ou Enter num item: "abrir" (o programa pega com `take()`).
    pub ativado: Option<usize>,
    /// (instante, item) do último clique, para reconhecer o clique duplo.
    ultimo_clique: (u64, usize),
}

/// Uma caixa de diálogo: um título, uma mensagem, os botões e (às vezes)
/// um campo de texto (ex.: "nome da pasta nova").
pub struct Dialogo {
    pub titulo: String,
    pub mensagem: String,
    pub botoes: Vec<String>,
    pub campo: Option<CampoTexto>,
}

impl Dialogo {
    /// Pergunta um texto. Depois da resposta, ele está em `Tela::texto_do_dialogo`.
    pub fn com_campo(titulo: &str, mensagem: &str, texto: &str, botoes: &[&str]) -> Dialogo {
        let mut d = Self::novo(titulo, mensagem, botoes);
        d.campo = Some(CampoTexto::novo(texto));
        d
    }

    pub fn novo(titulo: &str, mensagem: &str, botoes: &[&str]) -> Dialogo {
        Dialogo {
            campo: None,
            titulo: String::from(titulo),
            mensagem: String::from(mensagem),
            botoes: botoes.iter().map(|b| String::from(*b)).collect(),
        }
    }
}

/// O que dura entre um desenho e o seguinte: fontes, mouse, teclado, foco.
pub struct Ui {
    pub tema: Tema,
    pub tamanho: u16,
    normal: Option<FonteTT>,
    negrito: Option<FonteTT>,
    /// Monoespaçada (todas as letras da mesma largura): para editar texto.
    mono: Option<FonteTT>,
    vga: Option<Fonte>,
    /// O texto do campo da última caixa de diálogo respondida.
    resposta: String,
    mouse: (i32, i32),
    apertado: bool,
    /// Neste evento: o botão foi apertado / solto, a roda girou.
    clicou: bool,
    soltou: bool,
    roda: i32,
    teclas: Vec<Tecla>,
    /// O componente em que o botão foi apertado (e ainda não foi solto).
    ativo: Option<u32>,
    /// Quem recebe o teclado.
    foco: Option<u32>,
    menu_aberto: Option<usize>,
    /// Do desenho anterior: o menu suspenso aberto (o que está embaixo dele
    /// não recebe o mouse) e se havia uma caixa de diálogo (que bloqueia o resto).
    sobreposto: Option<(i32, i32, i32, i32)>,
    modal: bool,
    redesenhar: bool,
    sair: bool,
    /// Para o laço repassar à janela.
    pub pedidos: Vec<Pedido>,
    /// Desenhar de novo daqui a tantos ms, mesmo sem eventos (só desta vez).
    acordar: Option<i32>,
}

impl Default for Ui {
    fn default() -> Self {
        Self::novo()
    }
}

impl Ui {
    pub fn novo() -> Ui {
        Ui {
            tema: TEMA_CLARO,
            tamanho: 13,
            normal: FonteTT::carregar("/usr/share/fonts/DejaVuSans.ttf"),
            negrito: FonteTT::carregar("/usr/share/fonts/DejaVuSans-Bold.ttf"),
            mono: FonteTT::carregar("/usr/share/fonts/LiberationMono-Regular.ttf"),
            vga: Fonte::carregar(),
            resposta: String::new(),
            mouse: (-1, -1),
            apertado: false,
            clicou: false,
            soltou: false,
            roda: 0,
            teclas: Vec::new(),
            ativo: None,
            foco: None,
            menu_aberto: None,
            sobreposto: None,
            modal: false,
            redesenhar: false,
            sair: false,
            pedidos: Vec::new(),
            acordar: None,
        }
    }

    /// Anota um evento da janela (o próximo desenho reage a ele).
    pub fn receber(&mut self, evento: &Evento) {
        match *evento {
            Evento::Botao { x, y, botao: 1, apertado } => {
                self.mouse = (x, y);
                self.apertado = apertado;
                if apertado {
                    self.clicou = true;
                } else {
                    self.soltou = true;
                }
            }
            Evento::Botao { .. } => {}
            Evento::Mover { x, y } => self.mouse = (x, y),
            Evento::Roda { x, y, giro } => {
                self.mouse = (x, y);
                self.roda += giro;
            }
            Evento::Seta(s) => self.teclas.push(Tecla::Seta(s)),
            Evento::Tecla(c) => self.teclas.push(match c {
                '\u{8}' | '\u{7f}' => Tecla::Apagar,
                '\n' | '\r' => Tecla::Enter,
                '\t' => Tecla::Tab,
                '\u{1b}' => Tecla::Esc,
                '\u{3}' => Tecla::Copiar,
                '\u{16}' => Tecla::Colar,
                '\u{18}' => Tecla::Recortar,
                // Os outros Ctrl+letra: atalhos (Ctrl+S = 19 → 's').
                c @ '\u{1}'..='\u{1a}' => Tecla::Controle((c as u8 + 0x60) as char),
                c => Tecla::Letra(c),
            }),
            Evento::Fechar => self.sair = true,
            Evento::Tamanho { .. } => self.redesenhar = true,
            Evento::Tema { escuro, destaque } => {
                self.tema = if escuro { TEMA_ESCURO } else { TEMA_CLARO };
                if let Some(d) = destaque {
                    self.tema.destaque = d;
                    // o realce (item sob o mouse): um pouco do destaque sobre o fundo
                    let (fundo, peso) = if escuro { (self.tema.campo, 64) } else { (0xFFFFFF, 40) };
                    let canal = |c: u32, s: u32| ((c >> s & 0xFF) * peso + (fundo >> s & 0xFF) * (255 - peso)) / 255;
                    self.tema.realce = canal(d, 16) << 16 | canal(d, 8) << 8 | canal(d, 0);
                }
                self.redesenhar = true;
            }
            // O texto colado vai para quem tem o foco.
            Evento::Colar(ref texto) => self.teclas.push(Tecla::Texto(texto.clone())),
        }
    }

    /// Começa um desenho da janela (o `Tela` devolvido termina o desenho ao sumir).
    pub fn comecar<'a>(&'a mut self, mut q: Quadro<'a>) -> Tela<'a> {
        q.sem_recorte();
        q.retangulo(0, 0, q.largura, q.altura, self.tema.fundo);
        let largura = q.largura;
        Tela {
            ui: self,
            q,
            x0: MARGEM,
            cx: MARGEM,
            cy: MARGEM,
            horizontal: false,
            altura_linha: 0,
            largura,
            proximo_id: 0,
            focaveis: Vec::new(),
            clique_usado: false,
            menu_pendente: None,
            sobreposto_agora: None,
            modal_agora: false,
            dentro_do_modal: false,
            dentro_do_menu: false,
        }
    }

    /// Algo mudou por causa do último desenho (um clique): desenhe de novo.
    pub fn precisa_redesenhar(&mut self) -> bool {
        core::mem::take(&mut self.redesenhar)
    }

    /// O programa pediu para fechar (ou o servidor mandou fechar).
    pub fn saindo(&self) -> bool {
        self.sair
    }

    pub fn fechar(&mut self) {
        self.sair = true;
    }

    fn fonte(&mut self, negrito: bool) -> Option<&mut FonteTT> {
        if negrito && self.negrito.is_some() { self.negrito.as_mut() } else { self.normal.as_mut() }
    }

    pub fn altura_linha(&mut self, tamanho: u16) -> i32 {
        match self.fonte(false) {
            Some(f) => f.metricas(tamanho).1,
            None => ALTURA_LETRA,
        }
    }

    pub fn largura_texto(&mut self, texto: &str, tamanho: u16, negrito: bool) -> i32 {
        match self.fonte(negrito) {
            Some(f) => f.largura(texto, tamanho),
            None => texto.chars().count() as i32 * LARGURA_LETRA,
        }
    }

    /// Escreve uma linha de texto com o topo em `y`. Devolve a largura.
    pub fn escrever(&mut self, q: &mut Quadro, x: i32, y: i32, texto: &str, cor: u32, negrito: bool, tamanho: u16) -> i32 {
        match self.fonte(negrito) {
            Some(f) => q.texto_tt(f, x, y, texto, tamanho, cor),
            None => {
                if let Some(v) = &self.vga {
                    q.texto(v, x, y, texto, cor, None);
                }
                texto.chars().count() as i32 * LARGURA_LETRA
            }
        }
    }

    /// Texto em itálico (sintético: a fonte normal inclinada).
    #[allow(clippy::too_many_arguments)]
    pub fn escrever_italico(&mut self, q: &mut Quadro, x: i32, y: i32, texto: &str, cor: u32, negrito: bool, tamanho: u16) -> i32 {
        match self.fonte(negrito) {
            Some(f) => q.texto_tt_estilo(f, x, y, texto, tamanho, cor, true),
            None => self.escrever(q, x, y, texto, cor, negrito, tamanho),
        }
    }

    /// (largura de cada letra, altura da linha) da fonte monoespaçada.
    pub fn medidas_mono(&mut self, tamanho: u16) -> (i32, i32) {
        match self.mono.as_mut() {
            Some(f) => ((f.glifo('M', tamanho).avanco + 32) / 64, f.metricas(tamanho).1),
            None => (LARGURA_LETRA, ALTURA_LETRA),
        }
    }

    /// Texto na fonte monoespaçada (a da VGA, se não houver outra). Cada
    /// letra vai numa grade de `medidas_mono().0` pixels inteiros: o avanço
    /// da fonte é fracionário (7,8 px no tamanho 13), e quem posiciona o
    /// cursor ou mede o texto conta em colunas.
    pub fn escrever_mono(&mut self, q: &mut Quadro, x: i32, y: i32, texto: &str, cor: u32, tamanho: u16) {
        let (coluna, _) = self.medidas_mono(tamanho);
        match self.mono.as_mut() {
            Some(f) => {
                let mut buf = [0u8; 4];
                for (i, c) in texto.chars().enumerate() {
                    q.texto_tt(f, x + i as i32 * coluna, y, c.encode_utf8(&mut buf), tamanho, cor);
                }
            }
            None => {
                if let Some(v) = &self.vga {
                    q.texto(v, x, y, texto, cor, None);
                }
            }
        }
    }

    /// Quebra o texto em linhas de no máximo `largura` pixels.
    pub fn quebrar(&mut self, texto: &str, largura: i32, tamanho: u16) -> Vec<String> {
        let mut linhas = Vec::new();
        for paragrafo in texto.split('\n') {
            let mut atual = String::new();
            for palavra in paragrafo.split(' ') {
                let tentativa = if atual.is_empty() { String::from(palavra) } else { alloc::format!("{} {}", atual, palavra) };
                if !atual.is_empty() && self.largura_texto(&tentativa, tamanho, false) > largura {
                    linhas.push(core::mem::replace(&mut atual, String::from(palavra)));
                } else {
                    atual = tentativa;
                }
            }
            linhas.push(atual);
        }
        linhas
    }
}

/// Um desenho da janela em andamento: é por aqui que se chamam os componentes.
pub struct Tela<'a> {
    ui: &'a mut Ui,
    /// A imagem da janela (para desenhar à mão, além dos componentes).
    pub q: Quadro<'a>,
    x0: i32,
    cx: i32,
    cy: i32,
    horizontal: bool,
    altura_linha: i32,
    largura: i32,
    proximo_id: u32,
    focaveis: Vec<u32>,
    clique_usado: bool,
    menu_pendente: Option<((i32, i32, i32, i32), Vec<String>, Option<usize>)>,
    sobreposto_agora: Option<(i32, i32, i32, i32)>,
    modal_agora: bool,
    dentro_do_modal: bool,
    dentro_do_menu: bool,
}

fn contem((x, y, w, h): (i32, i32, i32, i32), (px, py): (i32, i32)) -> bool {
    px >= x && py >= y && px < x + w && py < y + h
}

impl Tela<'_> {
    // ------------------------------------------------------------- disposição

    /// Reserva espaço para um componente: embaixo do anterior, ou ao lado (em `linha`).
    fn alocar(&mut self, largura: i32, altura: i32) -> (i32, i32) {
        if self.horizontal {
            let p = (self.cx, self.cy);
            self.cx += largura + ESPACO;
            self.altura_linha = self.altura_linha.max(altura);
            p
        } else {
            let p = (self.x0, self.cy);
            self.cy += altura + ESPACO;
            p
        }
    }

    /// Reserva um espaço para o programa desenhar o que quiser (em `t.q`); devolve onde.
    pub fn reservar(&mut self, largura: i32, altura: i32) -> (i32, i32) {
        self.alocar(largura, altura)
    }

    /// Uma área que o programa desenha e que pode ser clicada (miniaturas,
    /// amostras de cor...): devolve (onde, mouse por cima, clicada).
    pub fn clicavel(&mut self, largura: i32, altura: i32) -> ((i32, i32), bool, bool) {
        let (x, y) = self.alocar(largura, altura);
        let id = self.novo_id();
        let (sobre, clicado) = self.interagir(id, (x, y, largura, altura), false);
        ((x, y), sobre, clicado)
    }

    /// Uma área que o programa desenha e controla por conta própria (a
    /// página do navegador). Clicar nela tira o foco dos campos: as teclas
    /// que sobrarem são dela (veja `tecla_da_area`).
    pub fn area(&mut self, largura: i32, altura: i32) -> Area {
        let (x, y) = self.alocar(largura, altura);
        let id = self.novo_id();
        let (sobre, clique) = self.interagir(id, (x, y, largura, altura), true);
        let mouse = if sobre && !self.bloqueado() { Some((self.ui.mouse.0 - x, self.ui.mouse.1 - y)) } else { None };
        let roda = if mouse.is_some() { core::mem::take(&mut self.ui.roda) } else { 0 };
        let focada = self.ui.foco.is_none() || self.focado(id);
        Area { x, y, largura, altura, mouse, clique, apertado: sobre && self.ui.apertado, roda, focada }
    }

    /// Uma tecla que sobrou para a área (ela tem o foco, ou ninguém tem).
    pub fn tecla_da_area(&mut self, area: &Area, tecla: Tecla) -> bool {
        if !area.focada {
            return false;
        }
        match self.ui.teclas.iter().position(|t| *t == tecla) {
            Some(i) => {
                self.ui.teclas.remove(i);
                true
            }
            None => false,
        }
    }

    /// Ninguém fica com o foco do teclado (depois do Enter na barra de
    /// endereço, o espaço volta a rolar a página).
    pub fn soltar_foco(&mut self) {
        self.ui.foco = None;
    }

    /// Desenha de novo daqui a `ms` milissegundos, mesmo sem eventos (um
    /// download em andamento, uma animação). Vale só para esta espera.
    pub fn acordar_em(&mut self, ms: i32) {
        self.ui.acordar = Some(self.ui.acordar.map_or(ms, |a| a.min(ms)));
    }

    /// Pede outro desenho logo depois deste (o estado mudou no meio).
    pub fn redesenhar(&mut self) {
        self.ui.redesenhar = true;
    }

    /// Uma seta que nenhum componente usou (um atalho do programa, como
    /// "próxima imagem"). Chame depois dos componentes.
    pub fn seta(&mut self, s: Seta) -> bool {
        match self.ui.teclas.iter().position(|t| *t == Tecla::Seta(s)) {
            Some(i) => {
                self.ui.teclas.remove(i);
                true
            }
            None => false,
        }
    }

    /// Os componentes chamados dentro de `f` ficam lado a lado.
    pub fn linha(&mut self, f: impl FnOnce(&mut Self)) {
        let (antes, linha_antes) = (self.horizontal, self.altura_linha);
        self.horizontal = true;
        self.cx = self.x0;
        self.altura_linha = 0;
        f(self);
        self.cy += self.altura_linha + ESPACO;
        (self.horizontal, self.altura_linha) = (antes, linha_antes);
    }

    /// Os próximos componentes começam em (x, y).
    pub fn em(&mut self, x: i32, y: i32) {
        (self.x0, self.cx, self.cy) = (x, x, y);
    }

    pub fn espaco(&mut self, pixels: i32) {
        if self.horizontal {
            self.cx += pixels;
        } else {
            self.cy += pixels;
        }
    }

    /// Quanto sobra de altura daqui até o fim da janela.
    pub fn altura_livre(&self) -> i32 {
        self.q.altura - self.cy - MARGEM
    }

    /// Quanto sobra de largura a partir da margem esquerda atual.
    pub fn largura_livre(&self) -> i32 {
        self.largura - self.x0 - MARGEM
    }

    pub fn posicao(&self) -> (i32, i32) {
        if self.horizontal { (self.cx, self.cy) } else { (self.x0, self.cy) }
    }

    pub fn fechar_janela(&mut self) {
        self.ui.sair = true;
    }

    pub fn tema(&self) -> Tema {
        self.ui.tema
    }

    pub fn escrever(&mut self, x: i32, y: i32, texto: &str, cor: u32, negrito: bool, tamanho: u16) -> i32 {
        self.ui.escrever(&mut self.q, x, y, texto, cor, negrito, tamanho)
    }

    pub fn escrever_italico(&mut self, x: i32, y: i32, texto: &str, cor: u32, negrito: bool, tamanho: u16) -> i32 {
        self.ui.escrever_italico(&mut self.q, x, y, texto, cor, negrito, tamanho)
    }

    /// Texto na fonte monoespaçada.
    pub fn escrever_mono(&mut self, x: i32, y: i32, texto: &str, cor: u32, tamanho: u16) {
        self.ui.escrever_mono(&mut self.q, x, y, texto, cor, tamanho)
    }

    /// A largura de um texto (sem desenhar); `mono` = fonte monoespaçada.
    pub fn medir(&mut self, texto: &str, tamanho: u16, negrito: bool, mono: bool) -> i32 {
        if mono {
            self.ui.medidas_mono(tamanho).0 * texto.chars().count() as i32
        } else {
            self.ui.largura_texto(texto, tamanho, negrito)
        }
    }

    /// A altura de uma linha de texto nesse tamanho.
    pub fn altura_texto(&mut self, tamanho: u16, mono: bool) -> i32 {
        if mono { self.ui.medidas_mono(tamanho).1 } else { self.ui.altura_linha(tamanho) }
    }

    // ------------------------------------------------------------- interação

    fn novo_id(&mut self) -> u32 {
        self.proximo_id += 1;
        self.proximo_id
    }

    /// O resto da janela está bloqueado por uma caixa de diálogo?
    fn bloqueado(&self) -> bool {
        self.ui.modal && !self.dentro_do_modal
    }

    fn mouse_em(&self, area: (i32, i32, i32, i32)) -> bool {
        if self.bloqueado() || !contem(area, self.ui.mouse) {
            return false;
        }
        // Debaixo de um menu aberto, quem recebe o mouse é o menu.
        !matches!(self.ui.sobreposto, Some(r) if !self.dentro_do_menu && contem(r, self.ui.mouse))
    }

    /// O mouse num componente: (está por cima, foi clicado neste evento).
    /// O clique é apertar e soltar o botão dentro do mesmo componente.
    fn interagir(&mut self, id: u32, area: (i32, i32, i32, i32), focavel: bool) -> (bool, bool) {
        let sobre = self.mouse_em(area);
        if focavel && !self.bloqueado() {
            self.focaveis.push(id);
        }
        if self.ui.clicou && sobre {
            self.ui.ativo = Some(id);
            self.clique_usado = true;
            if focavel {
                self.ui.foco = Some(id);
            }
        }
        let clicado = self.ui.soltou && sobre && self.ui.ativo == Some(id);
        if clicado {
            self.ui.redesenhar = true;
        }
        (sobre, clicado)
    }

    fn focado(&self, id: u32) -> bool {
        self.ui.foco == Some(id) && !self.bloqueado()
    }

    /// Tira uma tecla da fila, se este componente tem o foco e ela está lá.
    fn tecla(&mut self, id: u32, quais: &[Tecla]) -> Option<Tecla> {
        if !self.focado(id) {
            return None;
        }
        let i = self.ui.teclas.iter().position(|t| quais.contains(t))?;
        self.ui.redesenhar = true;
        Some(self.ui.teclas.remove(i))
    }

    fn apertado_em(&self, id: u32, sobre: bool) -> bool {
        sobre && self.ui.apertado && self.ui.ativo == Some(id)
    }

    // ------------------------------------------------------------- texto

    pub fn rotulo(&mut self, texto: &str) {
        let tamanho = self.ui.tamanho;
        let (w, h) = (self.ui.largura_texto(texto, tamanho, false), self.ui.altura_linha(tamanho).max(ALTURA));
        let (x, y) = self.alocar(w, h);
        let cor = self.ui.tema.texto;
        let topo = y + (h - self.ui.altura_linha(tamanho)) / 2;
        self.escrever(x, topo, texto, cor, false, tamanho);
    }

    /// Um texto maior, em negrito.
    pub fn titulo(&mut self, texto: &str) {
        let tamanho = self.ui.tamanho + 5;
        let (w, h) = (self.ui.largura_texto(texto, tamanho, true), self.ui.altura_linha(tamanho));
        let (x, y) = self.alocar(w, h);
        let cor = self.ui.tema.texto;
        self.escrever(x, y, texto, cor, true, tamanho);
    }

    /// Texto que quebra em várias linhas para caber em `largura`.
    pub fn paragrafo(&mut self, texto: &str, largura: i32) {
        let tamanho = self.ui.tamanho;
        let linhas = self.ui.quebrar(texto, largura, tamanho);
        let altura_linha = self.ui.altura_linha(tamanho) + 2;
        let (x, y) = self.alocar(largura, altura_linha * linhas.len() as i32);
        let cor = self.ui.tema.texto;
        for (i, l) in linhas.iter().enumerate() {
            self.escrever(x, y + i as i32 * altura_linha, l, cor, false, tamanho);
        }
    }

    /// Uma linha fina de um lado ao outro.
    pub fn separador(&mut self) {
        let largura = self.largura_livre();
        let (x, y) = self.alocar(largura, 1);
        let cor = self.ui.tema.borda;
        self.q.retangulo(x, y, largura, 1, cor);
    }

    // ------------------------------------------------------------- botões

    fn botao_em(&mut self, x: i32, y: i32, largura: i32, texto: &str) -> bool {
        let id = self.novo_id();
        let (sobre, mut clicado) = self.interagir(id, (x, y, largura, ALTURA), true);
        clicado |= self.tecla(id, &[Tecla::Enter, Tecla::Letra(' ')]).is_some();
        let t = self.ui.tema;
        let fundo = if self.apertado_em(id, sobre) {
            t.botao_apertado
        } else if sobre {
            t.botao_sobre
        } else {
            t.botao
        };
        let borda = if self.focado(id) { t.destaque } else { t.borda };
        self.q.retangulo_arredondado(x, y, largura, ALTURA, borda);
        self.q.retangulo_arredondado(x + 1, y + 1, largura - 2, ALTURA - 2, fundo);
        let tamanho = self.ui.tamanho;
        let w = self.ui.largura_texto(texto, tamanho, false);
        let topo = y + (ALTURA - self.ui.altura_linha(tamanho)) / 2;
        self.escrever(x + (largura - w) / 2, topo, texto, t.texto, false, tamanho);
        clicado
    }

    fn largura_botao(&mut self, texto: &str) -> i32 {
        let tamanho = self.ui.tamanho;
        (self.ui.largura_texto(texto, tamanho, false) + 24).max(80)
    }

    /// Um botão só do tamanho do texto (um ícone numa barra de ferramentas).
    pub fn botao_compacto(&mut self, texto: &str) -> bool {
        let tamanho = self.ui.tamanho;
        let largura = (self.ui.largura_texto(texto, tamanho, false) + 20).max(34);
        let (x, y) = self.alocar(largura, ALTURA);
        self.botao_em(x, y, largura, texto)
    }

    /// Um botão. Devolve true quando é clicado (ou apertado com Enter/espaço, com o foco).
    pub fn botao(&mut self, texto: &str) -> bool {
        let largura = self.largura_botao(texto);
        let (x, y) = self.alocar(largura, ALTURA);
        self.botao_em(x, y, largura, texto)
    }

    /// Caixa de seleção (liga/desliga). Devolve true quando muda.
    pub fn caixa(&mut self, texto: &str, marcada: &mut bool) -> bool {
        let tamanho = self.ui.tamanho;
        let largura = 22 + self.ui.largura_texto(texto, tamanho, false);
        let (x, y) = self.alocar(largura, ALTURA);
        let id = self.novo_id();
        let (sobre, mut mudou) = self.interagir(id, (x, y, largura, ALTURA), true);
        mudou |= self.tecla(id, &[Tecla::Letra(' ')]).is_some();
        if mudou {
            *marcada = !*marcada;
        }
        let t = self.ui.tema;
        let (bx, by) = (x, y + (ALTURA - 16) / 2);
        let borda = if self.focado(id) || sobre { t.destaque } else { t.borda };
        self.q.retangulo_arredondado(bx, by, 16, 16, borda);
        self.q.retangulo_arredondado(bx + 1, by + 1, 14, 14, if *marcada { t.destaque } else { t.campo });
        if *marcada {
            self.q.linha(bx + 4, by + 8, bx + 7, by + 11, 2, t.texto_destaque);
            self.q.linha(bx + 7, by + 11, bx + 12, by + 5, 2, t.texto_destaque);
        }
        let topo = y + (ALTURA - self.ui.altura_linha(tamanho)) / 2;
        self.escrever(x + 22, topo, texto, t.texto, false, tamanho);
        mudou
    }

    /// Uma opção entre várias (botão de rádio): marca `*escolhida = valor`. Devolve true quando muda.
    pub fn opcao(&mut self, texto: &str, escolhida: &mut usize, valor: usize) -> bool {
        let tamanho = self.ui.tamanho;
        let largura = 22 + self.ui.largura_texto(texto, tamanho, false);
        let (x, y) = self.alocar(largura, ALTURA);
        let id = self.novo_id();
        let (sobre, mut clicado) = self.interagir(id, (x, y, largura, ALTURA), true);
        clicado |= self.tecla(id, &[Tecla::Letra(' ')]).is_some();
        let mudou = clicado && *escolhida != valor;
        if clicado {
            *escolhida = valor;
        }
        let t = self.ui.tema;
        let (cx, cy) = (x + 8, y + ALTURA / 2);
        let borda = if self.focado(id) || sobre { t.destaque } else { t.borda };
        self.q.circulo(cx, cy, 8, borda);
        self.q.circulo(cx, cy, 7, t.campo);
        if *escolhida == valor {
            self.q.circulo(cx, cy, 4, t.destaque);
        }
        let topo = y + (ALTURA - self.ui.altura_linha(tamanho)) / 2;
        self.escrever(x + 22, topo, texto, t.texto, false, tamanho);
        mudou
    }

    // ------------------------------------------------------------- entrada de texto

    /// Campo de texto de uma linha. Devolve true quando se aperta Enter nele.
    pub fn campo(&mut self, campo: &mut CampoTexto, largura: i32) -> bool {
        let (x, y) = self.alocar(largura, ALTURA);
        let focar = core::mem::take(&mut campo.pedir_foco);
        self.campo_em(campo, x, y, largura, focar)
    }

    fn campo_em(&mut self, campo: &mut CampoTexto, x: i32, y: i32, largura: i32, focar: bool) -> bool {
        let id = self.novo_id();
        if focar {
            self.ui.foco = Some(id);
        }
        let tamanho = self.ui.tamanho;
        let clicou_aqui = self.ui.clicou && self.mouse_em((x, y, largura, ALTURA));
        self.interagir(id, (x, y, largura, ALTURA), true);
        let largura_ate = |ui: &mut Ui, c: &CampoTexto, n: usize| -> i32 {
            let fim = c.byte(n);
            ui.largura_texto(&c.texto[..fim], tamanho, false)
        };
        // O clique põe o cursor no caractere mais perto do mouse.
        if clicou_aqui {
            let alvo = self.ui.mouse.0 - (x + 6) + campo.rolagem;
            let n = campo.texto.chars().count();
            campo.cursor = (0..=n).min_by_key(|&i| (largura_ate(self.ui, campo, i) - alvo).abs()).unwrap_or(n);
        }
        let mut enter = false;
        let teclas = [
            Tecla::Apagar,
            Tecla::Enter,
            Tecla::Seta(Seta::Esquerda),
            Tecla::Seta(Seta::Direita),
            Tecla::Seta(Seta::Inicio),
            Tecla::Seta(Seta::Fim),
            Tecla::Seta(Seta::Delete),
            Tecla::Copiar,
            Tecla::Colar,
            Tecla::Recortar,
        ];
        while self.focado(id) {
            let Some(i) =
                self.ui.teclas.iter().position(|t| matches!(t, Tecla::Letra(_) | Tecla::Texto(_)) || teclas.contains(t))
            else {
                break;
            };
            self.ui.redesenhar = true;
            match self.ui.teclas.remove(i) {
                Tecla::Letra(c) => {
                    let b = campo.byte(campo.cursor);
                    campo.texto.insert(b, c);
                    campo.cursor += 1;
                }
                Tecla::Texto(t) => {
                    // Colado: numa linha só.
                    let t: String = t.chars().map(|c| if c == '\n' { ' ' } else { c }).filter(|c| !c.is_control()).collect();
                    let b = campo.byte(campo.cursor);
                    campo.texto.insert_str(b, &t);
                    campo.cursor += t.chars().count();
                }
                Tecla::Apagar if campo.cursor > 0 => {
                    campo.cursor -= 1;
                    let b = campo.byte(campo.cursor);
                    campo.texto.remove(b);
                }
                Tecla::Seta(Seta::Delete) if campo.cursor < campo.texto.chars().count() => {
                    let b = campo.byte(campo.cursor);
                    campo.texto.remove(b);
                }
                Tecla::Seta(Seta::Esquerda) => campo.cursor = campo.cursor.saturating_sub(1),
                Tecla::Seta(Seta::Direita) => campo.cursor = (campo.cursor + 1).min(campo.texto.chars().count()),
                Tecla::Seta(Seta::Inicio) => campo.cursor = 0,
                Tecla::Seta(Seta::Fim) => campo.cursor = campo.texto.chars().count(),
                Tecla::Enter => enter = true,
                // Sem seleção de trecho: Ctrl+C copia o campo inteiro (e Ctrl+X o recorta).
                Tecla::Copiar => self.ui.pedidos.push(Pedido::Copiar(campo.texto.clone())),
                Tecla::Recortar => {
                    self.ui.pedidos.push(Pedido::Copiar(core::mem::take(&mut campo.texto)));
                    campo.cursor = 0;
                }
                Tecla::Colar => self.ui.pedidos.push(Pedido::Colar),
                _ => {}
            }
        }
        // Rola o texto para o cursor ficar sempre à vista.
        let px = largura_ate(self.ui, campo, campo.cursor);
        let visivel = largura - 12;
        if px - campo.rolagem > visivel {
            campo.rolagem = px - visivel;
        }
        if px < campo.rolagem {
            campo.rolagem = px;
        }
        let t = self.ui.tema;
        let focado = self.focado(id);
        self.q.retangulo_arredondado(x, y, largura, ALTURA, if focado { t.destaque } else { t.borda });
        self.q.retangulo_arredondado(x + 1, y + 1, largura - 2, ALTURA - 2, t.campo);
        let antes = self.q.recorte;
        self.q.recortar(x + 3, y + 2, largura - 6, ALTURA - 4);
        let topo = y + (ALTURA - self.ui.altura_linha(tamanho)) / 2;
        let texto = campo.texto.clone();
        self.escrever(x + 6 - campo.rolagem, topo, &texto, t.texto, false, tamanho);
        if focado {
            let cursor_x = x + 6 + px - campo.rolagem;
            self.q.retangulo(cursor_x, y + 5, 1, ALTURA - 10, t.texto);
        }
        self.q.recorte = antes;
        enter
    }

    /// Uma área de texto de várias linhas, com rolagem e seleção pelo mouse
    /// (o editor de texto). Ctrl+C, Ctrl+X e Ctrl+V usam a área de transferência.
    pub fn area_texto(&mut self, area: &mut AreaTexto, largura: i32, altura: i32) {
        let (x, y) = self.alocar(largura, altura);
        let id = self.novo_id();
        let tamanho = self.ui.tamanho;
        let (cw, lh) = self.ui.medidas_mono(tamanho);
        // A margem com os números das linhas.
        let margem = cw * 4 + 12;
        let (tx, ty) = (x + margem, y + 4);
        let visiveis = ((altura - 8) / lh).max(1) as usize;
        let colunas_visiveis = ((largura - margem - 18) / cw).max(1) as usize;
        let posicao_do_mouse = |area: &AreaTexto, mx: i32, my: i32| -> Posicao {
            let l = (area.rolagem.0 as i32 + (my - ty) / lh).clamp(0, area.linhas.len() as i32 - 1) as usize;
            let c = (area.rolagem.1 as i32 + (mx - tx + cw / 2) / cw).max(0) as usize;
            (l, c.min(area.colunas(l)))
        };
        let clicou_aqui = self.ui.clicou && self.mouse_em((x, y, largura, altura));
        // A área de texto é o centro do programa: sem ninguém com o foco, ele é dela.
        if self.ui.foco.is_none() && !self.bloqueado() {
            self.ui.foco = Some(id);
        }
        let (sobre, _) = self.interagir(id, (x, y, largura, altura), true);
        if clicou_aqui {
            area.cursor = posicao_do_mouse(area, self.ui.mouse.0, self.ui.mouse.1);
            area.ancora = Some(area.cursor);
        } else if self.ui.ativo == Some(id) && self.ui.apertado {
            // arrastando: a seleção vai até o mouse
            area.cursor = posicao_do_mouse(area, self.ui.mouse.0, self.ui.mouse.1);
            self.ui.redesenhar = true;
        }
        if sobre && self.ui.roda != 0 {
            let r = area.rolagem.0 as i32 + self.ui.roda * 3;
            area.rolagem.0 = r.clamp(0, area.linhas.len().saturating_sub(1) as i32) as usize;
            self.ui.roda = 0;
            self.ui.redesenhar = true;
        }
        // O teclado (com o foco).
        let mut mexeu = false;
        while self.focado(id) {
            let Some(i) = self.ui.teclas.iter().position(|t| !matches!(t, Tecla::Esc | Tecla::Controle(_))) else {
                break;
            };
            mexeu = true;
            self.ui.redesenhar = true;
            let (l, c) = area.cursor;
            let ultima = area.linhas.len() - 1;
            match self.ui.teclas.remove(i) {
                Tecla::Letra(ch) => area.inserir(&String::from(ch)),
                Tecla::Texto(t) => area.inserir(&t),
                Tecla::Enter => area.inserir("\n"),
                Tecla::Tab => area.inserir("    "),
                Tecla::Apagar => area.apagar(true),
                Tecla::Seta(Seta::Delete) => area.apagar(false),
                Tecla::Copiar => {
                    if area.selecao().is_some() {
                        self.ui.pedidos.push(Pedido::Copiar(area.texto_selecionado()));
                    }
                }
                Tecla::Recortar => {
                    if area.selecao().is_some() {
                        self.ui.pedidos.push(Pedido::Copiar(area.texto_selecionado()));
                        area.apagar_selecao();
                    }
                }
                Tecla::Colar => self.ui.pedidos.push(Pedido::Colar),
                Tecla::Seta(s) => {
                    area.ancora = None;
                    area.cursor = match s {
                        Seta::Esquerda if c > 0 => (l, c - 1),
                        Seta::Esquerda if l > 0 => (l - 1, area.colunas(l - 1)),
                        Seta::Direita if c < area.colunas(l) => (l, c + 1),
                        Seta::Direita if l < ultima => (l + 1, 0),
                        Seta::Cima if l > 0 => (l - 1, c.min(area.colunas(l - 1))),
                        Seta::Baixo if l < ultima => (l + 1, c.min(area.colunas(l + 1))),
                        Seta::Inicio => (l, 0),
                        Seta::Fim => (l, area.colunas(l)),
                        Seta::PaginaAcima => {
                            let n = l.saturating_sub(visiveis);
                            (n, c.min(area.colunas(n)))
                        }
                        Seta::PaginaAbaixo => {
                            let n = (l + visiveis).min(ultima);
                            (n, c.min(area.colunas(n)))
                        }
                        _ => (l, c),
                    };
                }
                _ => {}
            }
        }
        // Rola para o cursor ficar à vista (depois de digitar ou andar).
        if mexeu || clicou_aqui {
            let (l, c) = area.cursor;
            if l < area.rolagem.0 {
                area.rolagem.0 = l;
            }
            if l >= area.rolagem.0 + visiveis {
                area.rolagem.0 = l + 1 - visiveis;
            }
            if c < area.rolagem.1 {
                area.rolagem.1 = c;
            }
            if c >= area.rolagem.1 + colunas_visiveis {
                area.rolagem.1 = c + 1 - colunas_visiveis;
            }
        }

        // O desenho.
        let t = self.ui.tema;
        let focado = self.focado(id);
        self.q.retangulo(x, y, largura, altura, if focado { t.destaque } else { t.borda });
        self.q.retangulo(x + 1, y + 1, largura - 2, altura - 2, t.campo);
        self.q.retangulo(x + 1, y + 1, margem - 6, altura - 2, t.painel);
        let antes = self.q.recorte;
        let selecao = area.selecao();
        let ultima_visivel = (area.rolagem.0 + visiveis).min(area.linhas.len());
        for l in area.rolagem.0..ultima_visivel {
            let ly = ty + (l - area.rolagem.0) as i32 * lh;
            let numero = alloc::format!("{:>4}", l + 1);
            self.q.recortar(x + 1, y + 1, margem - 8, altura - 2);
            self.ui.escrever_mono(&mut self.q, x + 2, ly, &numero, t.texto_fraco, tamanho);
            self.q.recortar(tx - 2, y + 1, largura - margem - 4, altura - 2);
            // a parte selecionada desta linha
            if let Some((a, b)) = selecao {
                if (a.0..=b.0).contains(&l) {
                    let de = if l == a.0 { a.1 } else { 0 };
                    let ate = if l == b.0 { b.1 } else { area.colunas(l) + 1 };
                    let sx = tx + (de as i32 - area.rolagem.1 as i32) * cw;
                    self.q.retangulo(sx, ly, (ate - de) as i32 * cw, lh, t.realce);
                }
            }
            let texto: String = area.linhas[l].chars().skip(area.rolagem.1).take(colunas_visiveis + 1).collect();
            self.ui.escrever_mono(&mut self.q, tx, ly, &texto, t.texto, tamanho);
        }
        if focado {
            let (l, c) = area.cursor;
            if (area.rolagem.0..ultima_visivel).contains(&l) && c >= area.rolagem.1 {
                let cx = tx + (c - area.rolagem.1) as i32 * cw;
                let cy = ty + (l - area.rolagem.0) as i32 * lh;
                self.q.retangulo(cx, cy, 2, lh, t.destaque);
            }
        }
        self.q.recorte = antes;
    }

    /// Põe um texto na área de transferência.
    pub fn copiar(&mut self, texto: &str) {
        self.ui.pedidos.push(Pedido::Copiar(String::from(texto)));
    }

    /// Pede o texto da área de transferência: ele chega a quem tem o foco.
    pub fn pedir_colagem(&mut self) {
        self.ui.pedidos.push(Pedido::Colar);
    }

    /// Pede ao servidor para reler os ajustes (~/.janelas).
    pub fn pedir_recarga(&mut self) {
        self.ui.pedidos.push(Pedido::Recarregar);
    }

    /// Troca o título da janela.
    pub fn mudar_titulo(&mut self, titulo: &str) {
        self.ui.pedidos.push(Pedido::Titulo(String::from(titulo)));
    }

    /// Um atalho de teclado (Ctrl+letra): true se foi apertado neste evento.
    pub fn atalho(&mut self, letra: char) -> bool {
        match self.ui.teclas.iter().position(|t| *t == Tecla::Controle(letra)) {
            Some(i) => {
                self.ui.teclas.remove(i);
                true
            }
            None => false,
        }
    }

    // ------------------------------------------------------------- listas

    /// O que a lista e a tabela têm em comum: rolagem (roda, barra, setas),
    /// seleção (clique, setas), ativação (clique duplo, Enter). `desenhar`
    /// desenha o item `i` na faixa (x, y, largura, altura), com a cor do texto.
    #[allow(clippy::too_many_arguments)]
    fn lista_base(
        &mut self,
        lista: &mut Lista,
        n: usize,
        largura: i32,
        altura: i32,
        cabecalho: i32,
        item: i32,
        mut desenhar: impl FnMut(&mut Ui, &mut Quadro, usize, (i32, i32, i32, i32), u32),
    ) -> ((i32, i32), Option<usize>) {
        let (x, y) = self.alocar(largura, altura);
        let id = self.novo_id();
        let id_barra = self.novo_id();
        let corpo_y = y + cabecalho;
        let visivel = altura - 2 - cabecalho;
        let total = n as i32 * item;
        let maximo = (total - visivel).max(0);
        let barra = if total > visivel { 12 } else { 0 };
        let area_itens = (x, corpo_y, largura - barra, visivel + 1);
        let area_barra = (x + largura - barra - 1, corpo_y + 1, barra, visivel);
        let mut escolhido = None;

        let clicou_itens = self.ui.clicou && self.mouse_em(area_itens);
        let clicou_barra = barra > 0 && self.ui.clicou && self.mouse_em(area_barra);
        let (sobre, _) = self.interagir(id, (x, y, largura, altura), true);
        if sobre && self.ui.roda != 0 {
            lista.rolagem += self.ui.roda * item * 3;
            self.ui.roda = 0;
            self.ui.redesenhar = true;
        }
        if clicou_itens {
            let i = ((self.ui.mouse.1 - corpo_y - 1 + lista.rolagem) / item) as usize;
            if i < n {
                // Dois cliques rápidos no mesmo item: "ativa" (abre).
                let agora = crate::sys::tempo_ms();
                if lista.ultimo_clique.1 == i && agora - lista.ultimo_clique.0 < 450 {
                    lista.ativado = Some(i);
                }
                lista.ultimo_clique = (agora, i);
                lista.selecionado = Some(i);
                escolhido = Some(i);
                self.ui.redesenhar = true;
            }
        }
        // A barra: clicar ou arrastar leva a rolagem para onde está o mouse.
        let polegar = if total > 0 { (visivel * visivel / total.max(1)).max(20) } else { visivel };
        if clicou_barra {
            self.ui.ativo = Some(id_barra);
        }
        if barra > 0 && self.ui.ativo == Some(id_barra) && (self.ui.apertado || self.ui.clicou) {
            let curso = (visivel - polegar).max(1);
            lista.rolagem = (self.ui.mouse.1 - corpo_y - 1 - polegar / 2) * maximo / curso;
            self.ui.redesenhar = true;
        }
        let pagina = (visivel / item).max(1) as usize;
        let teclas = [
            Tecla::Seta(Seta::Cima),
            Tecla::Seta(Seta::Baixo),
            Tecla::Seta(Seta::Inicio),
            Tecla::Seta(Seta::Fim),
            Tecla::Seta(Seta::PaginaAcima),
            Tecla::Seta(Seta::PaginaAbaixo),
            Tecla::Enter,
        ];
        while let Some(t) = self.tecla(id, &teclas) {
            if n == 0 {
                continue;
            }
            let atual = lista.selecionado;
            lista.selecionado = Some(match t {
                Tecla::Seta(Seta::Cima) => atual.map_or(0, |s| s.saturating_sub(1)),
                Tecla::Seta(Seta::Baixo) => atual.map_or(0, |s| (s + 1).min(n - 1)),
                Tecla::Seta(Seta::Inicio) => 0,
                Tecla::Seta(Seta::Fim) => n - 1,
                Tecla::Seta(Seta::PaginaAcima) => atual.map_or(0, |s| s.saturating_sub(pagina)),
                Tecla::Seta(Seta::PaginaAbaixo) => atual.map_or(0, |s| (s + pagina).min(n - 1)),
                _ => {
                    lista.ativado = atual;
                    atual.unwrap_or(0)
                }
            });
            // o escolhido fica à vista
            if let Some(s) = lista.selecionado {
                let topo = s as i32 * item;
                if topo < lista.rolagem {
                    lista.rolagem = topo;
                }
                if topo + item > lista.rolagem + visivel {
                    lista.rolagem = topo + item - visivel;
                }
            }
        }
        lista.rolagem = lista.rolagem.clamp(0, maximo);
        if lista.selecionado.is_some_and(|s| s >= n) {
            lista.selecionado = None;
        }

        let t = self.ui.tema;
        let focado = self.focado(id);
        self.q.retangulo(x, y, largura, altura, if focado { t.destaque } else { t.borda });
        self.q.retangulo(x + 1, y + 1, largura - 2, altura - 2, t.campo);
        let antes = self.q.recorte;
        self.q.recortar(x + 1, corpo_y + 1, largura - 2 - barra, visivel);
        let primeiro = (lista.rolagem / item) as usize;
        let ultimo = (((lista.rolagem + visivel) / item) as usize + 1).min(n);
        for i in primeiro..ultimo {
            let iy = corpo_y + 1 + i as i32 * item - lista.rolagem;
            let faixa = (x + 1, iy, largura - 2 - barra, item);
            let cor = if lista.selecionado == Some(i) {
                self.q.retangulo(faixa.0, faixa.1, faixa.2, faixa.3, t.destaque);
                t.texto_destaque
            } else {
                if sobre && contem(faixa, self.ui.mouse) {
                    self.q.retangulo(faixa.0, faixa.1, faixa.2, faixa.3, t.realce);
                }
                t.texto
            };
            desenhar(self.ui, &mut self.q, i, faixa, cor);
        }
        self.q.recorte = antes;
        if barra > 0 {
            let (bx, by, bw, bh) = area_barra;
            self.q.retangulo(bx, by, bw, bh, t.painel);
            let py = by + if maximo > 0 { lista.rolagem * (visivel - polegar) / maximo } else { 0 };
            let cor = if self.ui.ativo == Some(id_barra) { t.destaque } else { t.borda };
            self.q.retangulo_arredondado(bx + 2, py + 1, bw - 4, polegar - 2, cor);
        }
        ((x, y), escolhido)
    }

    /// Lista de itens com rolagem (roda do mouse, barra ou setas). Devolve o
    /// item escolhido neste evento (clique ou Enter); o clique duplo e o
    /// Enter também marcam `lista.ativado`.
    pub fn lista<S: AsRef<str>>(&mut self, lista: &mut Lista, itens: &[S], largura: i32, altura: i32) -> Option<usize> {
        let tamanho = self.ui.tamanho;
        let item = self.ui.altura_linha(tamanho) + 6;
        let topo = (item - self.ui.altura_linha(tamanho)) / 2;
        let ((_, _), escolhido) = self.lista_base(lista, itens.len(), largura, altura, 0, item, |ui, q, i, (x, y, _, _), cor| {
            ui.escrever(q, x + 7, y + topo, itens[i].as_ref(), cor, false, tamanho);
        });
        escolhido
    }

    /// Uma tabela: colunas com título (nome, largura) e, em cada linha, um
    /// ícone e os textos das colunas. Funciona como a lista.
    pub fn tabela(
        &mut self,
        lista: &mut Lista,
        colunas: &[(&str, i32)],
        linhas: &[(Icone, Vec<String>)],
        largura: i32,
        altura: i32,
    ) -> Option<usize> {
        let tamanho = self.ui.tamanho;
        let item = self.ui.altura_linha(tamanho) + 8;
        let topo = (item - self.ui.altura_linha(tamanho)) / 2;
        let cabecalho = item;
        let ((x, y), escolhido) =
            self.lista_base(lista, linhas.len(), largura, altura, cabecalho, item, |ui, q, i, (x, y, _, h), cor| {
                let (icone, textos) = &linhas[i];
                desenhar_icone(q, *icone, x + 6, y + (h - 16) / 2);
                let mut cx = x + 30;
                for (k, texto) in textos.iter().enumerate() {
                    let w = colunas.get(k).map_or(100, |c| c.1);
                    let antes = q.recorte;
                    let (rx, ry, rw, rh) = antes;
                    let fim = (cx + w - 8).min(rx + rw);
                    q.recorte = (cx.max(rx), ry, (fim - cx.max(rx)).max(0), rh);
                    ui.escrever(q, cx, y + topo, texto, cor, false, tamanho);
                    q.recorte = antes;
                    cx += w;
                }
            });
        // O cabeçalho das colunas.
        let t = self.ui.tema;
        self.q.retangulo(x + 1, y + 1, largura - 2, cabecalho - 1, t.painel);
        self.q.retangulo(x + 1, y + cabecalho, largura - 2, 1, t.borda);
        let mut cx = x + 30;
        for (nome, w) in colunas {
            self.escrever(cx, y + topo, nome, t.texto_fraco, true, tamanho - 1);
            cx += w;
        }
        escolhido
    }

    // ------------------------------------------------------------- valores

    /// Barra de progresso (`fracao` de 0.0 a 1.0).
    pub fn progresso(&mut self, fracao: f32, largura: i32) {
        let (x, y) = self.alocar(largura, 12);
        let t = self.ui.tema;
        self.q.retangulo_arredondado(x, y, largura, 12, t.borda);
        self.q.retangulo_arredondado(x + 1, y + 1, largura - 2, 10, t.painel);
        let cheio = ((largura - 2) as f32 * fracao.clamp(0.0, 1.0)) as i32;
        if cheio >= 6 {
            self.q.retangulo_arredondado(x + 1, y + 1, cheio, 10, t.destaque);
        }
    }

    /// Controle deslizante entre `minimo` e `maximo`. Devolve true quando o valor muda.
    pub fn deslizante(&mut self, valor: &mut i32, minimo: i32, maximo: i32, largura: i32) -> bool {
        let (x, y) = self.alocar(largura, ALTURA);
        let id = self.novo_id();
        let (sobre, _) = self.interagir(id, (x, y, largura, ALTURA), true);
        let antes = *valor;
        let curso = (largura - 16).max(1);
        if self.ui.ativo == Some(id) && (self.ui.apertado || self.ui.clicou) {
            *valor = minimo + ((self.ui.mouse.0 - x - 8) * (maximo - minimo) + curso / 2) / curso;
        }
        while let Some(t) = self.tecla(id, &[Tecla::Seta(Seta::Esquerda), Tecla::Seta(Seta::Direita)]) {
            *valor += if t == Tecla::Seta(Seta::Direita) { 1 } else { -1 };
        }
        *valor = (*valor).clamp(minimo, maximo);
        let t = self.ui.tema;
        let meio = y + ALTURA / 2;
        let px = x + 8 + if maximo > minimo { (*valor - minimo) * curso / (maximo - minimo) } else { 0 };
        self.q.retangulo_arredondado(x + 8, meio - 3, curso, 6, t.borda);
        self.q.retangulo(x + 8, meio - 2, px - x - 8, 4, t.destaque);
        let borda = if self.focado(id) || sobre { t.destaque } else { t.borda };
        self.q.circulo(px, meio, 8, borda);
        self.q.circulo(px, meio, 7, t.botao_sobre);
        if *valor != antes {
            self.ui.redesenhar = true;
        }
        *valor != antes
    }

    // ------------------------------------------------------------- menus

    /// A barra de menus, no alto da janela (chame primeiro). `menus`: (título,
    /// itens). Devolve (menu, item) quando um item é escolhido.
    pub fn barra_de_menus(&mut self, menus: &[(&str, &[&str])]) -> Option<(usize, usize)> {
        let t = self.ui.tema;
        let tamanho = self.ui.tamanho;
        let largura = self.largura;
        self.q.retangulo(0, 0, largura, ALTURA_MENU, t.painel);
        self.q.retangulo(0, ALTURA_MENU, largura, 1, t.borda);
        let topo_texto = (ALTURA_MENU - self.ui.altura_linha(tamanho)) / 2;
        let mut x = 4;
        let mut escolha = None;
        let mut posicoes = Vec::new();
        for (i, (nome, _)) in menus.iter().enumerate() {
            let w = self.ui.largura_texto(nome, tamanho, false) + 20;
            let area = (x, 0, w, ALTURA_MENU);
            let sobre = !self.bloqueado() && contem(area, self.ui.mouse);
            if sobre && self.ui.clicou {
                self.ui.menu_aberto = if self.ui.menu_aberto == Some(i) { None } else { Some(i) };
                self.clique_usado = true;
                self.ui.redesenhar = true;
            } else if sobre && self.ui.menu_aberto.is_some_and(|m| m != i) {
                self.ui.menu_aberto = Some(i); // com um menu aberto, passar por cima troca
                self.ui.redesenhar = true;
            }
            posicoes.push(x);
            x += w;
        }
        for (i, (nome, _)) in menus.iter().enumerate() {
            let w = self.ui.largura_texto(nome, tamanho, false) + 20;
            let aberto = self.ui.menu_aberto == Some(i);
            let sobre = !self.bloqueado() && contem((posicoes[i], 0, w, ALTURA_MENU), self.ui.mouse);
            let cor = if aberto {
                self.q.retangulo(posicoes[i], 0, w, ALTURA_MENU, t.destaque);
                t.texto_destaque
            } else {
                if sobre {
                    self.q.retangulo(posicoes[i], 0, w, ALTURA_MENU, t.realce);
                }
                t.texto
            };
            self.escrever(posicoes[i] + 10, topo_texto, nome, cor, false, tamanho);
        }
        // O menu suspenso aberto: os cliques nele são tratados aqui, mas ele é
        // desenhado no fim (por cima de tudo).
        if let Some(m) = self.ui.menu_aberto.filter(|&m| m < menus.len()) {
            let itens = menus[m].1;
            let item = ALTURA_MENU;
            let w = itens.iter().map(|i| self.ui.largura_texto(i, tamanho, false)).max().unwrap_or(0) + 40;
            let area = (posicoes[m], ALTURA_MENU, w.max(120), itens.len() as i32 * item + 8);
            let mut sobre = None;
            self.dentro_do_menu = true;
            if self.mouse_em(area) {
                let k = (self.ui.mouse.1 - ALTURA_MENU - 4) / item;
                if (0..itens.len() as i32).contains(&k) {
                    sobre = Some(k as usize);
                }
                if self.ui.clicou {
                    self.clique_usado = true;
                    if let Some(k) = sobre {
                        escolha = Some((m, k));
                        self.ui.menu_aberto = None;
                        self.ui.redesenhar = true;
                    }
                }
            }
            self.dentro_do_menu = false;
            if self.ui.menu_aberto.is_some() {
                self.menu_pendente = Some((area, itens.iter().map(|s| String::from(*s)).collect(), sobre));
                self.sobreposto_agora = Some(area);
            }
        }
        self.cy = self.cy.max(ALTURA_MENU + MARGEM);
        escolha
    }

    // ------------------------------------------------------------- diálogos

    /// Uma caixa de diálogo por cima de tudo (chame por último). Enquanto
    /// ela está aberta, o resto da janela não recebe nada. Devolve o botão
    /// escolhido (Enter = o primeiro, Esc = o último), e a fecha.
    pub fn dialogo(&mut self, dialogo: &mut Option<Dialogo>) -> Option<usize> {
        let d = dialogo.as_ref()?;
        let (titulo, mensagem, botoes) = (d.titulo.clone(), d.mensagem.clone(), d.botoes.clone());
        self.modal_agora = true;
        self.dentro_do_modal = true;
        let t = self.ui.tema;
        let tamanho = self.ui.tamanho;
        self.q.sem_recorte();
        self.q.escurecer(90);
        let largura = 380.min(self.largura - 40);
        let linhas = self.ui.quebrar(&mensagem, largura - 40, tamanho);
        let altura_linha = self.ui.altura_linha(tamanho) + 2;
        let com_campo = dialogo.as_ref().is_some_and(|d| d.campo.is_some());
        let altura = 20 + 26 + linhas.len() as i32 * altura_linha + 16 + ALTURA + 20 + if com_campo { ALTURA + 12 } else { 0 };
        let (x, y) = ((self.largura - largura) / 2, (self.q.altura - altura) / 2);
        self.q.retangulo(x + 3, y + 3, largura, altura, 0x404040);
        self.q.retangulo(x, y, largura, altura, t.borda);
        self.q.retangulo(x + 1, y + 1, largura - 2, altura - 2, t.fundo);
        self.q.retangulo(x + 1, y + 1, largura - 2, 3, t.destaque);
        self.escrever(x + 20, y + 18, &titulo, t.texto, true, tamanho + 2);
        for (i, l) in linhas.iter().enumerate() {
            self.escrever(x + 20, y + 46 + i as i32 * altura_linha, l, t.texto, false, tamanho);
        }
        let mut escolha = None;
        // O campo de texto (sempre com o foco; Enter = o primeiro botão).
        if let Some(campo) = dialogo.as_mut().and_then(|d| d.campo.as_mut()) {
            let cy = y + 46 + linhas.len() as i32 * altura_linha + 8;
            if self.campo_em(campo, x + 20, cy, largura - 40, true) {
                escolha = Some(0);
            }
        }
        // Os botões, alinhados à direita.
        let larguras: Vec<i32> = botoes.iter().map(|b| self.largura_botao(b)).collect();
        let mut bx = x + largura - 20 - larguras.iter().sum::<i32>() - ESPACO * (botoes.len() as i32 - 1);
        let by = y + altura - 20 - ALTURA;
        for (i, b) in botoes.iter().enumerate() {
            if self.botao_em(bx, by, larguras[i], b) {
                escolha = Some(i);
            }
            bx += larguras[i] + ESPACO;
        }
        if let Some(i) = self.ui.teclas.iter().position(|k| matches!(k, Tecla::Enter | Tecla::Esc)) {
            let k = self.ui.teclas.remove(i);
            escolha = Some(if k == Tecla::Enter { 0 } else { botoes.len() - 1 });
        }
        if self.ui.clicou {
            self.clique_usado = true; // um clique fora da caixa não vai para ninguém
        }
        self.dentro_do_modal = false;
        if escolha.is_some() {
            if let Some(campo) = dialogo.take().and_then(|d| d.campo) {
                self.ui.resposta = campo.texto;
            }
            self.ui.redesenhar = true;
        }
        escolha
    }

    /// O texto que foi digitado na última caixa de diálogo com campo.
    pub fn texto_do_dialogo(&mut self) -> String {
        core::mem::take(&mut self.ui.resposta)
    }
}

impl Drop for Tela<'_> {
    /// O fim do desenho: o menu suspenso por cima, o foco, e o evento consumido.
    fn drop(&mut self) {
        let t = self.ui.tema;
        if let Some(((x, y, w, h), itens, sobre)) = self.menu_pendente.take() {
            let tamanho = self.ui.tamanho;
            self.q.sem_recorte();
            self.q.retangulo(x + 2, y + 2, w, h, 0xA0A0A0);
            self.q.retangulo(x, y, w, h, t.borda);
            self.q.retangulo(x + 1, y + 1, w - 2, h - 2, t.campo);
            let topo = (ALTURA_MENU - self.ui.altura_linha(tamanho)) / 2;
            for (k, item) in itens.iter().enumerate() {
                let iy = y + 4 + k as i32 * ALTURA_MENU;
                let cor = if sobre == Some(k) {
                    self.q.retangulo(x + 1, iy, w - 2, ALTURA_MENU, t.destaque);
                    t.texto_destaque
                } else {
                    t.texto
                };
                self.ui.escrever(&mut self.q, x + 16, iy + topo, item, cor, false, tamanho);
            }
        }
        // Um clique que ninguém usou: tira o foco e fecha o menu.
        if self.ui.clicou && !self.clique_usado {
            self.ui.foco = None;
            if self.ui.menu_aberto.take().is_some() {
                self.ui.redesenhar = true;
            }
        }
        // Tab passa o foco adiante; Esc fecha o menu.
        for tecla in core::mem::take(&mut self.ui.teclas) {
            match tecla {
                Tecla::Tab if !self.focaveis.is_empty() => {
                    let atual = self.ui.foco.and_then(|f| self.focaveis.iter().position(|&i| i == f));
                    self.ui.foco = Some(self.focaveis[atual.map_or(0, |i| (i + 1) % self.focaveis.len())]);
                    self.ui.redesenhar = true;
                }
                Tecla::Esc if self.ui.menu_aberto.is_some() => {
                    self.ui.menu_aberto = None;
                    self.ui.redesenhar = true;
                }
                _ => {}
            }
        }
        if self.ui.soltou {
            self.ui.ativo = None;
        }
        self.ui.sobreposto = self.sobreposto_agora;
        if self.ui.modal != self.modal_agora {
            self.ui.redesenhar = true;
        }
        self.ui.modal = self.modal_agora;
        self.ui.clicou = false;
        self.ui.soltou = false;
        self.ui.roda = 0;
    }
}

/// O laço de um programa com janela: desenha com `f`, mostra, espera um
/// evento, e de novo — até a janela ser fechada (pelo "x" ou por `t.fechar_janela()`).
pub fn laco(janela: &mut Janela, ui: &mut Ui, f: impl FnMut(&mut Tela)) {
    laco_com_prazo(janela, ui, -1, f)
}

/// Como `laco`, mas redesenha também a cada `ms` milissegundos sem eventos
/// (para o que se atualiza sozinho, como um relógio ou o monitor do sistema).
pub fn laco_com_prazo(janela: &mut Janela, ui: &mut Ui, ms: i32, mut f: impl FnMut(&mut Tela)) {
    loop {
        let mut vezes = 0;
        loop {
            {
                let mut t = ui.comecar(janela.quadro());
                f(&mut t);
            }
            vezes += 1;
            // Um clique muda o estado do programa: desenha de novo com ele.
            if !ui.precisa_redesenhar() || vezes == 3 {
                break;
            }
        }
        for pedido in core::mem::take(&mut ui.pedidos) {
            match pedido {
                Pedido::Copiar(texto) => janela.copiar(&texto),
                Pedido::Colar => janela.pedir_colagem(),
                Pedido::Titulo(t) => janela.mudar_titulo(&t),
                Pedido::Recarregar => janela.pedir_recarga(),
            }
        }
        janela.mostrar();
        if ui.saindo() {
            return;
        }
        let prazo = match ui.acordar.take() {
            Some(a) if ms < 0 || a < ms => a,
            _ => ms,
        };
        match janela.evento_com_prazo(prazo) {
            Some(Some(e)) => ui.receber(&e),
            Some(None) => {} // o prazo passou: desenha de novo
            None => return,
        }
        if ui.saindo() {
            return;
        }
    }
}
