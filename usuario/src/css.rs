//! CSS: o leitor de folhas de estilo, os seletores, a cascata e o estilo
//! calculado de cada elemento.
//!
//! ```text
//!   texto ─▶ regras: seletor { propriedade: valor; ... }
//!   cada declaração vira um valor já interpretado (`Decl`): refazer o estilo
//!   da página inteira não pode reinterpretar texto (o JavaScript muda o DOM
//!   o tempo todo). Só as que usam var() esperam a hora de aplicar.
//!   para cada elemento: as regras cujo seletor casa, em ordem de cascata
//!   (origem, !important, especificidade, ordem) ─▶ `Estilo`
//! ```
//!
//! Quem desenha é o `caixas`: aqui só se decide o que vale para cada elemento.

use crate::dom::{Dom, NADA, NoId};
use crate::mat;
use alloc::collections::{BTreeMap, BTreeSet};
use alloc::format;
use alloc::rc::Rc;
use alloc::string::{String, ToString};
use alloc::vec;
use alloc::vec::Vec;

// -----------------------------------------------------------------------------
// Os valores
// -----------------------------------------------------------------------------

/// Um tamanho já calculado (só a porcentagem espera a diagramação).
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Tam {
    Auto,
    Px(f32),
    Pct(f32),
    /// calc(10px + 50%)
    PxPct(f32, f32),
    MinConteudo,
    MaxConteudo,
    AjustaConteudo,
}

impl Tam {
    /// Em pixels, dada a base das porcentagens. Auto = None.
    pub fn resolver(self, base: Option<f32>) -> Option<f32> {
        match self {
            Tam::Px(p) => Some(p),
            Tam::Pct(p) => base.map(|b| b * p / 100.0),
            Tam::PxPct(a, p) => base.map(|b| a + b * p / 100.0),
            _ => None,
        }
    }
    pub fn e_auto(self) -> bool {
        matches!(self, Tam::Auto)
    }
}

