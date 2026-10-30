//! janelas — o servidor de janelas: a interface gráfica do FerrugemOS.
//!
//! ```text
//!   janelas          (no console; Ctrl+Alt+Backspace derruba, se travar)
//! ```
//!
//! Ele abre o framebuffer (`/dev/fb0`, que põe a tela no modo gráfico), o
//! mouse e o teclado, e desenha tudo: o painel de cima (com o menu
//! Aplicativos e o relógio), a barra de tarefas embaixo e as janelas. Elas
//! são arrastadas pelo título, redimensionadas pelo canto de baixo,
//! minimizadas, maximizadas (também com dois cliques no título) e fechadas
//! pelos botões do título. Alt+Tab traz a próxima para a frente. Dois tipos
//! de janela:
//!
//! - **Terminais**: o próprio servidor emula um terminal ligado a um
//!   pseudo-terminal (`/dev/ptmx`), com um `sh` do outro lado. O que você
//!   digita vai para o mestre; o que o shell escreve volta por ele e é
//!   desenhado (com as cores ANSI). Arrastar seleciona e copia; o botão
//!   direito cola.
//! - **Programas**: qualquer programa pode pedir uma janela pelo socket
//!   `/tmp/.janelas` (veja `ulib::janela` e `ulib::ui`). A imagem dela fica
//!   numa memória compartilhada que o programa desenha e nós só copiamos.
//!   A área de transferência (copiar e colar) passa por aqui também.
//!
//! Tudo num laço só: um `poll` espera o mouse, o teclado, o socket e cada
//! janela. O que muda marca um retângulo como "sujo"; só a união deles é
//! redesenhada (numa imagem na memória) e copiada para a tela. O cursor do
//! mouse é desenhado por cima, direto na tela, para que mexer o mouse não
//! exija redesenhar nada.

#![no_std]
#![no_main]

use ulib::grafico::{Fonte, Quadro, ALTURA_LETRA, LARGURA_LETRA};
use ulib::truetype::FonteTT;
use ulib::sys;
use ulib::*;

programa!(principal);

const PAINEL: i32 = 24;
const TITULO: i32 = 20;
const BORDA: i32 = 2;

const COR_FUNDO: u32 = 0x1F4E5F;
const COR_PAINEL: u32 = 0x1B1F27;
const COR_ATIVA: u32 = 0xB7410E; // cor de ferrugem
const COR_INATIVA: u32 = 0x4C566A;
const COR_BORDA: u32 = 0x0B0D10;
const COR_TEXTO: u32 = 0xECEFF4;

/// As 16 cores ANSI (na ordem dos códigos 30..37 e 90..97).
const PALETA: [u32; 16] = [
    0x000000, 0xCC3333, 0x33AA33, 0xC8A020, 0x3366CC, 0xAA44AA, 0x2AA1B3, 0xC0C0C0, //
    0x606060, 0xFF6666, 0x66DD66, 0xFFE066, 0x6699FF, 0xFF77FF, 0x55E0F0, 0xFFFFFF,
];

/// O desenho do cursor: '#' = preto (contorno), '.' = branco.
const CURSOR: [&str; 16] = [
    "#", "##", "#.#", "#..#", "#...#", "#....#", "#.....#", "#......#", "#.......#", "#........#", "#.....####",
    "#..#..#", "#.# #..#", "##  #..#", "#    #..#", "      ##",
];

// -----------------------------------------------------------------------------
// As letras da interface
// -----------------------------------------------------------------------------

/// Painel, botões e títulos usam uma fonte TrueType (com bordas suaves), se
/// houver em /usr/share/fonts; os terminais, a fonte da VGA (monoespaçada).
struct Letras {
    vga: Fonte,
    normal: Option<FonteTT>,
    negrito: Option<FonteTT>,
}

const TAMANHO_LETRA: u16 = 13;

impl Letras {
    fn carregar(vga: Fonte) -> Letras {
        let normal = FonteTT::carregar("/usr/share/fonts/DejaVuSans.ttf");
        let negrito = FonteTT::carregar("/usr/share/fonts/DejaVuSans-Bold.ttf");
        Letras { vga, normal, negrito }
    }

    fn tt(&mut self, negrito: bool) -> Option<&mut FonteTT> {
        if negrito && self.negrito.is_some() { self.negrito.as_mut() } else { self.normal.as_mut() }
    }

    /// Texto centrado na vertical numa faixa de `altura` pixels que começa em `y`.
    fn escrever(&mut self, q: &mut Quadro, x: i32, y: i32, altura: i32, texto: &str, cor: u32, negrito: bool) -> i32 {
        match self.tt(negrito) {
            Some(f) => {
                let (_, linha) = f.metricas(TAMANHO_LETRA);
                q.texto_tt(f, x, y + (altura - linha) / 2, texto, TAMANHO_LETRA, cor)
            }
            None => {
                q.texto(&self.vga, x, y + (altura - ALTURA_LETRA) / 2, texto, cor, None);
                texto.chars().count() as i32 * LARGURA_LETRA
            }
        }
    }

    fn largura(&mut self, texto: &str, negrito: bool) -> i32 {
        self.largura_em(texto, negrito, TAMANHO_LETRA)
    }

    fn largura_em(&mut self, texto: &str, negrito: bool, tamanho: u16) -> i32 {
        match self.tt(negrito) {
            Some(f) => f.largura(texto, tamanho),
            None => texto.chars().count() as i32 * LARGURA_LETRA,
        }
    }

    /// Texto em qualquer tamanho, com o topo em `y` (a fonte da VGA não muda de tamanho).
    fn escrever_em(&mut self, q: &mut Quadro, x: i32, y: i32, texto: &str, cor: u32, negrito: bool, tamanho: u16) -> i32 {
        match self.tt(negrito) {
            Some(f) => q.texto_tt(f, x, y, texto, tamanho, cor),
            None => {
                q.texto(&self.vga, x, y, texto, cor, None);
                texto.chars().count() as i32 * LARGURA_LETRA
            }
        }
    }

    /// Texto centrado em `cx`.
    fn centrado(&mut self, q: &mut Quadro, cx: i32, y: i32, texto: &str, cor: u32, negrito: bool, tamanho: u16) {
        let w = self.largura_em(texto, negrito, tamanho);
        self.escrever_em(q, cx - w / 2, y, texto, cor, negrito, tamanho);
    }
}

// -----------------------------------------------------------------------------
// O emulador de terminal
// -----------------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq)]
struct Celula {
    c: char,
    frente: u8,
    fundo: u8,
}

const VAZIA: Celula = Celula { c: ' ', frente: 7, fundo: 0 };

struct Terminal {
    mestre: i32,
    pid: i32,
    colunas: usize,
    linhas: usize,
    celulas: Vec<Celula>,
    cx: usize,
    cy: usize,
    frente: u8,
    fundo: u8,
    claro: bool,
    /// 0 = texto normal, 1 = depois de ESC, 2 = dentro de ESC [ ... (CSI)
    escape: u8,
    parametros: Vec<u16>,
    utf8: Vec<u8>,
    /// Texto selecionado com o mouse: (primeira, última) célula, em ordem.
    selecao: Option<(usize, usize)>,
    /// Onde a seleção começou (o outro lado segue o mouse).
    ancora: usize,
}

impl Terminal {
    /// Cria o pseudo-terminal e um `sh` do outro lado.
    fn novo(colunas: usize, linhas: usize) -> Option<Terminal> {
        let (mestre, escravo) = sys::abrir_pty().ok()?;
        sys::fechar_no_exec(mestre);
        let _ = sys::definir_tamanho_terminal(mestre, linhas as u16, colunas as u16);
        let pid = match sys::fork() {
            Ok(0) => {
                // Uma sessão nova cujo terminal de controle é o escravo.
                let _ = sys::nova_sessao();
                for fd in 0..3 {
                    let _ = sys::fechar(fd);
                }
                for _ in 0..3 {
                    let _ = sys::abrir(&escravo, sys::O_RDWR);
                }
                let mut ambiente: Vec<String> =
                    ambiente().iter().filter(|v| !v.starts_with("TERM=")).map(|v| String::from(*v)).collect();
                ambiente.push(String::from("TERM=linux"));
                let erro = sys::executar_com_ambiente("/bin/sh", &["sh"], &ambiente);
                eprintln!("janelas: /bin/sh: {}", erro);
                sys::sair(127);
            }
            Ok(pid) => pid,
            Err(_) => {
                let _ = sys::fechar(mestre);
                return None;
            }
        };
        Some(Terminal {
            mestre,
            pid,
            colunas,
            linhas,
            celulas: vec![VAZIA; colunas * linhas],
            cx: 0,
            cy: 0,
            frente: 7,
            fundo: 0,
            claro: false,
            escape: 0,
            parametros: Vec::new(),
            utf8: Vec::new(),
            selecao: None,
            ancora: 0,
        })
    }

    /// Novo tamanho: o texto fica no canto de cima (as linhas de baixo, onde
    /// está o cursor, têm prioridade) e o shell é avisado (SIGWINCH).
    fn redimensionar(&mut self, colunas: usize, linhas: usize) {
        let mut novas = vec![VAZIA; colunas * linhas];
        let descartar = (self.cy + 1).saturating_sub(linhas);
        for l in 0..linhas.min(self.linhas - descartar) {
            for c in 0..colunas.min(self.colunas) {
                novas[l * colunas + c] = self.celulas[(l + descartar) * self.colunas + c];
            }
        }
        self.cy -= descartar;
        self.cx = self.cx.min(colunas);
        (self.colunas, self.linhas, self.celulas) = (colunas, linhas, novas);
        self.selecao = None;
        let _ = sys::definir_tamanho_terminal(self.mestre, linhas as u16, colunas as u16);
    }

    /// A célula sob um ponto do conteúdo da janela.
    fn celula_em(&self, x: i32, y: i32) -> usize {
        let coluna = (x / LARGURA_LETRA).clamp(0, self.colunas as i32 - 1) as usize;
        let linha = (y / ALTURA_LETRA).clamp(0, self.linhas as i32 - 1) as usize;
        linha * self.colunas + coluna
    }

    fn selecionar(&mut self, ate: usize) {
        self.selecao = Some((self.ancora.min(ate), self.ancora.max(ate)));
    }

    /// O texto selecionado, com os espaços do fim de cada linha cortados.
    fn texto_selecionado(&self) -> String {
        let Some((a, b)) = self.selecao else { return String::new() };
        let mut texto = String::new();
        let mut linha = String::new();
        for i in a..=b {
            linha.push(self.celulas[i].c);
            if i % self.colunas == self.colunas - 1 && i != b {
                texto.push_str(linha.trim_end());
                texto.push('\n');
                linha.clear();
            }
        }
        texto.push_str(linha.trim_end());
        texto
    }

    fn rolar(&mut self) {
        self.celulas.drain(..self.colunas);
        self.celulas.extend(core::iter::repeat_n(VAZIA, self.colunas));
    }

    fn nova_linha(&mut self) {
        if self.cy + 1 >= self.linhas {
            self.rolar();
        } else {
            self.cy += 1;
        }
    }

    fn apagar(&mut self, de: usize, ate: usize) {
        let fundo = Celula { c: ' ', frente: self.frente, fundo: self.fundo };
        let ate = ate.min(self.celulas.len());
        if de < ate {
            self.celulas[de..ate].fill(fundo);
        }
    }

