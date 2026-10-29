//! janelas — o servidor de janelas: a interface gráfica do FerrugemOS.
//!
//! ```text
//!   janelas          (no console; Ctrl+Alt+Backspace derruba, se travar)
//! ```
//!
//! Ele abre o framebuffer (`/dev/fb0`, que põe a tela no modo gráfico), o
//! mouse e o teclado, e desenha tudo: a área de trabalho, o painel de cima e
//! as janelas, que dá para arrastar pelo título e fechar no "x". Dois tipos
//! de janela:
//!
//! - **Terminais**: o próprio servidor emula um terminal ligado a um
//!   pseudo-terminal (`/dev/ptmx`), com um `sh` do outro lado. O que você
//!   digita vai para o mestre; o que o shell escreve volta por ele e é
//!   desenhado (com as cores ANSI).
//! - **Programas**: qualquer programa pode pedir uma janela pelo socket
//!   `/tmp/.janelas` (veja `ulib::janela` e o `pintar`). A imagem dela fica
//!   numa memória compartilhada que o programa desenha e nós só copiamos.
//!
//! Tudo num laço só: um `poll` espera o mouse, o teclado, o socket e cada
//! janela; o que mudou é redesenhado numa imagem na memória, que depois é
//! copiada para a tela. O cursor do mouse é desenhado por cima, direto na
//! tela, para que mexer o mouse não exija redesenhar tudo.

#![no_std]
#![no_main]

use ulib::grafico::{Fonte, Quadro, ALTURA_LETRA, LARGURA_LETRA};
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
        })
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
            q.letra(fonte, x, y, cel.c, PALETA[cel.frente as usize], Some(PALETA[cel.fundo as usize]));
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

struct Janela {
    x: i32,
    y: i32,
    largura: i32,
    altura: i32,
    titulo: String,
    conteudo: Conteudo,
}

impl Janela {
    fn largura_total(&self) -> i32 {
        self.largura + 2 * BORDA
    }
    fn altura_total(&self) -> i32 {
        self.altura + TITULO + 2 * BORDA
    }
    fn dentro(&self, x: i32, y: i32) -> bool {
        x >= self.x && y >= self.y && x < self.x + self.largura_total() && y < self.y + self.altura_total()
    }
    fn no_titulo(&self, x: i32, y: i32) -> bool {
        self.dentro(x, y) && y < self.y + BORDA + TITULO
    }
    /// O "x" de fechar, no canto direito do título.
    fn no_fechar(&self, x: i32, y: i32) -> bool {
        let fx = self.x + BORDA + self.largura - 18;
        self.no_titulo(x, y) && x >= fx && x < fx + 16
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

    fn desenhar(&self, q: &mut Quadro, fonte: &Fonte, ativa: bool) {
        let (w, h) = (self.largura_total(), self.altura_total());
        q.retangulo(self.x + 4, self.y + 4, w, h, 0x0E2630); // sombra
        q.retangulo(self.x, self.y, w, h, COR_BORDA);
        let (cx, cy) = (self.x + BORDA, self.y + BORDA);
        q.retangulo(cx, cy, self.largura, TITULO, if ativa { COR_ATIVA } else { COR_INATIVA });
        q.texto(fonte, cx + 6, cy + 2, &self.titulo, COR_TEXTO, None);
        let fx = cx + self.largura - 18;
        q.retangulo(fx, cy + 2, 16, 16, 0x2B2F38);
        q.letra(fonte, fx + 4, cy + 2, 'x', COR_TEXTO, None);
        let y0 = cy + TITULO;
        match &self.conteudo {
            Conteudo::Terminal(t) => t.desenhar(q, fonte, cx, y0, ativa),
            Conteudo::Cliente(c) => {
                let n = (self.largura * self.altura) as usize;
                let imagem = unsafe { core::slice::from_raw_parts(c.pixels as *const u32, n) };
                q.copiar(imagem, self.largura, self.altura, cx, y0);
            }
        }
    }
}

// -----------------------------------------------------------------------------
// O servidor
// -----------------------------------------------------------------------------

struct Botao {
    x: i32,
    largura: i32,
    rotulo: &'static str,
}

struct Servidor {
    tela: &'static mut [u32],
    imagem: Vec<u32>,
    largura: i32,
    altura: i32,
    fonte: Fonte,
    janelas: Vec<Janela>, // a última é a de cima (a ativa)
    pendentes: Vec<(i32, Vec<u8>)>, // conectados que ainda não pediram janela
    mouse: (i32, i32),
    botoes: u8,
    /// Arrastando uma janela pelo título: o deslocamento do clique.
    arrastando: Option<(i32, i32)>,
    /// O botão está apertado dentro de um programa (os movimentos vão para ele).
    segurando_cliente: bool,
    cursor_desenhado: Option<(i32, i32)>,
    painel: Vec<Botao>,
    sair: bool,
    sujo: bool,
    proxima_posicao: i32,
    hora_mostrada: String,
}

impl Servidor {
    fn compor(&mut self) {
        let hora = String::from(&sys::formatar_data(sys::hora_atual())[11..16]);
        // Empréstimos separados de cada campo: a imagem muda, o resto só é lido.
        let Servidor { imagem, largura, altura, fonte, janelas, painel, .. } = self;
        let (largura, altura) = (*largura, *altura);
        let mut q = Quadro { pixels: imagem, largura, altura };
        q.retangulo(0, PAINEL, largura, altura - PAINEL, COR_FUNDO);
        q.texto(fonte, largura / 2 - 5 * 8, altura / 2, "FerrugemOS", 0x2F6A7E, None);
        q.retangulo(0, 0, largura, PAINEL, COR_PAINEL);
        q.texto(fonte, 8, 4, "FerrugemOS", COR_ATIVA, None);
        for b in painel.iter() {
            q.retangulo(b.x, 3, b.largura, PAINEL - 6, 0x30353F);
            q.texto(fonte, b.x + 8, 4, b.rotulo, COR_TEXTO, None);
        }
        q.texto(fonte, largura - 8 * 10, 4, &format!("{} UTC", hora), COR_TEXTO, None);
        let n = janelas.len();
        for (i, j) in janelas.iter().enumerate() {
            j.desenhar(&mut q, fonte, i + 1 == n);
        }
        self.hora_mostrada = hora;
        // Copia para a tela (o cursor é redesenhado por cima em seguida).
        self.tela[..self.imagem.len()].copy_from_slice(&self.imagem);
        self.cursor_desenhado = None;
        self.desenhar_cursor();
        self.sujo = false;
    }