/// Um tamanho como foi escrito (a unidade é resolvida ao aplicar).
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum T {
    Auto,
    Px(f32),
    Em(f32),
    Rem(f32),
    Pct(f32),
    Vw(f32),
    Vh(f32),
    /// Soma de termos: px, em, rem, vw, vh, %.
    Soma([f32; 6]),
    Min,
    Max,
    Ajusta,
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Display {
    Nenhum,
    Bloco,
    Inline,
    InlineBloco,
    Flex,
    InlineFlex,
    Grid,
    InlineGrid,
    Tabela,
    InlineTabela,
    GrupoLinhas,
    Linha,
    Celula,
    Legenda,
    GrupoColunas,
    Coluna,
    ItemLista,
    /// `display: contents`: a caixa some, os filhos ficam.
    Conteudo,
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Posicao {
    Estatica,
    Relativa,
    Absoluta,
    Fixa,
    Grudada,
}

/// As trilhas do grid.
#[derive(Clone, Debug, PartialEq)]
pub enum Trilha {
    T(T),
    Fr(f32),
    MinMax(T, Box<Trilha>),
    Repete(u32, Vec<Trilha>),
    /// repeat(auto-fill, ...), repeat(auto-fit, ...)
    RepeteAuto(Vec<Trilha>),
}

use alloc::boxed::Box;

#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub struct LinhaGrid {
    /// 0 = automático.
    pub linha: i32,
    pub span: i32,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Sombra {
    pub x: f32,
    pub y: f32,
    pub borrao: f32,
    pub expande: f32,
    pub cor: u32,
    pub dentro: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Imagem {
    Url(String),
    /// (ângulo em graus, paradas: posição 0-1, cor)
    Gradiente(f32, Vec<(f32, u32)>),
}

/// O estilo de um elemento.
#[derive(Clone, Debug)]
pub struct Estilo {
    pub display: Display,
    pub posicao: Posicao,
    pub flutua: u8, // 0 nenhum, 1 esquerda, 2 direita
    pub limpa: u8,  // 1 esquerda, 2 direita, 3 ambos
    pub visivel: bool,
    pub recorta: bool,
    pub overflow_rola: bool,
    pub caixa_borda: bool,
    pub largura: Tam,
    pub altura: Tam,
    pub min_largura: Tam,
    pub max_largura: Tam,
    pub min_altura: Tam,
    pub max_altura: Tam,
    /// cima, direita, baixo, esquerda
    pub margem: [Tam; 4],
    pub preench: [Tam; 4],
    pub borda: [f32; 4],
    pub borda_cor: [u32; 4],
    pub borda_estilo: [u8; 4], // 0 none, 1 solid, 2 dashed, 3 dotted, 4 double
    pub raio: [f32; 4],
    pub topo: Tam,
    pub direita: Tam,
    pub baixo: Tam,
    pub esquerda: Tam,
    pub z: Option<i32>,
    pub cor: u32,
    pub fundo: u32,
    pub fundo_img: Option<Rc<Imagem>>,
    pub fundo_repete: u8, // 0 repeat, 1 repeat-x, 2 repeat-y, 3 no-repeat
    pub fundo_tam: (u8, Tam, Tam), // 0 explícito, 1 cover, 2 contain
    pub fundo_pos: (Tam, Tam),
    pub fonte: f32,
    pub negrito: bool,
    pub italico: bool,
    pub mono: bool,
    /// 0 = normal; > 0 número (múltiplo da fonte); < 0 pixels (negativo).
    pub altura_linha: f32,
    pub espaco_letras: f32,
    pub espaco_palavras: f32,
    pub alinha: u8, // 0 esquerda, 1 centro, 2 direita, 3 justificado
    pub recuo: Tam,
    pub transforma: u8, // 1 maiúsculas, 2 minúsculas, 3 iniciais
    pub branco: u8,     // 0 normal, 1 nowrap, 2 pre, 3 pre-wrap, 4 pre-line
    pub sublinhado: bool,
    pub riscado: bool,
    pub sobrelinha: bool,
    pub cor_decoracao: Option<u32>,
    pub alinha_v: u8, // 0 baseline, 1 middle, 2 top, 3 bottom, 4 sub, 5 super, 6 text-top, 7 text-bottom
    pub desloca_v: f32,
    pub quebra_palavra: bool,
    pub reticencias: bool,
    pub lista_tipo: u8, // 0 disc, 1 circle, 2 square, 3 decimal, 4 none, 5 lower-alpha, 6 upper-alpha, 7 lower-roman, 8 upper-roman
    pub lista_dentro: bool,
    pub flex_direcao: u8, // 0 row, 1 row-reverse, 2 column, 3 column-reverse
    pub flex_quebra: u8,  // 0 nowrap, 1 wrap, 2 wrap-reverse
    pub justifica: u8,    // 0 início, 1 fim, 2 centro, 3 space-between, 4 space-around, 5 space-evenly, 6 stretch
    pub alinha_itens: u8, // 0 início, 1 fim, 2 centro, 3 baseline, 4 stretch
    pub alinha_proprio: u8, // o mesmo, 5 = auto
    pub alinha_conteudo: u8, // como justifica; 6 = stretch
    pub justifica_itens: u8,
    pub justifica_proprio: u8,
    pub cresce: f32,
    pub encolhe: f32,
    pub base_flex: Tam,
    pub ordem: i32,
    pub vao_linha: Tam,
    pub vao_coluna: Tam,
    pub grid_colunas: Rc<Vec<Trilha>>,
    pub grid_linhas: Rc<Vec<Trilha>>,
    pub grid_auto_colunas: Rc<Vec<Trilha>>,
    pub grid_auto_linhas: Rc<Vec<Trilha>>,
    pub grid_fluxo_coluna: bool,
    pub grid_areas: Rc<Vec<Vec<String>>>,
    pub grid_coluna: (LinhaGrid, LinhaGrid),
    pub grid_linha: (LinhaGrid, LinhaGrid),
    pub grid_area: Option<Rc<str>>,
    pub opacidade: f32,
    pub sombras: Option<Rc<Vec<Sombra>>>,
    pub desloca_x: Tam,
    pub desloca_y: Tam,
    pub escala: f32,
    pub cursor_mao: bool,
    pub sem_eventos: bool,
    pub conteudo: Option<Rc<str>>,
    pub colapsa_bordas: bool,
    pub espaco_bordas: f32,
    pub tabela_fixa: bool,
    pub ajuste_objeto: u8, // 0 fill, 1 contain, 2 cover, 3 none
    pub proporcao: f32,
    /// Tem `mask-image`: o fundo é um ícone que não sabemos recortar.
    pub mascara: bool,
    pub vars: Option<Rc<BTreeMap<Rc<str>, Rc<str>>>>,
}

impl Estilo {
    pub fn inicial() -> Estilo {
        let vazio: Rc<Vec<Trilha>> = Rc::new(Vec::new());
        Estilo {
            display: Display::Inline,
            posicao: Posicao::Estatica,
            flutua: 0,
            limpa: 0,
            visivel: true,
            recorta: false,
            overflow_rola: false,
            caixa_borda: false,
            largura: Tam::Auto,
            altura: Tam::Auto,
            min_largura: Tam::Auto,
            max_largura: Tam::Auto,
            min_altura: Tam::Auto,
            max_altura: Tam::Auto,
            margem: [Tam::Px(0.0); 4],
            preench: [Tam::Px(0.0); 4],
            borda: [0.0; 4],
            borda_cor: [COR_ATUAL; 4],
            borda_estilo: [0; 4],
            raio: [0.0; 4],
            topo: Tam::Auto,
            direita: Tam::Auto,
            baixo: Tam::Auto,
            esquerda: Tam::Auto,
            z: None,
            cor: 0xFF000000,
            fundo: 0,
            fundo_img: None,
            fundo_repete: 0,
            fundo_tam: (0, Tam::Auto, Tam::Auto),
            fundo_pos: (Tam::Px(0.0), Tam::Px(0.0)),
            fonte: 16.0,
            negrito: false,
            italico: false,
            mono: false,
            altura_linha: 0.0,
            espaco_letras: 0.0,
            espaco_palavras: 0.0,
            alinha: 0,
            recuo: Tam::Px(0.0),
            transforma: 0,
            branco: 0,
            sublinhado: false,
            riscado: false,
            sobrelinha: false,
            cor_decoracao: None,
            alinha_v: 0,
            desloca_v: 0.0,
            quebra_palavra: false,
            reticencias: false,
            lista_tipo: 0,
            lista_dentro: false,
            flex_direcao: 0,
            flex_quebra: 0,
            justifica: 0,
            alinha_itens: 4,
            alinha_proprio: 5,
            alinha_conteudo: 6,
            justifica_itens: 4,
            justifica_proprio: 5,
            cresce: 0.0,
            encolhe: 1.0,
            base_flex: Tam::Auto,
            ordem: 0,
            vao_linha: Tam::Px(0.0),
            vao_coluna: Tam::Px(0.0),
            grid_colunas: vazio.clone(),
            grid_linhas: vazio.clone(),
            grid_auto_colunas: vazio.clone(),
            grid_auto_linhas: vazio,
            grid_fluxo_coluna: false,
            grid_areas: Rc::new(Vec::new()),
            grid_coluna: Default::default(),
            grid_linha: Default::default(),
            grid_area: None,
            opacidade: 1.0,
            sombras: None,
            desloca_x: Tam::Px(0.0),
            desloca_y: Tam::Px(0.0),
            escala: 1.0,
            cursor_mao: false,
            sem_eventos: false,
            conteudo: None,
            colapsa_bordas: false,
            espaco_bordas: 0.0,
            tabela_fixa: false,
            ajuste_objeto: 0,
            proporcao: 0.0,
            mascara: false,
            vars: None,
        }
    }

    /// O estilo de partida de um filho: só o que herda vem do pai.
    pub fn herdado(&self) -> Estilo {
        let mut e = Estilo::inicial();
        e.visivel = self.visivel;
        e.cor = self.cor;
        e.fonte = self.fonte;
        e.negrito = self.negrito;
        e.italico = self.italico;
        e.mono = self.mono;
        e.altura_linha = self.altura_linha;
        e.espaco_letras = self.espaco_letras;
        e.espaco_palavras = self.espaco_palavras;
        e.alinha = self.alinha;
        e.recuo = self.recuo;
        e.transforma = self.transforma;
        e.branco = self.branco;
        e.sublinhado = self.sublinhado;
        e.riscado = self.riscado;
        e.sobrelinha = self.sobrelinha;
        e.cor_decoracao = self.cor_decoracao;
        e.quebra_palavra = self.quebra_palavra;
        e.lista_tipo = self.lista_tipo;
        e.lista_dentro = self.lista_dentro;
        e.cursor_mao = self.cursor_mao;
        e.sem_eventos = self.sem_eventos;
        e.colapsa_bordas = self.colapsa_bordas;
        e.espaco_bordas = self.espaco_bordas;
        e.vars = self.vars.clone();
        e
    }

    pub fn e_inline(&self) -> bool {
        matches!(self.display, Display::Inline | Display::InlineBloco | Display::InlineFlex | Display::InlineGrid | Display::InlineTabela)
    }

    pub fn fora_do_fluxo(&self) -> bool {
        matches!(self.posicao, Posicao::Absoluta | Posicao::Fixa)
    }
}

/// Um marcador: a cor é a do texto (`currentcolor`), resolvida ao aplicar.
pub const COR_ATUAL: u32 = 0x01000000;

// -----------------------------------------------------------------------------
// Cores e tamanhos
// -----------------------------------------------------------------------------

const CORES: [(&str, u32); 148] = [
    ("aliceblue", 0xF0F8FF), ("antiquewhite", 0xFAEBD7), ("aqua", 0x00FFFF), ("aquamarine", 0x7FFFD4), ("azure", 0xF0FFFF),
    ("beige", 0xF5F5DC), ("bisque", 0xFFE4C4), ("black", 0x000000), ("blanchedalmond", 0xFFEBCD), ("blue", 0x0000FF),
    ("blueviolet", 0x8A2BE2), ("brown", 0xA52A2A), ("burlywood", 0xDEB887), ("cadetblue", 0x5F9EA0), ("chartreuse", 0x7FFF00),
    ("chocolate", 0xD2691E), ("coral", 0xFF7F50), ("cornflowerblue", 0x6495ED), ("cornsilk", 0xFFF8DC), ("crimson", 0xDC143C),
    ("cyan", 0x00FFFF), ("darkblue", 0x00008B), ("darkcyan", 0x008B8B), ("darkgoldenrod", 0xB8860B), ("darkgray", 0xA9A9A9),
    ("darkgreen", 0x006400), ("darkgrey", 0xA9A9A9), ("darkkhaki", 0xBDB76B), ("darkmagenta", 0x8B008B), ("darkolivegreen", 0x556B2F),
    ("darkorange", 0xFF8C00), ("darkorchid", 0x9932CC), ("darkred", 0x8B0000), ("darksalmon", 0xE9967A), ("darkseagreen", 0x8FBC8F),
    ("darkslateblue", 0x483D8B), ("darkslategray", 0x2F4F4F), ("darkslategrey", 0x2F4F4F), ("darkturquoise", 0x00CED1), ("darkviolet", 0x9400D3),
    ("deeppink", 0xFF1493), ("deepskyblue", 0x00BFFF), ("dimgray", 0x696969), ("dimgrey", 0x696969), ("dodgerblue", 0x1E90FF),
    ("firebrick", 0xB22222), ("floralwhite", 0xFFFAF0), ("forestgreen", 0x228B22), ("fuchsia", 0xFF00FF), ("gainsboro", 0xDCDCDC),
    ("ghostwhite", 0xF8F8FF), ("gold", 0xFFD700), ("goldenrod", 0xDAA520), ("gray", 0x808080), ("green", 0x008000),
    ("greenyellow", 0xADFF2F), ("grey", 0x808080), ("honeydew", 0xF0FFF0), ("hotpink", 0xFF69B4), ("indianred", 0xCD5C5C),
    ("indigo", 0x4B0082), ("ivory", 0xFFFFF0), ("khaki", 0xF0E68C), ("lavender", 0xE6E6FA), ("lavenderblush", 0xFFF0F5),
    ("lawngreen", 0x7CFC00), ("lemonchiffon", 0xFFFACD), ("lightblue", 0xADD8E6), ("lightcoral", 0xF08080), ("lightcyan", 0xE0FFFF),
    ("lightgoldenrodyellow", 0xFAFAD2), ("lightgray", 0xD3D3D3), ("lightgreen", 0x90EE90), ("lightgrey", 0xD3D3D3), ("lightpink", 0xFFB6C1),
    ("lightsalmon", 0xFFA07A), ("lightseagreen", 0x20B2AA), ("lightskyblue", 0x87CEFA), ("lightslategray", 0x778899), ("lightslategrey", 0x778899),
    ("lightsteelblue", 0xB0C4DE), ("lightyellow", 0xFFFFE0), ("lime", 0x00FF00), ("limegreen", 0x32CD32), ("linen", 0xFAF0E6),
    ("magenta", 0xFF00FF), ("maroon", 0x800000), ("mediumaquamarine", 0x66CDAA), ("mediumblue", 0x0000CD), ("mediumorchid", 0xBA55D3),
    ("mediumpurple", 0x9370DB), ("mediumseagreen", 0x3CB371), ("mediumslateblue", 0x7B68EE), ("mediumspringgreen", 0x00FA9A), ("mediumturquoise", 0x48D1CC),
    ("mediumvioletred", 0xC71585), ("midnightblue", 0x191970), ("mintcream", 0xF5FFFA), ("mistyrose", 0xFFE4E1), ("moccasin", 0xFFE4B5),
    ("navajowhite", 0xFFDEAD), ("navy", 0x000080), ("oldlace", 0xFDF5E6), ("olive", 0x808000), ("olivedrab", 0x6B8E23),
    ("orange", 0xFFA500), ("orangered", 0xFF4500), ("orchid", 0xDA70D6), ("palegoldenrod", 0xEEE8AA), ("palegreen", 0x98FB98),
    ("paleturquoise", 0xAFEEEE), ("palevioletred", 0xDB7093), ("papayawhip", 0xFFEFD5), ("peachpuff", 0xFFDAB9), ("peru", 0xCD853F),
    ("pink", 0xFFC0CB), ("plum", 0xDDA0DD), ("powderblue", 0xB0E0E6), ("purple", 0x800080), ("rebeccapurple", 0x663399),
    ("red", 0xFF0000), ("rosybrown", 0xBC8F8F), ("royalblue", 0x4169E1), ("saddlebrown", 0x8B4513), ("salmon", 0xFA8072),
    ("sandybrown", 0xF4A460), ("seagreen", 0x2E8B57), ("seashell", 0xFFF5EE), ("sienna", 0xA0522D), ("silver", 0xC0C0C0),
    ("skyblue", 0x87CEEB), ("slateblue", 0x6A5ACD), ("slategray", 0x708090), ("slategrey", 0x708090), ("snow", 0xFFFAFA),
    ("springgreen", 0x00FF7F), ("steelblue", 0x4682B4), ("tan", 0xD2B48C), ("teal", 0x008080), ("thistle", 0xD8BFD8),
    ("tomato", 0xFF6347), ("turquoise", 0x40E0D0), ("violet", 0xEE82EE), ("wheat", 0xF5DEB3), ("white", 0xFFFFFF),
    ("whitesmoke", 0xF5F5F5), ("yellow", 0xFFFF00), ("yellowgreen", 0x9ACD32),
];

fn com_alfa(rgb: u32, a: f32) -> u32 {
    ((a.clamp(0.0, 1.0) * 255.0 + 0.5) as u32) << 24 | (rgb & 0xFFFFFF)
}

/// Os argumentos de uma função: separados por vírgula, espaço ou `/`.
fn argumentos(t: &str) -> Vec<&str> {
    t.split([',', ' ', '/']).map(str::trim).filter(|p| !p.is_empty()).collect()
}

fn canal(p: &str) -> Option<f32> {
    if let Some(x) = p.strip_suffix('%') {
        return Some(x.parse::<f32>().ok()? * 2.55);
    }
    p.parse::<f32>().ok()
}

fn alfa_de(p: Option<&&str>) -> f32 {
    match p {
        None => 1.0,
        Some(x) => match x.strip_suffix('%') {
            Some(q) => q.parse::<f32>().unwrap_or(100.0) / 100.0,
            None => x.parse().unwrap_or(1.0),
        },
    }
}

fn hsl(h: f32, s: f32, l: f32) -> u32 {
    let h = ((h % 360.0) + 360.0) % 360.0;
    let c = (1.0 - (2.0 * l - 1.0).abs()) * s;
    let x = c * (1.0 - ((h / 60.0) % 2.0 - 1.0).abs());
    let m = l - c / 2.0;
    let (r, g, b) = match (h / 60.0) as u32 {
        0 => (c, x, 0.0),
        1 => (x, c, 0.0),
        2 => (0.0, c, x),
        3 => (0.0, x, c),
        4 => (x, 0.0, c),
        _ => (c, 0.0, x),
    };
    let v = |q: f32| ((q + m) * 255.0 + 0.5) as u32;
    v(r) << 16 | v(g) << 8 | v(b)
}

/// Uma cor: 0xAARRGGBB (0 = transparente).
pub fn ler_cor(texto: &str) -> Option<u32> {
    let t = texto.trim();
    if let Some(h) = t.strip_prefix('#') {
        let v = u32::from_str_radix(h, 16).ok()?;
        return Some(match h.len() {
            3 => 0xFF000000 | ((v >> 8 & 15) * 17) << 16 | ((v >> 4 & 15) * 17) << 8 | (v & 15) * 17,
            4 => (v & 15) * 17 << 24 | ((v >> 12 & 15) * 17) << 16 | ((v >> 8 & 15) * 17) << 8 | (v >> 4 & 15) * 17,
            6 => 0xFF000000 | v,
            8 => (v & 0xFF) << 24 | v >> 8,
            _ => return None,
        });
    }
    let baixo = t.to_ascii_lowercase();
    if let Some(a) = baixo.find('(') {
        let nome = &baixo[..a];
        let dentro = baixo[a + 1..].trim_end().strip_suffix(')')?;
        return match nome {
            "rgb" | "rgba" => {
                let p = argumentos(dentro);
                if p.len() < 3 {
                    return None;
                }
                let (r, g, b) = (canal(p[0])?, canal(p[1])?, canal(p[2])?);
                let v = |x: f32| (x.clamp(0.0, 255.0) + 0.5) as u32;
                Some(com_alfa(v(r) << 16 | v(g) << 8 | v(b), alfa_de(p.get(3))))
            }
            "hsl" | "hsla" => {
                let p = argumentos(dentro);
                if p.len() < 3 {
                    return None;
                }
                let h = p[0].trim_end_matches("deg").parse::<f32>().ok()?;
                let s = p[1].trim_end_matches('%').parse::<f32>().ok()? / 100.0;
                let l = p[2].trim_end_matches('%').parse::<f32>().ok()? / 100.0;
                Some(com_alfa(hsl(h, s.clamp(0.0, 1.0), l.clamp(0.0, 1.0)), alfa_de(p.get(3))))
            }
            // A página é clara.
            "light-dark" => ler_cor(dentro.split(',').next()?),
            _ => None,
        };
    }
    match baixo.as_str() {
        "transparent" => return Some(0),
        "currentcolor" => return Some(COR_ATUAL),
        // Cores do sistema (as da página clara).
        "canvas" | "field" | "buttonface" => return Some(0xFFFFFFFF),
        "canvastext" | "fieldtext" | "buttontext" => return Some(0xFF000000),
        "linktext" => return Some(0xFF0000EE),
        "graytext" => return Some(0xFF808080),
        _ => {}
    }
    CORES.binary_search_by(|(n, _)| (*n).cmp(baixo.as_str())).ok().map(|i| 0xFF000000 | CORES[i].1)
}

fn separar_numero(v: &str) -> (&str, &str) {
    let b = v.as_bytes();
    let mut i = 0;
    while i < b.len() {
        let c = b[i];
        if c.is_ascii_digit() || c == b'.' || ((c == b'+' || c == b'-') && (i == 0 || matches!(b[i - 1], b'e' | b'E'))) || (matches!(c, b'e' | b'E') && b.get(i + 1).is_some_and(|d| d.is_ascii_digit() || *d == b'-' || *d == b'+') && i > 0) {
            i += 1;
        } else {
            break;
        }
    }
    v.split_at(i)
}

/// Um tamanho do CSS. `None` se não é um tamanho.
pub fn ler_t(v: &str) -> Option<T> {
    let v = v.trim();
    match v {
        "auto" | "normal" | "none" | "initial" | "unset" => return Some(T::Auto),
        "min-content" => return Some(T::Min),
        "max-content" => return Some(T::Max),
        "fit-content" | "-webkit-fit-content" | "-moz-fit-content" | "stretch" | "-webkit-fill-available" | "-moz-available" => return Some(T::Ajusta),
        "0" => return Some(T::Px(0.0)),
        _ => {}
    }
    if v.contains('(') {
        return ler_calc(v);
    }
    let (n, u) = separar_numero(v);
    let x: f32 = n.parse().ok()?;
    let u = u.trim();
    Some(match u.to_ascii_lowercase().as_str() {
        "px" => T::Px(x),
        "em" => T::Em(x),
        "rem" => T::Rem(x),
        "%" => T::Pct(x),
        "vw" | "svw" | "lvw" | "dvw" | "cqw" => T::Vw(x),
        "vh" | "svh" | "lvh" | "dvh" | "cqh" => T::Vh(x),
        "vmin" | "vmax" => T::Vw(x),
        "pt" => T::Px(x * 4.0 / 3.0),
        "pc" => T::Px(x * 16.0),
        "in" => T::Px(x * 96.0),
        "cm" => T::Px(x * 96.0 / 2.54),
        "mm" => T::Px(x * 96.0 / 25.4),
        "q" => T::Px(x * 96.0 / 101.6),
        "ex" | "ch" => T::Em(x * 0.5),
        "" if x == 0.0 => T::Px(0.0),
        _ => return None,
    })
}

fn termos(t: T) -> Option<[f32; 6]> {
    let mut v = [0.0; 6];
    match t {
        T::Px(x) => v[0] = x,
        T::Em(x) => v[1] = x,
        T::Rem(x) => v[2] = x,
        T::Vw(x) => v[3] = x,
        T::Vh(x) => v[4] = x,
        T::Pct(x) => v[5] = x,
        T::Soma(s) => v = s,
        _ => return None,
    }
    Some(v)
}

/// calc(), min(), max(), clamp(): a conta fica como uma soma de termos.
fn ler_calc(v: &str) -> Option<T> {
    let mut l = LeitorCalc { c: v.as_bytes(), s: v, i: 0 };
    let r = l.soma()?;
    Some(simplificar(r))
}

fn simplificar(v: [f32; 6]) -> T {
    let nao_zero: Vec<usize> = (0..6).filter(|i| v[*i] != 0.0).collect();
    match nao_zero.as_slice() {
        [] => T::Px(0.0),
        [0] => T::Px(v[0]),
        [1] => T::Em(v[1]),
        [2] => T::Rem(v[2]),
        [3] => T::Vw(v[3]),
        [4] => T::Vh(v[4]),
        [5] => T::Pct(v[5]),
        _ => T::Soma(v),
    }
}

struct LeitorCalc<'a> {
    c: &'a [u8],
    s: &'a str,
    i: usize,
}

impl LeitorCalc<'_> {
    fn brancos(&mut self) {
        while self.c.get(self.i) == Some(&b' ') {
            self.i += 1;
        }
    }

    fn soma(&mut self) -> Option<[f32; 6]> {
        let mut r = self.produto()?.0;
        loop {
            self.brancos();
            match self.c.get(self.i) {
                Some(b'+') => {
                    self.i += 1;
                    let (x, _) = self.produto()?;
                    for k in 0..6 {
                        r[k] += x[k];
                    }
                }
                Some(b'-') => {
                    self.i += 1;
                    let (x, _) = self.produto()?;
                    for k in 0..6 {
                        r[k] -= x[k];
                    }
                }
                _ => return Some(r),
            }
        }
    }

    /// (termos, é um número puro?)
    fn produto(&mut self) -> Option<([f32; 6], bool)> {
        let (mut r, mut puro) = self.fator()?;
        loop {
            self.brancos();
            match self.c.get(self.i) {
                Some(b'*') => {
                    self.i += 1;
                    let (x, xp) = self.fator()?;
                    if xp {
                        for v in r.iter_mut() {
                            *v *= x[0];
                        }
                    } else if puro {
                        let k = r[0];
                        r = x;
                        for v in r.iter_mut() {
                            *v *= k;
                        }
                        puro = false;
                    } else {
                        return None;
                    }
                }
                Some(b'/') => {
                    self.i += 1;
                    let (x, xp) = self.fator()?;
                    if !xp || x[0] == 0.0 {
                        return None;
                    }
                    for v in r.iter_mut() {
                        *v /= x[0];
                    }
                }
                _ => return Some((r, puro)),
            }
        }
    }

    fn fator(&mut self) -> Option<([f32; 6], bool)> {
        self.brancos();
        let ini = self.i;
        if self.c.get(self.i) == Some(&b'(') {
            self.i += 1;
            let r = self.soma()?;
            self.brancos();
            if self.c.get(self.i) != Some(&b')') {
                return None;
            }
            self.i += 1;
            return Some((r, false));
        }
        // Uma função?
        let mut j = self.i;
        while self.c.get(j).is_some_and(|x| x.is_ascii_alphabetic() || *x == b'-') {
            j += 1;
        }
        if j > self.i && self.c.get(j) == Some(&b'(') {
            let nome = self.s[self.i..j].to_ascii_lowercase();
            self.i = j + 1;
            let mut valores = Vec::new();
            loop {
                valores.push(self.soma()?);
                self.brancos();
                match self.c.get(self.i) {
                    Some(b',') => self.i += 1,
                    Some(b')') => {
                        self.i += 1;
                        break;
                    }
                    _ => return None,
                }
            }
            // Com unidades diferentes não dá para decidir aqui: escolhemos o
            // valor mais "do meio" (o de referência, em clamp) ou o primeiro.
            let chave = |v: &[f32; 6]| v[0] + v[1] * 16.0 + v[2] * 16.0 + v[3] * 9.0 + v[4] * 6.0 + v[5] * 9.0;
            let r = match nome.as_str() {
                "calc" | "-webkit-calc" | "-moz-calc" => valores[0],
                "min" => valores.iter().copied().fold(valores[0], |a, b| if chave(&b) < chave(&a) { b } else { a }),
                "max" => valores.iter().copied().fold(valores[0], |a, b| if chave(&b) > chave(&a) { b } else { a }),
                "clamp" if valores.len() == 3 => {
                    let (a, b, c) = (valores[0], valores[1], valores[2]);
                    if chave(&b) < chave(&a) { a } else if chave(&b) > chave(&c) { c } else { b }
                }
                _ => return None,
            };
            return Some((r, false));
        }
        // Um número com ou sem unidade.
        while let Some(&x) = self.c.get(self.i) {
            let sinal_exp = (x == b'+' || x == b'-') && self.i > ini && matches!(self.c[self.i - 1], b'e' | b'E');
            if x.is_ascii_alphanumeric() || x == b'.' || x == b'%' || sinal_exp || ((x == b'-' || x == b'+') && self.i == ini) {
                self.i += 1;
            } else {
                break;
            }
        }
        let t = &self.s[ini..self.i];
        if let Ok(n) = t.parse::<f32>() {
            let mut v = [0.0; 6];
            v[0] = n;
            return Some((v, true));
        }
        Some((termos(ler_t(t)?)?, false))
    }
}

/// Um tamanho em pixels: "12px", "1.5em" (com `em` pixels), "2rem", "10pt".
/// Sem unidade, só o zero. Porcentagens e "auto" não são tamanhos aqui.
pub fn pixels(valor: &str, em: f32) -> Option<i32> {
    Some(match ler_t(valor)? {
        T::Px(x) => x,
        T::Em(x) => x * em,
        T::Rem(x) => x * 16.0,
        _ => return None,
    } as i32)
}

// -----------------------------------------------------------------------------
// Seletores
// -----------------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq)]
enum Operador {
    Existe,
    Igual,
    Palavra,
    Comeca,
    Termina,
    Contem,
    Hifen,
}

#[derive(Clone, Debug)]
enum Pseudo {
    Raiz,
    Sobre,
    Foco,
    FocoDentro,
    Ativo,
    Marcado,
    Desativado,
    Ativado,
    Link,
    Visitado,
    Alvo,
    Vazio,
    /// a*n + b; de trás; só do mesmo tipo.
    NFilho(i32, i32, bool, bool),
    Unico(bool),
    Nao(Vec<Seletor>),
    Algum(Vec<Seletor>),
    Tem(Vec<Seletor>),
    Obrigatorio,
    Opcional,
    SomenteLeitura,
    Nunca,
}

#[derive(Clone, Debug, Default)]
struct Composto {
    tag: Option<String>,
    id: Option<String>,
    classes: Vec<String>,
    atributos: Vec<(String, Operador, String, bool)>,
    pseudos: Vec<Pseudo>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Combinador {
    Descendente,
    Filho,
    Adjacente,
    Irmao,
}

/// Um seletor, da direita para a esquerda: o primeiro composto é o do
/// elemento; o combinador diz como chegar ao seguinte.
#[derive(Clone, Debug)]
pub struct Seletor {
    partes: Vec<(Composto, Combinador)>,
    /// 0 o elemento, 1 ::before, 2 ::after
    pseudo_elemento: u8,
    especificidade: u32,
}

/// Lê uma lista de seletores ("a, b > c"). None se algum não vale.
pub fn ler_lista_seletores(texto: &str) -> Option<Vec<Seletor>> {
    let mut v = Vec::new();
    for s in dividir_virgulas(texto) {
        v.push(ler_seletor(s.trim())?);
    }
    Some(v)
}

fn nome_css(t: &[u8], i: &mut usize, texto: &str) -> String {
    let inicio = *i;
    let mut escape = false;
    while *i < t.len() && (t[*i].is_ascii_alphanumeric() || t[*i] == b'-' || t[*i] == b'_' || t[*i] >= 0x80 || t[*i] == b'\\') {
        if t[*i] == b'\\' {
            escape = true;
            *i += 1;
        }
        *i += 1;
    }
    let cru = texto.get(inicio..(*i).min(t.len())).unwrap_or("");
    if escape { cru.replace('\\', "") } else { cru.to_string() }
}

fn ler_an_b(t: &str) -> Option<(i32, i32)> {
    let t: String = t.chars().filter(|c| !c.is_whitespace()).collect::<String>().to_ascii_lowercase();
    match t.as_str() {
        "odd" => return Some((2, 1)),
        "even" => return Some((2, 0)),
        _ => {}
    }
    if let Some(p) = t.find('n') {
        let a = match &t[..p] {
            "" | "+" => 1,
            "-" => -1,
            x => x.parse().ok()?,
        };
        let r = &t[p + 1..];
        let b = if r.is_empty() { 0 } else { r.parse().ok()? };
        Some((a, b))
    } else {
        Some((0, t.parse().ok()?))
    }
}

/// O conteúdo do parêntese em `t[i]` = '('. Devolve (conteúdo, posição depois).
fn entre_parenteses(texto: &str, i: usize) -> Option<(&str, usize)> {
    let b = texto.as_bytes();
    let mut prof = 0;
    for (j, &c) in b.iter().enumerate().skip(i) {
        match c {
            b'(' => prof += 1,
            b')' => {
                prof -= 1;
                if prof == 0 {
                    return Some((&texto[i + 1..j], j + 1));
                }
            }
            _ => {}
        }
    }
    None
}

fn ler_composto(texto: &str, pe: &mut u8) -> Option<Composto> {
    let mut c = Composto::default();
    let t = texto.as_bytes();
    let mut i = 0;
    if i < t.len() && t[i] == b'*' {
        i += 1;
    } else if i < t.len() && (t[i].is_ascii_alphabetic() || t[i] >= 0x80 || t[i] == b'-' || t[i] == b'_') {
        c.tag = Some(nome_css(t, &mut i, texto).to_ascii_lowercase());
    }
    while i < t.len() {
        match t[i] {
            b'.' => {
                i += 1;
                let n = nome_css(t, &mut i, texto);
                if n.is_empty() {
                    return None;
                }
                c.classes.push(n);
            }
            b'#' => {
                i += 1;
                let n = nome_css(t, &mut i, texto);
                if n.is_empty() {
                    return None;
                }
                c.id = Some(n);
            }
            b'[' => {
                let fim = texto[i..].find(']')? + i;
                let dentro = &texto[i + 1..fim];
                let (nome_attr, op, valor, sem_caixa) = match dentro.find('=') {
                    None => (dentro.trim(), Operador::Existe, "", false),
                    Some(p) => {
                        let (antes, depois) = (&dentro[..p], &dentro[p + 1..]);
                        let (nome_attr, op) = match antes.as_bytes().last() {
                            Some(b'~') => (&antes[..antes.len() - 1], Operador::Palavra),
                            Some(b'^') => (&antes[..antes.len() - 1], Operador::Comeca),
                            Some(b'$') => (&antes[..antes.len() - 1], Operador::Termina),
                            Some(b'*') => (&antes[..antes.len() - 1], Operador::Contem),
                            Some(b'|') => (&antes[..antes.len() - 1], Operador::Hifen),
                            _ => (antes, Operador::Igual),
                        };
                        let valor = depois.trim();
                        let (valor, si) = match valor.strip_suffix(" i").or_else(|| valor.strip_suffix(" I")) {
                            Some(v) => (v.trim(), true),
                            None => (valor.strip_suffix(" s").unwrap_or(valor).trim(), false),
                        };
                        (nome_attr.trim(), op, valor.trim_matches(['"', '\'']), si)
                    }
                };
                c.atributos.push((nome_attr.to_ascii_lowercase(), op, valor.into(), sem_caixa));
                i = fim + 1;
            }
            b':' => {
                i += 1;
                let duplo = t.get(i) == Some(&b':');
                if duplo {
                    i += 1;
                }
                let nome = nome_css(t, &mut i, texto).to_ascii_lowercase();
                let mut arg = "";
                if t.get(i) == Some(&b'(') {
                    let (d, depois) = entre_parenteses(texto, i)?;
                    arg = d;
                    i = depois;
                }
                match nome.as_str() {
                    "before" => {
                        *pe = 1;
                        continue;
                    }
                    "after" => {
                        *pe = 2;
                        continue;
                    }
                    _ if duplo => return None, // ::selection, ::placeholder...: não desenhamos
                    _ => {}
                }
                c.pseudos.push(match nome.as_str() {
                    "root" => Pseudo::Raiz,
                    "hover" => Pseudo::Sobre,
                    "focus" | "focus-visible" => Pseudo::Foco,
                    "focus-within" => Pseudo::FocoDentro,
                    "active" => Pseudo::Ativo,
                    "checked" => Pseudo::Marcado,
                    "disabled" => Pseudo::Desativado,
                    "enabled" => Pseudo::Ativado,
                    "link" | "any-link" | "-webkit-any-link" => Pseudo::Link,
                    "visited" => Pseudo::Visitado,
                    "target" => Pseudo::Alvo,
                    "empty" => Pseudo::Vazio,
                    "first-child" => Pseudo::NFilho(0, 1, false, false),
                    "last-child" => Pseudo::NFilho(0, 1, true, false),
                    "first-of-type" => Pseudo::NFilho(0, 1, false, true),
                    "last-of-type" => Pseudo::NFilho(0, 1, true, true),
                    "only-child" => Pseudo::Unico(false),
                    "only-of-type" => Pseudo::Unico(true),
                    "nth-child" | "nth-last-child" | "nth-of-type" | "nth-last-of-type" => {
                        let (a, b) = ler_an_b(arg.split(" of ").next()?)?;
                        Pseudo::NFilho(a, b, nome.contains("last"), nome.contains("type"))
                    }
                    "not" => Pseudo::Nao(ler_lista_seletores(arg)?),
                    "is" | "where" | "matches" | "-webkit-any" | "-moz-any" => Pseudo::Algum(ler_lista_seletores(arg).unwrap_or_default()),
                    "has" => Pseudo::Tem(ler_lista_seletores(arg.trim_start_matches([' ', '>']))?),
                    "required" => Pseudo::Obrigatorio,
                    "optional" => Pseudo::Opcional,
                    "read-only" => Pseudo::SomenteLeitura,
                    "read-write" | "defined" | "first" | "lang" | "dir" | "in-range" | "valid" => continue,
                    "invalid" | "placeholder-shown" | "fullscreen" | "indeterminate" | "out-of-range" | "modal" | "popover-open" | "autofill" | "-webkit-autofill" | "user-invalid" | "open" | "closed" => Pseudo::Nunca,
                    _ => return None,
                });
            }
            _ => return None,
        }
    }
    Some(c)
}

fn especificidade_de(s: &[Seletor]) -> u32 {
    s.iter().map(|x| x.especificidade).max().unwrap_or(0)
}

fn ler_seletor(texto: &str) -> Option<Seletor> {
    // Quebra em compostos e combinadores (espaços dentro de [] ou () não separam).
    let mut fichas: Vec<String> = Vec::new();
    let mut atual = String::new();
    let (mut colchetes, mut parenteses, mut aspa) = (0, 0, None);
    for ch in texto.chars() {
        if let Some(a) = aspa {
            if ch == a {
                aspa = None;
            }
            atual.push(ch);
            continue;
        }
        match ch {
            '"' | '\'' => aspa = Some(ch),
            '[' => colchetes += 1,
            ']' => colchetes -= 1,
            '(' => parenteses += 1,
            ')' => parenteses -= 1,
            _ => {}
        }
        if colchetes == 0 && parenteses == 0 {
            if ch.is_whitespace() {
                if !atual.is_empty() {
                    fichas.push(core::mem::take(&mut atual));
                }
                continue;
            }
            if matches!(ch, '>' | '+' | '~') {
                if !atual.is_empty() {
                    fichas.push(core::mem::take(&mut atual));
                }
                fichas.push(ch.to_string());
                continue;
            }
        }
        atual.push(ch);
    }
    if !atual.is_empty() {
        fichas.push(atual);
    }
    let mut partes: Vec<(Composto, Combinador)> = Vec::new();
    let mut combinador = Combinador::Descendente;
    let mut pseudo_elemento = 0;
    for f in fichas {
        match f.as_str() {
            ">" => combinador = Combinador::Filho,
            "+" => combinador = Combinador::Adjacente,
            "~" => combinador = Combinador::Irmao,
            _ => {
                partes.push((ler_composto(&f, &mut pseudo_elemento)?, combinador));
                combinador = Combinador::Descendente;
            }
        }
    }
    if partes.is_empty() {
        return None;
    }
    let n = partes.len();
    let mut invertidas: Vec<(Composto, Combinador)> = Vec::with_capacity(n);
    for i in (0..n).rev() {
        let liga = if i > 0 { partes[i].1 } else { Combinador::Descendente };
        invertidas.push((partes[i].0.clone(), liga));
    }
    let (mut a, mut b, mut c) = (0, 0, 0);
    for (p, _) in &invertidas {
        a += p.id.is_some() as u32;
        b += (p.classes.len() + p.atributos.len()) as u32;
        c += p.tag.is_some() as u32;
        for q in &p.pseudos {
            match q {
                Pseudo::Nao(s) | Pseudo::Algum(s) | Pseudo::Tem(s) => {
                    let e = especificidade_de(s);
                    a += e >> 16;
                    b += e >> 8 & 0xFF;
                    c += e & 0xFF;
                }
                _ => b += 1,
            }
        }
    }
    if pseudo_elemento != 0 {
        c += 1;
    }
    Some(Seletor { partes: invertidas, pseudo_elemento, especificidade: a.min(255) << 16 | b.min(255) << 8 | c.min(255) })
}

/// O estado da página que os seletores olham.
#[derive(Default)]
pub struct EstadoCss {
    pub sobre: Vec<NoId>,
    pub ativos: Vec<NoId>,
    pub foco: Option<NoId>,
    pub marcados: BTreeSet<NoId>,
    pub visitados: BTreeSet<NoId>,
    pub alvo: Option<NoId>,
    pub largura: i32,
    pub altura: i32,
}

struct Casador<'a> {
    d: &'a Dom,
    e: &'a EstadoCss,
}

impl Casador<'_> {
    fn indice_entre_irmaos(&self, n: NoId, de_tras: bool, mesmo_tipo: bool) -> i32 {
        let pai = self.d.pai(n);
        if pai == NADA {
            return 1;
        }
        let nome = self.d.nome(n);
        let lista = self.d.filhos(pai);
        let filtro = |c: &&NoId| self.d.e_elemento(**c) && (!mesmo_tipo || self.d.nome(**c) == nome);
        let mut i = 1;
        if de_tras {
            for c in lista.iter().rev().filter(filtro) {
                if *c == n {
                    return i;
                }
                i += 1;
            }
        } else {
            for c in lista.iter().filter(filtro) {
                if *c == n {
                    return i;
                }
                i += 1;
            }
        }
        i
    }

    fn pseudo(&self, p: &Pseudo, n: NoId) -> bool {
        let d = self.d;
        match p {
            Pseudo::Raiz => d.e(n, "html"),
            Pseudo::Sobre => self.e.sobre.contains(&n),
            Pseudo::Foco => self.e.foco == Some(n),
            Pseudo::FocoDentro => self.e.foco.is_some_and(|f| d.contem(n, f)),
            Pseudo::Ativo => self.e.ativos.contains(&n),
            Pseudo::Marcado => self.e.marcados.contains(&n),
            Pseudo::Desativado => d.atributo(n, "disabled").is_some(),
            Pseudo::Ativado => matches!(d.nome(n), "input" | "button" | "select" | "textarea" | "option") && d.atributo(n, "disabled").is_none(),
            Pseudo::Link => matches!(d.nome(n), "a" | "area") && d.atributo(n, "href").is_some() && !self.e.visitados.contains(&n),
            Pseudo::Visitado => self.e.visitados.contains(&n),
            Pseudo::Alvo => self.e.alvo == Some(n),
            Pseudo::Vazio => d.filhos(n).iter().all(|c| !d.e_elemento(*c) && d.texto_de_no(*c).is_none_or(str::is_empty)),
            Pseudo::NFilho(a, b, tras, tipo) => {
                let i = self.indice_entre_irmaos(n, *tras, *tipo);
                if *a == 0 {
                    i == *b
                } else {
                    let k = i - b;
                    k % a == 0 && k / a >= 0
                }
            }
            Pseudo::Unico(tipo) => self.indice_entre_irmaos(n, false, *tipo) == 1 && self.indice_entre_irmaos(n, true, *tipo) == 1,
            Pseudo::Nao(l) => !l.iter().any(|s| self.casa(s, n)),
            Pseudo::Algum(l) => l.iter().any(|s| self.casa(s, n)),
            Pseudo::Tem(l) => self.algum_descendente(n, l),
            Pseudo::Obrigatorio => d.atributo(n, "required").is_some(),
            Pseudo::Opcional => matches!(d.nome(n), "input" | "select" | "textarea") && d.atributo(n, "required").is_none(),
            Pseudo::SomenteLeitura => !matches!(d.nome(n), "input" | "textarea") || d.atributo(n, "readonly").is_some(),
            Pseudo::Nunca => false,
        }
    }

    fn algum_descendente(&self, n: NoId, l: &[Seletor]) -> bool {
        self.d.filhos(n).iter().any(|c| self.d.e_elemento(*c) && (l.iter().any(|s| self.casa(s, *c)) || self.algum_descendente(*c, l)))
    }

    fn composto(&self, c: &Composto, n: NoId) -> bool {
        let d = self.d;
        if let Some(t) = &c.tag {
            if t != d.nome(n) {
                return false;
            }
        }
        if let Some(id) = &c.id {
            if d.atributo(n, "id") != Some(id.as_str()) {
                return false;
            }
        }
        if !c.classes.is_empty() {
            let classes = d.atributo(n, "class").unwrap_or("");
            if !c.classes.iter().all(|k| classes.split_ascii_whitespace().any(|x| x == k)) {
                return false;
            }
        }
        for (nome, op, valor, sem_caixa) in &c.atributos {
            let Some(v) = d.atributo(n, nome) else { return false };
            let baixo_v;
            let baixo_a;
            let (v, valor): (&str, &str) = if *sem_caixa {
                baixo_v = v.to_ascii_lowercase();
                baixo_a = valor.to_ascii_lowercase();
                (&baixo_v, &baixo_a)
            } else {
                (v, valor)
            };
            let ok = match op {
                Operador::Existe => true,
                Operador::Igual => v == valor,
                Operador::Palavra => v.split_ascii_whitespace().any(|x| x == valor),
                Operador::Comeca => !valor.is_empty() && v.starts_with(valor),
                Operador::Termina => !valor.is_empty() && v.ends_with(valor),
                Operador::Contem => !valor.is_empty() && v.contains(valor),
                Operador::Hifen => v == valor || (v.starts_with(valor) && v[valor.len()..].starts_with('-')),
            };
            if !ok {
                return false;
            }
        }
        c.pseudos.iter().all(|p| self.pseudo(p, n))
    }

    fn casa(&self, s: &Seletor, n: NoId) -> bool {
        self.partes(&s.partes, n)
    }

    fn partes(&self, p: &[(Composto, Combinador)], n: NoId) -> bool {
        if !self.composto(&p[0].0, n) {
            return false;
        }
        if p.len() == 1 {
            return true;
        }
        let d = self.d;
        match p[0].1 {
            Combinador::Filho => {
                let pai = d.pai(n);
                pai != NADA && d.e_elemento(pai) && self.partes(&p[1..], pai)
            }
            Combinador::Descendente => {
                let mut x = d.pai(n);
                while x != NADA && d.e_elemento(x) {
                    if self.partes(&p[1..], x) {
                        return true;
                    }
                    x = d.pai(x);
                }
                false
            }
            Combinador::Adjacente => d.irmao_elemento(n, false).is_some_and(|a| self.partes(&p[1..], a)),
            Combinador::Irmao => {
                let mut x = d.irmao_elemento(n, false);
                while let Some(a) = x {
                    if self.partes(&p[1..], a) {
                        return true;
                    }
                    x = d.irmao_elemento(a, false);
                }
                false
            }
        }
    }
}