    /// Uma sequência ESC [ ... <letra> terminou.
    fn csi(&mut self, letra: char) {
        let p = |i: usize, padrao: u16| self.parametros.get(i).copied().filter(|&v| v != 0).unwrap_or(padrao) as usize;
        match letra {
            'm' => {
                let lista = if self.parametros.is_empty() { vec![0] } else { self.parametros.clone() };
                for v in lista {
                    match v {
                        0 => (self.frente, self.fundo, self.claro) = (7, 0, false),
                        1 => self.claro = true,
                        22 => self.claro = false,
                        30..=37 => self.frente = (v - 30) as u8,
                        39 => self.frente = 7,
                        40..=47 => self.fundo = (v - 40) as u8,
                        49 => self.fundo = 0,
                        90..=97 => self.frente = (v - 90 + 8) as u8,
                        100..=107 => self.fundo = (v - 100 + 8) as u8,
                        _ => {}
                    }
                }
            }
            'H' | 'f' => {
                self.cy = (p(0, 1) - 1).min(self.linhas - 1);
                self.cx = (p(1, 1) - 1).min(self.colunas - 1);
            }
            'J' => {
                let aqui = self.cy * self.colunas + self.cx;
                match self.parametros.first().copied().unwrap_or(0) {
                    0 => self.apagar(aqui, self.celulas.len()),
                    1 => self.apagar(0, aqui + 1),
                    _ => self.apagar(0, self.celulas.len()),
                }
            }
            'K' => {
                let linha = self.cy * self.colunas;
                match self.parametros.first().copied().unwrap_or(0) {
                    0 => self.apagar(linha + self.cx, linha + self.colunas),
                    1 => self.apagar(linha, linha + self.cx + 1),
                    _ => self.apagar(linha, linha + self.colunas),
                }
            }
            'A' => self.cy = self.cy.saturating_sub(p(0, 1)),
            'B' => self.cy = (self.cy + p(0, 1)).min(self.linhas - 1),
            'C' => self.cx = (self.cx + p(0, 1)).min(self.colunas - 1),
            'D' => self.cx = self.cx.saturating_sub(p(0, 1)),
            'G' => self.cx = (p(0, 1) - 1).min(self.colunas - 1),
            _ => {}
        }
    }

    fn caractere(&mut self, c: char) {
        match self.escape {
            1 => {
                self.escape = if c == '[' { 2 } else { 0 };
                self.parametros.clear();
                return;
            }
            2 => {
                if let Some(d) = c.to_digit(10) {
                    if self.parametros.is_empty() {
                        self.parametros.push(0);
                    }
                    let ultimo = self.parametros.last_mut().unwrap();
                    *ultimo = ultimo.saturating_mul(10).saturating_add(d as u16);
                } else if c == ';' {
                    if self.parametros.is_empty() {
                        self.parametros.push(0);
                    }
                    self.parametros.push(0);
                } else if c != '?' {
                    self.escape = 0;
                    self.csi(c);
                }
                return;
            }
            _ => {}
        }
        match c {
            '\x1b' => self.escape = 1,
            '\n' => self.nova_linha(),
            '\r' => self.cx = 0,
            '\x08' => self.cx = self.cx.saturating_sub(1),
            '\t' => self.cx = ((self.cx / 8 + 1) * 8).min(self.colunas - 1),
            c if c.is_control() => {}
            c => {
                if self.cx >= self.colunas {
                    self.cx = 0;
                    self.nova_linha();
                }
                let frente = if self.claro && self.frente < 8 { self.frente + 8 } else { self.frente };
                self.celulas[self.cy * self.colunas + self.cx] = Celula { c, frente, fundo: self.fundo };
                self.cx += 1;
            }
        }
    }

    /// O que o shell escreveu (UTF-8, talvez cortado no meio de um caractere).
    fn alimentar(&mut self, bytes: &[u8]) {
        self.selecao = None; // o texto vai mudar de lugar
        self.utf8.extend_from_slice(bytes);
        let pendente = core::mem::take(&mut self.utf8);
        let mut resto: &[u8] = &pendente;
        loop {
            match core::str::from_utf8(resto) {
                Ok(texto) => {
                    texto.chars().for_each(|c| self.caractere(c));
                    break;
                }
                Err(e) => {
                    let (bom, depois) = resto.split_at(e.valid_up_to());
                    core::str::from_utf8(bom).unwrap_or("").chars().for_each(|c| self.caractere(c));
                    match e.error_len() {
                        Some(n) => {
                            self.caractere('?');
                            resto = &depois[n..];
                        }
                        None => {
                            self.utf8 = depois.to_vec(); // falta o resto do caractere
                            break;
                        }
                    }
                }
            }
        }
    }

    fn desenhar(&self, q: &mut Quadro, fonte: &Fonte, x0: i32, y0: i32, focado: bool) {
        for (i, cel) in self.celulas.iter().enumerate() {
            let (col, lin) = ((i % self.colunas) as i32, (i / self.colunas) as i32);
            let (x, y) = (x0 + col * LARGURA_LETRA, y0 + lin * ALTURA_LETRA);
            let (mut frente, mut fundo) = (PALETA[cel.frente as usize], PALETA[cel.fundo as usize]);
            if self.selecao.is_some_and(|(a, b)| (a..=b).contains(&i)) {
                (frente, fundo) = (fundo, frente); // a seleção aparece invertida
            }
            q.letra(fonte, x, y, cel.c, frente, Some(fundo));
        }
        if focado {
            let cx = self.cx.min(self.colunas - 1) as i32;
            let (x, y) = (x0 + cx * LARGURA_LETRA, y0 + self.cy as i32 * ALTURA_LETRA);
            q.retangulo(x, y + ALTURA_LETRA - 3, LARGURA_LETRA, 2, PALETA[7]);
        }
    }
}

// -----------------------------------------------------------------------------
// Janelas
// -----------------------------------------------------------------------------

/// Um retângulo na tela: (x, y, largura, altura).
type Area = (i32, i32, i32, i32);

fn contem((x, y, w, h): Area, px: i32, py: i32) -> bool {
    px >= x && py >= y && px < x + w && py < y + h
}

/// O menor retângulo que cobre os dois.
fn uniao(a: Option<Area>, b: Area) -> Option<Area> {
    if b.2 <= 0 || b.3 <= 0 {
        return a;
    }
    Some(match a {
        None => b,
        Some((x, y, w, h)) => {
            let (x0, y0) = (x.min(b.0), y.min(b.1));
            let (x1, y1) = ((x + w).max(b.0 + b.2), (y + h).max(b.1 + b.3));
            (x0, y0, x1 - x0, y1 - y0)
        }
    })
}

/// Desenha com `f` só dentro de `a` (e dentro do recorte que já havia).
fn com_recorte(q: &mut Quadro, a: Area, f: impl FnOnce(&mut Quadro)) {
    let antigo = q.recorte;
    let (x0, y0) = (a.0.max(antigo.0), a.1.max(antigo.1));
    let (x1, y1) = ((a.0 + a.2).min(antigo.0 + antigo.2), (a.1 + a.3).min(antigo.1 + antigo.3));
    q.recorte = (x0, y0, (x1 - x0).max(0), (y1 - y0).max(0));
    f(q);
    q.recorte = antigo;
}

struct Cliente {
    fd: i32,
    entrada: Vec<u8>,
    pixels: usize,
    shm: i32,
}

enum Conteudo {
    Terminal(Terminal),
    Cliente(Cliente),
}

#[derive(Clone, Copy, PartialEq)]
enum BotaoTitulo {
    Fechar,
    Maximizar,
    Minimizar,
}

const BOTOES_TITULO: [BotaoTitulo; 3] = [BotaoTitulo::Fechar, BotaoTitulo::Maximizar, BotaoTitulo::Minimizar];
const LARGURA_MINIMA: i32 = 160;
const ALTURA_MINIMA: i32 = 64;

struct Janela {
    /// A ordem de criação (a barra de tarefas mostra nessa ordem).
    id: u32,
    x: i32,
    y: i32,
    largura: i32,
    altura: i32,
    titulo: String,
    conteudo: Conteudo,
    minimizada: bool,
    /// Maximizada: onde e de que tamanho ela estava antes.
    restaurar: Option<Area>,
}

impl Janela {
    fn largura_total(&self) -> i32 {
        self.largura + 2 * BORDA
    }
    fn altura_total(&self) -> i32 {
        self.altura + TITULO + 2 * BORDA
    }
    /// Tudo o que ela ocupa na tela, com a sombra.
    fn area(&self) -> Area {
        (self.x, self.y, self.largura_total() + 4, self.altura_total() + 4)
    }
    fn area_conteudo(&self) -> Area {
        (self.x + BORDA, self.y + BORDA + TITULO, self.largura, self.altura)
    }
    fn dentro(&self, x: i32, y: i32) -> bool {
        !self.minimizada && contem((self.x, self.y, self.largura_total(), self.altura_total()), x, y)
    }
    fn no_titulo(&self, x: i32, y: i32) -> bool {
        self.dentro(x, y) && y < self.y + BORDA + TITULO
    }
    /// Os botões do título, da direita para a esquerda: fechar, maximizar, minimizar.
    fn posicao_botao(&self, i: usize) -> i32 {
        self.x + BORDA + self.largura - 18 - i as i32 * 18
    }
    fn botao_titulo(&self, x: i32, y: i32) -> Option<BotaoTitulo> {
        if !self.no_titulo(x, y) {
            return None;
        }
        (0..3).find(|&i| (self.posicao_botao(i)..self.posicao_botao(i) + 16).contains(&x)).map(|i| BOTOES_TITULO[i])
    }
    /// O canto de baixo à direita: arrastando dele, a janela muda de tamanho.
    fn no_canto(&self, x: i32, y: i32) -> bool {
        let (cx, cy) = (self.x + self.largura_total(), self.y + self.altura_total());
        self.dentro(x, y) && x >= cx - 14 && y >= cy - 14
    }
    /// Coordenadas dentro do conteúdo (para os programas).
    fn local(&self, x: i32, y: i32) -> (i32, i32) {
        (x - self.x - BORDA, y - self.y - BORDA - TITULO)
    }
    fn descritor(&self) -> i32 {
        match &self.conteudo {
            Conteudo::Terminal(t) => t.mestre,
            Conteudo::Cliente(c) => c.fd,
        }
    }

