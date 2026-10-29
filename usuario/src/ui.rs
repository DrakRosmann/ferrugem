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

const MARGEM: i32 = 12;
const ESPACO: i32 = 8;
const ALTURA: i32 = 26;
const ALTURA_MENU: i32 = 24;

#[derive(Clone, Copy, PartialEq)]
pub enum Tecla {
    Letra(char),
    Apagar,
    Enter,
    Tab,
    Esc,
    Seta(Seta),
    /// Ctrl+C e Ctrl+V.
    Copiar,
    Colar,
}

/// O que os componentes pedem à janela (a área de transferência).
pub enum Pedido {
    Copiar(String),
    Colar,
}

/// O estado de um campo de texto: o texto e onde está o cursor (em caracteres).
pub struct CampoTexto {
    pub texto: String,
    pub cursor: usize,
    rolagem: i32,
}

impl CampoTexto {
    pub fn novo(texto: &str) -> CampoTexto {
        CampoTexto { texto: String::from(texto), cursor: texto.chars().count(), rolagem: 0 }
    }

    fn byte(&self, caractere: usize) -> usize {
        self.texto.char_indices().nth(caractere).map_or(self.texto.len(), |(i, _)| i)
    }
}

/// O estado de uma lista: o item escolhido e quanto ela foi rolada (em pixels).
#[derive(Default)]
pub struct Lista {
    pub selecionado: Option<usize>,
    pub rolagem: i32,
}

/// Uma caixa de diálogo: um título, uma mensagem e os botões.
pub struct Dialogo {
    pub titulo: String,
    pub mensagem: String,
    pub botoes: Vec<String>,
}