/// `elemento.matches(seletor)`
pub fn casa_seletores(d: &Dom, e: &EstadoCss, l: &[Seletor], n: NoId) -> bool {
    let c = Casador { d, e };
    l.iter().any(|s| s.pseudo_elemento == 0 && c.casa(s, n))
}

// -----------------------------------------------------------------------------
// As declarações
// -----------------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum P {
    Display, Posicao, Flutua, Limpa, Visibilidade, Overflow, CaixaBorda,
    Largura, Altura, MinLargura, MaxLargura, MinAltura, MaxAltura,
    Margem(u8), Preench(u8), BordaL(u8), BordaCor(u8), BordaEstilo(u8), Raio(u8),
    Topo, Direita, Baixo, Esquerda, Z,
    Cor, Fundo, FundoImg, FundoRepete, FundoTam, FundoPos,
    FonteTam, Negrito, Italico, Familia, AlturaLinha, EspacoLetras, EspacoPalavras,
    Alinha, Recuo, Transforma, Branco, Decoracao, CorDecoracao, AlinhaV, QuebraPalavra, Reticencias,
    ListaTipo, ListaPos,
    FlexDirecao, FlexQuebra, Justifica, AlinhaItens, AlinhaProprio, AlinhaConteudo, JustificaItens, JustificaProprio,
    Cresce, Encolhe, BaseFlex, Ordem, VaoLinha, VaoColuna,
    GridColunas, GridLinhas, GridAutoColunas, GridAutoLinhas, GridFluxo, GridAreas,
    GridColunaIni, GridColunaFim, GridLinhaIni, GridLinhaFim, GridArea,
    Opacidade, Sombras, Transformacao, Cursor, SemEventos, Conteudo,
    ColapsaBordas, EspacoBordas, TabelaFixa, AjusteObjeto, Proporcao, Mascara,
}

#[derive(Clone, Debug)]
pub enum V {
    K(u8),
    Cor(u32),
    T(T),
    N(f32),
    Txt(Rc<str>),
    Img(Option<Rc<Imagem>>),
    Par(T, T),
    Trilhas(Rc<Vec<Trilha>>),
    Areas(Rc<Vec<Vec<String>>>),
    Linha(LinhaGrid),
    Sombras(Option<Rc<Vec<Sombra>>>),
    /// translate x, y; escala
    Transf(T, T, f32),
    Herdar,
    Inicial,
}