    fn desenhar(&self, q: &mut Quadro, letras: &mut Letras, ativa: bool) {
        let (w, h) = (self.largura_total(), self.altura_total());
        q.retangulo(self.x + 4, self.y + 4, w, h, 0x0E2630); // sombra
        q.retangulo(self.x, self.y, w, h, COR_BORDA);
        let (cx, cy) = (self.x + BORDA, self.y + BORDA);
        q.retangulo(cx, cy, self.largura, TITULO, if ativa { COR_ATIVA } else { COR_INATIVA });
        let espaco_titulo = (cx, cy, self.largura - 3 * 18 - 4, TITULO);
        com_recorte(q, espaco_titulo, |q| {
            letras.escrever(q, cx + 6, cy, TITULO, &self.titulo, COR_TEXTO, true);
        });
        for (i, botao) in BOTOES_TITULO.iter().enumerate() {
            let bx = self.posicao_botao(i);
            q.retangulo(bx, cy + 2, 16, 16, 0x2B2F38);
            match botao {
                BotaoTitulo::Fechar => {
                    q.linha(bx + 4, cy + 6, bx + 11, cy + 13, 1, COR_TEXTO);
                    q.linha(bx + 11, cy + 6, bx + 4, cy + 13, 1, COR_TEXTO);
                }
                BotaoTitulo::Maximizar => {
                    q.moldura(bx + 4, cy + 6, 8, 8, COR_TEXTO);
                    q.retangulo(bx + 4, cy + 6, 8, 2, COR_TEXTO);
                }
                BotaoTitulo::Minimizar => q.retangulo(bx + 4, cy + 12, 8, 2, COR_TEXTO),
            }
        }
        let area = self.area_conteudo();
        com_recorte(q, area, |q| match &self.conteudo {
            Conteudo::Terminal(t) => t.desenhar(q, &letras.vga, area.0, area.1, ativa),
            Conteudo::Cliente(c) => {
                let n = (self.largura * self.altura) as usize;
                let imagem = unsafe { core::slice::from_raw_parts(c.pixels as *const u32, n) };
                q.copiar(imagem, self.largura, self.altura, area.0, area.1);
            }
        });
        // O "pegador" do canto, para redimensionar.
        if self.restaurar.is_none() {
            let (px, py) = (self.x + w - 3, self.y + h - 3);
            for d in [4, 8] {
                q.linha(px - d, py, px, py - d, 1, 0x808890);
            }
        }
    }
}

// -----------------------------------------------------------------------------
// O servidor
// -----------------------------------------------------------------------------

/// A barra de tarefas, embaixo.
const BARRA: i32 = 30;
const ITEM_MENU: i32 = 26;

/// O menu Aplicativos: (rótulo, programa). O primeiro e o último são especiais.
const APLICATIVOS: [(&str, &str); 9] = [
    ("Terminal", ""),
    ("Arquivos", "arquivos"),
    ("Editor de texto", "editor"),
    ("Imagens", "imagens"),
    ("Monitor do sistema", "monitor"),
    ("Componentes", "componentes"),
    ("Fontes", "fontes"),
    ("Pintar", "pintar"),
    ("Sair da interface gráfica", ""),
];

enum Arraste {
    /// Pelo título: onde o mouse pegou a janela.
    Mover { dx: i32, dy: i32 },
    /// Pelo canto: o tamanho novo (aplicado ao soltar; até lá, só um contorno).
    Redimensionar { largura: i32, altura: i32 },
}

struct Servidor {
    tela: &'static mut [u32],
    imagem: Vec<u32>,
    largura: i32,
    altura: i32,
    letras: Letras,
    janelas: Vec<Janela>, // a última é a de cima (a ativa)
    pendentes: Vec<(i32, Vec<u8>)>, // conectados que ainda não pediram janela
    mouse: (i32, i32),
    botoes: u8,
    arraste: Option<Arraste>,
    /// O botão está apertado dentro de um programa (os movimentos vão para ele).
    segurando_cliente: bool,
    /// O mouse está sobre o conteúdo do programa da janela de cima.
    mouse_no_cliente: bool,
    /// Arrastando sobre um terminal: selecionando texto.
    selecionando: bool,
    cursor_desenhado: Option<(i32, i32)>,
    /// O que precisa ser redesenhado (a união do que mudou).
    sujo: Option<Area>,
    menu_aberto: bool,
    area_aplicativos: Area,
    area_menu: Area,
    /// A área de transferência (copiar e colar), a mesma para todos.
    transferencia: String,
    /// (instante, x, y) do último clique: dois cliques rápidos no título maximizam.
    ultimo_clique: (u64, i32, i32),
    proximo_id: u32,
    proxima_posicao: i32,
    hora_mostrada: String,
    sair: bool,
    // O painel de controle
    ajustes: Ajustes,
    controle_aberto: bool,
    menu_desligar: bool,
    arrastando_brilho: bool,
    area_status: Area,
    /// O filtro de cor mudou: a tela toda é copiada de novo (sem redesenhar).
    refiltrar: bool,
    aviso: Option<(String, u64)>,
    cpus: usize,
    /// O papel de parede, do tamanho da área de trabalho.
    papel: Option<Vec<u32>>,
    // A tela bloqueada
    travada: bool,
    senha: String,
    senha_errada: bool,
    /// Só dá para bloquear se dá para conferir a senha (ler o /etc/shadow).
    pode_bloquear: bool,
}

impl Servidor {
    // ------------------------------------------------------------- o que redesenhar

    fn marcar(&mut self, (x, y, w, h): Area) {
        let (x0, y0) = (x.max(0), y.max(0));
        let (x1, y1) = ((x + w).min(self.largura), (y + h).min(self.altura));
        if x0 < x1 && y0 < y1 {
            self.sujo = uniao(self.sujo, (x0, y0, x1 - x0, y1 - y0));
        }
    }

    fn marcar_tudo(&mut self) {
        self.marcar((0, 0, self.largura, self.altura));
    }

    fn marcar_janela(&mut self, i: usize) {
        if let Some(a) = self.janelas.get(i).map(|j| j.area()) {
            self.marcar(a);
        }
    }

    fn marcar_barra(&mut self) {
        self.marcar((0, self.altura - BARRA, self.largura, BARRA));
    }

    fn marcar_painel(&mut self) {
        self.marcar((0, 0, self.largura, PAINEL));
    }

    /// A área das janelas (entre o painel e a barra de tarefas).
    fn area_util(&self) -> Area {
        (0, PAINEL, self.largura, self.altura - PAINEL - BARRA)
    }

    /// A janela que recebe o teclado: a de cima, se não estiver minimizada.
    fn ativa(&self) -> Option<usize> {
        let n = self.janelas.len();
        (n > 0 && !self.janelas[n - 1].minimizada).then(|| n - 1)
    }

    /// Os botões da barra de tarefas: (índice da janela, área), na ordem de criação.
    fn entradas_barra(&self) -> Vec<(usize, Area)> {
        let mut ordem: Vec<usize> = (0..self.janelas.len()).collect();
        ordem.sort_by_key(|&i| self.janelas[i].id);
        let n = ordem.len().max(1) as i32;
        let largura = ((self.largura - 12 - 6 * (n - 1)) / n).min(200);
        ordem
            .into_iter()
            .enumerate()
            .map(|(k, i)| (i, (6 + k as i32 * (largura + 6), self.altura - BARRA + 4, largura, BARRA - 8)))
            .collect()
    }

    // ------------------------------------------------------------- desenho

    fn compor(&mut self) {
        let Some(area) = self.sujo.take() else { return };
        // A imagem sai do servidor enquanto é desenhada: assim os métodos de
        // desenho podem usar o resto dele à vontade.
        let mut imagem = core::mem::take(&mut self.imagem);
        let (largura, altura) = (self.largura, self.altura);
        let mut q = Quadro::novo(&mut imagem, largura, altura);
        q.recorte = area;
        if self.travada {
            self.desenhar_bloqueio(&mut q);
        } else {
            self.desenhar_area_de_trabalho(&mut q);
        }
        drop(q);
        self.imagem = imagem;
        self.hora_mostrada = self.texto_do_relogio();
        // Para a tela, só o pedaço que mudou (ou tudo, se o filtro de cor
        // mudou), passando pelo filtro; o cursor vai por cima.
        let area = if core::mem::take(&mut self.refiltrar) { (0, 0, largura, altura) } else { area };
        let filtro = self.filtro();
        let (x, y, w, h) = area;
        for linha in y..y + h {
            let i = (linha * largura + x) as usize;
            let (destino, origem) = (&mut self.tela[i..i + w as usize], &self.imagem[i..i + w as usize]);
            if filtro.is_none() {
                destino.copy_from_slice(origem);
            } else {
                for (d, &o) in destino.iter_mut().zip(origem) {
                    *d = Self::filtrar(filtro, o);
                }
            }
        }
        self.desenhar_cursor();
    }

    fn desenhar_area_de_trabalho(&mut self, q: &mut Quadro) {
        let (largura, altura) = (self.largura, self.altura);
        let entradas = self.entradas_barra();
        let ativa = self.ativa();
        let contorno = match (&self.arraste, self.janelas.last()) {
            (Some(Arraste::Redimensionar { largura, altura }), Some(j)) => {
                Some((j.x, j.y, largura + 2 * BORDA, altura + TITULO + 2 * BORDA))
            }
            _ => None,
        };
        // A área de trabalho (o papel de parede, ou a cor com o nome) e as
        // janelas, de baixo para cima.
        match &self.papel {
            Some(p) => q.copiar(p, largura, altura - PAINEL - BARRA, 0, PAINEL),
            None => {
                q.retangulo(0, PAINEL, largura, altura - PAINEL - BARRA, COR_FUNDO);
                let marca = self.letras.largura("FerrugemOS", true);
                self.letras.escrever(q, (largura - marca) / 2, altura / 2, 20, "FerrugemOS", 0x2F6A7E, true);
            }
        }
        for (i, j) in self.janelas.iter().enumerate().filter(|(_, j)| !j.minimizada) {
            j.desenhar(q, &mut self.letras, Some(i) == ativa);
        }
        if let Some((x, y, w, h)) = contorno {
            q.moldura(x, y, w, h, COR_ATIVA);
            q.moldura(x + 1, y + 1, w - 2, h - 2, COR_ATIVA);
        }
        // O painel de cima: Aplicativos, o relógio no meio e o botão do painel de controle.
        q.retangulo(0, 0, largura, PAINEL, COR_PAINEL);
        let (ax, ay, aw, ah) = self.area_aplicativos;
        q.retangulo_redondo(ax, ay, aw, ah, 9, if self.menu_aberto { COR_ATIVA } else { 0x30353F });
        self.letras.escrever(q, ax + 10, ay, ah, "Aplicativos", COR_TEXTO, true);
        let relogio = self.texto_do_relogio();
        let w = self.letras.largura(&relogio, true);
        self.letras.escrever(q, (largura - w) / 2, 0, PAINEL, &relogio, COR_TEXTO, true);
        let (sx, sy, sw, sh) = self.area_status;
        let fundo_status = if self.controle_aberto { COR_ATIVA } else { COR_PAINEL };
        q.retangulo_redondo(sx, sy, sw, sh, 9, fundo_status);
        let mut x = sx + 10;
        if self.ajustes.noturna {
            self.letras.escrever(q, x, sy, sh, "☾", COR_TEXTO, false);
            x += 18;
        }
        self.letras.escrever(q, x, sy, sh, if self.ajustes.escuro { "◐" } else { "☀" }, COR_TEXTO, false);
        icone_desligar(q, sx + sw - 18, sy + sh / 2, COR_TEXTO, fundo_status);
        // A barra de tarefas.
        q.retangulo(0, altura - BARRA, largura, BARRA, COR_PAINEL);
        for (i, (x, y, w, h)) in entradas {
            let j = &self.janelas[i];
            let cor = if Some(i) == ativa {
                COR_ATIVA
            } else if j.minimizada {
                0x252930
            } else {
                0x30353F
            };
            q.retangulo_redondo(x, y, w, h, 6, cor);
            let texto = if j.minimizada { 0x9AA0AA } else { COR_TEXTO };
            let titulo = j.titulo.clone();
            let letras = &mut self.letras;
            com_recorte(q, (x + 4, y, w - 8, h), |q| {
                letras.escrever(q, x + 8, y, h, &titulo, texto, false);
            });
        }
        // O menu Aplicativos, por cima de tudo.
        if self.menu_aberto {
            let (mx, my, mw, mh) = self.area_menu;
            q.retangulo(mx + 3, my + 3, mw, mh, 0x0E2630);
            q.retangulo(mx, my, mw, mh, COR_BORDA);
            q.retangulo(mx + 1, my + 1, mw - 2, mh - 2, 0x262B34);
            let sobre = contem(self.area_menu, self.mouse.0, self.mouse.1).then(|| (self.mouse.1 - my - 4) / ITEM_MENU);
            for (k, (rotulo, _)) in APLICATIVOS.iter().enumerate() {
                let iy = my + 4 + k as i32 * ITEM_MENU;
                if sobre == Some(k as i32) {
                    q.retangulo(mx + 1, iy, mw - 2, ITEM_MENU, COR_ATIVA);
                }
                if k == APLICATIVOS.len() - 1 {
                    q.retangulo(mx + 8, iy - 1, mw - 16, 1, 0x4C566A);
                }
                self.letras.escrever(q, mx + 16, iy, ITEM_MENU, rotulo, COR_TEXTO, false);
            }
        }
        if self.controle_aberto {
            self.desenhar_controle(q);
        }
        // Um aviso (captura de tela salva...), no alto, no meio.
        if let Some((texto, _)) = self.aviso.clone() {
            let w = self.letras.largura(&texto, false) + 40;
            let (x, y) = ((largura - w) / 2, PAINEL + 10);
            q.retangulo_redondo(x, y, w, 36, 18, 0x2E2E33);
            self.letras.escrever(q, x + 20, y, 36, &texto, PC_TEXTO, false);
        }
    }

