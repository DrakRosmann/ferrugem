//! navegador — um navegador web: HTML e imagens PNG (sem JavaScript, e do
//! CSS só o atributo `style` mais simples). Rode dentro do `janelas`.
//!
//! O caminho de uma página:
//!
//! ```text
//!   endereço ─▶ processo filho: DNS, TCP, HTTP ─▶ pipe ─▶ bytes da resposta
//!                (a janela continua respondendo enquanto isso)
//!   bytes ─▶ html::analisar ─▶ árvore de elementos
//!   árvore ─▶ diagramação (para a largura da janela) ─▶ lista de desenhos
//!                (palavras, retângulos, imagens, cada um no seu lugar)
//!   desenhos ─▶ a parte visível é desenhada a cada evento
//! ```
//!
//! As imagens vêm depois do texto, uma a uma; cada uma que chega refaz a
//! diagramação. Atalhos: Ctrl+L (endereço), Ctrl+R (recarregar), Backspace
//! (voltar), espaço/PgDn/PgUp/setas/Home/End (rolar).

#![no_std]
#![no_main]

use ulib::html::{self, Elemento, No};
use ulib::http::{self, Url};
use ulib::janela::{Janela, Seta};
use ulib::png;
use ulib::sys;
use ulib::ui::{self, CampoTexto, Tecla, Tela, Ui};
use ulib::*;

programa!(principal);

const COR_TEXTO: u32 = 0x1E1E1E;
const COR_FRACA: u32 = 0x6A6A6A;
const COR_LINK: u32 = 0x1A5FB4;
const COR_LINK_SOBRE: u32 = 0x0B3D7A;
const COR_VISITADO: u32 = 0x813D9C;
const COR_PAGINA: u32 = 0xFFFFFF;
/// A margem dos lados da página.
const MARGEM: i32 = 18;
const BARRA_ESTADO: i32 = 22;
const TAMANHO: u16 = 14;
const INICIO: &str = "sobre:inicio";

// -----------------------------------------------------------------------------
// A diagramação
// -----------------------------------------------------------------------------

#[derive(Clone)]
struct Estilo {
    tamanho: u16,
    negrito: bool,
    italico: bool,
    mono: bool,
    cor: u32,
    sublinhado: bool,
    riscado: bool,
    /// O índice do link (em `Pagina::links`).
    link: Option<usize>,
    /// Espaços e quebras de linha valem como estão (`<pre>`).
    pre: bool,
}

const ESTILO_BASE: Estilo = Estilo {
    tamanho: TAMANHO,
    negrito: false,
    italico: false,
    mono: false,
    cor: COR_TEXTO,
    sublinhado: false,
    riscado: false,
    link: None,
    pre: false,
};

/// Uma coisa a desenhar, já no seu lugar (coordenadas da página).
enum Desenho {
    Texto { x: i32, y: i32, largura: i32, altura: i32, texto: String, estilo: Estilo },
    Retangulo { x: i32, y: i32, largura: i32, altura: i32, cor: u32 },
    /// Um campo ou botão de formulário (só a aparência).
    Controle { x: i32, y: i32, largura: i32, altura: i32, texto: String, botao: bool },
    Imagem { x: i32, y: i32, largura: i32, altura: i32, indice: usize, link: Option<usize> },
}

impl Desenho {
    fn caixa(&self) -> (i32, i32, i32, i32) {
        match *self {
            Desenho::Texto { x, y, largura, altura, .. }
            | Desenho::Retangulo { x, y, largura, altura, .. }
            | Desenho::Controle { x, y, largura, altura, .. }
            | Desenho::Imagem { x, y, largura, altura, .. } => (x, y, largura, altura),
        }
    }

    fn mover(&mut self, dx: i32, dy: i32) {
        match self {
            Desenho::Texto { x, y, .. }
            | Desenho::Retangulo { x, y, .. }
            | Desenho::Controle { x, y, .. }
            | Desenho::Imagem { x, y, .. } => {
                *x += dx;
                *y += dy;
            }
        }
    }

    fn link(&self) -> Option<usize> {
        match self {
            Desenho::Texto { estilo, .. } => estilo.link,
            Desenho::Imagem { link, .. } => *link,
            _ => None,
        }
    }
}

enum EstadoImagem {
    Esperando,
    Pronta(png::Imagem),
    /// Não deu para baixar, ou não é PNG.
    Falhou,
}

struct ImagemPagina {
    url: Url,
    estado: EstadoImagem,
}

/// Uma cor do HTML: #rgb, #rrggbb, rgb(r, g, b) ou um nome. Cores claras
/// demais são ignoradas: como não desenhamos os fundos do CSS, um texto
/// branco (feito para um fundo escuro) sumiria na página branca.
fn ler_cor(texto: &str) -> Option<u32> {
    let t = texto.trim().to_ascii_lowercase();
    let cor = if let Some(hexa) = t.strip_prefix('#') {
        match hexa.len() {
            3 => {
                let v = u32::from_str_radix(hexa, 16).ok()?;
                let (r, g, b) = (v >> 8 & 0xF, v >> 4 & 0xF, v & 0xF);
                (r * 17) << 16 | (g * 17) << 8 | b * 17
            }
            6 => u32::from_str_radix(hexa, 16).ok()?,
            _ => return None,
        }
    } else if let Some(dentro) = t.strip_prefix("rgb(").or_else(|| t.strip_prefix("rgba(")) {
        let v: Vec<u32> = dentro.trim_end_matches(')').split(',').take(3).filter_map(|p| p.trim().parse().ok()).collect();
        if v.len() != 3 {
            return None;
        }
        v[0].min(255) << 16 | v[1].min(255) << 8 | v[2].min(255)
    } else {
        match t.as_str() {
            "black" => 0x000000,
            "red" => 0xCC0000,
            "green" => 0x008000,
            "blue" => 0x0000CC,
            "navy" => 0x000080,
            "maroon" => 0x800000,
            "purple" => 0x800080,
            "teal" => 0x008080,
            "olive" => 0x808000,
            "gray" | "grey" => 0x808080,
            "darkred" => 0x8B0000,
            "darkgreen" => 0x006400,
            "darkblue" => 0x00008B,
            "orange" => 0xD2691E,
            "brown" => 0x8B4513,
            _ => return None,
        }
    };
    let (r, g, b) = (cor >> 16 & 0xFF, cor >> 8 & 0xFF, cor & 0xFF);
    if r * 3 + g * 6 + b > 2000 {
        return None;
    }
    Some(cor)
}

/// As declarações do atributo `style` (`"color: red; display: none"`).
fn estilo_inline(e: &Elemento) -> Vec<(String, String)> {
    e.atributo("style")
        .unwrap_or("")
        .split(';')
        .filter_map(|d| d.split_once(':'))
        .map(|(k, v)| (k.trim().to_ascii_lowercase(), v.trim().to_ascii_lowercase()))
        .collect()
}