#[derive(Clone, Debug)]
pub enum Decl {
    D(P, V),
    /// Uma variável: `--x: valor`.
    Var(Rc<str>, Rc<str>),
    /// Usa var(): só dá para interpretar quando se sabe o valor da variável.
    ComVar(Rc<str>, Rc<str>),
}

fn k(v: u8) -> V {
    V::K(v)
}

fn lados(v: &str) -> Vec<&str> {
    // Separa por espaços, respeitando parênteses.
    let (mut prof, mut ini) = (0, None);
    let mut r = Vec::new();
    for (i, c) in v.char_indices() {
        match c {
            '(' => prof += 1,
            ')' => prof -= 1,
            _ => {}
        }
        if c.is_whitespace() && prof == 0 {
            if let Some(a) = ini.take() {
                r.push(&v[a..i]);
            }
        } else if ini.is_none() {
            ini = Some(i);
        }
    }
    if let Some(a) = ini {
        r.push(&v[a..]);
    }
    r
}

fn quatro(v: &str) -> Option<[&str; 4]> {
    let l = lados(v);
    Some(match l.len() {
        1 => [l[0], l[0], l[0], l[0]],
        2 => [l[0], l[1], l[0], l[1]],
        3 => [l[0], l[1], l[2], l[1]],
        4 => [l[0], l[1], l[2], l[3]],
        _ => return None,
    })
}

fn borda_largura(v: &str) -> Option<T> {
    match v {
        "thin" => Some(T::Px(1.0)),
        "medium" => Some(T::Px(3.0)),
        "thick" => Some(T::Px(5.0)),
        _ => ler_t(v).filter(|t| !matches!(t, T::Auto)),
    }
}

fn borda_estilo(v: &str) -> Option<u8> {
    Some(match v {
        "none" | "hidden" => 0,
        "solid" | "inset" | "outset" | "groove" | "ridge" => 1,
        "dashed" => 2,
        "dotted" => 3,
        "double" => 4,
        _ => return None,
    })
}

fn expandir_borda(lado: Option<u8>, v: &str, s: &mut Vec<Decl>) {
    let (mut l, mut e, mut c) = (None, None, None);
    for p in lados(v) {
        if let Some(x) = borda_estilo(p) {
            e = Some(x);
        } else if let Some(x) = borda_largura(p) {
            l = Some(x);
        } else if let Some(x) = ler_cor(p) {
            c = Some(x);
        }
    }
    let alvo: &[u8] = match lado {
        Some(0) => &[0],
        Some(1) => &[1],
        Some(2) => &[2],
        Some(3) => &[3],
        _ => &[0, 1, 2, 3],
    };
    for &i in alvo {
        s.push(Decl::D(P::BordaL(i), V::T(l.unwrap_or(T::Px(3.0)))));
        s.push(Decl::D(P::BordaEstilo(i), k(e.unwrap_or(0))));
        s.push(Decl::D(P::BordaCor(i), V::Cor(c.unwrap_or(COR_ATUAL))));
    }
}

fn funcao<'a>(v: &'a str, nome: &str) -> Option<&'a str> {
    let r = v.strip_prefix(nome)?.strip_prefix('(')?;
    r.trim_end().strip_suffix(')')
}

fn ler_gradiente(v: &str) -> Option<Imagem> {
    let dentro = funcao(v, "linear-gradient").or_else(|| funcao(v, "-webkit-linear-gradient")).or_else(|| funcao(v, "repeating-linear-gradient"))?;
    let partes = dividir_virgulas(dentro);
    let mut angulo = 180.0;
    let mut i = 0;
    let primeiro = partes.first()?.trim();
    if let Some(a) = primeiro.strip_suffix("deg") {
        angulo = a.trim().parse().ok()?;
        i = 1;
    } else if let Some(d) = primeiro.strip_prefix("to ") {
        angulo = match d.trim() {
            "top" => 0.0,
            "right" => 90.0,
            "bottom" => 180.0,
            "left" => 270.0,
            "top right" | "right top" => 45.0,
            "bottom right" | "right bottom" => 135.0,
            "bottom left" | "left bottom" => 225.0,
            _ => 315.0,
        };
        i = 1;
    }
    let mut paradas: Vec<(f32, Option<f32>, u32)> = Vec::new();
    for p in &partes[i..] {
        let l = lados(p.trim());
        let cor = ler_cor(l.first()?)?;
        let pos = l.get(1).and_then(|x| x.strip_suffix('%')).and_then(|x| x.parse::<f32>().ok()).map(|x| x / 100.0);
        paradas.push((0.0, pos, cor));
    }
    let n = paradas.len();
    if n == 0 {
        return None;
    }
    let lista = paradas.iter().enumerate().map(|(j, (_, pos, c))| (pos.unwrap_or(if n > 1 { j as f32 / (n - 1) as f32 } else { 0.0 }), *c)).collect();
    Some(Imagem::Gradiente(angulo, lista))
}

/// `url("a.png")`
pub fn ler_url(v: &str) -> Option<&str> {
    let d = v.strip_prefix("url(")?;
    let fim = d.find(')')?;
    Some(d[..fim].trim().trim_matches(['"', '\'']))
}

fn ler_imagem(v: &str) -> Option<Option<Rc<Imagem>>> {
    if v == "none" {
        return Some(None);
    }
    if let Some(u) = ler_url(v) {
        if u.is_empty() || u.starts_with("data:") {
            return Some(None);
        }
        return Some(Some(Rc::new(Imagem::Url(u.to_string()))));
    }
    if v.contains("gradient") {
        return ler_gradiente(v).map(|g| Some(Rc::new(g)));
    }
    None
}

fn ler_repete(v: &str) -> Option<u8> {
    Some(match v {
        "repeat" | "repeat repeat" | "space" | "round" => 0,
        "repeat-x" | "repeat no-repeat" => 1,
        "repeat-y" | "no-repeat repeat" => 2,
        "no-repeat" | "no-repeat no-repeat" => 3,
        _ => return None,
    })
}

fn ler_pos_fundo(l: &[&str]) -> Option<(T, T)> {
    let mapa = |p: &str| -> Option<(u8, T)> {
        Some(match p {
            "left" => (1, T::Pct(0.0)),
            "right" => (1, T::Pct(100.0)),
            "top" => (2, T::Pct(0.0)),
            "bottom" => (2, T::Pct(100.0)),
            "center" => (0, T::Pct(50.0)),
            o => (0, ler_t(o)?),
        })
    };
    match l {
        [a] => {
            let (ka, va) = mapa(a)?;
            Some(if ka == 2 { (T::Pct(50.0), va) } else { (va, T::Pct(50.0)) })
        }
        [a, b, ..] => {
            let ((ka, va), (kb, vb)) = (mapa(a)?, mapa(b)?);
            Some(if ka == 2 || kb == 1 { (vb, va) } else { (va, vb) })
        }
        _ => None,
    }
}

fn expandir_fundo(v: &str, s: &mut Vec<Decl>) {
    // Só a última camada vale (a de baixo, onde costuma estar a cor).
    let camadas = dividir_virgulas(v);
    let camada = camadas.last().copied().unwrap_or(v).trim();
    let (mut cor, mut img, mut rep, mut pos, mut tam) = (0, None, 0, Vec::new(), None);
    let pedacos = lados(camada);
    let mut i = 0;
    while i < pedacos.len() {
        let p = pedacos[i];
        if p == "/" {
            let a = pedacos.get(i + 1).copied().unwrap_or("auto");
            let b = pedacos.get(i + 2).copied().filter(|x| ler_t(x).is_some());
            tam = Some(ler_tam_fundo(a, b));
            i += if b.is_some() { 3 } else { 2 };
            continue;
        }
        if let Some(c) = ler_cor(p) {
            cor = c;
        } else if let Some(m) = ler_imagem(p) {
            img = m;
        } else if let Some(r) = ler_repete(p) {
            rep = r;
        } else if matches!(p, "fixed" | "scroll" | "local" | "border-box" | "padding-box" | "content-box" | "text") {
        } else if matches!(p, "left" | "right" | "top" | "bottom" | "center") || ler_t(p).is_some() {
            pos.push(p);
        }
        i += 1;
    }
    // A cor pode estar em outra camada (a primeira, às vezes).
    if camadas.len() > 1 {
        for c in &camadas {
            for p in lados(c.trim()) {
                if let Some(x) = ler_cor(p) {
                    cor = x;
                }
                if img.is_none() {
                    if let Some(m) = ler_imagem(p) {
                        img = m;
                    }
                }
            }
        }
    }
    s.push(Decl::D(P::Fundo, V::Cor(cor)));
    s.push(Decl::D(P::FundoImg, V::Img(img)));
    s.push(Decl::D(P::FundoRepete, k(rep)));
    let (px, py) = ler_pos_fundo(&pos).unwrap_or((T::Px(0.0), T::Px(0.0)));
    s.push(Decl::D(P::FundoPos, V::Par(px, py)));
    let (m, a, b) = tam.unwrap_or((0, T::Auto, T::Auto));
    s.push(Decl::D(P::FundoTam, V::Transf(a, b, m as f32)));
}

fn ler_tam_fundo(a: &str, b: Option<&str>) -> (u8, T, T) {
    match a {
        "cover" => (1, T::Auto, T::Auto),
        "contain" => (2, T::Auto, T::Auto),
        _ => (0, ler_t(a).unwrap_or(T::Auto), b.and_then(ler_t).unwrap_or(T::Auto)),
    }
}

fn eh_mono(v: &str) -> bool {
    let b = v.to_ascii_lowercase();
    b.contains("mono") || b.contains("courier") || b.contains("consolas") || b.contains("menlo") || b.contains("code")
}

fn tamanho_fonte(v: &str) -> Option<V> {
    Some(match v {
        "xx-small" => V::T(T::Px(9.0)),
        "x-small" => V::T(T::Px(10.0)),
        "small" => V::T(T::Px(13.0)),
        "medium" => V::T(T::Px(16.0)),
        "large" => V::T(T::Px(18.0)),
        "x-large" => V::T(T::Px(24.0)),
        "xx-large" => V::T(T::Px(32.0)),
        "xxx-large" => V::T(T::Px(48.0)),
        "smaller" => V::T(T::Em(0.833)),
        "larger" => V::T(T::Em(1.2)),
        _ => V::T(ler_t(v).filter(|t| !matches!(t, T::Auto))?),
    })
}

fn altura_linha(v: &str) -> Option<V> {
    if v == "normal" {
        return Some(V::N(0.0));
    }
    if let Ok(n) = v.parse::<f32>() {
        return Some(V::N(n));
    }
    Some(V::T(ler_t(v).filter(|t| !matches!(t, T::Auto))?))
}

fn expandir_fonte(v: &str, s: &mut Vec<Decl>) {
    if matches!(v, "caption" | "icon" | "menu" | "message-box" | "small-caption" | "status-bar") {
        return;
    }
    let mut negrito = false;
    let mut italico = false;
    let mut resto = Vec::new();
    let l = lados(v);
    let mut i = 0;
    while i < l.len() {
        let p = l[i];
        match p {
            "italic" | "oblique" => italico = true,
            "normal" | "small-caps" => {}
            "bold" | "bolder" => negrito = true,
            "lighter" => negrito = false,
            _ if p.parse::<f32>().is_ok() && resto.is_empty() => negrito = p.parse::<f32>().unwrap_or(0.0) >= 600.0,
            _ => {
                resto.extend_from_slice(&l[i..]);
                break;
            }
        }
        i += 1;
    }
    let Some(tam) = resto.first() else { return };
    let (t, lh) = tam.split_once('/').map_or((*tam, None), |(a, b)| (a, Some(b)));
    let Some(fs) = tamanho_fonte(t) else { return };
    s.push(Decl::D(P::Negrito, k(negrito as u8)));
    s.push(Decl::D(P::Italico, k(italico as u8)));
    s.push(Decl::D(P::FonteTam, fs));
    s.push(Decl::D(P::AlturaLinha, lh.and_then(altura_linha).unwrap_or(V::N(0.0))));
    let familia = resto[1..].join(" ");
    s.push(Decl::D(P::Familia, k(eh_mono(&familia) as u8)));
}

fn ler_trilha(p: &str) -> Option<Trilha> {
    if let Some(f) = p.strip_suffix("fr") {
        if let Ok(x) = f.parse::<f32>() {
            return Some(Trilha::Fr(x));
        }
    }
    if let Some(d) = funcao(p, "minmax") {
        let (a, b) = d.split_once(',')?;
        return Some(Trilha::MinMax(ler_t(a)?, Box::new(ler_trilha(b.trim())?)));
    }
    if let Some(d) = funcao(p, "fit-content") {
        return Some(Trilha::MinMax(T::Min, Box::new(Trilha::T(ler_t(d)?))));
    }
    if let Some(d) = funcao(p, "repeat") {
        let (n, r) = d.split_once(',')?;
        let lista = ler_trilhas(r)?;
        return Some(match n.trim() {
            "auto-fill" | "auto-fit" => Trilha::RepeteAuto(lista),
            x => Trilha::Repete(x.parse::<u32>().ok()?.min(1000), lista),
        });
    }
    Some(Trilha::T(ler_t(p)?))
}

fn ler_trilhas(v: &str) -> Option<Vec<Trilha>> {
    let mut r = Vec::new();
    for p in lados(v) {
        if p.starts_with('[') || p.ends_with(']') {
            continue; // nomes de linhas
        }
        r.push(ler_trilha(p)?);
    }
    Some(r)
}

fn ler_linha_grid(v: &str) -> V {
    let l = lados(v);
    let mut g = LinhaGrid::default();
    let mut i = 0;
    while i < l.len() {
        if l[i] == "span" {
            g.span = l.get(i + 1).and_then(|x| x.parse().ok()).unwrap_or(1);
            i += 2;
            continue;
        }
        if let Ok(n) = l[i].parse::<i32>() {
            g.linha = n;
        }
        i += 1;
    }
    V::Linha(g)
}

fn ler_alinhamento(v: &str) -> Option<u8> {
    let v = v.rsplit(' ').next()?;
    Some(match v {
        "flex-start" | "start" | "left" | "self-start" | "normal" | "safe" if v != "normal" => 0,
        "flex-end" | "end" | "right" | "self-end" => 1,
        "center" => 2,
        "space-between" => 3,
        "space-around" => 4,
        "space-evenly" => 5,
        "stretch" | "normal" => 6,
        "baseline" | "first" | "last" => 0,
        _ => return None,
    })
}

fn ler_alinha_item(v: &str) -> Option<u8> {
    let v = v.rsplit(' ').next()?;
    Some(match v {
        "flex-start" | "start" | "self-start" | "left" => 0,
        "flex-end" | "end" | "self-end" | "right" => 1,
        "center" => 2,
        "baseline" | "first" | "last" => 3,
        "stretch" | "normal" => 4,
        "auto" => 5,
        _ => return None,
    })
}

fn ler_sombras(v: &str) -> Option<Rc<Vec<Sombra>>> {
    if v == "none" {
        return None;
    }
    let mut r = Vec::new();
    for c in dividir_virgulas(v) {
        let mut s = Sombra { x: 0.0, y: 0.0, borrao: 0.0, expande: 0.0, cor: 0x80000000, dentro: false };
        let mut nums = Vec::new();
        for p in lados(c.trim()) {
            if p == "inset" {
                s.dentro = true;
            } else if let Some(cor) = ler_cor(p) {
                s.cor = cor;
            } else if let Some(T::Px(x)) = ler_t(p) {
                nums.push(x);
            } else if let Some(T::Em(x)) | Some(T::Rem(x)) = ler_t(p) {
                nums.push(x * 16.0);
            }
        }
        s.x = nums.first().copied().unwrap_or(0.0);
        s.y = nums.get(1).copied().unwrap_or(0.0);
        s.borrao = nums.get(2).copied().unwrap_or(0.0);
        s.expande = nums.get(3).copied().unwrap_or(0.0);
        r.push(s);
    }
    Some(Rc::new(r))
}

fn ler_transformacao(v: &str) -> V {
    let (mut x, mut y, mut e) = (T::Px(0.0), T::Px(0.0), 1.0);
    for f in lados(v) {
        if let Some(d) = funcao(f, "translate") {
            let (a, b) = d.split_once(',').unwrap_or((d, "0"));
            x = ler_t(a).unwrap_or(x);
            y = ler_t(b).unwrap_or(y);
        } else if let Some(d) = funcao(f, "translateX") {
            x = ler_t(d).unwrap_or(x);
        } else if let Some(d) = funcao(f, "translateY") {
            y = ler_t(d).unwrap_or(y);
        } else if let Some(d) = funcao(f, "translate3d") {
            let p: Vec<&str> = d.split(',').collect();
            x = p.first().and_then(|q| ler_t(q)).unwrap_or(x);
            y = p.get(1).and_then(|q| ler_t(q)).unwrap_or(y);
        } else if let Some(d) = funcao(f, "scale") {
            e = d.split(',').next().and_then(|q| q.trim().parse().ok()).unwrap_or(e);
        }
    }
    V::Transf(x, y, e)
}