    /// Apaga o cursor (repondo o que havia embaixo) e o desenha na posição nova.
    fn desenhar_cursor(&mut self) {
        let (l, a) = (self.largura, self.altura);
        let filtro = self.filtro();
        // Repõe o que estava embaixo (o tamanho do cursor grande, que cobre o normal).
        if let Some((x0, y0)) = self.cursor_desenhado.take() {
            for y in y0..(y0 + 32).min(a) {
                for x in x0..(x0 + 24).min(l) {
                    let i = (y * l + x) as usize;
                    self.tela[i] = Self::filtrar(filtro, self.imagem[i]);
                }
            }
        }
        let (mx, my) = self.mouse;
        let escala = if self.ajustes.cursor_grande { 2 } else { 1 };
        for (dy, linha) in CURSOR.iter().enumerate() {
            for (dx, c) in linha.bytes().enumerate() {
                if c == b' ' {
                    continue;
                }
                for (ex, ey) in [(0, 0), (1, 0), (0, 1), (1, 1)].into_iter().take(escala * escala) {
                    let (x, y) = (mx + dx as i32 * escala as i32 + ex, my + dy as i32 * escala as i32 + ey);
                    if x < l && y < a {
                        self.tela[(y * l + x) as usize] = if c == b'#' { 0 } else { 0xFFFFFF };
                    }
                }
            }
        }
        self.cursor_desenhado = Some((mx, my));
    }

    // ------------------------------------------------------------- janelas

    fn nova_janela(&mut self, largura: i32, altura: i32, titulo: String, conteudo: Conteudo) {
        let (x, y) = self.posicao_nova();
        self.proximo_id += 1;
        let id = self.proximo_id;
        self.janelas.push(Janela { id, x, y, largura, altura, titulo, conteudo, minimizada: false, restaurar: None });
        if self.janelas.len() > 1 {
            self.marcar_janela(self.janelas.len() - 2); // a que era a ativa muda de cor
        }
        self.marcar_janela(self.janelas.len() - 1);
        self.marcar_barra();
    }

    fn abrir_terminal(&mut self) {
        let (colunas, linhas) = (80, 25);
        let Some(t) = Terminal::novo(colunas, linhas) else { return };
        let titulo = format!("Terminal (sh {})", t.pid);
        let (w, h) = (colunas as i32 * LARGURA_LETRA, linhas as i32 * ALTURA_LETRA);
        self.nova_janela(w, h, titulo, Conteudo::Terminal(t));
    }

    /// Cada janela nova um pouco abaixo e à direita da anterior.
    fn posicao_nova(&mut self) -> (i32, i32) {
        let p = self.proxima_posicao;
        self.proxima_posicao = (p + 1) % 8;
        (40 + p * 30, PAINEL + 20 + p * 30)
    }

    fn lancar(&self, programa: &str) {
        if sys::fork() == Ok(0) {
            let caminho = format!("/bin/{}", programa);
            let erro = sys::executar(&caminho, &[programa]);
            eprintln!("janelas: {}: {}", caminho, erro);
            sys::sair(127);
        }
    }

    fn fechar_janela(&mut self, i: usize) {
        self.marcar_janela(i);
        self.marcar_barra();
        let j = self.janelas.remove(i);
        match j.conteudo {
            // Fechar o mestre "derruba a linha": o shell recebe SIGHUP.
            Conteudo::Terminal(t) => {
                let _ = sys::fechar(t.mestre);
            }
            Conteudo::Cliente(c) => {
                let _ = sys::escrever_tudo(c.fd, b"fechar\n");
                let _ = sys::shmdt(c.pixels);
                let _ = sys::shm_remover(c.shm);
                let _ = sys::fechar(c.fd);
            }
        }
        if let Some(n) = self.janelas.len().checked_sub(1) {
            self.marcar_janela(n); // a nova ativa
        }
    }

    /// A janela `i` vai para cima de todas (e vira a ativa).
    fn trazer_para_frente(&mut self, i: usize) {
        let n = self.janelas.len();
        if i + 1 == n {
            return;
        }
        self.marcar_janela(n - 1);
        let j = self.janelas.remove(i);
        self.janelas.push(j);
        self.marcar_janela(n - 1);
        self.marcar_barra();
    }

    /// Some da tela (fica na barra de tarefas) e vai para baixo de todas.
    fn minimizar(&mut self, i: usize) {
        self.marcar_janela(i);
        self.marcar_barra();
        let mut j = self.janelas.remove(i);
        j.minimizada = true;
        self.janelas.insert(0, j);
        if let Some(n) = self.janelas.len().checked_sub(1) {
            self.marcar_janela(n);
        }
    }

    fn restaurar(&mut self, i: usize) {
        self.janelas[i].minimizada = false;
        self.marcar_janela(i);
        self.trazer_para_frente(i);
        self.marcar_barra();
    }

    /// Maximiza (ocupa toda a área das janelas) ou volta ao tamanho de antes.
    fn maximizar(&mut self, i: usize) {
        let j = &self.janelas[i];
        let (x, y, largura, altura) = match j.restaurar {
            Some(antes) => antes,
            None => {
                let (ux, uy, uw, uh) = self.area_util();
                (ux, uy, uw - 2 * BORDA, uh - TITULO - 2 * BORDA)
            }
        };
        let antes = (j.x, j.y, j.largura, j.altura);
        let maximizada = j.restaurar.is_none();
        self.marcar_janela(i);
        self.janelas[i].x = x;
        self.janelas[i].y = y;
        self.redimensionar(i, largura, altura);
        self.janelas[i].restaurar = maximizada.then_some(antes);
    }

    /// Muda o tamanho do conteúdo. Um terminal ganha linhas e colunas; um
    /// programa recebe uma imagem nova (outro segmento de memória
    /// compartilhada) e a mensagem "tamanho".
    fn redimensionar(&mut self, i: usize, largura: i32, altura: i32) {
        let (ux, uy, uw, uh) = self.area_util();
        let _ = (ux, uy);
        let largura = largura.clamp(LARGURA_MINIMA, uw - 2 * BORDA);
        let altura = altura.clamp(ALTURA_MINIMA, uh - TITULO - 2 * BORDA);
        self.marcar_janela(i);
        let j = &mut self.janelas[i];
        match &mut j.conteudo {
            Conteudo::Terminal(t) => {
                let (colunas, linhas) = ((largura / LARGURA_LETRA) as usize, (altura / ALTURA_LETRA) as usize);
                t.redimensionar(colunas, linhas);
                j.largura = colunas as i32 * LARGURA_LETRA;
                j.altura = linhas as i32 * ALTURA_LETRA;
            }
            Conteudo::Cliente(c) => {
                let Ok(shm) = sys::shmget(sys::IPC_PRIVATE, (largura * altura * 4) as usize, 0o600) else { return };
                let Ok(pixels) = sys::shmat(shm) else {
                    let _ = sys::shm_remover(shm);
                    return;
                };
                // O que já estava desenhado fica no canto (até o programa redesenhar).
                let novo = unsafe { core::slice::from_raw_parts_mut(pixels as *mut u32, (largura * altura) as usize) };
                let velho = unsafe { core::slice::from_raw_parts(c.pixels as *const u32, (j.largura * j.altura) as usize) };
                novo.fill(0xFFFFFF);
                let mut q = Quadro::novo(novo, largura, altura);
                q.copiar(velho, j.largura, j.altura, 0, 0);
                if let Ok((_, uid, gid)) = sys::credenciais_do_par(c.fd) {
                    let _ = sys::shm_dar_para(shm, uid, gid);
                }
                let _ = sys::escrever_tudo(c.fd, format!("tamanho {} {} {}\n", largura, altura, shm).as_bytes());
                // O programa ainda tem o velho anexado: ele some quando ele se desanexar.
                let _ = sys::shmdt(c.pixels);
                let _ = sys::shm_remover(c.shm);
                (c.pixels, c.shm) = (pixels, shm);
                (j.largura, j.altura) = (largura, altura);
            }
        }
        self.marcar_janela(i);
    }

    fn mandar_ao_cliente(&self, texto: &str) {
        if let Some(Janela { conteudo: Conteudo::Cliente(c), .. }) = self.ativa().map(|i| &self.janelas[i]) {
            let _ = sys::escrever_tudo(c.fd, texto.as_bytes());
        }
    }

    // ------------------------------------------------------------- mouse

    fn acionar_menu(&mut self, k: usize) {
        match k {
            0 => self.abrir_terminal(),
            k if k == APLICATIVOS.len() - 1 => self.sair = true,
            k if k < APLICATIVOS.len() => self.lancar(APLICATIVOS[k].1),
            _ => {}
        }
    }