    /// Apaga o cursor (repondo o que havia embaixo) e o desenha na posição nova.
    fn desenhar_cursor(&mut self) {
        let (l, a) = (self.largura, self.altura);
        if let Some((x0, y0)) = self.cursor_desenhado.take() {
            for y in y0..(y0 + 16).min(a) {
                for x in x0..(x0 + 12).min(l) {
                    let i = (y * l + x) as usize;
                    self.tela[i] = self.imagem[i];
                }
            }
        }
        let (mx, my) = self.mouse;
        for (dy, linha) in CURSOR.iter().enumerate() {
            for (dx, c) in linha.bytes().enumerate() {
                let (x, y) = (mx + dx as i32, my + dy as i32);
                if x < l && y < a && c != b' ' {
                    self.tela[(y * l + x) as usize] = if c == b'#' { 0 } else { 0xFFFFFF };
                }
            }
        }
        self.cursor_desenhado = Some((mx, my));
    }

    fn abrir_terminal(&mut self) {
        let (colunas, linhas) = (80, 25);
        let Some(t) = Terminal::novo(colunas, linhas) else { return };
        let (x, y) = self.posicao_nova();
        self.janelas.push(Janela {
            x,
            y,
            largura: colunas as i32 * LARGURA_LETRA,
            altura: linhas as i32 * ALTURA_LETRA,
            titulo: format!("Terminal (sh {})", t.pid),
            conteudo: Conteudo::Terminal(t),
        });
        self.sujo = true;
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
        self.sujo = true;
    }

    fn mandar_ao_cliente(&self, texto: &str) {
        if let Some(Janela { conteudo: Conteudo::Cliente(c), .. }) = self.janelas.last() {
            let _ = sys::escrever_tudo(c.fd, texto.as_bytes());
        }
    }

    fn clique(&mut self, apertado: bool, botao: u8) {
        let (x, y) = self.mouse;
        if !apertado {
            self.arrastando = None;
            if core::mem::take(&mut self.segurando_cliente) {
                if let Some(j) = self.janelas.last() {
                    let (lx, ly) = j.local(x, y);
                    self.mandar_ao_cliente(&format!("botao {} {} {} 0\n", lx, ly, botao));
                }
            }
            return;
        }
        if y < PAINEL {
            let rotulo = self.painel.iter().find(|b| x >= b.x && x < b.x + b.largura).map(|b| b.rotulo);
            match rotulo {
                Some("Terminal") => self.abrir_terminal(),
                Some("Pintar") => self.lancar("pintar"),
                Some("Sair") => self.sair = true,
                _ => {}
            }
            return;
        }
        let Some(i) = self.janelas.iter().rposition(|j| j.dentro(x, y)) else { return };
        // Clicar numa janela a traz para a frente (vira a ativa).
        let j = self.janelas.remove(i);
        self.janelas.push(j);
        self.sujo = true;
        let j = &self.janelas[self.janelas.len() - 1];
        if j.no_fechar(x, y) {
            let n = self.janelas.len() - 1;
            self.fechar_janela(n);
        } else if j.no_titulo(x, y) {
            self.arrastando = Some((x - j.x, y - j.y));
        } else if let Conteudo::Cliente(_) = j.conteudo {
            let (lx, ly) = j.local(x, y);
            if lx >= 0 && ly >= 0 {
                self.segurando_cliente = true;
                self.mandar_ao_cliente(&format!("botao {} {} {} 1\n", lx, ly, botao));
            }
        }
    }