/// `content: "x" attr(y)`: o texto, com os `attr()` marcados por \u{1}nome\u{2}.
fn ler_conteudo(v: &str) -> Option<Rc<str>> {
    if v == "none" || v == "normal" {
        return None;
    }
    let mut s = String::new();
    for p in lados(v) {
        if let Some(d) = p.strip_prefix(['"', '\'']) {
            let d = &d[..d.len().saturating_sub(1)];
            // \2022: um escape com o código da letra.
            let c: Vec<char> = d.chars().collect();
            let mut i = 0;
            while i < c.len() {
                if c[i] == '\\' && i + 1 < c.len() {
                    let mut j = i + 1;
                    let mut h = String::new();
                    while j < c.len() && c[j].is_ascii_hexdigit() && h.len() < 6 {
                        h.push(c[j]);
                        j += 1;
                    }
                    if !h.is_empty() {
                        s.push(u32::from_str_radix(&h, 16).ok().and_then(char::from_u32).unwrap_or('\u{FFFD}'));
                        i = if c.get(j) == Some(&' ') { j + 1 } else { j };
                    } else {
                        s.push(c[i + 1]);
                        i += 2;
                    }
                } else {
                    s.push(c[i]);
                    i += 1;
                }
            }
        } else if let Some(d) = funcao(p, "attr") {
            s.push('\u{1}');
            s.push_str(d.trim());
            s.push('\u{2}');
        }
    }
    Some(Rc::from(s.as_str()))
}

fn lista_tipo(v: &str) -> Option<u8> {
    Some(match v {
        "disc" => 0,
        "circle" => 1,
        "square" => 2,
        "decimal" | "decimal-leading-zero" => 3,
        "none" => 4,
        "lower-alpha" | "lower-latin" => 5,
        "upper-alpha" | "upper-latin" => 6,
        "lower-roman" => 7,
        "upper-roman" => 8,
        _ => return None,
    })
}

fn ler_display(v: &str) -> Option<Display> {
    Some(match v {
        "none" => Display::Nenhum,
        "block" | "flow-root" | "block flow" | "flow" | "block flow-root" => Display::Bloco,
        "inline" | "inline flow" | "ruby" | "ruby-text" => Display::Inline,
        "inline-block" | "inline flow-root" => Display::InlineBloco,
        "flex" | "block flex" | "-webkit-flex" | "-webkit-box" | "-ms-flexbox" | "-moz-box" => Display::Flex,
        "inline-flex" | "inline flex" | "-webkit-inline-flex" | "-webkit-inline-box" | "-ms-inline-flexbox" => Display::InlineFlex,
        "grid" | "block grid" | "-ms-grid" => Display::Grid,
        "inline-grid" | "inline grid" => Display::InlineGrid,
        "table" => Display::Tabela,
        "inline-table" => Display::InlineTabela,
        "table-row-group" | "table-header-group" | "table-footer-group" => Display::GrupoLinhas,
        "table-row" => Display::Linha,
        "table-cell" => Display::Celula,
        "table-caption" => Display::Legenda,
        "table-column-group" => Display::GrupoColunas,
        "table-column" => Display::Coluna,
        "list-item" | "block list-item" => Display::ItemLista,
        "contents" => Display::Conteudo,
        _ => return None,
    })
}

/// Interpreta uma declaração em uma ou mais `Decl` (os atalhos se expandem).
pub fn interpretar(nome: &str, valor: &str, s: &mut Vec<Decl>) {
    if let Some(n) = nome.strip_prefix("--") {
        let mut chave = String::from("--");
        chave.push_str(n);
        s.push(Decl::Var(Rc::from(chave.as_str()), Rc::from(valor)));
        return;
    }
    if valor.contains("var(") {
        s.push(Decl::ComVar(Rc::from(nome), Rc::from(valor)));
        return;
    }
    let baixo = valor.to_ascii_lowercase();
    let v = baixo.trim();
    let tamanho = |p: P, s: &mut Vec<Decl>| {
        if let Some(t) = global(v).or_else(|| ler_t(v).map(V::T)) {
            s.push(Decl::D(p, t));
        }
    };
    let cada = |p: fn(u8) -> P, f: &dyn Fn(&str) -> Option<V>, s: &mut Vec<Decl>| {
        if let Some(g) = global(v) {
            for i in 0..4 {
                s.push(Decl::D(p(i), g.clone()));
            }
            return;
        }
        if let Some(q) = quatro(v) {
            for i in 0..4 {
                if let Some(x) = f(q[i]) {
                    s.push(Decl::D(p(i as u8), x));
                }
            }
        }
    };
    let lado_de = |n: &str| -> Option<u8> {
        Some(match n {
            "top" | "block-start" => 0,
            "right" | "inline-end" => 1,
            "bottom" | "block-end" => 2,
            "left" | "inline-start" => 3,
            _ => return None,
        })
    };
    let simples = |p: P, x: Option<V>, s: &mut Vec<Decl>| {
        if let Some(g) = global(v) {
            s.push(Decl::D(p, g));
        } else if let Some(x) = x {
            s.push(Decl::D(p, x));
        }
    };
    match nome {
        "display" => simples(P::Display, ler_display(v).map(|d| V::K(d as u8)), s),
        "position" => simples(P::Posicao, match v { "static" => Some(k(0)), "relative" => Some(k(1)), "absolute" => Some(k(2)), "fixed" => Some(k(3)), "sticky" | "-webkit-sticky" => Some(k(4)), _ => None }, s),
        "float" => simples(P::Flutua, match v { "none" => Some(k(0)), "left" | "inline-start" => Some(k(1)), "right" | "inline-end" => Some(k(2)), _ => None }, s),
        "clear" => simples(P::Limpa, match v { "none" => Some(k(0)), "left" => Some(k(1)), "right" => Some(k(2)), "both" => Some(k(3)), _ => None }, s),
        "visibility" => simples(P::Visibilidade, match v { "visible" => Some(k(1)), "hidden" | "collapse" => Some(k(0)), _ => None }, s),
        "overflow" | "overflow-y" | "overflow-x" => simples(P::Overflow, match v.split(' ').next().unwrap_or("") { "visible" => Some(k(0)), "hidden" | "clip" => Some(k(1)), "auto" | "scroll" | "overlay" => Some(k(2)), _ => None }, s),
        "box-sizing" | "-webkit-box-sizing" | "-moz-box-sizing" => simples(P::CaixaBorda, Some(k((v == "border-box") as u8)), s),
        "width" | "inline-size" => tamanho(P::Largura, s),
        "height" | "block-size" => tamanho(P::Altura, s),
        "min-width" | "min-inline-size" => tamanho(P::MinLargura, s),
        "max-width" | "max-inline-size" => tamanho(P::MaxLargura, s),
        "min-height" | "min-block-size" => tamanho(P::MinAltura, s),
        "max-height" | "max-block-size" => tamanho(P::MaxAltura, s),
        "margin" => cada(P::Margem, &|p| ler_t(p).map(V::T), s),
        "padding" => cada(P::Preench, &|p| ler_t(p).map(V::T), s),
        "margin-inline" | "margin-block" | "padding-inline" | "padding-block" => {
            let l = lados(v);
            let (a, b) = (l.first().copied().unwrap_or("0"), l.get(1).or(l.first()).copied().unwrap_or("0"));
            let idx: [u8; 2] = if nome.ends_with("inline") { [3, 1] } else { [0, 2] };
            let f: fn(u8) -> P = if nome.starts_with('m') { P::Margem } else { P::Preench };
            if let (Some(x), Some(y)) = (ler_t(a), ler_t(b)) {
                s.push(Decl::D(f(idx[0]), V::T(x)));
                s.push(Decl::D(f(idx[1]), V::T(y)));
            }
        }
        "border" => expandir_borda(None, v, s),
        "border-width" => cada(P::BordaL, &|p| borda_largura(p).map(V::T), s),
        "border-style" => cada(P::BordaEstilo, &|p| borda_estilo(p).map(k), s),
        "border-color" => cada(P::BordaCor, &|p| ler_cor(p).map(V::Cor), s),
        "border-radius" | "-webkit-border-radius" | "-moz-border-radius" => {
            let primeiro = v.split('/').next().unwrap_or(v);
            if let Some(q) = quatro(primeiro.trim()) {
                for i in 0..4 {
                    if let Some(t) = ler_t(q[i]) {
                        s.push(Decl::D(P::Raio(i as u8), V::T(t)));
                    }
                }
            }
        }
        "border-top-left-radius" | "border-start-start-radius" => tamanho(P::Raio(0), s),
        "border-top-right-radius" | "border-start-end-radius" => tamanho(P::Raio(1), s),
        "border-bottom-right-radius" | "border-end-end-radius" => tamanho(P::Raio(2), s),
        "border-bottom-left-radius" | "border-end-start-radius" => tamanho(P::Raio(3), s),
        "top" | "inset-block-start" => tamanho(P::Topo, s),
        "right" | "inset-inline-end" => tamanho(P::Direita, s),
        "bottom" | "inset-block-end" => tamanho(P::Baixo, s),
        "left" | "inset-inline-start" => tamanho(P::Esquerda, s),
        "inset" => {
            if let Some(q) = quatro(v) {
                for (i, p) in [P::Topo, P::Direita, P::Baixo, P::Esquerda].into_iter().enumerate() {
                    if let Some(t) = ler_t(q[i]) {
                        s.push(Decl::D(p, V::T(t)));
                    }
                }
            }
        }
        "z-index" => simples(P::Z, if v == "auto" { Some(V::T(T::Auto)) } else { v.parse::<f32>().ok().map(V::N) }, s),
        "color" => simples(P::Cor, ler_cor(v).map(V::Cor), s),
        "background" => {
            if let Some(g) = global(v) {
                s.push(Decl::D(P::Fundo, g));
            } else {
                // A url pode ter maiúsculas: usa o valor original.
                expandir_fundo(valor.trim(), s);
            }
        }
        "background-color" => simples(P::Fundo, ler_cor(v).map(V::Cor), s),
        "background-image" => simples(P::FundoImg, ler_imagem(dividir_virgulas(valor.trim()).first().copied().unwrap_or("").trim()).map(V::Img), s),
        "background-repeat" => simples(P::FundoRepete, ler_repete(v).map(k), s),
        "background-position" => simples(P::FundoPos, ler_pos_fundo(&lados(v)).map(|(a, b)| V::Par(a, b)), s),
        "background-size" => {
            let l = lados(v);
            let (m, a, b) = ler_tam_fundo(l.first().copied().unwrap_or("auto"), l.get(1).copied());
            s.push(Decl::D(P::FundoTam, V::Transf(a, b, m as f32)));
        }
        "font" => expandir_fonte(v, s),
        "font-size" => simples(P::FonteTam, tamanho_fonte(v), s),
        "font-weight" => simples(P::Negrito, match v { "bold" | "bolder" => Some(k(1)), "normal" | "lighter" => Some(k(0)), _ => v.parse::<f32>().ok().map(|n| k((n >= 600.0) as u8)) }, s),
        "font-style" => simples(P::Italico, Some(k((v.starts_with("italic") || v.starts_with("oblique")) as u8)), s),
        "font-family" => simples(P::Familia, Some(k(eh_mono(v) as u8)), s),
        "line-height" => simples(P::AlturaLinha, altura_linha(v), s),
        "letter-spacing" => simples(P::EspacoLetras, if v == "normal" { Some(V::T(T::Px(0.0))) } else { ler_t(v).map(V::T) }, s),
        "word-spacing" => simples(P::EspacoPalavras, if v == "normal" { Some(V::T(T::Px(0.0))) } else { ler_t(v).map(V::T) }, s),
        "text-align" => simples(P::Alinha, match v { "left" | "start" | "-webkit-left" | "-moz-left" => Some(k(0)), "center" | "-webkit-center" | "-moz-center" => Some(k(1)), "right" | "end" | "-webkit-right" | "-moz-right" => Some(k(2)), "justify" => Some(k(3)), _ => None }, s),
        "text-indent" => tamanho(P::Recuo, s),
        "text-transform" => simples(P::Transforma, match v { "uppercase" => Some(k(1)), "lowercase" => Some(k(2)), "capitalize" => Some(k(3)), "none" => Some(k(0)), _ => None }, s),
        "white-space" | "white-space-collapse" | "text-wrap" | "text-wrap-mode" => simples(P::Branco, match v { "normal" | "wrap" | "collapse" | "balance" | "pretty" => Some(k(0)), "nowrap" => Some(k(1)), "pre" | "preserve" => Some(k(2)), "pre-wrap" | "break-spaces" => Some(k(3)), "pre-line" | "preserve-breaks" => Some(k(4)), _ => None }, s),
        "text-decoration" | "text-decoration-line" | "-webkit-text-decoration" => {
            if let Some(g) = global(v) {
                s.push(Decl::D(P::Decoracao, g));
            } else {
                let m = v.contains("underline") as u8 | (v.contains("line-through") as u8) << 1 | (v.contains("overline") as u8) << 2;
                s.push(Decl::D(P::Decoracao, k(m)));
                if nome != "text-decoration-line" {
                    for p in lados(v) {
                        if let Some(c) = ler_cor(p) {
                            s.push(Decl::D(P::CorDecoracao, V::Cor(c)));
                        }
                    }
                }
            }
        }
        "text-decoration-color" => simples(P::CorDecoracao, ler_cor(v).map(V::Cor), s),
        "vertical-align" => simples(P::AlinhaV, match v { "baseline" => Some(k(0)), "middle" => Some(k(1)), "top" => Some(k(2)), "bottom" => Some(k(3)), "sub" => Some(k(4)), "super" => Some(k(5)), "text-top" => Some(k(6)), "text-bottom" => Some(k(7)), _ => ler_t(v).map(V::T) }, s),
        "word-break" | "overflow-wrap" | "word-wrap" => simples(P::QuebraPalavra, Some(k(matches!(v, "break-all" | "break-word" | "anywhere") as u8)), s),
        "text-overflow" => simples(P::Reticencias, Some(k((v == "ellipsis") as u8)), s),
        "list-style" => {
            for p in lados(v) {
                if let Some(t) = lista_tipo(p) {
                    s.push(Decl::D(P::ListaTipo, k(t)));
                } else if p == "inside" || p == "outside" {
                    s.push(Decl::D(P::ListaPos, k((p == "inside") as u8)));
                }
            }
        }
        "list-style-type" => simples(P::ListaTipo, lista_tipo(v).map(k), s),
        "list-style-position" => simples(P::ListaPos, Some(k((v == "inside") as u8)), s),
        "flex-direction" | "-webkit-flex-direction" => simples(P::FlexDirecao, match v { "row" => Some(k(0)), "row-reverse" => Some(k(1)), "column" => Some(k(2)), "column-reverse" => Some(k(3)), _ => None }, s),
        "flex-wrap" | "-webkit-flex-wrap" => simples(P::FlexQuebra, match v { "nowrap" => Some(k(0)), "wrap" => Some(k(1)), "wrap-reverse" => Some(k(2)), _ => None }, s),
        "flex-flow" => {
            for p in lados(v) {
                interpretar(if p.contains("wrap") { "flex-wrap" } else { "flex-direction" }, p, s);
            }
        }
        "justify-content" | "-webkit-justify-content" => simples(P::Justifica, ler_alinhamento(v).map(k), s),
        "align-content" => simples(P::AlinhaConteudo, ler_alinhamento(v).map(k), s),
        "align-items" | "-webkit-align-items" => simples(P::AlinhaItens, ler_alinha_item(v).map(k), s),
        "align-self" | "-webkit-align-self" => simples(P::AlinhaProprio, ler_alinha_item(v).map(k), s),
        "justify-items" => simples(P::JustificaItens, ler_alinha_item(v).map(k), s),
        "justify-self" => simples(P::JustificaProprio, ler_alinha_item(v).map(k), s),
        "place-items" | "place-content" | "place-self" => {
            let l = lados(v);
            let (a, b) = (l.first().copied().unwrap_or("normal"), l.get(1).or(l.first()).copied().unwrap_or("normal"));
            match nome {
                "place-items" => {
                    interpretar("align-items", a, s);
                    interpretar("justify-items", b, s);
                }
                "place-self" => {
                    interpretar("align-self", a, s);
                    interpretar("justify-self", b, s);
                }
                _ => {
                    interpretar("align-content", a, s);
                    interpretar("justify-content", b, s);
                }
            }
        }
        "flex-grow" | "-webkit-flex-grow" => simples(P::Cresce, v.parse().ok().map(V::N), s),
        "flex-shrink" | "-webkit-flex-shrink" => simples(P::Encolhe, v.parse().ok().map(V::N), s),
        "flex-basis" | "-webkit-flex-basis" => {
            if v == "content" {
                s.push(Decl::D(P::BaseFlex, V::T(T::Max)));
            } else {
                tamanho(P::BaseFlex, s);
            }
        }
        "flex" | "-webkit-flex" | "-ms-flex" => {
            let l = lados(v);
            let (mut g, mut e, mut b) = (0.0, 1.0, T::Auto);
            match l.as_slice() {
                ["none"] => e = 0.0,
                ["auto"] => g = 1.0,
                ["initial"] => {}
                _ => {
                    let mut n = 0;
                    let mut tem_base = false;
                    for p in &l {
                        if let Ok(x) = p.parse::<f32>() {
                            if n == 0 {
                                g = x;
                            } else if n == 1 {
                                e = x;
                            } else {
                                b = T::Px(x);
                                tem_base = true;
                            }
                            n += 1;
                        } else if let Some(t) = ler_t(p) {
                            b = t;
                            tem_base = true;
                        }
                    }
                    if !tem_base {
                        b = T::Px(0.0);
                    }
                }
            }
            s.push(Decl::D(P::Cresce, V::N(g)));
            s.push(Decl::D(P::Encolhe, V::N(e)));
            s.push(Decl::D(P::BaseFlex, V::T(b)));
        }
        "order" => simples(P::Ordem, v.parse().ok().map(V::N), s),
        "gap" | "grid-gap" => {
            let l = lados(v);
            if let Some(a) = l.first().and_then(|x| ler_t(x)) {
                s.push(Decl::D(P::VaoLinha, V::T(a)));
                s.push(Decl::D(P::VaoColuna, V::T(l.get(1).and_then(|x| ler_t(x)).unwrap_or(a))));
            }
        }
        "row-gap" | "grid-row-gap" => tamanho(P::VaoLinha, s),
        "column-gap" | "grid-column-gap" => tamanho(P::VaoColuna, s),
        "grid-template-columns" => simples(P::GridColunas, ler_trilhas(v).map(|t| V::Trilhas(Rc::new(t))), s),
        "grid-template-rows" => simples(P::GridLinhas, ler_trilhas(v).map(|t| V::Trilhas(Rc::new(t))), s),
        "grid-auto-columns" => simples(P::GridAutoColunas, ler_trilhas(v).map(|t| V::Trilhas(Rc::new(t))), s),
        "grid-auto-rows" => simples(P::GridAutoLinhas, ler_trilhas(v).map(|t| V::Trilhas(Rc::new(t))), s),
        "grid-auto-flow" => simples(P::GridFluxo, Some(k(v.contains("column") as u8)), s),
        "grid-template-areas" => {
            let mut linhas = Vec::new();
            for p in v.split('"').skip(1).step_by(2) {
                linhas.push(p.split_whitespace().map(String::from).collect::<Vec<_>>());
            }
            s.push(Decl::D(P::GridAreas, V::Areas(Rc::new(linhas))));
        }
        "grid-template" => {
            if let Some((a, b)) = v.split_once('/') {
                interpretar("grid-template-rows", a.trim(), s);
                interpretar("grid-template-columns", b.trim(), s);
            }
        }
        "grid-column-start" => s.push(Decl::D(P::GridColunaIni, ler_linha_grid(v))),
        "grid-column-end" => s.push(Decl::D(P::GridColunaFim, ler_linha_grid(v))),
        "grid-row-start" => s.push(Decl::D(P::GridLinhaIni, ler_linha_grid(v))),
        "grid-row-end" => s.push(Decl::D(P::GridLinhaFim, ler_linha_grid(v))),
        "grid-column" | "grid-row" => {
            let (a, b) = v.split_once('/').unwrap_or((v, ""));
            let (pi, pf) = if nome == "grid-column" { (P::GridColunaIni, P::GridColunaFim) } else { (P::GridLinhaIni, P::GridLinhaFim) };
            s.push(Decl::D(pi, ler_linha_grid(a)));
            s.push(Decl::D(pf, ler_linha_grid(b)));
        }
        "grid-area" => {
            if v.contains('/') || v.parse::<i32>().is_ok() || v.starts_with("span") {
                let p: Vec<&str> = v.split('/').map(str::trim).collect();
                let pega = |i: usize| ler_linha_grid(p.get(i).copied().unwrap_or(""));
                s.push(Decl::D(P::GridLinhaIni, pega(0)));
                s.push(Decl::D(P::GridColunaIni, pega(1)));
                s.push(Decl::D(P::GridLinhaFim, pega(2)));
                s.push(Decl::D(P::GridColunaFim, pega(3)));
            } else {
                s.push(Decl::D(P::GridArea, V::Txt(Rc::from(v))));
            }
        }
        "opacity" => simples(P::Opacidade, v.strip_suffix('%').map_or_else(|| v.parse().ok(), |x| x.parse::<f32>().ok().map(|n| n / 100.0)).map(V::N), s),
        "box-shadow" | "-webkit-box-shadow" | "-moz-box-shadow" => s.push(Decl::D(P::Sombras, V::Sombras(ler_sombras(v)))),
        "transform" | "-webkit-transform" | "-ms-transform" => s.push(Decl::D(P::Transformacao, ler_transformacao(v))),
        "cursor" => simples(P::Cursor, Some(k((v == "pointer") as u8)), s),
        "pointer-events" => simples(P::SemEventos, Some(k((v == "none") as u8)), s),
        "content" => s.push(Decl::D(P::Conteudo, V::Txt(Rc::from(ler_conteudo(valor.trim()).unwrap_or_else(|| Rc::from("\u{3}")).as_ref())))),
        "border-collapse" => simples(P::ColapsaBordas, Some(k((v == "collapse") as u8)), s),
        "border-spacing" => tamanho(P::EspacoBordas, s),
        "table-layout" => simples(P::TabelaFixa, Some(k((v == "fixed") as u8)), s),
        "mask" | "mask-image" | "-webkit-mask" | "-webkit-mask-image" => s.push(Decl::D(P::Mascara, k((v != "none") as u8))),
        "object-fit" => simples(P::AjusteObjeto, match v { "fill" => Some(k(0)), "contain" | "scale-down" => Some(k(1)), "cover" => Some(k(2)), "none" => Some(k(3)), _ => None }, s),
        "aspect-ratio" => {
            let t = v.split_whitespace().filter(|x| *x != "auto").collect::<Vec<_>>().join("");
            let r = match t.split_once('/') {
                Some((a, b)) => a.parse::<f32>().ok().zip(b.parse::<f32>().ok()).map(|(x, y)| x / y),
                None => t.parse().ok(),
            };
            s.push(Decl::D(P::Proporcao, V::N(r.unwrap_or(0.0))));
        }
        _ => {
            // margin-top, padding-left, border-bottom, border-left-color...
            if let Some(l) = nome.strip_prefix("margin-").and_then(lado_de) {
                tamanho(P::Margem(l), s);
            } else if let Some(l) = nome.strip_prefix("padding-").and_then(lado_de) {
                tamanho(P::Preench(l), s);
            } else if let Some(r) = nome.strip_prefix("border-") {
                if let Some(l) = lado_de(r) {
                    expandir_borda(Some(l), v, s);
                } else if let Some(a) = r.strip_suffix("-width").and_then(lado_de) {
                    if let Some(t) = borda_largura(v) {
                        s.push(Decl::D(P::BordaL(a), V::T(t)));
                    }
                } else if let Some(a) = r.strip_suffix("-style").and_then(lado_de) {
                    if let Some(t) = borda_estilo(v) {
                        s.push(Decl::D(P::BordaEstilo(a), k(t)));
                    }
                } else if let Some(a) = r.strip_suffix("-color").and_then(lado_de) {
                    if let Some(t) = ler_cor(v) {
                        s.push(Decl::D(P::BordaCor(a), V::Cor(t)));
                    }
                }
            }
        }
    }
}