    fn apertou(&mut self, botao: u8) {
        let (x, y) = self.mouse;
        if self.travada {
            return; // na tela bloqueada, só o teclado (a senha)
        }
        // O painel de controle: o clique dentro dele é dele; fora, fecha.
        if self.controle_aberto {
            let (px, py, _, _) = self.area_controle();
            if contem(self.area_controle(), x, y) {
                self.clique_no_controle(x - px, y - py);
            } else {
                self.fechar_controle();
            }
            return;
        }
        if contem(self.area_status, x, y) {
            self.controle_aberto = true;
            self.menu_aberto = false;
            self.marcar(self.area_controle());
            self.marcar_painel();
            return;
        }
        // Com o menu aberto, o clique é dele (escolher, ou fechar clicando fora).
        if self.menu_aberto {
            self.menu_aberto = false;
            self.marcar(self.area_menu);
            self.marcar_painel();
            if contem(self.area_menu, x, y) {
                self.acionar_menu(((y - self.area_menu.1 - 4) / ITEM_MENU) as usize);
            }
            return;
        }
        if contem(self.area_aplicativos, x, y) {
            self.menu_aberto = true;
            self.marcar(self.area_menu);
            self.marcar_painel();
            return;
        }
        if y >= self.altura - BARRA {
            // Na barra: a janela ativa minimiza; as outras vêm para a frente.
            if let Some((i, _)) = self.entradas_barra().into_iter().find(|(_, a)| contem(*a, x, y)) {
                if Some(i) == self.ativa() {
                    self.minimizar(i);
                } else {
                    self.restaurar(i);
                }
            }
            return;
        }
        let Some(i) = self.janelas.iter().rposition(|j| j.dentro(x, y)) else { return };
        self.trazer_para_frente(i);
        let n = self.janelas.len() - 1;
        let j = &self.janelas[n];
        if botao == 2 {
            // Botão direito num terminal: cola a área de transferência.
            match &j.conteudo {
                Conteudo::Terminal(t) => {
                    let _ = sys::escrever_tudo(t.mestre, self.transferencia.as_bytes());
                }
                Conteudo::Cliente(_) => {
                    let (lx, ly) = j.local(x, y);
                    self.mandar_ao_cliente(&format!("botao {} {} 2 1\n", lx, ly));
                }
            }
            return;
        }
        let agora = sys::tempo_ms();
        let (antes, ax, ay) = self.ultimo_clique;
        let duplo = agora - antes < 400 && (x - ax).abs() < 4 && (y - ay).abs() < 4;
        self.ultimo_clique = (agora, x, y);
        if let Some(b) = j.botao_titulo(x, y) {
            match b {
                BotaoTitulo::Fechar => self.fechar_janela(n),
                BotaoTitulo::Maximizar => self.maximizar(n),
                BotaoTitulo::Minimizar => self.minimizar(n),
            }
        } else if j.no_canto(x, y) && j.restaurar.is_none() {
            self.arraste = Some(Arraste::Redimensionar { largura: j.largura, altura: j.altura });
        } else if j.no_titulo(x, y) {
            if duplo {
                self.maximizar(n);
            } else if j.restaurar.is_none() {
                self.arraste = Some(Arraste::Mover { dx: x - j.x, dy: y - j.y });
            }
        } else {
            let (lx, ly) = j.local(x, y);
            if lx < 0 || ly < 0 {
                return;
            }
            let area = j.area_conteudo();
            match &mut self.janelas[n].conteudo {
                Conteudo::Terminal(t) => {
                    // Começa uma seleção de texto (ao soltar, ela é copiada).
                    t.ancora = t.celula_em(lx, ly);
                    t.selecao = None;
                    self.selecionando = true;
                    self.marcar(area);
                }
                Conteudo::Cliente(_) => {
                    self.segurando_cliente = true;
                    self.mandar_ao_cliente(&format!("botao {} {} 1 1\n", lx, ly));
                }
            }
        }
    }

    fn soltou(&mut self, botao: u8) {
        let (x, y) = self.mouse;
        if core::mem::take(&mut self.arrastando_brilho) {
            self.ajustes.salvar();
        }
        if let Some(Arraste::Redimensionar { largura, altura }) = self.arraste.take() {
            if let Some(n) = self.janelas.len().checked_sub(1) {
                self.redimensionar(n, largura, altura);
            }
        }
        if core::mem::take(&mut self.selecionando) {
            if let Some(Conteudo::Terminal(t)) = self.janelas.last().map(|j| &j.conteudo) {
                let texto = t.texto_selecionado();
                if !texto.is_empty() {
                    self.transferencia = texto;
                }
            }
        }
        if core::mem::take(&mut self.segurando_cliente) || botao == 2 {
            if let Some(j) = self.ativa().map(|i| &self.janelas[i]) {
                if matches!(j.conteudo, Conteudo::Cliente(_)) {
                    let (lx, ly) = j.local(x, y);
                    self.mandar_ao_cliente(&format!("botao {} {} {} 0\n", lx, ly, botao));
                }
            }
        }
    }

    /// Pacotes de 4 bytes do /dev/mouse (o 4º é a roda).
    fn pacotes_do_mouse(&mut self, dados: &[u8]) {
        let mut andou = false;
        for p in dados.chunks_exact(4) {
            if p[0] & 0xC0 != 0 {
                continue; // estouro: pacote sem sentido
            }
            let dx = p[1] as i32 - if p[0] & 0x10 != 0 { 256 } else { 0 };
            let dy = p[2] as i32 - if p[0] & 0x20 != 0 { 256 } else { 0 };
            self.mouse.0 = (self.mouse.0 + dx).clamp(0, self.largura - 1);
            self.mouse.1 = (self.mouse.1 - dy).clamp(0, self.altura - 1);
            andou |= dx != 0 || dy != 0;
            // A roda vai para o programa que está debaixo do cursor.
            let giro = p[3] as i8 as i32;
            if giro != 0 {
                let (x, y) = self.mouse;
                if let Some(j) = self.janelas.iter().rev().find(|j| j.dentro(x, y)) {
                    if let Conteudo::Cliente(c) = &j.conteudo {
                        let (lx, ly) = j.local(x, y);
                        let _ = sys::escrever_tudo(c.fd, format!("roda {} {} {}\n", lx, ly, giro).as_bytes());
                    }
                }
            }
            let botoes = p[0] & 3;
            let mudou = botoes ^ self.botoes;
            self.botoes = botoes;
            for (bit, numero) in [(1, 1), (2, 2)] {
                if mudou & bit != 0 {
                    if botoes & bit != 0 {
                        self.apertou(numero);
                    } else {
                        self.soltou(numero);
                    }
                }
            }
        }
        if !andou {
            return;
        }
        let (mx, my) = self.mouse;
        let n = self.janelas.len().wrapping_sub(1);
        match self.arraste {
            Some(Arraste::Mover { dx, dy }) => {
                self.marcar_janela(n);
                let (_, uy, _, uh) = self.area_util();
                let j = &mut self.janelas[n];
                j.x = mx - dx;
                j.y = (my - dy).clamp(uy, uy + uh - TITULO);
                self.marcar_janela(n);
                return;
            }
            Some(Arraste::Redimensionar { largura, altura }) => {
                let j = &self.janelas[n];
                let antes = (j.x, j.y, largura + 2 * BORDA, altura + TITULO + 2 * BORDA);
                let largura = (mx - j.x - 2 * BORDA).max(LARGURA_MINIMA);
                let altura = (my - j.y - TITULO - 2 * BORDA).max(ALTURA_MINIMA);
                let depois = (j.x, j.y, largura + 2 * BORDA, altura + TITULO + 2 * BORDA);
                self.arraste = Some(Arraste::Redimensionar { largura, altura });
                self.marcar(antes);
                self.marcar(depois);
                return;
            }
            None => {}
        }
        if self.menu_aberto {
            self.marcar(self.area_menu); // o item sob o mouse muda de cor
        }
        if self.arrastando_brilho {
            let x = mx - self.area_controle().0;
            self.ajustar_brilho(x);
            return;
        }
        if self.controle_aberto {
            self.marcar(self.area_controle());
        }
        if self.selecionando {
            if let Some(j) = self.janelas.last_mut() {
                let (lx, ly) = j.local(mx, my);
                let area = j.area_conteudo();
                if let Conteudo::Terminal(t) = &mut j.conteudo {
                    let fim = t.celula_em(lx, ly);
                    t.selecionar(fim);
                }
                self.marcar(area);
            }
            return;
        }
        // Um "mover" por leitura (a posição final) para o programa da janela
        // ativa: com o botão apertado nele, ou com o mouse por cima dele
        // (e mais um ao sair, para ele apagar o realce do que estava embaixo).
        if let Some(j) = self.ativa().map(|i| &self.janelas[i]) {
            let (lx, ly) = j.local(mx, my);
            let dentro = lx >= 0 && ly >= 0 && lx < j.largura && ly < j.altura;
            if self.segurando_cliente || dentro || self.mouse_no_cliente {
                self.mandar_ao_cliente(&format!("mover {} {}\n", lx, ly));
            }
            self.mouse_no_cliente = dentro;
        }
    }

    // ------------------------------------------------------------- teclado

    fn teclado(&mut self, dados: &[u8]) {
        if self.travada {
            self.tecla_na_tela_bloqueada(dados);
            return;
        }
        if self.controle_aberto && dados == b"\x1b" {
            self.fechar_controle();
            return;
        }
        // Alt+Tab (ESC + Tab, veja o teclado do kernel): a janela de baixo vem
        // para a frente — repetindo, passa por todas.
        let mut resto: &[u8] = dados;
        while let Some(i) = resto.windows(2).position(|w| w == b"\x1b\t") {
            self.mandar_teclas(&resto[..i]);
            if !self.janelas.is_empty() {
                self.restaurar(0);
            }
            resto = &resto[i + 2..];
        }
        self.mandar_teclas(resto);
    }

    fn mandar_teclas(&mut self, dados: &[u8]) {
        let Some(i) = self.ativa() else { return };
        if dados.is_empty() {
            return;
        }
        match &self.janelas[i].conteudo {
            Conteudo::Terminal(t) => {
                let _ = sys::escrever_tudo(t.mestre, dados);
            }
            Conteudo::Cliente(c) => {
                // As setas chegam como ESC [ A..D: para os programas, viram "seta A".
                let texto = String::from_utf8_lossy(dados);
                let mut letras = texto.chars().peekable();
                while let Some(ch) = letras.next() {
                    if ch == '\x1b' && letras.peek() == Some(&'[') {
                        letras.next();
                        // setas, Home e End (ESC [ letra); PgUp, PgDn e Delete (ESC [ número ~)
                        match letras.next() {
                            Some(l @ ('A'..='D' | 'H' | 'F')) => {
                                let _ = sys::escrever_tudo(c.fd, format!("seta {}\n", l).as_bytes());
                            }
                            Some(n @ ('3' | '5' | '6')) => {
                                letras.next(); // o "~"
                                let _ = sys::escrever_tudo(c.fd, format!("seta {}\n", n).as_bytes());
                            }
                            _ => {}
                        }
                        continue;
                    }
                    let _ = sys::escrever_tudo(c.fd, format!("tecla {}\n", ch as u32).as_bytes());
                }
            }
        }
    }

    // ------------------------------------------------------------- programas