    /// Pacotes de 3 bytes do /dev/mouse.
    fn pacotes_do_mouse(&mut self, dados: &[u8]) {
        for p in dados.chunks_exact(3) {
            if p[0] & 0xC0 != 0 {
                continue; // estouro: pacote sem sentido
            }
            let dx = p[1] as i32 - if p[0] & 0x10 != 0 { 256 } else { 0 };
            let dy = p[2] as i32 - if p[0] & 0x20 != 0 { 256 } else { 0 };
            self.mouse.0 = (self.mouse.0 + dx).clamp(0, self.largura - 1);
            self.mouse.1 = (self.mouse.1 - dy).clamp(0, self.altura - 1);
            let botoes = p[0] & 3;
            let mudou = botoes ^ self.botoes;
            self.botoes = botoes;
            for (bit, numero) in [(1, 1), (2, 2)] {
                if mudou & bit != 0 {
                    self.clique(botoes & bit != 0, numero);
                }
            }
            if let (Some((ox, oy)), Some(j)) = (self.arrastando, self.janelas.last_mut()) {
                j.x = self.mouse.0 - ox;
                j.y = (self.mouse.1 - oy).max(PAINEL);
                self.sujo = true;
            } else if self.segurando_cliente && (dx != 0 || dy != 0) {
                if let Some(j) = self.janelas.last() {
                    let (lx, ly) = j.local(self.mouse.0, self.mouse.1);
                    self.mandar_ao_cliente(&format!("mover {} {}\n", lx, ly));
                }
            }
        }
    }

    fn teclado(&mut self, dados: &[u8]) {
        let Some(j) = self.janelas.last() else { return };
        match &j.conteudo {
            Conteudo::Terminal(t) => {
                let _ = sys::escrever_tudo(t.mestre, dados);
            }
            Conteudo::Cliente(c) => {
                for ch in String::from_utf8_lossy(dados).chars() {
                    let _ = sys::escrever_tudo(c.fd, format!("tecla {}\n", ch as u32).as_bytes());
                }
            }
        }
    }

    /// Um programa conectado mandou um pedido: "janela L A título" ou "atualizar".
    fn pedido(&mut self, fd: i32, linha: &str) -> bool {
        if let Some(resto) = linha.strip_prefix("janela ") {
            let mut partes = resto.splitn(3, ' ');
            let largura: i32 = partes.next().and_then(|v| v.parse().ok()).unwrap_or(0);
            let altura: i32 = partes.next().and_then(|v| v.parse().ok()).unwrap_or(0);
            let titulo = String::from(partes.next().unwrap_or("?"));
            if !(1..=1000).contains(&largura) || !(1..=700).contains(&altura) {
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
            let (x, y) = self.posicao_nova();
            let cliente = Cliente { fd, entrada: Vec::new(), pixels, shm };
            self.janelas.push(Janela { x, y, largura, altura, titulo, conteudo: Conteudo::Cliente(cliente) });
            let _ = sys::escrever_tudo(fd, format!("ok {}\n", shm).as_bytes());
            self.sujo = true;
            return true;
        }
        false
    }

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
                if prontos[3 + i] == 0 {
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
                let j = &mut self.janelas[k];
                match (&mut j.conteudo, lido) {
                    (Conteudo::Terminal(t), Ok(n)) if n > 0 => {
                        t.alimentar(&buf[..n]);
                        self.sujo = true;
                    }
                    (Conteudo::Cliente(c), Ok(n)) if n > 0 => {
                        c.entrada.extend_from_slice(&buf[..n]);
                        while let Some(fim) = c.entrada.iter().position(|&b| b == b'\n') {
                            let linha: Vec<u8> = c.entrada.drain(..=fim).collect();
                            if linha.starts_with(b"atualizar") {
                                self.sujo = true;
                            }
                        }
                    }
                    // O shell saiu (EIO/fim) ou o programa fechou a conexão.
                    _ => self.fechar_janela(k),
                }
            }
            while let Ok(Some(_)) = sys::recolher_terminado() {}
            let hora = &sys::formatar_data(sys::hora_atual())[11..16];
            if self.sujo || hora != self.hora_mostrada {
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

    let mut painel = Vec::new();
    let mut x = 110;
    for rotulo in ["Terminal", "Pintar", "Sair"] {
        let largura = rotulo.len() as i32 * 8 + 16;
        painel.push(Botao { x, largura, rotulo });
        x += largura + 8;
    }
    let mut s = Servidor {
        tela,
        imagem: vec![0; (largura * altura) as usize],
        largura,
        altura,
        fonte,
        janelas: Vec::new(),
        pendentes: Vec::new(),
        mouse: (largura / 2, altura / 2),
        botoes: 0,
        arrastando: None,
        segurando_cliente: false,
        cursor_desenhado: None,
        painel,
        sair: false,
        sujo: true,
        proxima_posicao: 0,
        hora_mostrada: String::new(),
    };
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