fn escondido(e: &Elemento) -> bool {
    e.atributo("hidden").is_some()
        || e.atributo("aria-hidden") == Some("true") && e.nome != "body"
        || estilo_inline(e).iter().any(|(k, v)| k == "display" && v.starts_with("none") || k == "visibility" && v == "hidden")
}

/// Um número de um atributo de tamanho ("120", "120px"; "50%" não vale).
fn ler_pixels(v: Option<&str>) -> Option<i32> {
    let v = v?.trim().trim_end_matches("px");
    v.parse::<i32>().ok().filter(|&n| n > 0 && n < 10_000)
}

struct Diagramador<'a, 'b> {
    t: &'a mut Tela<'b>,
    base: Url,
    esquerda: i32,
    direita: i32,
    /// O topo da linha que está sendo montada.
    y: i32,
    /// Onde vai a próxima palavra.
    x: i32,
    /// A linha em montagem (com y relativo a ela; vão para `saida` ao quebrar).
    linha: Vec<Desenho>,
    /// Quantos itens da linha são marcadores (de lista): eles esperam o texto.
    marcadores: usize,
    centro: bool,
    /// A largura do espaço pendente antes da próxima palavra (0 = nenhum).
    espaco: i32,
    /// A margem vertical pendente (margens seguidas não se somam: vale a maior).
    margem: i32,
    saida: Vec<Desenho>,
    links: Vec<Url>,
    imagens: &'a mut Vec<ImagemPagina>,
    ancoras: Vec<(String, i32)>,
    /// As listas abertas: o próximo número (`<ol>`) ou nenhum (`<ul>`).
    listas: Vec<Option<u32>>,
    primeira_celula: bool,
}

impl Diagramador<'_, '_> {
    fn altura(&mut self, e: &Estilo) -> i32 {
        self.t.altura_texto(e.tamanho, e.mono)
    }

    fn comecar_linha(&mut self) {
        if self.linha.is_empty() {
            self.y += self.margem;
            self.margem = 0;
            self.x = self.esquerda;
        }
    }

    /// Fecha a linha: alinha as coisas pela base e passa para a saída.
    fn quebrar(&mut self) {
        if self.linha.len() == self.marcadores {
            return; // só o marcador da lista: espera o texto do item
        }
        let altura = self.linha.iter().map(|d| d.caixa().3).max().unwrap_or(0);
        let deslocamento = if self.centro { ((self.direita - self.x) / 2).max(0) } else { 0 };
        let y = self.y;
        for mut d in self.linha.drain(..) {
            let h = d.caixa().3;
            d.mover(deslocamento, y + altura - h);
            self.saida.push(d);
        }
        self.marcadores = 0;
        self.y += altura;
        self.x = self.esquerda;
        self.espaco = 0;
    }

    /// `<br>`: numa linha vazia, uma linha em branco.
    fn quebra_forcada(&mut self, e: &Estilo) {
        if self.linha.is_empty() {
            self.comecar_linha();
            self.y += self.altura(e);
        } else {
            self.quebrar();
        }
    }

    fn bloco(&mut self, margem: i32) {
        self.quebrar();
        self.margem = self.margem.max(margem);
    }

    /// Põe uma coisa na linha (quebrando antes se não couber).
    fn colocar(&mut self, mut d: Desenho, largura: i32) {
        let mut espaco = if self.linha.len() > self.marcadores { self.espaco } else { 0 };
        if self.linha.len() > self.marcadores && self.x + espaco + largura > self.direita {
            self.quebrar();
            espaco = 0;
        }
        self.comecar_linha();
        d.mover(self.x + espaco, 0);
        self.x += espaco + largura;
        self.linha.push(d);
        self.espaco = 0;
    }

    fn palavra(&mut self, texto: String, e: &Estilo) {
        let largura = self.t.medir(&texto, e.tamanho, e.negrito, e.mono);
        let altura = self.altura(e);
        self.colocar(Desenho::Texto { x: 0, y: 0, largura, altura, texto, estilo: e.clone() }, largura);
    }

    fn texto(&mut self, t: &str, e: &Estilo) {
        if e.pre {
            for (i, linha) in t.split('\n').enumerate() {
                if i > 0 {
                    self.quebra_forcada(e);
                }
                let linha = linha.replace('\t', "        ").replace('\r', "");
                if !linha.is_empty() {
                    self.palavra(linha, e);
                }
            }
            return;
        }
        let branco = |c: char| matches!(c, ' ' | '\n' | '\t' | '\r' | '\x0c');
        let largura_espaco = self.t.medir(" ", e.tamanho, e.negrito, e.mono);
        if t.starts_with(branco) {
            self.espaco = largura_espaco;
        }
        for (i, palavra) in t.split(branco).filter(|p| !p.is_empty()).enumerate() {
            if i > 0 {
                self.espaco = largura_espaco;
            }
            self.palavra(palavra.replace('\u{A0}', " ").replace('\u{AD}', ""), e);
        }
        if t.ends_with(branco) && !t.trim_matches(branco).is_empty() {
            self.espaco = largura_espaco;
        }
    }

    fn imagem(&mut self, e: &Elemento, s: &Estilo) {
        let fonte = e.atributo("src").filter(|v| !v.is_empty()).or(e.atributo("data-src"));
        let alt = e.atributo("alt").unwrap_or("").trim();
        let Some(url) = fonte.filter(|f| !f.starts_with("data:")).and_then(|f| self.base.juntar(f)) else {
            if !alt.is_empty() {
                self.texto(&format!("[{}]", alt), &Estilo { cor: COR_FRACA, ..s.clone() });
            }
            return;
        };
        let indice = match self.imagens.iter().position(|i| i.url == url) {
            Some(i) => i,
            None => {
                self.imagens.push(ImagemPagina { url, estado: EstadoImagem::Esperando });
                self.imagens.len() - 1
            }
        };
        let (aw, ah) = (ler_pixels(e.atributo("width")), ler_pixels(e.atributo("height")));
        let (mut w, mut h) = match &self.imagens[indice].estado {
            EstadoImagem::Pronta(img) => {
                let (iw, ih) = (img.largura as i32, img.altura as i32);
                match (aw, ah) {
                    (Some(w), Some(h)) => (w, h),
                    (Some(w), None) => (w, w * ih / iw.max(1)),
                    (None, Some(h)) => (h * iw / ih.max(1), h),
                    (None, None) => (iw, ih),
                }
            }
            estado => match (aw, ah) {
                (Some(w), Some(h)) if !matches!(estado, EstadoImagem::Falhou) || alt.is_empty() => (w, h),
                _ => {
                    // Sem tamanho (ou sem imagem): o texto alternativo.
                    if !alt.is_empty() {
                        self.texto(&format!("[{}]", alt), &Estilo { cor: COR_FRACA, ..s.clone() });
                    }
                    return;
                }
            },
        };
        let maximo = (self.direita - self.esquerda).max(1);
        if w > maximo {
            h = h * maximo / w;
            w = maximo;
        }
        if w > 0 && h > 0 {
            self.colocar(Desenho::Imagem { x: 0, y: 0, largura: w, altura: h, indice, link: s.link }, w);
        }
    }