fn global(v: &str) -> Option<V> {
    match v {
        "inherit" => Some(V::Herdar),
        "initial" | "revert" | "revert-layer" => Some(V::Inicial),
        _ => None,
    }
}

const DISPLAYS: [Display; 18] = [
    Display::Nenhum, Display::Bloco, Display::Inline, Display::InlineBloco, Display::Flex, Display::InlineFlex, Display::Grid, Display::InlineGrid,
    Display::Tabela, Display::InlineTabela, Display::GrupoLinhas, Display::Linha, Display::Celula, Display::Legenda, Display::GrupoColunas,
    Display::Coluna, Display::ItemLista, Display::Conteudo,
];

// -----------------------------------------------------------------------------
// A folha de estilo
// -----------------------------------------------------------------------------

struct Bloco {
    decls: Vec<(Decl, bool)>,
}

struct Regra {
    seletor: Seletor,
    /// Índices em `Folha::blocos` e `Folha::condicoes` (u16::MAX = sem @media).
    bloco: u32,
    midia: u16,
}

/// Uma folha de estilo já lida.
#[derive(Default)]
pub struct Folha {
    regras: Vec<Regra>,
    blocos: Vec<Bloco>,
    condicoes: Vec<String>,
    /// Os endereços de `@import`, a baixar.
    pub importacoes: Vec<String>,
}

fn sem_comentarios(texto: &str) -> String {
    let mut saida = String::with_capacity(texto.len());
    let mut resto = texto;
    while let Some(i) = resto.find("/*") {
        saida.push_str(&resto[..i]);
        resto = match resto[i + 2..].find("*/") {
            Some(f) => &resto[i + 2 + f + 2..],
            None => "",
        };
    }
    saida.push_str(resto);
    saida
}

/// O índice do `}` que fecha o bloco que começa em `inicio` (que é o `{`).
fn fim_do_bloco(t: &[u8], inicio: usize) -> usize {
    let mut profundidade = 0;
    let mut aspas = 0u8;
    let mut i = inicio;
    while i < t.len() {
        let c = t[i];
        if aspas != 0 {
            if c == b'\\' {
                i += 1;
            } else if c == aspas {
                aspas = 0;
            }
        } else {
            match c {
                b'"' | b'\'' => aspas = c,
                b'{' => profundidade += 1,
                b'}' => {
                    profundidade -= 1;
                    if profundidade == 0 {
                        return i;
                    }
                }
                _ => {}
            }
        }
        i += 1;
    }
    t.len()
}

/// "a, b:not(.c, .d)": vírgulas fora de parênteses separam.
fn dividir_virgulas(t: &str) -> Vec<&str> {
    let mut v = Vec::new();
    let (mut inicio, mut prof, mut aspa) = (0, 0, None);
    for (i, ch) in t.char_indices() {
        if let Some(a) = aspa {
            if ch == a {
                aspa = None;
            }
            continue;
        }
        match ch {
            '"' | '\'' => aspa = Some(ch),
            '(' | '[' => prof += 1,
            ')' | ']' => prof -= 1,
            ',' if prof == 0 => {
                v.push(&t[inicio..i]);
                inicio = i + 1;
            }
            _ => {}
        }
    }
    v.push(&t[inicio..]);
    v
}

/// As declarações de um bloco ("cor: azul; margem: 0 !important").
pub fn ler_declaracoes(texto: &str) -> Vec<(Decl, bool)> {
    let mut saida = Vec::new();
    let mut atual_ini = 0;
    let (mut parenteses, mut aspas) = (0, None);
    let terminar = |d: &str, saida: &mut Vec<(Decl, bool)>| {
        if let Some((nome, valor)) = d.split_once(':') {
            let nome_t = nome.trim();
            let nome = if nome_t.starts_with("--") { nome_t.to_string() } else { nome_t.to_ascii_lowercase() };
            let mut valor = valor.trim();
            let importante = valor.len() >= 10 && valor[valor.len() - 10..].eq_ignore_ascii_case("!important");
            if importante {
                valor = valor[..valor.len() - 10].trim();
            }
            if !nome.is_empty() && !valor.is_empty() {
                let mut v = Vec::new();
                interpretar(&nome, valor, &mut v);
                saida.extend(v.into_iter().map(|x| (x, importante)));
            }
        }
    };
    for (i, ch) in texto.char_indices() {
        match ch {
            '"' | '\'' if aspas.is_none() => aspas = Some(ch),
            c if Some(c) == aspas => aspas = None,
            '(' if aspas.is_none() => parenteses += 1,
            ')' if aspas.is_none() => parenteses -= 1,
            ';' if aspas.is_none() && parenteses <= 0 => {
                terminar(&texto[atual_ini..i], &mut saida);
                atual_ini = i + 1;
            }
            _ => {}
        }
    }
    terminar(&texto[atual_ini..], &mut saida);
    saida
}

impl Folha {
    /// Alguma regra com `:hover` pode ligar para este elemento?
    pub fn sobre_importa(&self, d: &Dom, n: NoId) -> bool {
        self.regras.iter().any(|r| r.seletor.partes.iter().any(|(c, _)| composto_sobre(c, d, n)))
    }

    pub fn ler(texto: &str) -> Folha {
        let mut f = Folha::default();
        let texto = sem_comentarios(texto);
        f.ler_regras(&texto, u16::MAX, None);
        f
    }

    pub fn regras(&self) -> usize {
        self.regras.len()
    }

    fn ler_regras(&mut self, texto: &str, midia: u16, pai: Option<&str>) {
        let t = texto.as_bytes();
        let mut i = 0;
        while i < t.len() {
            while i < t.len() && (t[i].is_ascii_whitespace() || t[i] == b';' || t[i] == b'}') {
                i += 1;
            }
            if i >= t.len() {
                break;
            }
            let chave = texto[i..].find('{').map(|p| p + i);
            let ponto_e_virgula = texto[i..].find(';').map(|p| p + i);
            if t[i] == b'@' {
                // @import, @charset...: até o ";" (se vier antes de um "{").
                let curto = match (chave, ponto_e_virgula) {
                    (Some(c), Some(p)) => (p < c).then_some(p),
                    (None, Some(p)) => Some(p),
                    (None, None) => break,
                    _ => None,
                };
                if let Some(p) = curto {
                    let cabeca = texto[i + 1..p].trim();
                    if let Some(r) = cabeca.strip_prefix("import") {
                        let r = r.trim();
                        let url = ler_url(r).map(str::to_string).or_else(|| {
                            let a = r.chars().next().filter(|c| *c == '"' || *c == '\'')?;
                            r[1..].split(a).next().map(str::to_string)
                        });
                        if let Some(u) = url {
                            self.importacoes.push(u);
                        }
                    }
                    i = p + 1;
                    continue;
                }
                let c = chave.unwrap();
                let cabeca = texto[i + 1..c].trim();
                let fim = fim_do_bloco(t, c);
                let dentro = &texto[c + 1..fim.min(texto.len())];
                let (nome, condicao) = cabeca.split_once(|ch: char| ch.is_whitespace() || ch == '(').map_or((cabeca, ""), |(n, _)| (n, &cabeca[n.len()..]));
                match nome.to_ascii_lowercase().as_str() {
                    "media" => {
                        let n = self.condicoes.len().min(u16::MAX as usize - 1) as u16;
                        let condicao = condicao.trim().to_ascii_lowercase();
                        // Um @media dentro de outro: as duas condições.
                        let condicao = if midia != u16::MAX { format!("{} and {}", self.condicoes[midia as usize], condicao) } else { condicao };
                        self.condicoes.push(condicao);
                        self.ler_regras(dentro, n, pai);
                    }
                    "supports" => {
                        // Sabemos o básico; "not (...)" conta como falso.
                        if !condicao.trim_start().to_ascii_lowercase().starts_with("not") {
                            self.ler_regras(dentro, midia, pai);
                        }
                    }
                    "layer" | "document" | "-moz-document" | "container" | "scope" => self.ler_regras(dentro, midia, pai),
                    _ => {} // @font-face, @keyframes, @page...
                }
                i = fim + 1;
                continue;
            }
            let Some(c) = chave else { break };
            let fim = fim_do_bloco(t, c);
            let seletores = texto[i..c].trim();
            let corpo = &texto[c + 1..fim.min(texto.len())];
            i = fim + 1;
            // CSS aninhado: o seletor completo, com `&`.
            let completo: String;
            let seletores = match pai {
                Some(p) => {
                    let mut r = Vec::new();
                    for pp in dividir_virgulas(p) {
                        for f in dividir_virgulas(seletores) {
                            let f = f.trim();
                            r.push(if f.contains('&') { f.replace('&', pp.trim()) } else { format!("{} {}", pp.trim(), f) });
                        }
                    }
                    completo = r.join(",");
                    completo.as_str()
                }
                None => seletores,
            };
            let (proprias, aninhadas) = separar_aninhadas(corpo);
            let decls = ler_declaracoes(&proprias);
            if !decls.is_empty() {
                let bloco = self.blocos.len() as u32;
                let mut algum = false;
                for s in dividir_virgulas(seletores) {
                    if let Some(seletor) = ler_seletor(s.trim()) {
                        self.regras.push(Regra { seletor, bloco, midia });
                        algum = true;
                    }
                }
                if algum {
                    self.blocos.push(Bloco { decls });
                }
            }
            if !aninhadas.is_empty() {
                self.ler_regras(&aninhadas, midia, Some(seletores));
            }
        }
    }
}

fn composto_sobre(c: &Composto, d: &Dom, n: NoId) -> bool {
    let tem = c.pseudos.iter().any(|p| match p {
        Pseudo::Sobre => true,
        Pseudo::Nao(l) | Pseudo::Algum(l) | Pseudo::Tem(l) => l.iter().any(|s| s.partes.iter().any(|(x, _)| x.pseudos.iter().any(|q| matches!(q, Pseudo::Sobre)))),
        _ => false,
    });
    if !tem {
        return false;
    }
    c.tag.as_ref().is_none_or(|t| t == d.nome(n)) && c.id.as_ref().is_none_or(|i| d.atributo(n, "id") == Some(i.as_str())) && c.classes.iter().all(|k| d.tem_classe(n, k))
}

/// Divide o corpo de uma regra em declarações e regras aninhadas.
fn separar_aninhadas(corpo: &str) -> (String, String) {
    if !corpo.contains('{') {
        return (corpo.to_string(), String::new());
    }
    let (mut proprias, mut aninhadas) = (String::new(), String::new());
    let t = corpo.as_bytes();
    let mut i = 0;
    let mut ini = 0;
    while i < t.len() {
        match t[i] {
            b';' => {
                proprias.push_str(&corpo[ini..=i]);
                ini = i + 1;
            }
            b'{' => {
                let fim = fim_do_bloco(t, i).min(t.len() - 1);
                aninhadas.push_str(&corpo[ini..=fim]);
                i = fim;
                ini = i + 1;
            }
            _ => {}
        }
        i += 1;
    }
    proprias.push_str(&corpo[ini.min(corpo.len())..]);
    (proprias, aninhadas)
}

/// Uma condição de `@media` vale para uma tela de `largura` pixels?
pub fn midia_vale(condicao: &str, largura: i32) -> bool {
    midia_vale_tela(condicao, largura, 700)
}