impl Dialogo {
    pub fn novo(titulo: &str, mensagem: &str, botoes: &[&str]) -> Dialogo {
        Dialogo {
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
    vga: Option<Fonte>,
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
            vga: Fonte::carregar(),
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
                c => Tecla::Letra(c),
            }),
            Evento::Fechar => self.sair = true,
            Evento::Tamanho { .. } => self.redesenhar = true,
            // O texto colado chega como se tivesse sido digitado.
            Evento::Colar(ref texto) => {
                self.teclas.extend(texto.chars().filter(|c| !c.is_control()).map(Tecla::Letra));
            }
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
        let id = self.novo_id();
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
            Tecla::Copiar,
            Tecla::Colar,
        ];
        while self.focado(id) {
            let Some(i) = self.ui.teclas.iter().position(|t| matches!(t, Tecla::Letra(_)) || teclas.contains(t)) else {
                break;
            };
            self.ui.redesenhar = true;
            match self.ui.teclas.remove(i) {
                Tecla::Letra(c) => {
                    let b = campo.byte(campo.cursor);
                    campo.texto.insert(b, c);
                    campo.cursor += 1;
                }
                Tecla::Apagar if campo.cursor > 0 => {
                    campo.cursor -= 1;
                    let b = campo.byte(campo.cursor);
                    campo.texto.remove(b);
                }
                Tecla::Seta(Seta::Esquerda) => campo.cursor = campo.cursor.saturating_sub(1),
                Tecla::Seta(Seta::Direita) => campo.cursor = (campo.cursor + 1).min(campo.texto.chars().count()),
                Tecla::Enter => enter = true,
                // Sem seleção de trecho: Ctrl+C copia o campo inteiro.
                Tecla::Copiar => self.ui.pedidos.push(Pedido::Copiar(campo.texto.clone())),
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
        self.q.recortar(x + 3, y + 2, largura - 6, ALTURA - 4);
        let topo = y + (ALTURA - self.ui.altura_linha(tamanho)) / 2;
        let texto = campo.texto.clone();
        self.escrever(x + 6 - campo.rolagem, topo, &texto, t.texto, false, tamanho);
        if focado {
            let cursor_x = x + 6 + px - campo.rolagem;
            self.q.retangulo(cursor_x, y + 5, 1, ALTURA - 10, t.texto);
        }
        self.q.sem_recorte();
        enter
    }

    // ------------------------------------------------------------- listas

    /// Lista de itens com rolagem (roda do mouse, barra ou setas). Devolve o
    /// item escolhido neste evento (clique ou Enter).
    pub fn lista<S: AsRef<str>>(&mut self, lista: &mut Lista, itens: &[S], largura: i32, altura: i32) -> Option<usize> {
        let (x, y) = self.alocar(largura, altura);
        let id = self.novo_id();
        let id_barra = self.novo_id();
        let tamanho = self.ui.tamanho;
        let item = self.ui.altura_linha(tamanho) + 6;
        let visivel = altura - 2;
        let total = itens.len() as i32 * item;
        let maximo = (total - visivel).max(0);
        let barra = if total > visivel { 12 } else { 0 };
        let area_itens = (x, y, largura - barra, altura);
        let area_barra = (x + largura - barra - 1, y + 1, barra, visivel);
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
            let i = ((self.ui.mouse.1 - y - 1 + lista.rolagem) / item) as usize;
            if i < itens.len() {
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
            lista.rolagem = (self.ui.mouse.1 - y - 1 - polegar / 2) * maximo / curso;
            self.ui.redesenhar = true;
        }
        // Setas e Enter com o foco.
        let teclas = [Tecla::Seta(Seta::Cima), Tecla::Seta(Seta::Baixo), Tecla::Enter];
        while let Some(t) = self.tecla(id, &teclas) {
            let n = itens.len();
            match t {
                Tecla::Seta(Seta::Cima) if n > 0 => lista.selecionado = Some(lista.selecionado.map_or(0, |s| s.saturating_sub(1))),
                Tecla::Seta(Seta::Baixo) if n > 0 => {
                    lista.selecionado = Some(lista.selecionado.map_or(0, |s| (s + 1).min(n - 1)))
                }
                Tecla::Enter => escolhido = lista.selecionado,
                _ => {}
            }
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

        let t = self.ui.tema;
        let focado = self.focado(id);
        self.q.retangulo(x, y, largura, altura, if focado { t.destaque } else { t.borda });
        self.q.retangulo(x + 1, y + 1, largura - 2, altura - 2, t.campo);
        self.q.recortar(x + 1, y + 1, largura - 2 - barra, visivel);
        let primeiro = (lista.rolagem / item) as usize;
        let ultimo = (((lista.rolagem + visivel) / item) as usize + 1).min(itens.len());
        let topo_texto = (item - self.ui.altura_linha(tamanho)) / 2;
        for i in primeiro..ultimo {
            let iy = y + 1 + i as i32 * item - lista.rolagem;
            let selecionado = lista.selecionado == Some(i);
            let cor = if selecionado {
                self.q.retangulo(x + 1, iy, largura - 2 - barra, item, t.destaque);
                t.texto_destaque
            } else {
                if sobre && contem((x, iy, largura - barra, item), self.ui.mouse) {
                    self.q.retangulo(x + 1, iy, largura - 2 - barra, item, t.realce);
                }
                t.texto
            };
            self.escrever(x + 8, iy + topo_texto, itens[i].as_ref(), cor, false, tamanho);
        }
        self.q.sem_recorte();
        if barra > 0 {
            let (bx, by, bw, bh) = area_barra;
            self.q.retangulo(bx, by, bw, bh, t.painel);
            let py = by + if maximo > 0 { lista.rolagem * (visivel - polegar) / maximo } else { 0 };
            let cor = if self.ui.ativo == Some(id_barra) { t.destaque } else { t.borda };
            self.q.retangulo_arredondado(bx + 2, py + 1, bw - 4, polegar - 2, cor);
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
        let altura = 20 + 26 + linhas.len() as i32 * altura_linha + 16 + ALTURA + 20;
        let (x, y) = ((self.largura - largura) / 2, (self.q.altura - altura) / 2);
        self.q.retangulo(x + 3, y + 3, largura, altura, 0x404040);
        self.q.retangulo(x, y, largura, altura, t.borda);
        self.q.retangulo(x + 1, y + 1, largura - 2, altura - 2, t.fundo);
        self.q.retangulo(x + 1, y + 1, largura - 2, 3, t.destaque);
        self.escrever(x + 20, y + 18, &titulo, t.texto, true, tamanho + 2);
        for (i, l) in linhas.iter().enumerate() {
            self.escrever(x + 20, y + 46 + i as i32 * altura_linha, l, t.texto, false, tamanho);
        }
        // Os botões, alinhados à direita.
        let larguras: Vec<i32> = botoes.iter().map(|b| self.largura_botao(b)).collect();
        let mut bx = x + largura - 20 - larguras.iter().sum::<i32>() - ESPACO * (botoes.len() as i32 - 1);
        let by = y + altura - 20 - ALTURA;
        let mut escolha = None;
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
            *dialogo = None;
            self.ui.redesenhar = true;
        }
        escolha
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
pub fn laco(janela: &mut Janela, ui: &mut Ui, mut f: impl FnMut(&mut Tela)) {
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
            }
        }
        janela.mostrar();
        if ui.saindo() {
            return;
        }
        match janela.evento() {
            Some(e) => ui.receber(&e),
            None => return,
        }
        if ui.saindo() {
            return;
        }
    }
}