    fn controle(&mut self, texto: String, largura: i32, botao: bool) {
        let altura = 24;
        let largura = largura.max(self.t.medir(&texto, 13, false, false) + 16);
        self.espaco = self.espaco.max(4);
        self.colocar(Desenho::Controle { x: 0, y: 0, largura, altura, texto, botao }, largura);
        self.espaco = 4;
    }

    fn entrada(&mut self, e: &Elemento, s: &Estilo) {
        let tipo = e.atributo("type").unwrap_or("text").to_ascii_lowercase();
        let valor = e.atributo("value").unwrap_or("");
        match tipo.as_str() {
            "hidden" => {}
            "submit" | "button" | "reset" => {
                let padrao = if tipo == "reset" { "Limpar" } else { "Enviar" };
                self.controle(String::from(if valor.is_empty() { padrao } else { valor }), 0, true);
            }
            "checkbox" | "radio" => self.controle(String::new(), 14, false),
            "image" => self.imagem(e, s),
            _ => {
                let texto = if valor.is_empty() { e.atributo("placeholder").unwrap_or("") } else { valor };
                let colunas = e.atributo("size").and_then(|v| v.parse::<i32>().ok()).unwrap_or(20).clamp(4, 60);
                self.controle(String::from(texto), colunas * 8, false);
            }
        }
    }

    fn filhos(&mut self, e: &Elemento, s: &Estilo) {
        for f in &e.filhos {
            match f {
                No::Texto(t) => self.texto(t, s),
                No::Elemento(filho) => self.elemento(filho, s),
            }
        }
    }

    fn elemento(&mut self, e: &Elemento, pai: &Estilo) {
        if escondido(e) {
            return;
        }
        let nome = e.nome.as_str();
        match nome {
            "head" | "script" | "style" | "template" | "title" | "meta" | "link" | "base" | "svg" | "canvas" | "video"
            | "audio" | "object" | "embed" | "map" | "datalist" | "noembed" | "param" | "source" | "track" | "wbr" => {
                return;
            }
            "br" => return self.quebra_forcada(pai),
            "hr" => {
                self.bloco(10);
                self.comecar_linha();
                let (x, largura) = (self.esquerda, self.direita - self.esquerda);
                self.saida.push(Desenho::Retangulo { x, y: self.y, largura, altura: 1, cor: 0xC8C8C8 });
                self.y += 1;
                return self.bloco(10);
            }
            "img" => return self.imagem(e, pai),
            "input" => return self.entrada(e, pai),
            "select" => {
                let primeira = e.achar("option").map(|o| o.texto()).unwrap_or_default();
                return self.controle(format!("{} ▾", primeira.trim()), 0, false);
            }
            "textarea" => return self.controle(e.texto().trim().into(), 240, false),
            "button" => return self.controle(e.texto().split_whitespace().collect::<Vec<_>>().join(" "), 0, true),
            "iframe" => {
                if let Some(url) = e.atributo("src").and_then(|s| self.base.juntar(s)) {
                    self.links.push(url);
                    let s = Estilo { link: Some(self.links.len() - 1), cor: COR_LINK, sublinhado: true, ..pai.clone() };
                    self.texto(" [quadro incorporado] ", &s);
                }
                return;
            }
            _ => {}
        }
        let mut s = pai.clone();
        // (margem antes, margem depois, recuo) dos blocos
        let mut bloco: Option<(i32, i32, i32)> = match nome {
            "p" | "dl" | "figure" | "fieldset" => Some((12, 12, 0)),
            "pre" | "listing" | "xmp" | "plaintext" => Some((10, 10, 0)),
            "blockquote" => Some((10, 10, 28)),
            "ul" | "ol" | "dir" | "menu" => {
                let margem = if self.listas.is_empty() { 10 } else { 0 };
                Some((margem, margem, 28))
            }
            "dd" => Some((0, 4, 28)),
            "table" => Some((8, 8, 0)),
            "h1" => Some((18, 10, 0)),
            "h2" => Some((16, 8, 0)),
            "h3" => Some((14, 6, 0)),
            "h4" | "h5" | "h6" => Some((12, 6, 0)),
            "li" => Some((3, 3, 0)),
            "div" | "section" | "article" | "header" | "footer" | "nav" | "main" | "aside" | "center" | "address"
            | "details" | "summary" | "figcaption" | "caption" | "tr" | "dt" | "body" | "html" | "noscript" | "hgroup"
            | "legend" | "form" | "option" | "documento" => Some((0, 0, 0)),
            _ => None,
        };
        match nome {
            "b" | "strong" | "th" | "dt" | "summary" | "legend" => s.negrito = true,
            "i" | "em" | "cite" | "dfn" | "address" => s.italico = true,
            "code" | "kbd" | "samp" | "tt" | "var" => s.mono = true,
            "pre" | "listing" | "xmp" | "plaintext" => {
                s.mono = true;
                s.pre = true;
                s.tamanho = 13;
            }
            "u" | "ins" => s.sublinhado = true,
            "s" | "strike" | "del" => s.riscado = true,
            "small" | "sub" | "sup" => s.tamanho = (s.tamanho * 85 / 100).max(10),
            "big" => s.tamanho += 3,
            "h1" => (s.tamanho, s.negrito) = (28, true),
            "h2" => (s.tamanho, s.negrito) = (22, true),
            "h3" => (s.tamanho, s.negrito) = (18, true),
            "h4" => (s.tamanho, s.negrito) = (15, true),
            "h5" => (s.tamanho, s.negrito) = (14, true),
            "h6" => (s.tamanho, s.negrito) = (13, true),
            "font" => {
                if let Some(c) = e.atributo("color").and_then(ler_cor) {
                    s.cor = c;
                }
            }
            "a" => {
                if let Some(url) = e.atributo("href").and_then(|h| self.base.juntar(h)) {
                    self.links.push(url);
                    s.link = Some(self.links.len() - 1);
                    s.cor = COR_LINK;
                    s.sublinhado = true;
                }
            }
            _ => {}
        }
        let mut centro = nome == "center" || e.atributo("align").is_some_and(|a| a.eq_ignore_ascii_case("center"));
        for (k, v) in estilo_inline(e) {
            match k.as_str() {
                "color" if s.link.is_none() => {
                    if let Some(c) = ler_cor(&v) {
                        s.cor = c;
                    }
                }
                "font-weight" => s.negrito = v.starts_with("bold") || v.parse::<u32>().is_ok_and(|p| p >= 600),
                "font-style" => s.italico = v.starts_with("italic") || v.starts_with("oblique"),
                "text-align" => centro = v.starts_with("center"),
                "font-family" if v.contains("monospace") => s.mono = true,
                "white-space" if v.starts_with("pre") => s.pre = true,
                "text-decoration" if v.contains("underline") => s.sublinhado = true,
                "text-decoration" if v.contains("line-through") => s.riscado = true,
                "display" if v == "block" || v == "flex" => {
                    bloco.get_or_insert((0, 0, 0));
                }
                _ => {}
            }
        }
        if let Some(id) = e.atributo("id").or(if nome == "a" { e.atributo("name") } else { None }) {
            self.ancoras.push((String::from(id), self.y + self.margem));
        }
        let (antes_esquerda, antes_centro) = (self.esquerda, self.centro);
        if let Some((antes, _, recuo)) = bloco {
            self.bloco(antes);
            self.esquerda += recuo;
            self.x = self.x.max(self.esquerda);
            if centro {
                self.centro = true;
            } else if bloco.is_some() && nome != "documento" && nome != "body" && nome != "html" {
                // um bloco sem alinhamento próprio herda o do pai (é o que já está)
            }
        }
        match nome {
            "ul" | "dir" | "menu" => self.listas.push(None),
            "ol" => self.listas.push(Some(e.atributo("start").and_then(|v| v.parse().ok()).unwrap_or(1))),
            "li" => {
                let marcador = match self.listas.last_mut() {
                    Some(Some(n)) => {
                        *n += 1;
                        format!("{}.", *n - 1)
                    }
                    _ => String::from("•"),
                };
                let base = Estilo { link: None, sublinhado: false, riscado: false, ..s.clone() };
                let largura = self.t.medir(&marcador, base.tamanho, base.negrito, base.mono);
                let altura = self.altura(&base);
                self.comecar_linha();
                let x = self.esquerda - largura - 8;
                self.linha.push(Desenho::Texto { x, y: 0, largura, altura, texto: marcador, estilo: base });
                self.marcadores = self.linha.len();
            }
            "tr" => self.primeira_celula = true,
            "td" | "th" => {
                if !self.primeira_celula {
                    self.espaco = self.espaco.max(18);
                }
                self.primeira_celula = false;
            }
            "q" => self.texto("“", &s),
            _ => {}
        }
        self.filhos(e, &s);
        match nome {
            "ul" | "ol" | "dir" | "menu" => {
                self.listas.pop();
            }
            "q" => self.texto("”", &s),
            "td" | "th" => self.espaco = self.espaco.max(18),
            _ => {}
        }
        if let Some((_, depois, _)) = bloco {
            self.bloco(depois);
            self.esquerda = antes_esquerda;
            self.centro = antes_centro;
        }
    }
}