pub fn midia_vale_tela(condicao: &str, largura: i32, altura: i32) -> bool {
    condicao.split(',').any(|alternativa| {
        let alternativa = alternativa.trim();
        let (negar, alternativa) = match alternativa.strip_prefix("not ") {
            Some(r) => (true, r),
            None => (false, alternativa),
        };
        let vale = alternativa.split(" and ").all(|parte| {
            let parte = parte.trim().trim_start_matches("only ").trim();
            if let Some(dentro) = parte.strip_prefix('(').and_then(|p| p.strip_suffix(')')) {
                let (nome, valor) = dentro.split_once(':').map_or((dentro.trim(), ""), |(n, v)| (n.trim(), v.trim()));
                let px = || pixels(valor, 16.0).unwrap_or(0);
                // A sintaxe nova: (width >= 600px), (600px <= width)
                if nome == dentro.trim() {
                    for (op, f) in [(">=", 0), ("<=", 1), (">", 2), ("<", 3)] {
                        if let Some((a, b)) = dentro.split_once(op) {
                            let (a, b) = (a.trim(), b.trim());
                            let (dim, val, inverte) = if a == "width" || a == "height" { (a, b, false) } else { (b, a, true) };
                            let d = if dim == "height" { altura } else { largura };
                            let p = pixels(val, 16.0).unwrap_or(0);
                            let f = if inverte { [1, 0, 3, 2][f] } else { f };
                            return match f {
                                0 => d >= p,
                                1 => d <= p,
                                2 => d > p,
                                _ => d < p,
                            };
                        }
                    }
                }
                match nome {
                    "max-width" | "max-device-width" => largura <= px(),
                    "min-width" | "min-device-width" => largura >= px(),
                    "max-height" => altura <= px(),
                    "min-height" => altura >= px(),
                    "width" => largura == px(),
                    "height" => altura == px(),
                    "orientation" => (valor == "landscape") == (largura >= altura),
                    "prefers-color-scheme" => valor == "light",
                    "hover" | "any-hover" => valor == "hover",
                    "pointer" | "any-pointer" => valor == "fine",
                    "prefers-reduced-motion" => valor == "reduce",
                    "prefers-contrast" | "forced-colors" | "inverted-colors" => valor == "none" || valor == "no-preference",
                    "color" | "scripting" => true,
                    "min-resolution" | "-webkit-min-device-pixel-ratio" | "min--moz-device-pixel-ratio" => valor.trim_end_matches("dppx").parse::<f32>().is_ok_and(|v| v <= 1.0),
                    "max-resolution" | "-webkit-max-device-pixel-ratio" => true,
                    _ => false,
                }
            } else {
                matches!(parte, "screen" | "all" | "")
            }
        });
        vale != negar
    })
}

// -----------------------------------------------------------------------------
// A folha padrão do navegador
// -----------------------------------------------------------------------------