    /// O primeiro pedido de um programa: "janela L A título".
    fn pedido(&mut self, fd: i32, linha: &str) -> bool {
        let Some(resto) = linha.strip_prefix("janela ") else { return false };
        let mut partes = resto.splitn(3, ' ');
        let largura: i32 = partes.next().and_then(|v| v.parse().ok()).unwrap_or(0);
        let altura: i32 = partes.next().and_then(|v| v.parse().ok()).unwrap_or(0);
        let titulo = String::from(partes.next().unwrap_or("?"));
        let (_, _, uw, uh) = self.area_util();
        if !(1..=uw - 2 * BORDA).contains(&largura) || !(1..=uh - TITULO - 2 * BORDA).contains(&altura) {
            let _ = sys::escrever_tudo(fd, b"erro tamanho\n");
            return false;
        }
        let Ok(shm) = sys::shmget(sys::IPC_PRIVATE, (largura * altura * 4) as usize, 0o600) else { return false };
        let Ok(pixels) = sys::shmat(shm) else { return false };
        unsafe { core::slice::from_raw_parts_mut(pixels as *mut u32, (largura * altura) as usize) }.fill(0xFFFFFF);
        // O segmento passa a ser do dono do programa (só ele e nós o usamos).
        if let Ok((_, uid, gid)) = sys::credenciais_do_par(fd) {
            let _ = sys::shm_dar_para(shm, uid, gid);
        }
        let cliente = Cliente { fd, entrada: Vec::new(), pixels, shm };
        self.nova_janela(largura, altura, titulo, Conteudo::Cliente(cliente));
        let _ = sys::escrever_tudo(fd, format!("ok {}\n", shm).as_bytes());
        self.mandar_tema(fd);
        true
    }

    /// As mensagens seguintes: "atualizar", "copiar texto", "colar".
    fn mensagem(&mut self, k: usize, linha: &str) {
        if linha == "atualizar" {
            if !self.janelas[k].minimizada {
                let area = self.janelas[k].area_conteudo();
                self.marcar(area);
            }
        } else if let Some(texto) = linha.strip_prefix("copiar ") {
            self.transferencia = ulib::janela::desescapar(texto);
        } else if let Some(titulo) = linha.strip_prefix("titulo ") {
            self.janelas[k].titulo = String::from(titulo);
            self.marcar_janela(k);
            self.marcar_barra();
        } else if linha == "recarregar" {
            self.recarregar();
        } else if linha == "colar" {
            if let Conteudo::Cliente(c) = &self.janelas[k].conteudo {
                let resposta = format!("colar {}\n", ulib::janela::escapar(&self.transferencia));
                let _ = sys::escrever_tudo(c.fd, resposta.as_bytes());
            }
        }
    }

    // ------------------------------------------------------------- o laço

    fn rodar(&mut self, mouse: i32, teclado: i32, ouvinte: i32) {
        let mut buf = vec![0u8; 4096];
        while !self.sair {
            let mut itens = vec![(mouse, sys::POLLIN), (teclado, sys::POLLIN), (ouvinte, sys::POLLIN)];
            itens.extend(self.pendentes.iter().map(|(fd, _)| (*fd, sys::POLLIN)));
            itens.extend(self.janelas.iter().map(|j| (j.descritor(), sys::POLLIN)));
            // Acorda pelo menos a cada segundo, para o relógio do painel.
            let Ok(prontos) = sys::poll(&itens, 1000) else { continue };
            let mut cursor_mudou = false;
            if prontos[0] & sys::POLLIN != 0 {
                if let Ok(n) = sys::ler(mouse, &mut buf) {
                    let antes = self.mouse;
                    self.pacotes_do_mouse(&buf[..n]);
                    cursor_mudou = self.mouse != antes;
                }
            }
            if prontos[1] & sys::POLLIN != 0 {
                if let Ok(n) = sys::ler(teclado, &mut buf) {
                    let dados = buf[..n].to_vec();
                    self.teclado(&dados);
                }
            }
            if prontos[2] & sys::POLLIN != 0 {
                if let Ok(fd) = sys::aceitar(ouvinte) {
                    sys::fechar_no_exec(fd);
                    self.pendentes.push((fd, Vec::new()));
                }
            }
            // Os que ainda não têm janela: o primeiro pedido.
            let n_pendentes = itens.len() - 3 - self.janelas.len();
            let mut i = n_pendentes;
            while i > 0 {
                i -= 1;
                if prontos[3 + i] == 0 || i >= self.pendentes.len() {
                    continue;
                }
                let (fd, mut entrada) = self.pendentes.remove(i);
                match sys::ler(fd, &mut buf) {
                    Ok(n) if n > 0 => {
                        entrada.extend_from_slice(&buf[..n]);
                        if let Some(fim) = entrada.iter().position(|&b| b == b'\n') {
                            let linha = String::from_utf8_lossy(&entrada[..fim]).into_owned();
                            if !self.pedido(fd, &linha) {
                                let _ = sys::fechar(fd);
                            }
                        } else {
                            self.pendentes.push((fd, entrada));
                        }
                    }
                    _ => {
                        let _ = sys::fechar(fd);
                    }
                }
            }
            // As janelas, achadas pelo descritor (um clique pode tê-las
            // reordenado ou fechado depois do poll).
            let base = 3 + n_pendentes;
            let prontas: Vec<i32> = (base..itens.len()).filter(|&i| prontos[i] != 0).map(|i| itens[i].0).collect();
            for fd in prontas {
                let Some(k) = self.janelas.iter().position(|j| j.descritor() == fd) else { continue };
                let lido = sys::ler(fd, &mut buf);
                let area = self.janelas[k].area_conteudo();
                let visivel = !self.janelas[k].minimizada;
                let mut mensagens = Vec::new();
                match (&mut self.janelas[k].conteudo, lido) {
                    (Conteudo::Terminal(t), Ok(n)) if n > 0 => {
                        t.alimentar(&buf[..n]);
                        if visivel {
                            self.marcar(area);
                        }
                    }
                    (Conteudo::Cliente(c), Ok(n)) if n > 0 => {
                        c.entrada.extend_from_slice(&buf[..n]);
                        while let Some(fim) = c.entrada.iter().position(|&b| b == b'\n') {
                            let linha: Vec<u8> = c.entrada.drain(..=fim).collect();
                            mensagens.push(String::from_utf8_lossy(&linha[..linha.len() - 1]).into_owned());
                        }
                    }
                    // O shell saiu (EIO/fim) ou o programa fechou a conexão.
                    _ => {
                        self.fechar_janela(k);
                        continue;
                    }
                }
                for m in mensagens {
                    self.mensagem(k, &m);
                }
            }
            while let Ok(Some(_)) = sys::recolher_terminado() {}
            if self.texto_do_relogio() != self.hora_mostrada {
                self.marcar_painel();
                if self.travada {
                    self.marcar_tudo();
                }
            }
            if self.aviso.as_ref().is_some_and(|(_, ate)| sys::tempo_ms() > *ate) {
                self.aviso = None;
                self.marcar((0, PAINEL, self.largura, 60));
            }
            if self.sujo.is_some() {
                self.compor();
            } else if cursor_mudou {
                self.desenhar_cursor();
            }
        }
    }
}

fn principal(_args: &[&str]) -> i32 {
    let Some(fonte) = Fonte::carregar() else {
        eprintln!("janelas: não consegui ler /dev/fonte (não há placa de vídeo gráfica?)");
        return 1;
    };
    let fb = match sys::abrir("/dev/fb0", sys::O_RDWR) {
        Ok(fd) => fd,
        Err(e) => {
            eprintln!("janelas: /dev/fb0: {}", e);
            return 1;
        }
    };
    sys::fechar_no_exec(fb);
    // O tamanho da tela e de cada linha (ioctl do Linux).
    let mut var = [0u8; 160];
    let mut fixo = [0u8; 80];
    if sys::ioctl(fb, 0x4600, &mut var).is_err() || sys::ioctl(fb, 0x4602, &mut fixo).is_err() {
        eprintln!("janelas: o framebuffer não respondeu");
        return 1;
    }
    let campo = |b: &[u8], i: usize| u32::from_le_bytes(b[i..i + 4].try_into().unwrap()) as i32;
    let (largura, altura, por_linha) = (campo(&var, 0), campo(&var, 4), campo(&fixo, 48));
    if por_linha != largura * 4 {
        eprintln!("janelas: formato de tela inesperado");
        return 1;
    }
    let tamanho = (largura * altura * 4) as usize;
    let Ok(endereco) = sys::mmap(tamanho, sys::PROT_READ | sys::PROT_WRITE, sys::MAP_SHARED, fb, 0) else {
        eprintln!("janelas: mmap do framebuffer falhou");
        return 1;
    };
    let tela = unsafe { core::slice::from_raw_parts_mut(endereco as *mut u32, (largura * altura) as usize) };
    let abrir = |caminho: &str| {
        let fd = sys::abrir(caminho, sys::O_RDONLY);
        if let Ok(fd) = fd {
            sys::fechar_no_exec(fd);
        }
        fd
    };
    let (Ok(mouse), Ok(teclado)) = (abrir("/dev/mouse"), abrir("/dev/teclado")) else {
        eprintln!("janelas: não consegui abrir o mouse ou o teclado (já há outro servidor?)");
        return 1;
    };
    let _ = sys::remover(ulib::janela::SOCKET);
    let ouvinte = match sys::socket(sys::SOCK_STREAM) {
        Ok(fd) if sys::ligar(fd, ulib::janela::SOCKET).is_ok() && sys::ouvir(fd, 8).is_ok() => fd,
        _ => {
            eprintln!("janelas: não consegui criar {}", ulib::janela::SOCKET);
            return 1;
        }
    };
    sys::fechar_no_exec(ouvinte);
    let _ = sys::mudar_permissoes(ulib::janela::SOCKET, 0o666);
    sys::ignorar_sinal(sys::SIGPIPE, true); // um programa que fechou não derruba o servidor

    let mut letras = Letras::carregar(fonte);
    let area_aplicativos = (6, 3, letras.largura("Aplicativos", true) + 20, PAINEL - 6);
    let largura_menu = APLICATIVOS.iter().map(|(r, _)| letras.largura(r, false)).max().unwrap_or(100) + 40;
    let area_menu = (6, PAINEL, largura_menu, APLICATIVOS.len() as i32 * ITEM_MENU + 8);
    let mut s = Servidor {
        tela,
        imagem: vec![0; (largura * altura) as usize],
        largura,
        altura,
        letras,
        janelas: Vec::new(),
        pendentes: Vec::new(),
        mouse: (largura / 2, altura / 2),
        botoes: 0,
        arraste: None,
        segurando_cliente: false,
        mouse_no_cliente: false,
        selecionando: false,
        cursor_desenhado: None,
        sujo: None,
        menu_aberto: false,
        area_aplicativos,
        area_menu,
        transferencia: String::new(),
        ultimo_clique: (0, 0, 0),
        proximo_id: 0,
        proxima_posicao: 0,
        hora_mostrada: String::new(),
        sair: false,
        ajustes: Ajustes::carregar(),
        controle_aberto: false,
        menu_desligar: false,
        arrastando_brilho: false,
        area_status: (largura - 82, 3, 76, PAINEL - 6),
        refiltrar: true,
        aviso: None,
        papel: None,
        cpus: String::from_utf8_lossy(&sys::ler_arquivo("/proc/cpuinfo").unwrap_or_default()).matches("processor").count(),
        travada: false,
        senha: String::new(),
        senha_errada: false,
        pode_bloquear: sys::ler_arquivo("/etc/shadow").is_ok(),
    };
    s.carregar_papel();
    s.marcar_tudo();
    s.abrir_terminal();
    s.compor();
    s.rodar(mouse, teclado, ouvinte);

    // Saindo: os terminais recebem SIGHUP; ao fechar o /dev/fb0, volta o modo texto.
    while !s.janelas.is_empty() {
        s.fechar_janela(0);
    }
    let _ = sys::remover(ulib::janela::SOCKET);
    0
}