// -----------------------------------------------------------------------------
// A página
// -----------------------------------------------------------------------------

struct Pagina {
    url: Url,
    titulo: String,
    raiz: Elemento,
    imagens: Vec<ImagemPagina>,
    /// A largura da última diagramação (0 = refazer).
    largura: i32,
    desenhos: Vec<Desenho>,
    links: Vec<Url>,
    ancoras: Vec<(String, i32)>,
    altura: i32,
    /// Uma linha para a barra de estado ("HTTP 404", "123 KiB"...).
    info: String,
}

impl Pagina {
    fn de_html(url: Url, texto: &str, info: String) -> Pagina {
        let raiz = html::analisar(texto);
        let titulo = raiz.achar("title").map(|t| t.texto().split_whitespace().collect::<Vec<_>>().join(" ")).unwrap_or_default();
        Pagina {
            url,
            titulo,
            raiz,
            imagens: Vec::new(),
            largura: 0,
            desenhos: Vec::new(),
            links: Vec::new(),
            ancoras: Vec::new(),
            altura: 0,
            info,
        }
    }

    fn de_texto(url: Url, texto: &str, info: String) -> Pagina {
        let mut pre = Elemento { nome: "pre".into(), atributos: Vec::new(), filhos: Vec::new() };
        pre.filhos.push(No::Texto(texto.into()));
        let mut p = Pagina::de_html(url, "", info);
        p.raiz.filhos.push(No::Elemento(pre));
        p
    }

    fn de_imagem(url: Url, imagem: png::Imagem, info: String) -> Pagina {
        let nome = url.caminho.rsplit('/').next().unwrap_or("").to_string();
        let titulo = format!("{} ({} × {})", nome, imagem.largura, imagem.altura);
        let mut p = Pagina::de_html(url.clone(), "<center><img src=\"\"></center>", info);
        // O src vazio aponta para a própria página; a imagem já está aqui.
        p.imagens.push(ImagemPagina { url, estado: EstadoImagem::Pronta(imagem) });
        p.titulo = titulo;
        p
    }

    fn diagramar(&mut self, t: &mut Tela, largura: i32) {
        let base = self
            .raiz
            .achar("base")
            .and_then(|b| b.atributo("href"))
            .and_then(|h| self.url.juntar(h))
            .unwrap_or_else(|| self.url.clone());
        let raiz = core::mem::replace(&mut self.raiz, Elemento { nome: String::new(), atributos: Vec::new(), filhos: Vec::new() });
        let mut d = Diagramador {
            t,
            base,
            esquerda: MARGEM,
            direita: largura - MARGEM,
            y: 12,
            x: MARGEM,
            linha: Vec::new(),
            marcadores: 0,
            centro: false,
            espaco: 0,
            margem: 0,
            saida: Vec::new(),
            links: Vec::new(),
            imagens: &mut self.imagens,
            ancoras: Vec::new(),
            listas: Vec::new(),
            primeira_celula: true,
        };
        // Uma imagem sem src (a página de imagem) é a própria URL.
        if raiz.achar("img").is_some_and(|i| i.atributo("src") == Some("")) {
            d.base = self.url.clone();
        }
        d.elemento(&raiz, &ESTILO_BASE);
        d.bloco(0);
        self.altura = d.y + d.margem + 24;
        self.desenhos = d.saida;
        self.links = d.links;
        self.ancoras = d.ancoras;
        self.largura = largura;
        self.raiz = raiz;
    }
}

/// Uma página feita aqui mesmo (erros, a página inicial).
fn pagina_interna(url: Url, titulo: &str, corpo: &str) -> Pagina {
    let texto = format!("<html><head><title>{}</title></head><body>{}</body></html>", titulo, corpo);
    Pagina::de_html(url, &texto, String::new())
}