/// O estilo de cada elemento antes das folhas da página.
pub const FOLHA_PADRAO: &str = r#"
html, address, blockquote, body, center, dialog, div, dl, dt, dd, fieldset, figcaption, figure, footer, form, h1, h2, h3, h4, h5, h6, header, hgroup, hr, listing, main, menu, nav, ol, p, plaintext, pre, search, section, summary, ul, xmp, article, aside, details, dir, legend, optgroup { display: block }
head, link, meta, script, style, title, base, template, area, param, source, track, datalist, noembed, noframes, noscript, dialog:not([open]), [hidden] { display: none }
option { display: block }
body { margin: 8px }
p, dl, multicol { margin: 1em 0 }
blockquote, figure { margin: 1em 40px }
pre, listing, plaintext, xmp { margin: 1em 0; white-space: pre; font-family: monospace; font-size: 13px }
h1 { font-size: 2em; margin: 0.67em 0; font-weight: bold }
h2 { font-size: 1.5em; margin: 0.83em 0; font-weight: bold }
h3 { font-size: 1.17em; margin: 1em 0; font-weight: bold }
h4 { margin: 1.33em 0; font-weight: bold }
h5 { font-size: 0.83em; margin: 1.67em 0; font-weight: bold }
h6 { font-size: 0.67em; margin: 2.33em 0; font-weight: bold }
article h1, section h1, aside h1, nav h1 { font-size: 1.5em; margin: 0.83em 0 }
ul, menu, dir { margin: 1em 0; padding-left: 40px; list-style-type: disc }
ol { margin: 1em 0; padding-left: 40px; list-style-type: decimal }
ul ul, ol ul, menu ul, ul menu { list-style-type: circle; margin-top: 0; margin-bottom: 0 }
ul ol, ol ol { margin-top: 0; margin-bottom: 0 }
ul ul ul, ol ul ul, ul ol ul { list-style-type: square }
li { display: list-item }
dd { margin-left: 40px }
hr { margin: 0.5em auto; border: 1px inset #808080; color: gray }
b, strong, th, dt, summary, legend { font-weight: bold }
i, em, cite, dfn, var, address { font-style: italic }
code, kbd, samp, tt { font-family: monospace; font-size: 13px }
u, ins { text-decoration: underline }
s, strike, del { text-decoration: line-through }
small { font-size: smaller }
big { font-size: larger }
sub { vertical-align: sub; font-size: smaller }
sup { vertical-align: super; font-size: smaller }
mark { background-color: yellow; color: black }
center { text-align: center }
a:link { color: #0000EE; text-decoration: underline; cursor: pointer }
a:visited { color: #551A8B; text-decoration: underline; cursor: pointer }
table { display: table; border-spacing: 2px; border-collapse: separate; box-sizing: border-box; text-align: left }
caption { display: table-caption; text-align: center }
colgroup { display: table-column-group }
col { display: table-column }
thead, tbody, tfoot { display: table-row-group; vertical-align: middle }
tr { display: table-row }
td, th { display: table-cell; padding: 1px; vertical-align: inherit }
th { text-align: center }
fieldset { margin: 0 2px; padding: 0.35em 0.75em 0.625em; border: 2px groove #c0c0c0 }
legend { padding: 0 2px }
img, video, canvas, svg, iframe, object, embed { display: inline-block }
input, select, textarea, button { display: inline-block; font-size: 13.33px; text-align: left }
input[type=hidden] { display: none }
button, input[type=submit], input[type=button], input[type=reset], input[type=file] { text-align: center; padding: 3px 10px; background-color: #e9e9ed; border: 1px solid #8f8f9d; border-radius: 3px; color: #000; cursor: pointer }
button:hover, input[type=submit]:hover, input[type=button]:hover, input[type=reset]:hover { background-color: #d0d0d7 }
button:active, input[type=submit]:active, input[type=button]:active, input[type=reset]:active { background-color: #b1b1b9 }
details:not([open]) > *:not(summary) { display: none }
q::before { content: "\201C" }
q::after { content: "\201D" }
nobr { white-space: nowrap }
"#;

// -----------------------------------------------------------------------------
// A cascata
// -----------------------------------------------------------------------------

/// O estilo de cada elemento (pelo número do nó) e dos pseudo-elementos.
#[derive(Default)]
pub struct Calculado {
    pub estilos: BTreeMap<NoId, Rc<Estilo>>,
    pub antes: BTreeMap<NoId, Rc<Estilo>>,
    pub depois: BTreeMap<NoId, Rc<Estilo>>,
}

struct Indice {
    por_id: BTreeMap<String, Vec<(u16, u32)>>,
    por_classe: BTreeMap<String, Vec<(u16, u32)>>,
    por_tag: BTreeMap<String, Vec<(u16, u32)>>,
    outras: Vec<(u16, u32)>,
}

struct Ctx<'a> {
    c: Casador<'a>,
    folhas: &'a [&'a Folha],
    indice: Indice,
    raiz_fonte: f32,
    vw: f32,
    vh: f32,
    saida: Calculado,
}

/// O atributo `style=""`: as declarações dele.
pub fn estilo_inline(texto: &str) -> Vec<(Decl, bool)> {
    ler_declaracoes(texto)
}

/// Os atributos antigos do HTML, como declarações de CSS.
fn dicas_html(d: &Dom, n: NoId, s: &mut Vec<(Decl, bool)>) {
    if d.atributos(n).is_empty() {
        return;
    }
    let nome = d.nome(n);
    let mut pon = |prop: &str, valor: &str| {
        let mut v = Vec::new();
        interpretar(prop, valor, &mut v);
        s.extend(v.into_iter().map(|x| (x, false)));
    };
    let tam_attr = |v: &str| -> String {
        let v = v.trim();
        if v.ends_with('%') { v.to_string() } else { format!("{}px", v.trim_end_matches("px")) }
    };
    for (k, v) in d.atributos(n) {
        match k.as_str() {
            "bgcolor" => pon("background-color", v),
            "background" if matches!(nome, "body" | "table" | "td" | "th") => pon("background-image", &format!("url({})", v)),
            "color" if nome == "font" => pon("color", v),
            "text" if nome == "body" => pon("color", v),
            "face" if nome == "font" => pon("font-family", v),
            "size" if nome == "font" => {
                let tam = match v.trim() {
                    "1" | "-2" => "x-small",
                    "2" | "-1" => "small",
                    "3" | "+0" => "medium",
                    "4" | "+1" => "large",
                    "5" | "+2" => "x-large",
                    "6" | "+3" => "xx-large",
                    "7" | "+4" => "xxx-large",
                    _ => "medium",
                };
                pon("font-size", tam);
            }
            "width" if matches!(nome, "img" | "table" | "td" | "th" | "col" | "hr" | "video" | "canvas" | "iframe" | "embed" | "object" | "svg" | "applet") => pon("width", &tam_attr(v)),
            "height" if matches!(nome, "img" | "table" | "td" | "th" | "tr" | "video" | "canvas" | "iframe" | "embed" | "object" | "svg" | "applet") => pon("height", &tam_attr(v)),
            "align" => match nome {
                "img" | "iframe" | "object" | "table" if v.eq_ignore_ascii_case("left") || v.eq_ignore_ascii_case("right") => pon("float", &v.to_ascii_lowercase()),
                "table" if v.eq_ignore_ascii_case("center") => {
                    pon("margin-left", "auto");
                    pon("margin-right", "auto");
                }
                "td" | "th" | "tr" | "div" | "p" | "h1" | "h2" | "h3" | "h4" | "h5" | "h6" | "thead" | "tbody" | "tfoot" | "col" | "caption" => pon("text-align", &v.to_ascii_lowercase()),
                _ => {}
            },
            "valign" if matches!(nome, "td" | "th" | "tr" | "tbody" | "thead" | "tfoot") => pon("vertical-align", &v.to_ascii_lowercase()),
            "nowrap" if matches!(nome, "td" | "th") => pon("white-space", "nowrap"),
            "border" if nome == "table" && v.trim() != "0" => {
                let t = tam_attr(v);
                pon("border", &format!("{} outset #808080", t));
            }
            "border" if nome == "img" => pon("border", &format!("{} solid", tam_attr(v))),
            "cellspacing" if nome == "table" => pon("border-spacing", &tam_attr(v)),
            "hspace" if nome == "img" => {
                pon("margin-left", &tam_attr(v));
                pon("margin-right", &tam_attr(v));
            }
            "vspace" if nome == "img" => {
                pon("margin-top", &tam_attr(v));
                pon("margin-bottom", &tam_attr(v));
            }
            "hidden" => pon("display", "none"),
            _ => {}
        }
    }
}

fn resolver_t(t: T, e: &Estilo, ctx: &Ctx) -> Tam {
    resolver_t_com(t, e.fonte, ctx)
}

fn resolver_t_com(t: T, fonte: f32, ctx: &Ctx) -> Tam {
    match t {
        T::Auto => Tam::Auto,
        T::Px(x) => Tam::Px(x),
        T::Em(x) => Tam::Px(x * fonte),
        T::Rem(x) => Tam::Px(x * ctx.raiz_fonte),
        T::Pct(x) => Tam::Pct(x),
        T::Vw(x) => Tam::Px(x * ctx.vw / 100.0),
        T::Vh(x) => Tam::Px(x * ctx.vh / 100.0),
        T::Soma(v) => {
            let px = v[0] + v[1] * fonte + v[2] * ctx.raiz_fonte + v[3] * ctx.vw / 100.0 + v[4] * ctx.vh / 100.0;
            if v[5] == 0.0 { Tam::Px(px) } else { Tam::PxPct(px, v[5]) }
        }
        T::Min => Tam::MinConteudo,
        T::Max => Tam::MaxConteudo,
        T::Ajusta => Tam::AjustaConteudo,
    }
}

fn px_de(t: Tam) -> f32 {
    match t {
        Tam::Px(x) => x,
        _ => 0.0,
    }
}

fn aplicar(e: &mut Estilo, p: P, v: &V, pai: &Estilo, ctx: &Ctx) {
    let i0 = Estilo::inicial;
    macro_rules! tam {
        ($campo:expr, $ini:expr) => {
            match v {
                V::T(t) => $campo = resolver_t(*t, e, ctx),
                V::Inicial => $campo = $ini,
                _ => {}
            }
        };
    }
    macro_rules! herda {
        ($campo:ident) => {
            match v {
                V::Herdar => {
                    e.$campo = pai.$campo.clone();
                    return;
                }
                V::Inicial => {
                    e.$campo = i0().$campo;
                    return;
                }
                _ => {}
            }
        };
    }
    macro_rules! chave {
        ($campo:ident, $t:ty) => {{
            herda!($campo);
            if let V::K(x) = v {
                e.$campo = *x as $t;
            }
        }};
    }
    macro_rules! logico {
        ($campo:ident) => {{
            herda!($campo);
            if let V::K(x) = v {
                e.$campo = *x != 0;
            }
        }};
    }
    match p {
        P::Display => {
            herda!(display);
            if let V::K(x) = v {
                e.display = DISPLAYS[*x as usize];
            }
        }
        P::Posicao => {
            herda!(posicao);
            if let V::K(x) = v {
                e.posicao = [Posicao::Estatica, Posicao::Relativa, Posicao::Absoluta, Posicao::Fixa, Posicao::Grudada][*x as usize];
            }
        }
        P::Flutua => chave!(flutua, u8),
        P::Limpa => chave!(limpa, u8),
        P::Visibilidade => {
            herda!(visivel);
            if let V::K(x) = v {
                e.visivel = *x != 0;
            }
        }
        P::Overflow => match v {
            V::K(0) => {
                e.recorta = false;
                e.overflow_rola = false;
            }
            V::K(1) => {
                e.recorta = true;
                e.overflow_rola = false;
            }
            V::K(_) => {
                e.recorta = false;
                e.overflow_rola = true;
            }
            _ => {}
        },
        P::CaixaBorda => logico!(caixa_borda),
        P::Largura => {
            herda!(largura);
            tam!(e.largura, Tam::Auto)
        }
        P::Altura => {
            herda!(altura);
            tam!(e.altura, Tam::Auto)
        }
        P::MinLargura => tam!(e.min_largura, Tam::Auto),
        P::MaxLargura => tam!(e.max_largura, Tam::Auto),
        P::MinAltura => tam!(e.min_altura, Tam::Auto),
        P::MaxAltura => tam!(e.max_altura, Tam::Auto),
        P::Margem(i) => tam!(e.margem[i as usize], Tam::Px(0.0)),
        P::Preench(i) => tam!(e.preench[i as usize], Tam::Px(0.0)),
        P::BordaL(i) => match v {
            V::T(t) => e.borda[i as usize] = px_de(resolver_t(*t, e, ctx)),
            V::Inicial => e.borda[i as usize] = 3.0,
            _ => {}
        },
        P::BordaCor(i) => {
            if let V::Cor(c) = v {
                e.borda_cor[i as usize] = *c;
            }
        }
        P::BordaEstilo(i) => {
            if let V::K(x) = v {
                e.borda_estilo[i as usize] = *x;
            }
        }
        P::Raio(i) => {
            if let V::T(t) = v {
                e.raio[i as usize] = match resolver_t(*t, e, ctx) {
                    Tam::Px(x) => x,
                    // 50% é o caso comum (um círculo): vale como "muito".
                    Tam::Pct(x) => -x,
                    _ => 0.0,
                };
            }
        }
        P::Topo => tam!(e.topo, Tam::Auto),
        P::Direita => tam!(e.direita, Tam::Auto),
        P::Baixo => tam!(e.baixo, Tam::Auto),
        P::Esquerda => tam!(e.esquerda, Tam::Auto),
        P::Z => match v {
            V::N(x) => e.z = Some(*x as i32),
            V::T(_) | V::Inicial => e.z = None,
            _ => {}
        },
        P::Cor => match v {
            V::Cor(COR_ATUAL) => e.cor = pai.cor,
            V::Cor(c) => e.cor = *c,
            V::Herdar => e.cor = pai.cor,
            V::Inicial => e.cor = 0xFF000000,
            _ => {}
        },
        P::Fundo => match v {
            V::Cor(c) => e.fundo = *c,
            V::Herdar => e.fundo = pai.fundo,
            V::Inicial => {
                e.fundo = 0;
                e.fundo_img = None;
            }
            _ => {}
        },
        P::FundoImg => match v {
            V::Img(i) => e.fundo_img = i.clone(),
            V::Herdar => e.fundo_img = pai.fundo_img.clone(),
            V::Inicial => e.fundo_img = None,
            _ => {}
        },
        P::FundoRepete => chave!(fundo_repete, u8),
        P::FundoTam => {
            if let V::Transf(a, b, m) = v {
                e.fundo_tam = (*m as u8, resolver_t(*a, e, ctx), resolver_t(*b, e, ctx));
            }
        }
        P::FundoPos => {
            if let V::Par(a, b) = v {
                e.fundo_pos = (resolver_t(*a, e, ctx), resolver_t(*b, e, ctx));
            }
        }
        P::FonteTam => {}
        P::Negrito => logico!(negrito),
        P::Italico => logico!(italico),
        P::Familia => logico!(mono),
        P::AlturaLinha => match v {
            V::N(x) => e.altura_linha = *x,
            V::T(t) => {
                e.altura_linha = match resolver_t(*t, e, ctx) {
                    Tam::Px(x) => -x,
                    Tam::Pct(x) => -(x * e.fonte / 100.0),
                    _ => 0.0,
                }
            }
            V::Herdar => e.altura_linha = pai.altura_linha,
            V::Inicial => e.altura_linha = 0.0,
            _ => {}
        },
        P::EspacoLetras => {
            if let V::T(t) = v {
                e.espaco_letras = px_de(resolver_t(*t, e, ctx));
            }
        }
        P::EspacoPalavras => {
            if let V::T(t) = v {
                e.espaco_palavras = px_de(resolver_t(*t, e, ctx));
            }
        }
        P::Alinha => chave!(alinha, u8),
        P::Recuo => tam!(e.recuo, Tam::Px(0.0)),
        P::Transforma => chave!(transforma, u8),
        P::Branco => chave!(branco, u8),
        P::Decoracao => match v {
            V::K(m) => {
                // A decoração soma com a do pai (um `<a>` num texto sublinhado).
                // "none" num elemento comum tira só a dele: o herdado já veio
                // do pai, e os navegadores não conseguem "tirar" o do pai; aqui
                // tiramos, que é o que a página quer em quase todo caso.
                e.sublinhado = m & 1 != 0;
                e.riscado = m & 2 != 0;
                e.sobrelinha = m & 4 != 0;
            }
            V::Herdar => {
                e.sublinhado = pai.sublinhado;
                e.riscado = pai.riscado;
                e.sobrelinha = pai.sobrelinha;
            }
            V::Inicial => {
                e.sublinhado = false;
                e.riscado = false;
                e.sobrelinha = false;
            }
            _ => {}
        },
        P::CorDecoracao => {
            if let V::Cor(c) = v {
                e.cor_decoracao = if *c == COR_ATUAL { None } else { Some(*c) };
            }
        }
        P::AlinhaV => match v {
            V::K(x) => {
                e.alinha_v = *x;
                e.desloca_v = 0.0;
            }
            V::T(t) => {
                e.alinha_v = 0;
                e.desloca_v = match resolver_t(*t, e, ctx) {
                    Tam::Px(x) => x,
                    Tam::Pct(x) => x * e.fonte * 1.2 / 100.0,
                    _ => 0.0,
                };
            }
            V::Herdar => {
                e.alinha_v = pai.alinha_v;
                e.desloca_v = pai.desloca_v;
            }
            _ => {}
        },
        P::QuebraPalavra => logico!(quebra_palavra),
        P::Reticencias => logico!(reticencias),
        P::ListaTipo => chave!(lista_tipo, u8),
        P::ListaPos => logico!(lista_dentro),
        P::FlexDirecao => chave!(flex_direcao, u8),
        P::FlexQuebra => chave!(flex_quebra, u8),
        P::Justifica => chave!(justifica, u8),
        P::AlinhaItens => chave!(alinha_itens, u8),
        P::AlinhaProprio => chave!(alinha_proprio, u8),
        P::AlinhaConteudo => chave!(alinha_conteudo, u8),
        P::JustificaItens => chave!(justifica_itens, u8),
        P::JustificaProprio => chave!(justifica_proprio, u8),
        P::Cresce => {
            if let V::N(x) = v {
                e.cresce = *x;
            }
        }
        P::Encolhe => {
            if let V::N(x) = v {
                e.encolhe = *x;
            }
        }
        P::BaseFlex => tam!(e.base_flex, Tam::Auto),
        P::Ordem => {
            if let V::N(x) = v {
                e.ordem = *x as i32;
            }
        }
        P::VaoLinha => tam!(e.vao_linha, Tam::Px(0.0)),
        P::VaoColuna => tam!(e.vao_coluna, Tam::Px(0.0)),
        P::GridColunas => {
            if let V::Trilhas(t) = v {
                e.grid_colunas = t.clone();
            }
        }
        P::GridLinhas => {
            if let V::Trilhas(t) = v {
                e.grid_linhas = t.clone();
            }
        }
        P::GridAutoColunas => {
            if let V::Trilhas(t) = v {
                e.grid_auto_colunas = t.clone();
            }
        }
        P::GridAutoLinhas => {
            if let V::Trilhas(t) = v {
                e.grid_auto_linhas = t.clone();
            }
        }
        P::GridFluxo => logico!(grid_fluxo_coluna),
        P::GridAreas => {
            if let V::Areas(a) = v {
                e.grid_areas = a.clone();
            }
        }
        P::GridColunaIni => {
            if let V::Linha(l) = v {
                e.grid_coluna.0 = *l;
            }
        }
        P::GridColunaFim => {
            if let V::Linha(l) = v {
                e.grid_coluna.1 = *l;
            }
        }
        P::GridLinhaIni => {
            if let V::Linha(l) = v {
                e.grid_linha.0 = *l;
            }
        }
        P::GridLinhaFim => {
            if let V::Linha(l) = v {
                e.grid_linha.1 = *l;
            }
        }
        P::GridArea => {
            if let V::Txt(t) = v {
                e.grid_area = Some(t.clone());
            }
        }
        P::Opacidade => {
            if let V::N(x) = v {
                e.opacidade = x.clamp(0.0, 1.0);
            }
        }
        P::Sombras => {
            if let V::Sombras(s) = v {
                e.sombras = s.clone();
            }
        }
        P::Transformacao => {
            if let V::Transf(x, y, s) = v {
                e.desloca_x = resolver_t(*x, e, ctx);
                e.desloca_y = resolver_t(*y, e, ctx);
                e.escala = *s;
            }
        }
        P::Cursor => logico!(cursor_mao),
        P::SemEventos => logico!(sem_eventos),
        P::Conteudo => {
            if let V::Txt(t) = v {
                e.conteudo = if &**t == "\u{3}" { None } else { Some(t.clone()) };
            }
        }
        P::ColapsaBordas => logico!(colapsa_bordas),
        P::EspacoBordas => {
            if let V::T(t) = v {
                e.espaco_bordas = px_de(resolver_t(*t, e, ctx));
            }
        }
        P::TabelaFixa => logico!(tabela_fixa),
        P::AjusteObjeto => chave!(ajuste_objeto, u8),
        P::Mascara => {
            if let V::K(x) = v {
                e.mascara = *x != 0;
            }
        }
        P::Proporcao => {
            if let V::N(x) = v {
                e.proporcao = *x;
            }
        }
    }
}

/// Troca os `var(--x, reserva)` pelos valores.
fn trocar_variaveis(valor: &str, vars: Option<&BTreeMap<Rc<str>, Rc<str>>>) -> Option<String> {
    let mut v = String::from(valor);
    for _ in 0..16 {
        let Some(i) = v.find("var(") else { return Some(v) };
        let (dentro, fim) = entre_parenteses(&v, i + 3)?;
        let (nome, reserva) = dentro.split_once(',').map_or((dentro.trim(), None), |(n, r)| (n.trim(), Some(r.trim())));
        let troca = vars.and_then(|m| m.get(nome)).map(|s| s.to_string()).or_else(|| reserva.map(str::to_string))?;
        v = format!("{}{}{}", &v[..i], troca, &v[fim..]);
    }
    None
}

fn calcular_elemento(ctx: &mut Ctx, n: NoId, pai: &Rc<Estilo>) {
    let d = ctx.c.d;
    let mut candidatas: Vec<(u16, u32)> = Vec::new();
    if let Some(id) = d.atributo(n, "id") {
        if let Some(v) = ctx.indice.por_id.get(id) {
            candidatas.extend_from_slice(v);
        }
    }
    if let Some(cl) = d.atributo(n, "class") {
        for c in cl.split_ascii_whitespace() {
            if let Some(v) = ctx.indice.por_classe.get(c) {
                candidatas.extend_from_slice(v);
            }
        }
    }
    if let Some(v) = ctx.indice.por_tag.get(d.nome(n)) {
        candidatas.extend_from_slice(v);
    }
    candidatas.extend_from_slice(&ctx.indice.outras);
    candidatas.sort_unstable();
    candidatas.dedup();
    // (pseudo-elemento, importante + origem, especificidade, folha, regra)
    let mut valem: [Vec<(u32, u16, u32)>; 3] = [Vec::new(), Vec::new(), Vec::new()];
    for (f, r) in candidatas {
        let regra = &ctx.folhas[f as usize].regras[r as usize];
        if ctx.c.casa(&regra.seletor, n) {
            valem[regra.seletor.pseudo_elemento as usize].push((regra.seletor.especificidade, f, r));
        }
    }
    let dicas = {
        let mut v = Vec::new();
        dicas_html(d, n, &mut v);
        v
    };
    let inline = d.atributo(n, "style").map(estilo_inline).unwrap_or_default();
    let mut e = montar(ctx, pai, &mut valem[0], &dicas, &inline);
    if matches!(d.nome(n), "html") && e.fonte > 0.0 {
        ctx.raiz_fonte = e.fonte;
    }
    if e.flutua != 0 || e.fora_do_fluxo() {
        // Floats e posicionados viram blocos.
        e.display = match e.display {
            Display::Inline | Display::InlineBloco | Display::Celula | Display::Linha | Display::GrupoLinhas => Display::Bloco,
            Display::InlineFlex => Display::Flex,
            Display::InlineGrid => Display::Grid,
            Display::InlineTabela => Display::Tabela,
            o => o,
        };
    }
    let e = Rc::new(e);
    for (i, lista) in [(1usize, 0), (2, 1)] {
        let _ = lista;
        if !valem[i].is_empty() {
            let mut lv = core::mem::take(&mut valem[i]);
            let mut pe = montar(ctx, &e, &mut lv, &[], &[]);
            if pe.conteudo.is_some() && pe.display != Display::Nenhum {
                // O atributo de attr(): resolvido aqui, onde se sabe o elemento.
                if let Some(c) = &pe.conteudo {
                    if c.contains('\u{1}') {
                        let mut s = String::new();
                        let mut resto: &str = c;
                        while let Some(a) = resto.find('\u{1}') {
                            s.push_str(&resto[..a]);
                            let r = &resto[a + 1..];
                            let f = r.find('\u{2}').unwrap_or(r.len());
                            s.push_str(d.atributo(n, &r[..f]).unwrap_or(""));
                            resto = r.get(f + 1..).unwrap_or("");
                        }
                        s.push_str(resto);
                        pe.conteudo = Some(Rc::from(s.as_str()));
                    }
                }
                if i == 1 {
                    ctx.saida.antes.insert(n, Rc::new(pe));
                } else {
                    ctx.saida.depois.insert(n, Rc::new(pe));
                }
            }
        }
    }
    ctx.saida.estilos.insert(n, e.clone());
    if e.display != Display::Nenhum {
        for c in d.filhos(n) {
            if d.e_elemento(*c) {
                calcular_elemento(ctx, *c, &e);
            }
        }
    }
}

fn montar(ctx: &mut Ctx, pai: &Rc<Estilo>, regras: &mut [(u32, u16, u32)], dicas: &[(Decl, bool)], inline: &[(Decl, bool)]) -> Estilo {
    regras.sort_unstable();
    let mut e = pai.herdado();
    let mut vars_novas: Option<BTreeMap<Rc<str>, Rc<str>>> = None;
    // Por ordem de prioridade: origem da folha (a 0 é a do navegador), sem e
    // com !important; as dicas do HTML entram depois da folha padrão.
    let folhas = ctx.folhas;
    let mut lista: Vec<&(Decl, bool)> = Vec::new();
    let (padrao, autor): (Vec<&(u32, u16, u32)>, Vec<&(u32, u16, u32)>) = regras.iter().partition(|r| r.1 == 0);
    let blocos = |r: &(u32, u16, u32)| &folhas[r.1 as usize].blocos[folhas[r.1 as usize].regras[r.2 as usize].bloco as usize].decls;
    for r in &padrao {
        lista.extend(blocos(r).iter().filter(|d| !d.1));
    }
    lista.extend(dicas.iter());
    for r in &autor {
        lista.extend(blocos(r).iter().filter(|d| !d.1));
    }
    lista.extend(inline.iter().filter(|d| !d.1));
    for r in &autor {
        lista.extend(blocos(r).iter().filter(|d| d.1));
    }
    lista.extend(inline.iter().filter(|d| d.1));
    for r in &padrao {
        lista.extend(blocos(r).iter().filter(|d| d.1));
    }
    // As variáveis e o tamanho da fonte primeiro: os outros dependem deles.
    for (d, _) in lista.iter().map(|x| (&x.0, x.1)) {
        if let Decl::Var(nome, valor) = d {
            let m = vars_novas.get_or_insert_with(|| e.vars.as_deref().cloned().unwrap_or_default());
            m.insert(nome.clone(), valor.clone());
        }
    }
    if let Some(m) = vars_novas {
        e.vars = Some(Rc::new(m));
    }
    let pai_ref: &Estilo = pai;
    // `var()` vira declarações normais aqui.
    let mut expandidas: Vec<Decl> = Vec::new();
    let tem_var = lista.iter().any(|x| matches!(x.0, Decl::ComVar(..)));
    if tem_var {
        for x in &lista {
            match &x.0 {
                Decl::ComVar(nome, valor) => {
                    if let Some(t) = trocar_variaveis(valor, e.vars.as_deref()) {
                        interpretar(nome, &t, &mut expandidas);
                    }
                }
                outra => expandidas.push(outra.clone()),
            }
        }
    }
    let aplicar_fonte = |e: &mut Estilo, v: &V, ctx: &Ctx| match v {
        V::T(t) => {
            e.fonte = match resolver_t_com(*t, pai_ref.fonte, ctx) {
                Tam::Px(x) => x,
                Tam::Pct(x) => x * pai_ref.fonte / 100.0,
                Tam::PxPct(a, x) => a + x * pai_ref.fonte / 100.0,
                _ => pai_ref.fonte,
            }
        }
        V::Herdar => e.fonte = pai_ref.fonte,
        V::Inicial => e.fonte = 16.0,
        _ => {}
    };
    let ctx_ref: &Ctx = ctx;
    if tem_var {
        for d in &expandidas {
            if let Decl::D(P::FonteTam, v) = d {
                aplicar_fonte(&mut e, v, ctx_ref);
            }
        }
        for d in &expandidas {
            if let Decl::D(p, v) = d {
                aplicar(&mut e, *p, v, pai_ref, ctx_ref);
            }
        }
    } else {
        for x in &lista {
            if let Decl::D(P::FonteTam, v) = &x.0 {
                aplicar_fonte(&mut e, v, ctx_ref);
            }
        }
        for x in &lista {
            if let Decl::D(p, v) = &x.0 {
                aplicar(&mut e, *p, v, pai_ref, ctx_ref);
            }
        }
    }
    // As cores `currentcolor` das bordas e o que depende da cor final.
    for i in 0..4 {
        if e.borda_cor[i] == COR_ATUAL {
            e.borda_cor[i] = e.cor;
        }
        if e.borda_estilo[i] == 0 {
            e.borda[i] = 0.0;
        }
    }
    if e.fundo == COR_ATUAL {
        e.fundo = e.cor;
    }
    e
}

/// Aplica as folhas (na ordem do documento; a primeira deve ser a padrão) à árvore.
pub fn calcular(d: &Dom, folhas: &[&Folha], estado: &EstadoCss) -> Calculado {
    let (largura, altura) = (estado.largura.max(1), estado.altura.max(1));
    let mut indice = Indice { por_id: BTreeMap::new(), por_classe: BTreeMap::new(), por_tag: BTreeMap::new(), outras: Vec::new() };
    for (f, folha) in folhas.iter().enumerate() {
        let condicoes: Vec<bool> = folha.condicoes.iter().map(|c| midia_vale_tela(c, largura, altura)).collect();
        for (r, regra) in folha.regras.iter().enumerate() {
            if regra.midia != u16::MAX && !condicoes[regra.midia as usize] {
                continue;
            }
            let direita = &regra.seletor.partes[0].0;
            let par = (f as u16, r as u32);
            if let Some(id) = &direita.id {
                indice.por_id.entry(id.clone()).or_default().push(par);
            } else if let Some(c) = direita.classes.first() {
                indice.por_classe.entry(c.clone()).or_default().push(par);
            } else if let Some(t) = &direita.tag {
                indice.por_tag.entry(t.clone()).or_default().push(par);
            } else {
                indice.outras.push(par);
            }
        }
    }
    let mut ctx = Ctx { c: Casador { d, e: estado }, folhas, indice, raiz_fonte: 16.0, vw: largura as f32, vh: altura as f32, saida: Calculado::default() };
    let raiz = Rc::new(Estilo::inicial());
    for c in d.filhos(d.raiz) {
        if d.e_elemento(*c) {
            calcular_elemento(&mut ctx, *c, &raiz);
        }
    }
    ctx.saida
}

/// O primeiro/todos os elementos que casam (querySelector, querySelectorAll).
pub fn selecionar(d: &Dom, e: &EstadoCss, de: NoId, l: &[Seletor], so_um: bool, saida: &mut Vec<NoId>) {
    let c = Casador { d, e };
    fn andar(c: &Casador, n: NoId, l: &[Seletor], so_um: bool, saida: &mut Vec<NoId>) {
        for f in c.d.filhos(n) {
            if so_um && !saida.is_empty() {
                return;
            }
            if c.d.e_elemento(*f) {
                if l.iter().any(|s| s.pseudo_elemento == 0 && c.casa(s, *f)) {
                    saida.push(*f);
                    if so_um {
                        return;
                    }
                }
                andar(c, *f, l, so_um, saida);
            }
        }
    }
    andar(&c, de, l, so_um, saida);
}

#[allow(dead_code)]
fn _usa(x: f64) -> f64 {
    mat::abs(x)
}

#[allow(dead_code)]
fn _usa2() -> Vec<u8> {
    vec![]
}