// -----------------------------------------------------------------------------
// O painel de controle (as "configurações rápidas", no canto do painel)
// -----------------------------------------------------------------------------

/// O que o painel de controle liga e desliga. Fica guardado em ~/.janelas.
struct Ajustes {
    escuro: bool,
    noturna: bool,
    segundos: bool,
    cursor_grande: bool,
    /// Brilho, de 30 a 100 (%).
    brilho: i32,
    /// O papel de parede (um PNG), ou vazio.
    fundo: String,
}

impl Ajustes {
    fn arquivo() -> String {
        format!("{}/.janelas", variavel("HOME").unwrap_or("/"))
    }

    fn carregar() -> Ajustes {
        let mut a = Ajustes {
            escuro: false,
            noturna: false,
            segundos: false,
            cursor_grande: false,
            brilho: 100,
            fundo: String::new(),
        };
        let texto = sys::ler_arquivo(&Self::arquivo()).unwrap_or_default();
        for linha in String::from_utf8_lossy(&texto).lines() {
            let Some((chave, valor)) = linha.split_once('=') else { continue };
            let ligado = valor.trim() == "1";
            match chave.trim() {
                "escuro" => a.escuro = ligado,
                "noturna" => a.noturna = ligado,
                "segundos" => a.segundos = ligado,
                "cursor_grande" => a.cursor_grande = ligado,
                "brilho" => a.brilho = valor.trim().parse().unwrap_or(100),
                "fundo" => a.fundo = String::from(valor.trim()),
                _ => {}
            }
        }
        a.brilho = a.brilho.clamp(30, 100);
        a
    }

    fn salvar(&self) {
        let texto = format!(
            "escuro={}\nnoturna={}\nsegundos={}\ncursor_grande={}\nbrilho={}\nfundo={}\n",
            self.escuro as u8, self.noturna as u8, self.segundos as u8, self.cursor_grande as u8, self.brilho, self.fundo
        );
        let fd = sys::abrir_com_permissoes(&Self::arquivo(), sys::O_WRONLY | sys::O_CREAT | sys::O_TRUNC, 0o644);
        if let Ok(fd) = fd {
            let _ = sys::escrever_tudo(fd, texto.as_bytes());
            let _ = sys::fechar(fd);
        }
    }
}

const PC_LARGURA: i32 = 420;
const PC_FUNDO: u32 = 0x232326;
const PC_BLOCO: u32 = 0x3A3A3F;
const PC_BLOCO_SOBRE: u32 = 0x47474D;
const PC_TEXTO: u32 = 0xF2F2F2;
const PC_TEXTO_FRACO: u32 = 0xB0B0B6;

#[derive(Clone, Copy, PartialEq)]
enum Acao {
    Captura,
    Configuracoes,
    Bloquear,
    Desligar,
}

/// Os blocos que ligam e desligam: (rótulo, ícone).
const BLOCOS: [(&str, &str); 4] = [("Estilo escuro", "◐"), ("Luz noturna", "☾"), ("Segundos", "◔"), ("Cursor grande", "")];
const OPCOES_DESLIGAR: [&str; 3] = ["Sair da interface gráfica", "Reiniciar", "Desligar"];

impl Servidor {
    fn bloco_ligado(&self, i: usize) -> bool {
        match i {
            0 => self.ajustes.escuro,
            1 => self.ajustes.noturna,
            2 => self.ajustes.segundos,
            _ => self.ajustes.cursor_grande,
        }
    }

    /// Onde fica o painel de controle (o tamanho depende do que está aberto nele).
    fn area_controle(&self) -> Area {
        let altura = if self.menu_desligar { 70 + 3 * 52 + 12 } else { 376 };
        (self.largura - PC_LARGURA - 8, PAINEL + 6, PC_LARGURA, altura)
    }

    /// Os botões redondos do alto: (ação, centro x relativo).
    fn botoes_controle(&self) -> Vec<(Acao, i32)> {
        let mut acoes = vec![Acao::Desligar];
        if self.pode_bloquear {
            acoes.push(Acao::Bloquear);
        }
        acoes.push(Acao::Configuracoes);
        acoes.push(Acao::Captura);
        acoes.iter().enumerate().map(|(k, &a)| (a, PC_LARGURA - 36 - k as i32 * 52)).collect()
    }

    fn area_bloco(i: usize) -> Area {
        let w = (PC_LARGURA - 32 - 12) / 2;
        (16 + (i as i32 % 2) * (w + 12), 124 + (i as i32 / 2) * 68, w, 56)
    }

    /// O filtro de cor (brilho e luz noturna), por canal, em 1/256; None = nenhum.
    fn filtro(&self) -> Option<[u32; 3]> {
        if self.ajustes.brilho >= 100 && !self.ajustes.noturna {
            return None;
        }
        let b = (self.ajustes.brilho as u32 * 256) / 100;
        Some(if self.ajustes.noturna { [b, b * 210 / 256, b * 140 / 256] } else { [b, b, b] })
    }

    fn filtrar(filtro: Option<[u32; 3]>, p: u32) -> u32 {
        match filtro {
            None => p,
            Some([r, g, b]) => {
                ((p >> 16 & 0xFF) * r >> 8) << 16 | ((p >> 8 & 0xFF) * g >> 8) << 8 | (p & 0xFF) * b >> 8
            }
        }
    }

    /// Carrega o papel de parede, já do tamanho da área de trabalho: cobre
    /// tudo, cortando o que sobrar (sem deformar).
    fn carregar_papel(&mut self) {
        self.papel = None;
        if self.ajustes.fundo.is_empty() {
            return;
        }
        let Some(img) = sys::ler_arquivo(&self.ajustes.fundo).ok().and_then(|d| ulib::png::decodificar(&d)) else {
            self.avisar(format!("Não consegui ler o papel de parede {}", self.ajustes.fundo));
            return;
        };
        let (l, a) = (self.largura, self.altura - PAINEL - BARRA);
        let (iw, ih) = (img.largura as i32, img.altura as i32);
        let escala = (l as f32 / iw as f32).max(a as f32 / ih as f32);
        let (w, h) = ((iw as f32 * escala) as i32 + 1, (ih as f32 * escala) as i32 + 1);
        let mut pixels = vec![COR_FUNDO; (l * a) as usize];
        let mut q = Quadro::novo(&mut pixels, l, a);
        q.desenhar_imagem(&img.pixels, iw, ih, (l - w) / 2, (a - h) / 2, w, h);
        self.papel = Some(pixels);
    }

    /// Um programa (as configurações, o visualizador de imagens) mudou o
    /// ~/.janelas: relê tudo.
    fn recarregar(&mut self) {
        self.ajustes = Ajustes::carregar();
        self.carregar_papel();
        let clientes: Vec<i32> =
            self.janelas.iter().filter_map(|j| if let Conteudo::Cliente(c) = &j.conteudo { Some(c.fd) } else { None }).collect();
        for fd in clientes {
            self.mandar_tema(fd);
        }
        self.refiltrar = true;
        self.marcar_tudo();
    }

    /// Um aviso que aparece por alguns segundos no alto da tela.
    fn avisar(&mut self, texto: String) {
        self.aviso = Some((texto, sys::tempo_ms() + 3500));
        self.marcar((0, PAINEL, self.largura, 60));
    }

    fn mandar_tema(&self, fd: i32) {
        let tema = if self.ajustes.escuro { "tema escuro\n" } else { "tema claro\n" };
        let _ = sys::escrever_tudo(fd, tema.as_bytes());
    }

    fn fechar_controle(&mut self) {
        self.marcar(self.area_controle());
        self.marcar_painel();
        self.controle_aberto = false;
        self.menu_desligar = false;
    }

    /// Um clique dentro do painel de controle, em coordenadas dele.
    fn clique_no_controle(&mut self, x: i32, y: i32) {
        self.marcar(self.area_controle());
        // Os botões redondos do alto.
        if (16..56).contains(&y) {
            if let Some(&(acao, _)) = self.botoes_controle().iter().find(|(_, cx)| (x - cx).abs() <= 20) {
                match acao {
                    Acao::Desligar => self.menu_desligar = !self.menu_desligar,
                    Acao::Captura => {
                        self.fechar_controle();
                        self.capturar_tela();
                    }
                    Acao::Configuracoes => {
                        self.fechar_controle();
                        if sys::stat("/bin/configuracoes").is_ok() {
                            self.lancar("configuracoes");
                        } else {
                            self.avisar(String::from("O aplicativo de configurações ainda não está instalado"));
                        }
                    }
                    Acao::Bloquear => {
                        self.fechar_controle();
                        self.travada = true;
                        self.senha.clear();
                        self.senha_errada = false;
                        self.marcar_tudo();
                    }
                }
                self.marcar(self.area_controle());
            }
            return;
        }
        if self.menu_desligar {
            let k = (y - 70) / 52;
            if (70..70 + 3 * 52).contains(&y) {
                match k {
                    0 => self.sair = true,
                    1 => self.lancar("reiniciar"),
                    _ => self.lancar("desligar"),
                }
            }
            return;
        }
        // O brilho: clicar (e arrastar) no controle.
        if (70..110).contains(&y) && (16..PC_LARGURA - 16).contains(&x) {
            self.arrastando_brilho = true;
            self.ajustar_brilho(x);
            return;
        }
        if let Some(i) = (0..BLOCOS.len()).find(|&i| contem(Self::area_bloco(i), x, y)) {
            match i {
                0 => {
                    self.ajustes.escuro = !self.ajustes.escuro;
                    let clientes: Vec<i32> = self
                        .janelas
                        .iter()
                        .filter_map(|j| if let Conteudo::Cliente(c) = &j.conteudo { Some(c.fd) } else { None })
                        .collect();
                    for fd in clientes {
                        self.mandar_tema(fd);
                    }
                }
                1 => {
                    self.ajustes.noturna = !self.ajustes.noturna;
                    self.refiltrar = true;
                }
                2 => {
                    self.ajustes.segundos = !self.ajustes.segundos;
                    self.marcar_painel();
                }
                _ => self.ajustes.cursor_grande = !self.ajustes.cursor_grande,
            }
            self.ajustes.salvar();
        }
    }

    fn ajustar_brilho(&mut self, x: i32) {
        let novo = (30 + (x - 16) * 70 / (PC_LARGURA - 32)).clamp(30, 100);
        if novo != self.ajustes.brilho {
            self.ajustes.brilho = novo;
            self.refiltrar = true;
            self.marcar(self.area_controle());
        }
    }

    /// Salva a tela num PNG em ~/captura-AAAAMMDD-HHMMSS.png.
    fn capturar_tela(&mut self) {
        self.compor();
        let d = sys::data(sys::hora_atual());
        let nome = format!(
            "{}/captura-{:04}{:02}{:02}-{:02}{:02}{:02}.png",
            variavel("HOME").unwrap_or(""),
            d.ano,
            d.mes,
            d.dia,
            d.hora,
            d.minuto,
            d.segundo
        );
        let png = ulib::png::codificar(&self.imagem, self.largura as u32, self.altura as u32);
        let gravado = sys::abrir_com_permissoes(&nome, sys::O_WRONLY | sys::O_CREAT | sys::O_TRUNC, 0o644)
            .and_then(|fd| sys::escrever_tudo(fd, &png).map(|_| fd))
            .map(|fd| sys::fechar(fd));
        match gravado {
            Ok(_) => self.avisar(format!("Captura de tela salva em {}", nome)),
            Err(e) => self.avisar(format!("Não consegui salvar {}: {}", nome, e)),
        }
    }