fn escapar(t: &str) -> String {
    t.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")
}

fn pagina_de_erro(url: Url, mensagem: &str) -> Pagina {
    let endereco = escapar(&url.texto());
    let mut corpo = format!(
        "<h1>Não deu para abrir a página</h1><p><b>{}</b></p><p>{}</p>",
        endereco,
        escapar(mensagem)
    );
    if url.esquema == "https" {
        let http = format!("http{}", &url.texto()[5..]);
        corpo += &format!(
            "<p>Este navegador ainda não fala HTTPS (a conexão segura, com TLS). Tente a \
             <a href=\"{0}\">versão sem segurança</a>, se o site tiver uma, ou leia a página pelo \
             <a href=\"http://frogfind.com/read.php?a={1}\">FrogFind</a>, que a busca por nós e a \
             simplifica.</p>",
            escapar(&http),
            escapar(&url.texto())
        );
    }
    corpo += "<p><a href=\"sobre:inicio\">Página inicial</a></p>";
    pagina_interna(url, "Erro", &corpo)
}

const PAGINA_INICIAL: &str = r#"
<h1>Navegador do FerrugemOS</h1>
<p>Um navegador escrito do zero, como o resto do sistema: o TCP, o DNS e o HTTP são nossos, assim como o leitor de HTML, a diagramação e o decodificador de PNG. Ainda não há JavaScript nem folhas de estilo, e as conexões seguras (HTTPS) estão a caminho.</p>
<p>Digite um endereço lá em cima, ou palavras para buscar no <b>FrogFind</b> (um buscador que entrega páginas simples, feito para computadores antigos).</p>
<h2>Para começar</h2>
<ul>
<li><a href="http://info.cern.ch/hypertext/WWW/TheProject.html">O primeiro site da web</a> (CERN, 1991)</li>
<li><a href="http://frogfind.com/">FrogFind</a>: busca na web, com as páginas simplificadas</li>
<li><a href="http://68k.news/">68k.news</a>: notícias do Google Notícias, em HTML simples</li>
<li><a href="http://example.com/">example.com</a></li>
<li><a href="http://textfiles.com/">textfiles.com</a>: arquivos de texto da era das BBSs</li>
<li><a href="http://neverssl.com/">neverssl.com</a></li>
</ul>
<h2>Atalhos</h2>
<p><code>Ctrl+L</code> vai para o endereço, <code>Ctrl+R</code> recarrega, <code>Backspace</code> volta; <code>espaço</code>, <code>PgDn</code>, <code>PgUp</code>, as setas, <code>Home</code> e <code>End</code> rolam a página.</p>
"#;

// -----------------------------------------------------------------------------
// Os downloads (num processo filho, que manda o resultado por um pipe)
// -----------------------------------------------------------------------------

/// O que vem pelo pipe: `p` + 8 bytes (quantos bytes já chegaram), várias
/// vezes; e no fim `r` + u32 (tamanho) + "estado\nurl\ntipo" + o corpo, ou
/// `e` + a mensagem de erro.
struct Busca {
    pid: i32,
    fd: i32,
    bruto: Vec<u8>,
    recebidos: usize,
    url: Url,
}

enum Resultado {
    Resposta { status: u16, url: Url, tipo: String, corpo: Vec<u8> },
    Erro(String),
}

impl Busca {
    fn comecar(url: &Url) -> Option<Busca> {
        let (leitura, escrita) = sys::pipe().ok()?;
        match sys::fork() {
            Ok(0) => {
                let _ = sys::fechar(leitura);
                let mut ultimo = 0;
                let r = http::obter(url, http::abrir_simples, &mut |n| {
                    if n - ultimo >= 16 * 1024 {
                        ultimo = n;
                        let mut m = [b'p'; 9];
                        m[1..].copy_from_slice(&(n as u64).to_le_bytes());
                        let _ = sys::escrever_tudo(escrita, &m);
                    }
                });
                let mut saida = Vec::new();
                match r {
                    Ok(r) => {
                        let meta = format!("{}\n{}\n{}", r.status, r.url.texto(), r.tipo());
                        saida.push(b'r');
                        saida.extend_from_slice(&(meta.len() as u32).to_le_bytes());
                        saida.extend_from_slice(meta.as_bytes());
                        saida.extend_from_slice(&r.corpo);
                    }
                    Err(e) => {
                        saida.push(b'e');
                        saida.extend_from_slice(e.as_bytes());
                    }
                }
                let _ = sys::escrever_tudo(escrita, &saida);
                sys::sair(0);
            }
            Ok(pid) => {
                let _ = sys::fechar(escrita);
                Some(Busca { pid, fd: leitura, bruto: Vec::new(), recebidos: 0, url: url.clone() })
            }
            Err(_) => {
                let _ = sys::fechar(leitura);
                let _ = sys::fechar(escrita);
                None
            }
        }
    }

    /// Lê o que já chegou, sem esperar. Devolve o resultado quando termina.
    fn avancar(&mut self) -> Option<Resultado> {
        let mut buf = vec![0u8; 64 * 1024];
        loop {
            match sys::poll(&[(self.fd, sys::POLLIN)], 0) {
                Ok(r) if r[0] != 0 => {}
                _ => return None,
            }
            match sys::ler(self.fd, &mut buf) {
                Ok(0) | Err(_) => break,
                Ok(n) => self.bruto.extend_from_slice(&buf[..n]),
            }
            // As mensagens de progresso do começo.
            while self.bruto.len() >= 9 && self.bruto[0] == b'p' {
                self.recebidos = u64::from_le_bytes(self.bruto[1..9].try_into().unwrap()) as usize;
                self.bruto.drain(..9);
            }
        }
        // Fim: o filho fechou o pipe.
        while self.bruto.len() >= 9 && self.bruto[0] == b'p' {
            self.bruto.drain(..9);
        }
        Some(match self.bruto.first() {
            Some(b'r') if self.bruto.len() >= 5 => {
                let n = u32::from_le_bytes(self.bruto[1..5].try_into().unwrap()) as usize;
                let meta = String::from_utf8_lossy(&self.bruto[5..5 + n]).into_owned();
                let mut partes = meta.splitn(3, '\n');
                let status = partes.next().and_then(|s| s.parse().ok()).unwrap_or(0);
                let url = partes.next().and_then(Url::ler).unwrap_or_else(|| self.url.clone());
                let tipo = String::from(partes.next().unwrap_or(""));
                Resultado::Resposta { status, url, tipo, corpo: self.bruto.split_off(5 + n) }
            }
            Some(b'e') => Resultado::Erro(String::from_utf8_lossy(&self.bruto[1..]).into_owned()),
            _ => Resultado::Erro(String::from("o download terminou sem resposta")),
        })
    }