    /// Com a tela bloqueada, o teclado vai para a senha.
    fn tecla_na_tela_bloqueada(&mut self, dados: &[u8]) {
        let texto = String::from_utf8_lossy(dados).into_owned();
        let mut letras = texto.chars();
        while let Some(c) = letras.next() {
            match c {
                '\r' | '\n' => {
                    let nome = ulib::usuarios::nome_do_usuario(sys::uid());
                    if ulib::usuarios::conferir_senha(&nome, &self.senha) {
                        self.travada = false;
                    } else {
                        self.senha_errada = true;
                    }
                    self.senha.clear();
                }
                '\u{8}' | '\u{7f}' => {
                    self.senha.pop();
                }
                '\u{1b}' => {
                    letras.next(); // uma sequência (seta etc.): ignora
                    letras.next();
                }
                c if !c.is_control() => {
                    self.senha.push(c);
                    self.senha_errada = false;
                }
                _ => {}
            }
        }
        self.marcar_tudo();
    }

    /// O texto do relógio do painel: "qui 29 de out 21:53".
    fn texto_do_relogio(&self) -> String {
        const DIAS: [&str; 7] = ["dom", "seg", "ter", "qua", "qui", "sex", "sáb"];
        const MESES: [&str; 12] = ["jan", "fev", "mar", "abr", "mai", "jun", "jul", "ago", "set", "out", "nov", "dez"];
        let d = sys::data(sys::hora_atual());
        let hora = if self.ajustes.segundos {
            format!("{:02}:{:02}:{:02}", d.hora, d.minuto, d.segundo)
        } else {
            format!("{:02}:{:02}", d.hora, d.minuto)
        };
        format!("{} {} de {}  {}", DIAS[d.dia_da_semana as usize], d.dia, MESES[d.mes as usize - 1], hora)
    }
}

/// O botão de desligar: um anel aberto em cima, com um traço.
fn icone_desligar(q: &mut Quadro, cx: i32, cy: i32, cor: u32, fundo: u32) {
    q.anel(cx * 2, cy * 2, 15, 11, cor);
    q.retangulo(cx - 3, cy - 8, 6, 6, fundo);
    q.retangulo(cx - 1, cy - 9, 2, 8, cor);
}

/// Um cadeado.
fn icone_cadeado(q: &mut Quadro, cx: i32, cy: i32, cor: u32) {
    q.anel(cx * 2 + 1, cy * 2 - 4, 10, 6, cor);
    q.retangulo_redondo(cx - 6, cy - 1, 13, 10, 2, cor);
}

/// Uma câmera fotográfica.
fn icone_camera(q: &mut Quadro, cx: i32, cy: i32, cor: u32, fundo: u32) {
    q.retangulo_redondo(cx - 9, cy - 5, 18, 13, 3, cor);
    q.retangulo(cx - 3, cy - 8, 6, 3, cor);
    q.circulo_suave(cx * 2, cy * 2 + 2, 9, fundo);
    q.circulo_suave(cx * 2, cy * 2 + 2, 5, cor);
}

/// O cursor do mouse em miniatura (para o bloco "Cursor grande").
fn icone_cursor(q: &mut Quadro, x: i32, y: i32, cor: u32) {
    for (dy, linha) in CURSOR.iter().enumerate() {
        for (dx, c) in linha.bytes().enumerate() {
            if c == b'#' || c == b'.' {
                q.ponto(x + dx as i32, y + dy as i32, cor);
            }
        }
    }
}

impl Servidor {
    /// Desenha o painel de controle aberto.
    fn desenhar_controle(&mut self, q: &mut Quadro) {
        let (px, py, pw, ph) = self.area_controle();
        let destaque = COR_ATIVA;
        let botoes = self.botoes_controle();
        let ligados: Vec<bool> = (0..BLOCOS.len()).map(|i| self.bloco_ligado(i)).collect();
        q.retangulo_redondo(px + 2, py + 4, pw, ph, 20, 0x0A0B0D);
        q.retangulo_redondo(px, py, pw, ph, 20, 0x3A3A40);
        q.retangulo_redondo(px + 1, py + 1, pw - 2, ph - 2, 19, PC_FUNDO);
        let letras = &mut self.letras;
        // A pílula do sistema e os botões redondos.
        let resumo = format!("⚡ {} CPUs", self.cpus);
        let w = letras.largura_em(&resumo, true, 14) + 32;
        q.retangulo_redondo(px + 16, py + 16, w, 40, 20, PC_BLOCO);
        letras.escrever_em(q, px + 32, py + 26, &resumo, PC_TEXTO, true, 14);
        for (acao, cx) in botoes {
            let (cx, cy) = (px + cx, py + 36);
            let ligado = acao == Acao::Desligar && self.menu_desligar;
            let fundo = if ligado { destaque } else { PC_BLOCO };
            q.circulo_suave(cx * 2, cy * 2, 40, fundo);
            match acao {
                Acao::Desligar => icone_desligar(q, cx, cy, PC_TEXTO, fundo),
                Acao::Bloquear => icone_cadeado(q, cx, cy, PC_TEXTO),
                Acao::Captura => icone_camera(q, cx, cy, PC_TEXTO, fundo),
                Acao::Configuracoes => letras.centrado(q, cx, cy - 11, "⚙", PC_TEXTO, false, 18),
            }
        }
        if self.menu_desligar {
            for (k, opcao) in OPCOES_DESLIGAR.iter().enumerate() {
                let y = py + 70 + k as i32 * 52;
                let sobre = contem((px + 16, y, pw - 32, 44), self.mouse.0, self.mouse.1);
                q.retangulo_redondo(px + 16, y, pw - 32, 44, 22, if sobre { PC_BLOCO_SOBRE } else { PC_BLOCO });
                letras.escrever_em(q, px + 36, y + 13, opcao, PC_TEXTO, true, 14);
            }
            return;
        }
        // O brilho: a parte cheia, na cor de destaque, até o valor atual.
        let (bx, by, bw) = (px + 16, py + 70, pw - 32);
        q.retangulo_redondo(bx, by, bw, 40, 20, PC_BLOCO);
        let cheio = 40 + (bw - 40) * (self.ajustes.brilho - 30) / 70;
        q.retangulo_redondo(bx, by, cheio, 40, 20, destaque);
        letras.escrever_em(q, bx + 14, by + 10, "☀", PC_TEXTO, false, 16);
        letras.escrever_em(q, bx + bw - 52, by + 12, &format!("{}%", self.ajustes.brilho), PC_TEXTO, true, 13);
        // Os blocos que ligam e desligam.
        for (i, (rotulo, icone)) in BLOCOS.iter().enumerate() {
            let (x, y, w, h) = Self::area_bloco(i);
            let ligado = ligados[i];
            q.retangulo_redondo(px + x, py + y, w, h, 28, if ligado { destaque } else { PC_BLOCO });
            if icone.is_empty() {
                icone_cursor(q, px + x + 20, py + y + 20, PC_TEXTO);
            } else {
                letras.escrever_em(q, px + x + 18, py + y + 18, icone, PC_TEXTO, false, 16);
            }
            letras.escrever_em(q, px + x + 48, py + y + 11, rotulo, PC_TEXTO, true, 13);
            let estado = if ligado { "Ligado" } else { "Desligado" };
            letras.escrever_em(q, px + x + 48, py + y + 29, estado, PC_TEXTO_FRACO, false, 11);
        }
        // O cartão do sistema: memória e tempo ligado (do /proc).
        let (cx, cy, cw) = (px + 16, py + 260, pw - 32);
        q.retangulo_redondo(cx, cy, cw, 100, 16, 0x2E2E33);
        let meminfo = String::from_utf8_lossy(&sys::ler_arquivo("/proc/meminfo").unwrap_or_default()).into_owned();
        let campo = |nome: &str| -> u64 {
            meminfo.lines().find(|l| l.starts_with(nome)).and_then(|l| l.split_whitespace().nth(1)?.parse().ok()).unwrap_or(0)
        };
        let (total, livre) = (campo("MemTotal:") / 1024, campo("MemFree:") / 1024);
        let uptime = String::from_utf8_lossy(&sys::ler_arquivo("/proc/uptime").unwrap_or_default()).into_owned();
        let segundos: u64 = uptime.split('.').next().and_then(|s| s.parse().ok()).unwrap_or(0);
        letras.escrever_em(q, cx + 18, cy + 14, "Sistema", PC_TEXTO, true, 14);
        let ligado = format!("ligado há {} min · {} janelas", segundos / 60, self.janelas.len());
        let w = letras.largura_em(&ligado, false, 12);
        letras.escrever_em(q, cx + cw - 18 - w, cy + 16, &ligado, PC_TEXTO_FRACO, false, 12);
        letras.escrever_em(q, cx + 18, cy + 42, &format!("Memória: {} de {} MiB em uso", total - livre, total), PC_TEXTO, false, 12);
        q.retangulo_redondo(cx + 18, cy + 66, cw - 36, 10, 5, PC_BLOCO);
        let usado = if total > 0 { ((cw - 36) as u64 * (total - livre) / total) as i32 } else { 0 };
        q.retangulo_redondo(cx + 18, cy + 66, usado.max(10), 10, 5, destaque);
    }

    /// A tela bloqueada: o relógio grande e a senha.
    fn desenhar_bloqueio(&mut self, q: &mut Quadro) {
        let (l, a) = (self.largura, self.altura);
        q.retangulo(0, 0, l, a, 0x0E1A20);
        let d = sys::data(sys::hora_atual());
        let relogio = self.texto_do_relogio();
        let letras = &mut self.letras;
        letras.centrado(q, l / 2, a / 2 - 170, &format!("{:02}:{:02}", d.hora, d.minuto), PC_TEXTO, true, 72);
        let data = relogio.rsplit_once("  ").map_or(relogio.as_str(), |(dia, _)| dia).to_string();
        letras.centrado(q, l / 2, a / 2 - 70, &data, PC_TEXTO_FRACO, false, 18);
        let nome = ulib::usuarios::nome_do_usuario(sys::uid());
        q.circulo_suave(l, a - 40, 48, COR_ATIVA);
        letras.centrado(q, l / 2, a / 2 - 20 - 15, &nome[..nome.len().min(1)].to_uppercase(), PC_TEXTO, true, 24);
        letras.centrado(q, l / 2, a / 2 + 34, &nome, PC_TEXTO, true, 16);
        let (fx, fy, fw) = (l / 2 - 140, a / 2 + 70, 280);
        q.retangulo_redondo(fx, fy, fw, 40, 20, if self.senha_errada { 0x7A2020 } else { PC_BLOCO });
        let pontos: String = self.senha.chars().map(|_| '•').collect();
        let dica = if self.senha.is_empty() { "Senha" } else { &pontos };
        let cor = if self.senha.is_empty() { PC_TEXTO_FRACO } else { PC_TEXTO };
        letras.escrever_em(q, fx + 20, fy + 11, dica, cor, false, 14);
        if self.senha_errada {
            letras.centrado(q, l / 2, fy + 56, "Senha incorreta", 0xFF8A80, false, 13);
        }
    }
}