    fn cancelar(self) {
        let _ = sys::matar(self.pid, sys::SIGKILL);
    }
}

impl Drop for Busca {
    fn drop(&mut self) {
        let _ = sys::fechar(self.fd);
        let _ = sys::esperar(self.pid);
    }
}

fn tamanho_legivel(n: usize) -> String {
    if n < 1024 {
        format!("{} bytes", n)
    } else if n < 1024 * 1024 {
        format!("{} KiB", n / 1024)
    } else {
        format!("{}.{} MiB", n / (1024 * 1024), n % (1024 * 1024) * 10 / (1024 * 1024))
    }
}

// -----------------------------------------------------------------------------
// O navegador
// -----------------------------------------------------------------------------

struct Navegador {
    pagina: Option<Pagina>,
    /// (endereço, rolagem) de cada página visitada; `atual` é a de agora.
    historico: Vec<(Url, i32)>,
    atual: usize,
    busca: Option<Busca>,
    /// A página que está sendo baixada entra no histórico ao chegar?
    busca_nova: bool,
    busca_imagem: Option<(Busca, usize)>,
    endereco: CampoTexto,
    rolagem: i32,
    visitados: Vec<String>,
    /// Ir para esta âncora depois da diagramação.
    ancora: Option<String>,
    rolagem_pendente: Option<i32>,
}

impl Navegador {
    /// O que foi digitado na barra: um endereço, ou palavras para buscar.
    fn interpretar_endereco(texto: &str) -> Option<Url> {
        let t = texto.trim();
        if t.is_empty() {
            return None;
        }
        if let Some(u) = Url::ler(t) {
            return Some(u);
        }
        if t.starts_with('/') {
            return Url::ler(&format!("file://{}", t));
        }
        if !t.contains(' ') && (t.contains('.') || t.contains(':') || t == "localhost") {
            return Url::ler(&format!("http://{}", t));
        }
        let termos: String = t
            .bytes()
            .map(|b| match b {
                b' ' => String::from("+"),
                b'a'..=b'z' | b'A'..=b'Z' | b'0'..=b'9' | b'-' | b'_' | b'.' => String::from(b as char),
                b => format!("%{:02X}", b),
            })
            .collect();
        Url::ler(&format!("http://frogfind.com/?q={}", termos))
    }

    fn ir(&mut self, url: Url, novo_no_historico: bool) {
        if let Some(b) = self.busca.take() {
            b.cancelar();
        }
        if let Some((b, _)) = self.busca_imagem.take() {
            b.cancelar();
        }
        // Só o fragmento mudou: a mesma página, outro lugar.
        if let Some(p) = &self.pagina {
            let mut sem = url.clone();
            sem.fragmento = p.url.fragmento.clone();
            if sem == p.url && !url.fragmento.is_empty() && novo_no_historico {
                self.lembrar_rolagem();
                self.historico.truncate(self.atual + 1);
                self.historico.push((url.clone(), 0));
                self.atual = self.historico.len() - 1;
                self.pagina.as_mut().unwrap().url = url.clone();
                self.ancora = Some(url.fragmento.clone());
                self.endereco.mudar(&url.texto());
                return;
            }
        }
        self.endereco.mudar(&url.texto());
        match url.esquema.as_str() {
            "sobre" => {
                let p = match url.caminho.as_str() {
                    "inicio" => pagina_interna(url.clone(), "Página inicial", PAGINA_INICIAL),
                    _ => pagina_de_erro(url.clone(), "Essa página interna não existe."),
                };
                self.mostrar(p, novo_no_historico);
            }
            "file" => {
                let caminho = url.caminho.split('?').next().unwrap_or("/").to_string();
                let p = match sys::ler_arquivo(&caminho) {
                    Ok(dados) => {
                        let tipo = if caminho.ends_with(".png") {
                            "image/png"
                        } else if caminho.ends_with(".html") || caminho.ends_with(".htm") {
                            "text/html"
                        } else {
                            "text/plain"
                        };
                        let info = tamanho_legivel(dados.len());
                        self.pagina_da_resposta(url.clone(), 200, tipo, dados, info)
                    }
                    Err(e) => pagina_de_erro(url.clone(), &format!("{}: {}", caminho, e)),
                };
                self.mostrar(p, novo_no_historico);
            }
            _ => {
                self.busca = Busca::comecar(&url);
                self.busca_nova = novo_no_historico;
                if self.busca.is_none() {
                    self.mostrar(pagina_de_erro(url, "não consegui criar o processo do download"), novo_no_historico);
                }
            }
        }
    }

    fn lembrar_rolagem(&mut self) {
        if let Some(h) = self.historico.get_mut(self.atual) {
            h.1 = self.rolagem;
        }
    }

    fn mostrar(&mut self, p: Pagina, novo_no_historico: bool) {
        if novo_no_historico {
            self.lembrar_rolagem();
            self.historico.truncate(if self.historico.is_empty() { 0 } else { self.atual + 1 });
            self.historico.push((p.url.clone(), 0));
            self.atual = self.historico.len() - 1;
            self.rolagem_pendente = Some(0);
        } else {
            // voltar/avançar/recarregar: a rolagem de quando saímos
            if let Some(h) = self.historico.get_mut(self.atual) {
                h.0 = p.url.clone();
            }
            self.rolagem_pendente = Some(self.historico.get(self.atual).map_or(0, |h| h.1));
        }
        let texto = p.url.texto();
        if !self.visitados.contains(&texto) {
            self.visitados.push(texto.clone());
        }
        self.ancora = (!p.url.fragmento.is_empty()).then(|| p.url.fragmento.clone());
        self.endereco.mudar(&texto);
        self.pagina = Some(p);
    }

    fn pagina_da_resposta(&self, url: Url, status: u16, tipo: &str, corpo: Vec<u8>, info: String) -> Pagina {
        let info = if status == 200 { info } else { format!("HTTP {} · {}", status, info) };
        let parece_html = corpo.iter().take(512).skip_while(|b| b.is_ascii_whitespace()).next() == Some(&b'<');
        if tipo == "image/png" || corpo.starts_with(b"\x89PNG") {
            return match png::decodificar(&corpo) {
                Some(img) => Pagina::de_imagem(url, img, info),
                None => pagina_de_erro(url, "a imagem PNG está estragada"),
            };
        }
        if tipo == "text/html" || tipo == "application/xhtml+xml" || tipo.is_empty() && parece_html {
            return Pagina::de_html(url, &html::decodificar_bytes(&corpo), info);
        }
        if tipo.starts_with("text/") || tipo.ends_with("json") || tipo.ends_with("xml") || tipo.is_empty() {
            return Pagina::de_texto(url, &html::decodificar_bytes(&corpo), info);
        }
        let nome = url.caminho.split('?').next().unwrap_or("").rsplit('/').next().unwrap_or("").to_string();
        let nome = if nome.is_empty() { String::from("download") } else { nome };
        let destino = format!("{}/{}", variavel("HOME").unwrap_or("/tmp"), nome);
        let gravou = sys::escrever_arquivo(&destino, &corpo);
        let mensagem = match gravou {
            Ok(()) => format!(
                "<h1>Arquivo baixado</h1><p>O tipo <code>{}</code> não é mostrado aqui; os {} foram gravados em <code>{}</code>.</p>",
                escapar(tipo),
                tamanho_legivel(corpo.len()),
                escapar(&destino)
            ),
            Err(e) => format!("<h1>Não deu para gravar</h1><p>{}: {}</p>", escapar(&destino), e),
        };
        let mut p = pagina_interna(url, &nome, &mensagem);
        p.info = info;
        p
    }

    /// Chamado a cada desenho: acompanha os downloads.
    fn acompanhar(&mut self, t: &mut Tela) {
        if let Some(b) = self.busca.as_mut() {
            t.acordar_em(30);
            if let Some(r) = b.avancar() {
                let b = self.busca.take().unwrap();
                let pagina = match r {
                    Resultado::Resposta { status, url, tipo, corpo } => {
                        let info = tamanho_legivel(corpo.len());
                        self.pagina_da_resposta(url, status, &tipo, corpo, info)
                    }
                    Resultado::Erro(e) => pagina_de_erro(b.url.clone(), &e),
                };
                drop(b);
                let novo = self.busca_nova;
                self.mostrar(pagina, novo);
            }
        }
        // As imagens, uma de cada vez.
        if let Some((b, i)) = self.busca_imagem.as_mut() {
            t.acordar_em(30);
            if let Some(r) = b.avancar() {
                let i = *i;
                self.busca_imagem = None;
                if let Some(p) = self.pagina.as_mut() {
                    p.imagens[i].estado = match r {
                        Resultado::Resposta { status: 200, corpo, .. } => match png::decodificar(&corpo) {
                            Some(img) if img.largura as u64 * img.altura as u64 <= 16 << 20 => EstadoImagem::Pronta(img),
                            _ => EstadoImagem::Falhou,
                        },
                        _ => EstadoImagem::Falhou,
                    };
                    p.largura = 0; // diagramar de novo, com o tamanho da imagem
                }
            }
        }
        if self.busca_imagem.is_none() && self.busca.is_none() {
            if let Some(p) = self.pagina.as_mut() {
                if let Some(i) = p.imagens.iter().position(|im| matches!(im.estado, EstadoImagem::Esperando)) {
                    let url = p.imagens[i].url.clone();
                    match url.esquema.as_str() {
                        "http" => match Busca::comecar(&url) {
                            Some(b) => self.busca_imagem = Some((b, i)),
                            None => p.imagens[i].estado = EstadoImagem::Falhou,
                        },
                        "file" => {
                            p.imagens[i].estado = match sys::ler_arquivo(&url.caminho).ok().and_then(|d| png::decodificar(&d)) {
                                Some(img) => EstadoImagem::Pronta(img),
                                None => EstadoImagem::Falhou,
                            };
                            p.largura = 0;
                        }
                        _ => p.imagens[i].estado = EstadoImagem::Falhou,
                    }
                    t.redesenhar();
                }
            }
        }
    }

    fn voltar(&mut self) {
        if self.atual > 0 {
            self.lembrar_rolagem();
            self.atual -= 1;
            let url = self.historico[self.atual].0.clone();
            self.ir(url, false);
        }
    }

    fn avancar(&mut self) {
        if self.atual + 1 < self.historico.len() {
            self.lembrar_rolagem();
            self.atual += 1;
            let url = self.historico[self.atual].0.clone();
            self.ir(url, false);
        }
    }

    fn recarregar(&mut self) {
        if let Some(url) = self.pagina.as_ref().map(|p| p.url.clone()) {
            self.lembrar_rolagem();
            self.ir(url, false);
        }
    }

    fn desenhar(&mut self, t: &mut Tela) {
        self.acompanhar(t);
        let tema = t.tema();
        let largura_janela = t.q.largura;
        // A barra de ferramentas.
        let mut ir_para = None;
        t.linha(|t| {
            if t.botao_compacto("←") {
                self.voltar();
            }
            if t.botao_compacto("→") {
                self.avancar();
            }
            if self.busca.is_some() {
                if t.botao_compacto("✕") {
                    if let Some(b) = self.busca.take() {
                        b.cancelar();
                    }
                }
            } else if t.botao_compacto("↻") {
                self.recarregar();
            }
            if t.botao_compacto("⌂") {
                ir_para = Url::ler(INICIO);
            }
            let (x, _) = t.posicao();
            let largura = largura_janela - x - 12;
            if t.campo(&mut self.endereco, largura) {
                ir_para = Navegador::interpretar_endereco(&self.endereco.texto);
                t.soltar_foco();
            }
        });
        if let Some(url) = ir_para {
            self.ir(url, true);
        }
        if t.atalho('l') {
            // um endereço novo: a barra começa vazia
            self.endereco.mudar("");
            self.endereco.pedir_foco = true;
            t.redesenhar();
        }
        if t.atalho('r') {
            self.recarregar();
        }
        // A página.
        let (_, topo) = t.posicao();
        let altura_area = t.q.altura - topo - BARRA_ESTADO;
        t.em(0, topo);
        let area = t.area(largura_janela, altura_area);
        if let Some(p) = self.pagina.as_mut() {
            if p.largura != largura_janela {
                p.diagramar(t, largura_janela);
                let titulo = if p.titulo.is_empty() { p.url.texto() } else { p.titulo.clone() };
                t.mudar_titulo(&format!("{} — Navegador", titulo));
            }
        }
        let altura_pagina = self.pagina.as_ref().map_or(0, |p| p.altura);
        if let Some(r) = self.rolagem_pendente.take() {
            self.rolagem = r;
        }
        if let (Some(a), Some(p)) = (self.ancora.take(), self.pagina.as_ref()) {
            if let Some((_, y)) = p.ancoras.iter().find(|(id, _)| *id == a) {
                self.rolagem = *y - 8;
            }
        }
        // Rolar: roda, teclado.
        let pagina_inteira = altura_area - 40;
        self.rolagem += area.roda * 48;
        for (tecla, passo) in [
            (Tecla::Seta(Seta::Baixo), 40),
            (Tecla::Seta(Seta::Cima), -40),
            (Tecla::Seta(Seta::PaginaAbaixo), pagina_inteira),
            (Tecla::Seta(Seta::PaginaAcima), -pagina_inteira),
            (Tecla::Letra(' '), pagina_inteira),
            (Tecla::Seta(Seta::Inicio), -1 << 24),
            (Tecla::Seta(Seta::Fim), 1 << 24),
        ] {
            while t.tecla_da_area(&area, tecla.clone()) {
                self.rolagem += passo;
            }
        }
        if t.tecla_da_area(&area, Tecla::Apagar) {
            self.voltar();
        }
        self.rolagem = self.rolagem.clamp(0, (altura_pagina - altura_area).max(0));
        // Desenha a parte visível.
        let antes = t.q.recorte;
        t.q.recortar(area.x, area.y, area.largura, area.altura);
        t.q.retangulo(area.x, area.y, area.largura, area.altura, COR_PAGINA);
        let mouse = area.mouse.map(|(x, y)| (x, y + self.rolagem));
        let mut link_sobre = None;
        if let Some(p) = self.pagina.as_ref() {
            if let Some((mx, my)) = mouse {
                link_sobre = p.desenhos.iter().find_map(|d| {
                    let (x, y, l, a) = d.caixa();
                    (mx >= x && my >= y && mx < x + l && my < y + a).then(|| d.link()).flatten()
                });
            }
            let (y0, y1) = (self.rolagem, self.rolagem + altura_area);
            for d in &p.desenhos {
                let (x, y, l, a) = d.caixa();
                if y + a < y0 || y > y1 {
                    continue;
                }
                let (sx, sy) = (area.x + x, area.y + y - self.rolagem);
                match d {
                    Desenho::Texto { texto, estilo, .. } => {
                        let mut cor = estilo.cor;
                        if let Some(i) = estilo.link {
                            cor = if Some(i) == link_sobre {
                                COR_LINK_SOBRE
                            } else if p.links.get(i).is_some_and(|u| self.visitados.contains(&u.texto())) {
                                COR_VISITADO
                            } else {
                                COR_LINK
                            };
                        }
                        if estilo.mono {
                            t.escrever_mono(sx, sy, texto, cor, estilo.tamanho);
                        } else if estilo.italico {
                            t.escrever_italico(sx, sy, texto, cor, estilo.negrito, estilo.tamanho);
                        } else {
                            t.escrever(sx, sy, texto, cor, estilo.negrito, estilo.tamanho);
                        }
                        if estilo.sublinhado {
                            t.q.retangulo(sx, sy + a - 2, l, 1, cor);
                        }
                        if estilo.riscado {
                            t.q.retangulo(sx, sy + a / 2, l, 1, cor);
                        }
                    }
                    Desenho::Retangulo { cor, .. } => t.q.retangulo(sx, sy, l, a, *cor),
                    Desenho::Controle { texto, botao, .. } => {
                        t.q.retangulo_arredondado(sx, sy, l, a, 0xA0A0A0);
                        t.q.retangulo_arredondado(sx + 1, sy + 1, l - 2, a - 2, if *botao { 0xEDEDED } else { 0xFFFFFF });
                        let antes = t.q.recorte;
                        t.q.recortar(sx + 2, sy, l - 4, a);
                        let cor = if *botao { COR_TEXTO } else { COR_FRACA };
                        let tx = if *botao { sx + (l - t.medir(texto, 13, false, false)) / 2 } else { sx + 6 };
                        t.escrever(tx, sy + 4, texto, cor, false, 13);
                        t.q.recorte = antes;
                    }
                    Desenho::Imagem { indice, link, .. } => match &p.imagens[*indice].estado {
                        EstadoImagem::Pronta(img) => {
                            t.q.desenhar_imagem(&img.pixels, img.largura as i32, img.altura as i32, sx, sy, l, a);
                            if link.is_some() && *link == link_sobre {
                                t.q.moldura(sx, sy, l, a, COR_LINK);
                            }
                        }
                        _ => {
                            t.q.retangulo(sx, sy, l, a, 0xEEEEEE);
                            t.q.moldura(sx, sy, l, a, 0xD0D0D0);
                        }
                    },
                }
            }
            // A barra de rolagem.
            if altura_pagina > altura_area {
                let alca = (altura_area * altura_area / altura_pagina).max(24);
                let y = area.y + (altura_area - alca) * self.rolagem / (altura_pagina - altura_area).max(1);
                t.q.retangulo_redondo(area.x + area.largura - 8, y + 2, 5, alca - 4, 2, 0xB0B0B0);
            }
        }
        t.q.recorte = antes;
        // O clique num link.
        if area.clique {
            if let (Some(i), Some(p)) = (link_sobre, self.pagina.as_ref()) {
                let url = p.links[i].clone();
                self.ir(url, true);
                t.redesenhar();
            }
        }
        // A barra de estado.
        let y = t.q.altura - BARRA_ESTADO;
        t.q.retangulo(0, y, largura_janela, BARRA_ESTADO, tema.painel);
        t.q.retangulo(0, y, largura_janela, 1, tema.borda);
        let texto = if let (Some(i), Some(p)) = (link_sobre, self.pagina.as_ref()) {
            p.links[i].texto()
        } else if let Some(b) = &self.busca {
            format!("Carregando {}… {}", b.url.host, if b.recebidos > 0 { tamanho_legivel(b.recebidos) } else { String::new() })
        } else if let Some((b, _)) = &self.busca_imagem {
            format!("Carregando imagem {}…", b.url.caminho.rsplit('/').next().unwrap_or(""))
        } else {
            self.pagina.as_ref().map_or(String::new(), |p| p.info.clone())
        };
        t.escrever(10, y + 4, &texto, tema.texto_fraco, false, 12);
    }
}

fn principal(args: &[&str]) -> i32 {
    let mut janela = match Janela::abrir(900, 620, "Navegador") {
        Ok(j) => j,
        Err(e) => {
            eprintln!("navegador: {}", e);
            return 1;
        }
    };
    let mut ui = Ui::novo();
    let mut nav = Navegador {
        pagina: None,
        historico: Vec::new(),
        atual: 0,
        busca: None,
        busca_nova: true,
        busca_imagem: None,
        endereco: CampoTexto::novo(""),
        rolagem: 0,
        visitados: Vec::new(),
        ancora: None,
        rolagem_pendente: None,
    };
    let primeira = args.get(1).and_then(|a| Navegador::interpretar_endereco(a)).or_else(|| Url::ler(INICIO)).unwrap();
    nav.ir(primeira, true);
    ui::laco(&mut janela, &mut ui, |t| nav.desenhar(t));
    if let Some(b) = nav.busca.take() {
        b.cancelar();
    }
    if let Some((b, _)) = nav.busca_imagem.take() {
        b.cancelar();
    }
    0
}
