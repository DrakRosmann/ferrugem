//! A diagramação: do DOM com estilos para caixas com lugar e tamanho, e das
//! caixas para uma lista de desenho.
//!
//! ```text
//!   DOM + estilos ─▶ árvore de caixas (construir)
//!                 ─▶ medidas (dispor): fluxo de blocos, linhas de texto,
//!                    floats, flexbox, grid, tabelas, posicionadas
//!                 ─▶ lista de desenho (pintar), na ordem de empilhamento
//! ```
//!
//! As contas são em pixels inteiros (os programas são soft-float, e a tela
//! não tem meio pixel). Cada caixa guarda a posição em relação à caixa pai:
//! mudar um bloco de lugar não exige recalcular o que tem dentro.

use crate::css::{Calculado, Display, Estilo, Imagem, Posicao, Tam, Trilha, T};
use crate::dom::{Dom, NADA, NoId};
use alloc::collections::BTreeMap;
use alloc::format;
use alloc::rc::Rc;
use alloc::string::{String, ToString};
use alloc::vec;
use alloc::vec::Vec;

pub trait Medidor {
    fn largura(&mut self, texto: &str, tam: u16, negrito: bool, mono: bool) -> i32;
    /// (quanto sobe da linha de base, altura da linha)
    fn metricas(&mut self, tam: u16, mono: bool) -> (i32, i32);
}

/// O que a diagramação pergunta ao navegador.
pub trait Hospede {
    /// O tamanho natural de uma imagem, e o índice dela. A primeira vez que
    /// vê um endereço, o navegador põe na fila para baixar.
    fn imagem(&mut self, url: &str) -> (usize, Option<(i32, i32)>);
    /// (largura, altura) de um controle de formulário.
    fn controle(&mut self, no: NoId) -> (i32, i32);
}

/// O tamanho da fonte na tela: o 16px do CSS é a nossa fonte 14 (a DejaVu é larga).
pub fn tam_fonte(px: f32) -> u16 {
    ((px * 0.875 + 0.5) as i32).clamp(1, 200) as u16
}

const N: u32 = u32::MAX;

#[derive(Clone, Debug, PartialEq)]
enum Tipo {
    Bloco,
    Flex,
    Grid,
    Tabela,
    GrupoLinhas,
    Linha,
    Celula,
    Legenda,
    /// Uma caixa em linha com filhos.
    Inline,
    Texto(String),
    Quebra,
    Imagem(usize, Option<(i32, i32)>),
    Controle,
    /// canvas, svg, video, iframe...: um espaço reservado.
    Substituido,
}

#[derive(Clone)]
enum Item {
    Palavra { texto: String, largura: i32, caixa: u32 },
    Espaco { largura: i32, caixa: u32 },
    Quebra,
    Atomo(u32),
    Abre(u32),
    Fecha(u32),
}

/// Um pedaço de linha já no lugar (relativo à caixa que tem o texto).
#[derive(Clone)]
struct Pedaco {
    x: i32,
    y: i32,
    largura: i32,
    altura: i32,
    texto: String,
    caixa: u32,
}

struct Caixa {
    no: NoId,
    e: Rc<Estilo>,
    tipo: Tipo,
    filhos: Vec<u32>,
    pai: u32,
    /// O bloco contém linhas de texto (filhos em linha).
    em_linha: bool,
    x: i32,
    y: i32,
    /// O conteúdo.
    w: i32,
    h: i32,
    margem: [i32; 4],
    preench: [i32; 4],
    borda: [i32; 4],
    itens: Option<Rc<Vec<Item>>>,
    pedacos: Vec<Pedaco>,
    /// Fundos de caixas em linha: (caixa, x, y, w, h)
    fundos: Vec<(u32, i32, i32, i32, i32)>,
    marcador: Option<String>,
    /// Quem é a referência de uma caixa posicionada (e a lista delas).
    posicionadas: Vec<u32>,
    medidas: Option<(i32, i32)>,
    memo: Option<(i32, Option<i32>, i32)>,
    /// Onde ela estaria no fluxo (para `position: absolute` sem top/left).
    estatica: (i32, i32),
    extra_y: i32,
    baseline: i32,
}

/// Uma coisa a desenhar.
#[derive(Clone)]
pub enum Desenho {
    Ret { x: i32, y: i32, l: i32, a: i32, cor: u32 },
    Redondo { x: i32, y: i32, l: i32, a: i32, raios: [i32; 4], cor: u32 },
    Borda { x: i32, y: i32, l: i32, a: i32, raios: [i32; 4], larguras: [i32; 4], cores: [u32; 4], estilos: [u8; 4] },
    Texto { x: i32, y: i32, texto: String, tam: u16, negrito: bool, italico: bool, mono: bool, cor: u32 },
    Imagem { x: i32, y: i32, l: i32, a: i32, indice: usize, opac: u8 },
    Gradiente { x: i32, y: i32, l: i32, a: i32, angulo: f32, paradas: Rc<Vec<(f32, u32)>> },
    Sombra { x: i32, y: i32, l: i32, a: i32, raios: [i32; 4], borrao: i32, cor: u32 },
    Controle { x: i32, y: i32, l: i32, a: i32, no: NoId },
}

pub struct ItemDesenho {
    pub d: Desenho,
    /// O recorte (índice em `Lista::recortes`; 0 = nenhum).
    pub recorte: u32,
    /// Não rola com a página (`position: fixed`).
    pub fixo: bool,
    /// 255 = opaco (a opacidade do grupo).
    pub opac: u8,
}

pub struct Alvo {
    pub x: i32,
    pub y: i32,
    pub l: i32,
    pub a: i32,
    pub no: NoId,
    pub recorte: u32,
    pub fixo: bool,
}

#[derive(Default)]
pub struct Lista {
    pub itens: Vec<ItemDesenho>,
    pub recortes: Vec<(i32, i32, i32, i32)>,
    pub alvos: Vec<Alvo>,
    /// A caixa de cada elemento, em coordenadas da página.
    pub retangulos: BTreeMap<NoId, (i32, i32, i32, i32)>,
    pub altura: i32,
    pub largura: i32,
    pub fundo: u32,
    pub fundo_img: Option<Desenho>,
}

struct Floats {
    esquerda: Vec<(i32, i32, i32, i32)>,
    direita: Vec<(i32, i32, i32, i32)>,
}

pub struct Motor<'a, M: Medidor, H: Hospede> {
    d: &'a Dom,
    c: &'a Calculado,
    m: &'a mut M,
    h: &'a mut H,
    base_url: &'a dyn Fn(&str) -> Option<String>,
    cx: Vec<Caixa>,
    raiz: u32,
    vw: i32,
    vh: i32,
    floats: Vec<Floats>,
    /// A posição da caixa atual em relação à raiz do contexto de floats.
    desloc: (i32, i32),
    medidas_cache: BTreeMap<(u16, bool), (i32, i32)>,
    espacos_cache: BTreeMap<(u16, bool, bool), i32>,
}

fn r(t: Tam, base: i32) -> i32 {
    t.resolver(Some(base as f32)).map_or(0, |v| v as i32)
}

fn opt(t: Tam, base: Option<i32>) -> Option<i32> {
    t.resolver(base.map(|b| b as f32)).map(|v| v as i32)
}

impl<'a, M: Medidor, H: Hospede> Motor<'a, M, H> {
    pub fn novo(d: &'a Dom, c: &'a Calculado, m: &'a mut M, h: &'a mut H, base_url: &'a dyn Fn(&str) -> Option<String>, vw: i32, vh: i32) -> Self {
        Motor { d, c, m, h, base_url, cx: Vec::new(), raiz: N, vw, vh, floats: Vec::new(), desloc: (0, 0), medidas_cache: BTreeMap::new(), espacos_cache: BTreeMap::new() }
    }

    fn nova(&mut self, no: NoId, e: Rc<Estilo>, tipo: Tipo, pai: u32) -> u32 {
        self.cx.push(Caixa {
            no,
            e,
            tipo,
            filhos: Vec::new(),
            pai,
            em_linha: false,
            x: 0,
            y: 0,
            w: 0,
            h: 0,
            margem: [0; 4],
            preench: [0; 4],
            borda: [0; 4],
            itens: None,
            pedacos: Vec::new(),
            fundos: Vec::new(),
            marcador: None,
            posicionadas: Vec::new(),
            medidas: None,
            memo: None,
            estatica: (0, 0),
            extra_y: 0,
            baseline: 0,
        });
        self.cx.len() as u32 - 1
    }

    // ------------------------------------------------------------ construção

    /// O estilo de uma caixa anônima: herda só o que herda.
    fn anonimo(&self, pai: &Estilo) -> Rc<Estilo> {
        let mut e = pai.herdado();
        e.display = Display::Bloco;
        Rc::new(e)
    }

    fn tipo_de(&self, n: NoId, e: &Estilo) -> Tipo {
        match e.display {
            Display::Flex | Display::InlineFlex => Tipo::Flex,
            Display::Grid | Display::InlineGrid => Tipo::Grid,
            Display::Tabela | Display::InlineTabela => Tipo::Tabela,
            Display::GrupoLinhas => Tipo::GrupoLinhas,
            Display::Linha => Tipo::Linha,
            Display::Celula => Tipo::Celula,
            Display::Legenda => Tipo::Legenda,
            Display::Inline => {
                let _ = n;
                Tipo::Inline
            }
            _ => Tipo::Bloco,
        }
    }

    fn adicionar_pseudo(&mut self, pai: u32, e: Option<&Rc<Estilo>>) {
        let Some(e) = e else { return };
        let Some(t) = e.conteudo.clone() else { return };
        let no = self.cx[pai as usize].no;
        let tipo = self.tipo_de(no, e);
        let c = self.nova(no, e.clone(), tipo, pai);
        self.cx[pai as usize].filhos.push(c);
        if !t.is_empty() {
            let te = c;
            let et = self.cx[c as usize].e.clone();
            let txt = self.nova(no, et, Tipo::Texto(t.to_string()), te);
            self.cx[c as usize].filhos.push(txt);
            self.arrumar_filhos(c);
        }
    }

    fn construir_filhos(&mut self, n: NoId, b: u32, prof: u32) {
        let (antes, depois) = (self.c.antes.get(&n).cloned(), self.c.depois.get(&n).cloned());
        self.adicionar_pseudo(b, antes.as_ref());
        for f in self.d.filhos(n).to_vec() {
            self.construir_no(f, b, prof + 1);
        }
        self.adicionar_pseudo(b, depois.as_ref());
    }

    fn construir_no(&mut self, n: NoId, pai: u32, prof: u32) {
        let d = self.d;
        if let Some(t) = d.texto_de_no(n) {
            if !t.is_empty() {
                let e = self.cx[pai as usize].e.clone();
                let c = self.nova(n, e, Tipo::Texto(t.to_string()), pai);
                self.cx[pai as usize].filhos.push(c);
            }
            return;
        }
        if !d.e_elemento(n) || prof > 400 {
            return;
        }
        let Some(e) = self.c.estilos.get(&n).cloned() else { return };
        if e.display == Display::Nenhum || matches!(e.display, Display::GrupoColunas | Display::Coluna) {
            return;
        }
        if e.display == Display::Conteudo {
            self.construir_filhos(n, pai, prof);
            return;
        }
        let nome = d.nome(n);
        match nome {
            "br" => {
                let c = self.nova(n, e, Tipo::Quebra, pai);
                self.cx[pai as usize].filhos.push(c);
                return;
            }
            "img" => {
                return self.construir_imagem(n, e, pai);
            }
            "input" => {
                let tipo = d.atributo(n, "type").unwrap_or("text").to_ascii_lowercase();
                if matches!(tipo.as_str(), "submit" | "button" | "reset" | "file") {
                    let c = self.nova(n, e.clone(), Tipo::Bloco, pai);
                    self.cx[pai as usize].filhos.push(c);
                    let padrao = match tipo.as_str() {
                        "submit" => "Enviar",
                        "reset" => "Limpar",
                        "file" => "Escolher arquivo…",
                        _ => "",
                    };
                    let v = d.atributo(n, "value").filter(|v| !v.is_empty() && tipo != "file").unwrap_or(padrao);
                    let mut et = e.herdado();
                    et.display = Display::Inline;
                    let t = self.nova(n, Rc::new(et), Tipo::Texto(v.to_string()), c);
                    self.cx[c as usize].filhos.push(t);
                    self.cx[c as usize].em_linha = true;
                    return;
                }
                if tipo == "image" {
                    return self.construir_imagem(n, e, pai);
                }
                let c = self.nova(n, e, Tipo::Controle, pai);
                self.cx[pai as usize].filhos.push(c);
                return;
            }
            "select" | "textarea" => {
                let c = self.nova(n, e, Tipo::Controle, pai);
                self.cx[pai as usize].filhos.push(c);
                return;
            }
            "canvas" | "svg" | "video" | "audio" | "iframe" | "object" | "embed" | "applet" => {
                let c = self.nova(n, e, Tipo::Substituido, pai);
                self.cx[pai as usize].filhos.push(c);
                return;
            }
            _ => {}
        }
        let tipo = self.tipo_de(n, &e);
        let e_item = e.display == Display::ItemLista;
        let c = self.nova(n, e, tipo, pai);
        self.cx[pai as usize].filhos.push(c);
        if e_item {
            self.cx[c as usize].marcador = self.texto_marcador(n, c);
        }
        self.construir_filhos(n, c, prof);
        self.arrumar_filhos(c);
    }

    fn construir_imagem(&mut self, n: NoId, e: Rc<Estilo>, pai: u32) {
        let d = self.d;
        let fonte = d.atributo(n, "src").filter(|v| !v.is_empty()).or(d.atributo(n, "data-src"));
        let url = fonte.filter(|f| !f.starts_with("data:")).and_then(|f| (self.base_url)(f));
        let Some(url) = url else {
            return self.imagem_alternativa(n, e, pai);
        };
        let (i, tam) = self.h.imagem(&url);
        // Falhou, ou ainda não chegou e não se sabe o tamanho: o texto alternativo.
        let falhou = i == usize::MAX;
        if falhou {
            return self.imagem_alternativa(n, e, pai);
        }
        let c = self.nova(n, e, Tipo::Imagem(i, tam), pai);
        self.cx[pai as usize].filhos.push(c);
    }

    fn imagem_alternativa(&mut self, n: NoId, e: Rc<Estilo>, pai: u32) {
        let alt = self.d.atributo(n, "alt").unwrap_or("").trim().to_string();
        let tem_tam = !e.largura.e_auto();
        if alt.is_empty() && !tem_tam {
            return;
        }
        let c = self.nova(n, e.clone(), if tem_tam { Tipo::Bloco } else { Tipo::Inline }, pai);
        self.cx[pai as usize].filhos.push(c);
        if !alt.is_empty() {
            let mut et = e.herdado();
            et.display = Display::Inline;
            let t = self.nova(n, Rc::new(et), Tipo::Texto(alt), c);
            self.cx[c as usize].filhos.push(t);
        }
        self.cx[c as usize].em_linha = true;
    }

    fn texto_marcador(&self, n: NoId, c: u32) -> Option<String> {
        let e = &self.cx[c as usize].e;
        match e.lista_tipo {
            0 => Some(String::from("•")),
            1 => Some(String::from("◦")),
            2 => Some(String::from("▪")),
            4 => None,
            t => {
                // O número do item.
                let d = self.d;
                let p = d.pai(n);
                let mut i = p.ne(&NADA).then(|| d.atributo(p, "start").and_then(|s| s.parse::<i32>().ok()).unwrap_or(1)).unwrap_or(1);
                if p != NADA {
                    for f in d.filhos(p) {
                        if *f == n {
                            break;
                        }
                        if d.e(*f, "li") {
                            i = d.atributo(*f, "value").and_then(|v| v.parse().ok()).unwrap_or(i) + 1;
                        }
                    }
                }
                if let Some(v) = d.atributo(n, "value").and_then(|v| v.parse().ok()) {
                    i = v;
                }
                Some(match t {
                    5 => format!("{}.", (b'a' + ((i - 1).rem_euclid(26)) as u8) as char),
                    6 => format!("{}.", (b'A' + ((i - 1).rem_euclid(26)) as u8) as char),
                    7 => format!("{}.", romano(i).to_ascii_lowercase()),
                    8 => format!("{}.", romano(i)),
                    _ => format!("{}.", i),
                })
            }
        }
    }

    fn e_em_linha(&self, c: u32) -> bool {
        let x = &self.cx[c as usize];
        if x.e.fora_do_fluxo() || x.e.flutua != 0 {
            return false;
        }
        matches!(x.tipo, Tipo::Texto(_) | Tipo::Quebra | Tipo::Inline) || x.e.e_inline()
    }

    fn e_branco(&self, c: u32) -> bool {
        let x = &self.cx[c as usize];
        matches!(&x.tipo, Tipo::Texto(t) if x.e.branco < 2 && t.chars().all(|c| matches!(c, ' ' | '\n' | '\t' | '\r' | '\x0c')))
    }

    /// Depois de criar os filhos: blocos e textos não se misturam num pai.
    fn arrumar_filhos(&mut self, c: u32) {
        let tipo = self.cx[c as usize].tipo.clone();
        match tipo {
            Tipo::Inline => {
                // Um bloco dentro de um <a> ou <span>: a caixa vira bloco.
                let tem_bloco = self.cx[c as usize].filhos.clone().iter().any(|f| !self.e_em_linha(*f) && !self.cx[*f as usize].e.fora_do_fluxo() && self.cx[*f as usize].e.flutua == 0);
                if tem_bloco {
                    self.cx[c as usize].tipo = Tipo::Bloco;
                    let mut e2 = (*self.cx[c as usize].e).clone();
                    e2.display = Display::Bloco;
                    self.cx[c as usize].e = Rc::new(e2);
                    self.arrumar_filhos(c);
                }
            }
            Tipo::Bloco | Tipo::Celula | Tipo::Legenda => self.arrumar_bloco(c),
            Tipo::Flex | Tipo::Grid => self.arrumar_itens(c),
            Tipo::Tabela | Tipo::GrupoLinhas | Tipo::Linha => {
                // Só as caixas de tabela importam: o branco entre elas some.
                let f: Vec<u32> = self.cx[c as usize].filhos.iter().copied().filter(|x| !self.e_branco(*x)).collect();
                self.cx[c as usize].filhos = f;
                self.arrumar_tabela(c);
            }
            _ => {}
        }
    }

    fn arrumar_bloco(&mut self, c: u32) {
        let filhos = core::mem::take(&mut self.cx[c as usize].filhos);
        let tem_bloco = filhos.iter().any(|f| {
            let x = &self.cx[*f as usize];
            !self.e_em_linha(*f) && !x.e.fora_do_fluxo() && x.e.flutua == 0
        });
        let tem_linha = filhos.iter().any(|f| self.e_em_linha(*f) && !self.e_branco(*f));
        if !tem_bloco {
            self.cx[c as usize].filhos = filhos;
            self.cx[c as usize].em_linha = true;
            return;
        }
        if !tem_linha {
            // Só blocos: o branco entre eles some.
            let f = filhos.into_iter().filter(|x| !self.e_branco(*x)).collect();
            self.cx[c as usize].filhos = f;
            return;
        }
        // Misturados: os textos ganham blocos anônimos.
        let mut novos = Vec::new();
        let mut trecho: Vec<u32> = Vec::new();
        for f in filhos {
            if self.e_em_linha(f) || (!trecho.is_empty() && (self.cx[f as usize].e.flutua != 0 || self.cx[f as usize].e.fora_do_fluxo())) {
                trecho.push(f);
            } else {
                self.fechar_trecho(c, &mut trecho, &mut novos);
                novos.push(f);
            }
        }
        self.fechar_trecho(c, &mut trecho, &mut novos);
        self.cx[c as usize].filhos = novos;
    }

    fn fechar_trecho(&mut self, c: u32, trecho: &mut Vec<u32>, novos: &mut Vec<u32>) {
        if trecho.is_empty() {
            return;
        }
        let lista = core::mem::take(trecho);
        if lista.iter().all(|f| self.e_branco(*f) || self.cx[*f as usize].e.fora_do_fluxo()) {
            novos.extend(lista.into_iter().filter(|f| !self.e_branco(*f)));
            return;
        }
        let e = self.anonimo(&self.cx[c as usize].e.clone());
        let a = self.nova(NADA, e, Tipo::Bloco, c);
        for f in &lista {
            self.cx[*f as usize].pai = a;
        }
        self.cx[a as usize].filhos = lista;
        self.cx[a as usize].em_linha = true;
        novos.push(a);
    }

    fn arrumar_itens(&mut self, c: u32) {
        let filhos = core::mem::take(&mut self.cx[c as usize].filhos);
        let mut novos = Vec::new();
        let mut trecho = Vec::new();
        for f in filhos {
            let x = &mut self.cx[f as usize];
            let e_texto = matches!(x.tipo, Tipo::Texto(_) | Tipo::Quebra);
            if e_texto {
                trecho.push(f);
                continue;
            }
            self.fechar_trecho(c, &mut trecho, &mut novos);
            // Os itens são sempre blocos.
            let x = &mut self.cx[f as usize];
            if x.tipo == Tipo::Inline || x.e.e_inline() {
                let mut e2 = (*x.e).clone();
                e2.display = match e2.display {
                    Display::InlineFlex => Display::Flex,
                    Display::InlineGrid => Display::Grid,
                    Display::InlineTabela => Display::Tabela,
                    _ => Display::Bloco,
                };
                x.e = Rc::new(e2);
                if x.tipo == Tipo::Inline {
                    x.tipo = Tipo::Bloco;
                    self.arrumar_filhos(f);
                }
            }
            novos.push(f);
        }
        self.fechar_trecho(c, &mut trecho, &mut novos);
        self.cx[c as usize].filhos = novos;
    }

    /// Conserta as tabelas sem a estrutura completa (células soltas viram uma linha).
    fn arrumar_tabela(&mut self, c: u32) {
        let tipo = self.cx[c as usize].tipo.clone();
        if tipo == Tipo::Linha {
            let filhos = core::mem::take(&mut self.cx[c as usize].filhos);
            let mut novos = Vec::new();
            for f in filhos {
                if self.cx[f as usize].tipo == Tipo::Celula {
                    novos.push(f);
                } else {
                    // Qualquer coisa numa linha vira uma célula.
                    let e = self.anonimo(&self.cx[c as usize].e.clone());
                    let a = self.nova(NADA, e, Tipo::Celula, c);
                    self.cx[f as usize].pai = a;
                    self.cx[a as usize].filhos.push(f);
                    self.arrumar_filhos(a);
                    novos.push(a);
                }
            }
            self.cx[c as usize].filhos = novos;
            return;
        }
        let filhos = core::mem::take(&mut self.cx[c as usize].filhos);
        let mut novos = Vec::new();
        let mut soltos: Vec<u32> = Vec::new();
        let juntar = |m: &mut Self, soltos: &mut Vec<u32>, novos: &mut Vec<u32>| {
            if soltos.is_empty() {
                return;
            }
            let e = m.anonimo(&m.cx[c as usize].e.clone());
            let l = m.nova(NADA, e, Tipo::Linha, c);
            for s in soltos.iter() {
                m.cx[*s as usize].pai = l;
            }
            m.cx[l as usize].filhos = core::mem::take(soltos);
            m.arrumar_filhos(l);
            novos.push(l);
        };
        for f in filhos {
            let t = self.cx[f as usize].tipo.clone();
            let ok = matches!(t, Tipo::Linha | Tipo::Legenda) || (t == Tipo::GrupoLinhas && tipo == Tipo::Tabela);
            if ok || self.cx[f as usize].e.fora_do_fluxo() {
                juntar(self, &mut soltos, &mut novos);
                novos.push(f);
            } else {
                soltos.push(f);
            }
        }
        juntar(self, &mut soltos, &mut novos);
        self.cx[c as usize].filhos = novos;
    }

    fn achar_referencias(&mut self, c: u32, pos: u32, raiz: u32) {
        for f in self.cx[c as usize].filhos.clone() {
            let (posi, tem_transf) = {
                let x = &self.cx[f as usize];
                (x.e.posicao, !matches!(x.e.desloca_x, Tam::Px(v) if v == 0.0) || !matches!(x.e.desloca_y, Tam::Px(v) if v == 0.0))
            };
            match posi {
                Posicao::Absoluta => self.cx[pos as usize].posicionadas.push(f),
                Posicao::Fixa => self.cx[raiz as usize].posicionadas.push(f),
                _ => {}
            }
            let prox = if posi != Posicao::Estatica || tem_transf { f } else { pos };
            self.achar_referencias(f, prox, raiz);
        }
    }

    // ------------------------------------------------------------ medidas

    fn metricas_de(&mut self, e: &Estilo) -> (i32, i32) {
        let chave = (tam_fonte(e.fonte), e.mono);
        if let Some(v) = self.medidas_cache.get(&chave) {
            return *v;
        }
        let v = self.m.metricas(chave.0, chave.1);
        self.medidas_cache.insert(chave, v);
        v
    }

    fn espaco_de(&mut self, e: &Estilo) -> i32 {
        let chave = (tam_fonte(e.fonte), e.negrito, e.mono);
        if let Some(v) = self.espacos_cache.get(&chave) {
            return *v + (e.espaco_palavras + e.espaco_letras) as i32;
        }
        let v = self.m.largura(" ", chave.0, chave.1, chave.2).max(1);
        self.espacos_cache.insert(chave, v);
        v + (e.espaco_palavras + e.espaco_letras) as i32
    }

    fn altura_de_linha(&mut self, e: &Estilo) -> i32 {
        let (_, alt) = self.metricas_de(e);
        if e.altura_linha == 0.0 {
            alt.max((e.fonte * 1.2) as i32)
        } else if e.altura_linha > 0.0 {
            (e.altura_linha * e.fonte) as i32
        } else {
            (-e.altura_linha) as i32
        }
    }

    fn largura_texto(&mut self, t: &str, e: &Estilo) -> i32 {
        let w = self.m.largura(t, tam_fonte(e.fonte), e.negrito, e.mono);
        if e.espaco_letras != 0.0 { w + (e.espaco_letras * t.chars().count() as f32) as i32 } else { w }
    }

    /// Resolve margens, preenchimento e borda (as porcentagens são da largura).
    fn medir_caixa(&mut self, c: u32, cb: i32) {
        let x = &mut self.cx[c as usize];
        for i in 0..4 {
            x.margem[i] = if x.e.margem[i].e_auto() { 0 } else { r(x.e.margem[i], cb) };
            x.preench[i] = r(x.e.preench[i], cb).max(0);
            x.borda[i] = x.e.borda[i] as i32;
        }
        if x.tipo == Tipo::Inline {
            x.margem[0] = 0;
            x.margem[2] = 0;
        }
    }

    fn horizontal(&self, c: u32) -> i32 {
        let x = &self.cx[c as usize];
        x.preench[1] + x.preench[3] + x.borda[1] + x.borda[3]
    }

    fn vertical(&self, c: u32) -> i32 {
        let x = &self.cx[c as usize];
        x.preench[0] + x.preench[2] + x.borda[0] + x.borda[2]
    }

    /// (mínima, máxima) largura do conteúdo, sem as margens da própria caixa
    /// mas com preenchimento e borda.
    fn intrinsecas(&mut self, c: u32) -> (i32, i32) {
        if let Some(v) = self.cx[c as usize].medidas {
            return v;
        }
        self.medir_caixa(c, 0);
        let e = self.cx[c as usize].e.clone();
        let hor = self.horizontal(c);
        let tipo = self.cx[c as usize].tipo.clone();
        let (mut mi, mut ma) = match &tipo {
            Tipo::Imagem(..) | Tipo::Controle | Tipo::Substituido => {
                let (w, _) = self.tamanho_substituido(c, None, None);
                (w, w)
            }
            Tipo::Flex => {
                let linha = e.flex_direcao < 2;
                let gap = r(e.vao_coluna, 0);
                let filhos = self.cx[c as usize].filhos.clone();
                let (mut s_max, mut s_min, mut mx_max, mut mx_min, mut n) = (0, 0, 0, 0, 0);
                for f in filhos {
                    if self.cx[f as usize].e.fora_do_fluxo() {
                        continue;
                    }
                    let (a, b) = self.intrinsecas_externas(f);
                    s_max += b;
                    s_min += a;
                    mx_max = mx_max.max(b);
                    mx_min = mx_min.max(a);
                    n += 1;
                }
                let gaps = gap * (n - 1).max(0);
                if linha {
                    (if e.flex_quebra != 0 { mx_min } else { s_min + gaps }, s_max + gaps)
                } else {
                    (mx_min, mx_max)
                }
            }
            Tipo::Grid => {
                let filhos = self.cx[c as usize].filhos.clone();
                let n = expandir_contagem(&e.grid_colunas).max(1) as i32;
                let (mut mi, mut ma) = (0, 0);
                let mut soma_fixas = 0;
                for t in e.grid_colunas.iter() {
                    if let Trilha::T(T::Px(v)) = t {
                        soma_fixas += *v as i32;
                    }
                }
                for f in filhos {
                    if self.cx[f as usize].e.fora_do_fluxo() {
                        continue;
                    }
                    let (a, b) = self.intrinsecas_externas(f);
                    mi = mi.max(a);
                    ma = ma.max(b);
                }
                let gap = r(e.vao_coluna, 0) * (n - 1);
                ((mi * n + gap).max(soma_fixas), (ma * n + gap).max(soma_fixas))
            }
            Tipo::Tabela => {
                let (colunas_min, colunas_max) = self.colunas_intrinsecas(c);
                let esp = if e.colapsa_bordas { 0 } else { e.espaco_bordas as i32 };
                let g = esp * (colunas_min.len() as i32 + 1);
                let (a, b): (i32, i32) = (colunas_min.iter().sum(), colunas_max.iter().sum());
                (a + g, b + g)
            }
            _ if self.cx[c as usize].em_linha || matches!(tipo, Tipo::Inline) => self.intrinsecas_linhas(c),
            _ => {
                let (mut mi, mut ma, mut soma_float) = (0, 0, 0);
                for f in self.cx[c as usize].filhos.clone() {
                    if self.cx[f as usize].e.fora_do_fluxo() {
                        continue;
                    }
                    let (a, b) = self.intrinsecas_externas(f);
                    mi = mi.max(a);
                    if self.cx[f as usize].e.flutua != 0 {
                        soma_float += b;
                    } else {
                        ma = ma.max(b);
                    }
                }
                (mi, ma.max(soma_float))
            }
        };
        // Um tamanho fixo manda.
        if let Tam::Px(v) = e.largura {
            let v = if e.caixa_borda { v as i32 - hor } else { v as i32 }.max(0);
            mi = v;
            ma = v;
        }
        if let Tam::Px(v) = e.max_largura {
            let v = if e.caixa_borda { v as i32 - hor } else { v as i32 }.max(0);
            ma = ma.min(v);
            mi = mi.min(v);
        }
        if let Tam::Px(v) = e.min_largura {
            let v = if e.caixa_borda { v as i32 - hor } else { v as i32 }.max(0);
            ma = ma.max(v);
            mi = mi.max(v);
        }
        let v = (mi + hor, ma.max(mi) + hor);
        self.cx[c as usize].medidas = Some(v);
        v
    }

    fn intrinsecas_externas(&mut self, c: u32) -> (i32, i32) {
        let (a, b) = self.intrinsecas(c);
        let x = &self.cx[c as usize];
        let m = x.margem[1] + x.margem[3];
        (a + m, b + m)
    }

    fn intrinsecas_linhas(&mut self, c: u32) -> (i32, i32) {
        let itens = self.itens_de(c);
        let (mut mi, mut ma, mut atual, mut palavra) = (0, 0, 0, 0);
        let e_nowrap = self.cx[c as usize].e.branco == 1 || self.cx[c as usize].e.branco == 2;
        for it in itens.iter() {
            match it {
                Item::Palavra { largura, caixa, .. } => {
                    atual += largura;
                    let so = self.cx[*caixa as usize].e.branco;
                    if so == 1 || so == 2 || e_nowrap {
                        palavra += largura;
                    } else {
                        palavra = palavra.max(*largura);
                        mi = mi.max(palavra);
                        palavra = 0;
                        continue;
                    }
                    mi = mi.max(palavra);
                }
                Item::Espaco { largura, .. } => {
                    atual += largura;
                    if e_nowrap {
                        palavra += largura;
                    } else {
                        palavra = 0;
                    }
                }
                Item::Quebra => {
                    ma = ma.max(atual);
                    atual = 0;
                    palavra = 0;
                }
                Item::Atomo(f) => {
                    let (a, b) = self.intrinsecas_externas(*f);
                    atual += b;
                    mi = mi.max(a);
                    palavra = 0;
                }
                Item::Abre(f) => {
                    let x = &self.cx[*f as usize];
                    let v = x.e.margem[3].resolver(Some(0.0)).unwrap_or(0.0) as i32 + x.e.preench[3].resolver(Some(0.0)).unwrap_or(0.0) as i32 + x.e.borda[3] as i32;
                    atual += v;
                }
                Item::Fecha(f) => {
                    let x = &self.cx[*f as usize];
                    let v = x.e.margem[1].resolver(Some(0.0)).unwrap_or(0.0) as i32 + x.e.preench[1].resolver(Some(0.0)).unwrap_or(0.0) as i32 + x.e.borda[1] as i32;
                    atual += v;
                }
            }
        }
        (mi, ma.max(atual))
    }

    /// O tamanho (conteúdo) de uma imagem, controle ou espaço reservado.
    fn tamanho_substituido(&mut self, c: u32, cb_w: Option<i32>, cb_h: Option<i32>) -> (i32, i32) {
        let x = &self.cx[c as usize];
        let e = x.e.clone();
        let (nat, proporcao) = match &x.tipo {
            Tipo::Imagem(_, t) => (*t, t.map(|(w, h)| w as f32 / h.max(1) as f32)),
            Tipo::Controle => {
                let t = self.h.controle(x.no);
                (Some(t), None)
            }
            _ => {
                let d = self.d;
                let nome = d.nome(x.no);
                let (dw, dh) = match nome {
                    "audio" => (300, 32),
                    _ => (300, 150),
                };
                let aw = d.atributo(x.no, "width").and_then(|v| v.trim_end_matches("px").parse().ok()).unwrap_or(dw);
                let ah = d.atributo(x.no, "height").and_then(|v| v.trim_end_matches("px").parse().ok()).unwrap_or(dh);
                (Some((aw, ah)), Some(aw as f32 / (ah as i32).max(1) as f32))
            }
        };
        let hor = x.preench[1] + x.preench[3] + x.borda[1] + x.borda[3];
        let ver = x.preench[0] + x.preench[2] + x.borda[0] + x.borda[2];
        let ajusta = |v: i32, b: i32| if e.caixa_borda { (v - b).max(0) } else { v };
        let w_css = opt(e.largura, cb_w).map(|v| ajusta(v, hor));
        let h_css = opt(e.altura, cb_h).map(|v| ajusta(v, ver));
        let prop = if e.proporcao > 0.0 { Some(e.proporcao) } else { proporcao };
        let (mut w, mut h) = match (w_css, h_css, nat, prop) {
            (Some(a), Some(b), _, _) => (a, b),
            (Some(a), None, _, Some(p)) => (a, (a as f32 / p) as i32),
            (Some(a), None, Some((_, nh)), None) => (a, nh),
            (Some(a), None, None, None) => (a, 0),
            (None, Some(b), _, Some(p)) => ((b as f32 * p) as i32, b),
            (None, Some(b), Some((nw, _)), None) => (nw, b),
            (None, Some(b), None, None) => (0, b),
            (None, None, Some(t), _) => t,
            (None, None, None, _) => (0, 0),
        };
        if let Some(m) = opt(e.max_largura, cb_w).map(|v| ajusta(v, hor)) {
            if w > m {
                if h_css.is_none() {
                    if let Some(p) = prop {
                        h = (m as f32 / p) as i32;
                    }
                }
                w = m;
            }
        }
        if let Some(m) = opt(e.min_largura, cb_w).map(|v| ajusta(v, hor)) {
            w = w.max(m);
        }
        if let Some(m) = opt(e.max_altura, cb_h).map(|v| ajusta(v, ver)) {
            if h > m {
                if w_css.is_none() {
                    if let Some(p) = prop {
                        w = (m as f32 * p) as i32;
                    }
                }
                h = m;
            }
        }
        if let Some(m) = opt(e.min_altura, cb_h).map(|v| ajusta(v, ver)) {
            h = h.max(m);
        }
        (w, h)
    }

    // ------------------------------------------------------------ texto em linhas

    fn itens_de(&mut self, c: u32) -> Rc<Vec<Item>> {
        if let Some(i) = &self.cx[c as usize].itens {
            return i.clone();
        }
        let mut lista = Vec::new();
        let mut espaco_pendente = false;
        let mut comeco = true;
        for f in self.cx[c as usize].filhos.clone() {
            self.achatar(f, &mut lista, &mut espaco_pendente, &mut comeco);
        }
        let r = Rc::new(lista);
        self.cx[c as usize].itens = Some(r.clone());
        r
    }

    fn achatar(&mut self, c: u32, s: &mut Vec<Item>, espaco: &mut bool, comeco: &mut bool) {
        let e = self.cx[c as usize].e.clone();
        match self.cx[c as usize].tipo.clone() {
            Tipo::Texto(t) => self.achatar_texto(c, &t, &e, s, espaco, comeco),
            Tipo::Quebra => {
                s.push(Item::Quebra);
                *espaco = false;
                *comeco = true;
            }
            Tipo::Inline => {
                self.medir_caixa(c, 0);
                s.push(Item::Abre(c));
                for f in self.cx[c as usize].filhos.clone() {
                    self.achatar(f, s, espaco, comeco);
                }
                s.push(Item::Fecha(c));
            }
            _ => {
                // Um átomo (inline-block, imagem...), ou um float/posicionado.
                if *espaco && !*comeco {
                    let l = self.espaco_de(&e);
                    s.push(Item::Espaco { largura: l, caixa: c });
                }
                *espaco = false;
                s.push(Item::Atomo(c));
                if !e.fora_do_fluxo() && e.flutua == 0 {
                    *comeco = false;
                }
            }
        }
    }

    fn achatar_texto(&mut self, c: u32, t: &str, e: &Estilo, s: &mut Vec<Item>, espaco: &mut bool, comeco: &mut bool) {
        let transformar = |p: &str| -> String {
            match e.transforma {
                1 => p.to_uppercase(),
                2 => p.to_lowercase(),
                3 => {
                    let mut it = p.chars();
                    it.next().map_or(String::new(), |q| q.to_uppercase().chain(it).collect())
                }
                _ => p.to_string(),
            }
        };
        if e.branco >= 2 {
            // pre, pre-wrap, pre-line
            let linha_recolhe = e.branco == 4;
            for (i, linha) in t.split('\n').enumerate() {
                if i > 0 {
                    s.push(Item::Quebra);
                    *comeco = true;
                }
                let linha = linha.replace('\r', "").replace('\t', "        ");
                if linha.is_empty() {
                    continue;
                }
                if e.branco == 2 {
                    let tx = transformar(&linha);
                    let w = self.largura_texto(&tx, e);
                    s.push(Item::Palavra { texto: tx, largura: w, caixa: c });
                } else {
                    let le = self.espaco_de(e);
                    let mut primeira = true;
                    for p in linha.split(' ') {
                        if !primeira && !linha_recolhe {
                            s.push(Item::Espaco { largura: le, caixa: c });
                        } else if !primeira && !p.is_empty() {
                            s.push(Item::Espaco { largura: le, caixa: c });
                        }
                        primeira = false;
                        if !p.is_empty() {
                            let tx = transformar(p);
                            let w = self.largura_texto(&tx, e);
                            s.push(Item::Palavra { texto: tx, largura: w, caixa: c });
                        }
                    }
                }
                *comeco = false;
            }
            *espaco = false;
            return;
        }
        let branco = |x: char| matches!(x, ' ' | '\n' | '\t' | '\r' | '\x0c');
        let mut resto = t;
        while !resto.is_empty() {
            if resto.starts_with(branco) {
                *espaco = true;
                resto = resto.trim_start_matches(branco);
                continue;
            }
            let fim = resto.find(branco).unwrap_or(resto.len());
            let p = &resto[..fim];
            resto = &resto[fim..];
            if *espaco && !*comeco {
                let l = self.espaco_de(e);
                s.push(Item::Espaco { largura: l, caixa: c });
            }
            *espaco = false;
            *comeco = false;
            let limpa;
            let p2: &str = if p.contains(['\u{A0}', '\u{AD}', '\u{200B}']) {
                limpa = p.replace('\u{A0}', " ").replace(['\u{AD}', '\u{200B}'], "");
                &limpa
            } else {
                p
            };
            let tx = transformar(p2);
            let w = self.largura_texto(&tx, e);
            s.push(Item::Palavra { texto: tx, largura: w, caixa: c });
        }
    }

    /// A faixa livre de floats numa altura (coordenadas do contexto).
    fn faixa_livre(&self, y: i32, altura: i32, x0: i32, x1: i32) -> (i32, i32) {
        let Some(f) = self.floats.last() else { return (x0, x1) };
        let (mut a, mut b) = (x0, x1);
        for &(x, fy, w, h) in &f.esquerda {
            if y < fy + h && y + altura > fy {
                a = a.max(x + w);
            }
        }
        for &(x, fy, _, h) in &f.direita {
            if y < fy + h && y + altura > fy {
                b = b.min(x);
            }
        }
        (a, b)
    }

    fn dispor_linhas(&mut self, c: u32, largura: i32) -> i32 {
        let itens = self.itens_de(c);
        let e = self.cx[c as usize].e.clone();
        let mut y = 0;
        let (ox, oy) = self.desloc;
        let mut pedacos: Vec<Pedaco> = Vec::new();
        let mut fundos: Vec<(u32, i32, i32, i32, i32)> = Vec::new();
        let altura_padrao = self.altura_de_linha(&e);
        let mut primeira = true;
        // As caixas em linha abertas no começo da linha.
        let mut abertas: Vec<u32> = Vec::new();
        let mut i = 0;
        let n = itens.len();
        let reticencias = e.reticencias && e.branco == 1;
        let mut ultima_base = 0;
        while i < n {
            // Uma linha: até onde cabe.
            let (fa, fb) = self.faixa_livre(oy + y, altura_padrao, ox, ox + largura);
            let (mut esq, mut dir) = (fa - ox, fb - ox);
            let recuo = if primeira { r(e.recuo, largura) } else { 0 };
            let mut x = recuo;
            let inicio = i;
            let mut fim_linha = i;
            let mut largura_ate = x;
            let mut quebra_forcada = false;
            let mut algo = false;
            let mut j = i;
            let mut flutuantes: Vec<u32> = Vec::new();
            while j < n {
                match &itens[j] {
                    Item::Quebra => {
                        fim_linha = j + 1;
                        quebra_forcada = true;
                        largura_ate = x;
                        break;
                    }
                    Item::Atomo(a) if self.cx[*a as usize].e.fora_do_fluxo() => {
                        self.cx[*a as usize].estatica = (x, y);
                        j += 1;
                        if !algo {
                            fim_linha = j;
                        }
                        continue;
                    }
                    Item::Atomo(a) if self.cx[*a as usize].e.flutua != 0 => {
                        flutuantes.push(*a);
                        self.colocar_float(*a, largura, y);
                        let (fa2, fb2) = self.faixa_livre(oy + y, altura_padrao, ox, ox + largura);
                        esq = fa2 - ox;
                        dir = fb2 - ox;
                        j += 1;
                        if !algo {
                            fim_linha = j;
                        }
                        continue;
                    }
                    Item::Abre(f) => {
                        let v = self.lado_inline(*f, 3);
                        x += v;
                        j += 1;
                        continue;
                    }
                    Item::Fecha(f) => {
                        x += self.lado_inline(*f, 1);
                        j += 1;
                        if algo {
                            fim_linha = j;
                            largura_ate = x;
                        }
                        continue;
                    }
                    Item::Espaco { largura: l, .. } => {
                        if !algo {
                            // Espaço no começo da linha some.
                            j += 1;
                            if j <= n {
                                fim_linha = j;
                            }
                            continue;
                        }
                        x += l;
                        j += 1;
                        continue;
                    }
                    Item::Palavra { largura: l, caixa, .. } => {
                        let pre = self.cx[*caixa as usize].e.branco;
                        let quebra_ok = !(pre == 1 || pre == 2 || e.branco == 1 || e.branco == 2);
                        if algo && quebra_ok && x + l > dir - esq {
                            break;
                        }
                        x += l;
                    }
                    Item::Atomo(a) => {
                        let a = *a;
                        self.dispor_atomo(a, largura);
                        let w = self.externa_w(a);
                        if algo && x + w > dir - esq && e.branco != 1 && e.branco != 2 {
                            break;
                        }
                        x += w;
                    }
                }
                algo = true;
                j += 1;
                fim_linha = j;
                largura_ate = x;
            }
            if j >= n && fim_linha < n {
                fim_linha = n;
            }
            if fim_linha == inicio {
                fim_linha = (inicio + 1).min(n);
            }
            let _ = flutuantes;
            // A palavra sozinha é maior que a linha: `word-break` pode cortá-la.
            let itens_linha = &itens[inicio..fim_linha];
            let mut extra: Option<Vec<Item>> = None;
            if largura_ate > dir - esq && e.quebra_palavra {
                extra = self.cortar_palavra(itens_linha, dir - esq);
            }
            // A altura da linha.
            let (sobe_e, alt_e) = self.metricas_de(&e);
            let meia = (altura_padrao - alt_e) / 2;
            let (mut acima, mut abaixo) = (sobe_e + meia, altura_padrao - sobe_e - meia);
            if itens_linha.iter().all(|t| matches!(t, Item::Abre(_) | Item::Fecha(_) | Item::Espaco { .. })) && !itens_linha.iter().any(|t| matches!(t, Item::Espaco { .. })) && !quebra_forcada {
                (acima, abaixo) = (0, 0);
            }
            let lista: &[Item] = extra.as_deref().unwrap_or(itens_linha);
            let mut algum_alinhado = 0;
            for t in lista.iter() {
                match t {
                    Item::Palavra { caixa, .. } | Item::Espaco { caixa, .. } => {
                        let ei = self.cx[*caixa as usize].e.clone();
                        let (s, alt) = self.metricas_de(&ei);
                        let lh = self.altura_de_linha(&ei);
                        let (sub, _) = self.deslocamento_v(*caixa, &e);
                        let mei = (lh - alt) / 2;
                        acima = acima.max(s + mei - sub);
                        abaixo = abaixo.max(lh - s - mei + sub);
                    }
                    Item::Atomo(a) if !self.cx[*a as usize].e.fora_do_fluxo() && self.cx[*a as usize].e.flutua == 0 => {
                        let h = self.externa_h(*a);
                        let ea = self.cx[*a as usize].e.clone();
                        match ea.alinha_v {
                            1 => {
                                let meio = sobe_e / 2;
                                acima = acima.max(h / 2 + meio);
                                abaixo = abaixo.max(h - h / 2 - meio);
                            }
                            2 | 3 | 6 | 7 => algum_alinhado = algum_alinhado.max(h),
                            _ => {
                                let base = self.cx[*a as usize].baseline;
                                let s = if base > 0 { base + self.cx[*a as usize].margem[0] } else { h };
                                acima = acima.max(s + ea.desloca_v as i32);
                                abaixo = abaixo.max((h - s - ea.desloca_v as i32).max(0));
                            }
                        }
                    }
                    _ => {}
                }
            }
            let mut altura_l = acima + abaixo;
            if algum_alinhado > altura_l {
                abaixo += algum_alinhado - altura_l;
                altura_l = algum_alinhado;
            }
            // Com a altura certa, os floats podem apertar mais.
            if altura_l > altura_padrao {
                let (fa2, fb2) = self.faixa_livre(oy + y, altura_l, ox, ox + largura);
                esq = esq.max(fa2 - ox);
                dir = dir.min(fb2 - ox);
            }
            let disponivel = dir - esq;
            let mut x_inicio = esq + recuo;
            let total: i32 = self.largura_itens(lista);
            let (mut por_espaco, mut resto_esp) = (0, 0);
            match e.alinha {
                1 => x_inicio += (disponivel - recuo - total).max(0) / 2,
                2 => x_inicio += (disponivel - recuo - total).max(0),
                3 if !quebra_forcada && fim_linha < n => {
                    let espacos = lista.iter().filter(|t| matches!(t, Item::Espaco { .. })).count() as i32;
                    if espacos > 0 && total < disponivel {
                        por_espaco = (disponivel - recuo - total) / espacos;
                        resto_esp = (disponivel - recuo - total) % espacos;
                    }
                }
                _ => {}
            }
            // Coloca cada coisa no lugar.
            let base = y + acima;
            ultima_base = base;
            let mut px = x_inicio;
            // Caixas em linha que continuam da linha anterior.
            let mut inicio_caixas: Vec<(u32, i32)> = abertas.iter().map(|a| (*a, px)).collect();
            let ultimo_visivel = lista.iter().rposition(|t| !matches!(t, Item::Fecha(_) | Item::Abre(_)));
            for (idx, t) in lista.iter().enumerate() {
                match t {
                    Item::Abre(f) => {
                        px += self.cx[*f as usize].margem[3];
                        inicio_caixas.push((*f, px));
                        abertas.push(*f);
                        px += self.cx[*f as usize].borda[3] + self.cx[*f as usize].preench[3];
                    }
                    Item::Fecha(f) => {
                        px += self.cx[*f as usize].borda[1] + self.cx[*f as usize].preench[1];
                        if let Some(p) = inicio_caixas.iter().rposition(|(q, _)| q == f) {
                            let (_, ini) = inicio_caixas.remove(p);
                            self.fundo_inline(*f, ini, px - ini, base, &mut fundos);
                        }
                        abertas.retain(|q| q != f);
                        px += self.cx[*f as usize].margem[1];
                    }
                    Item::Espaco { largura: l, caixa } => {
                        if Some(idx) > ultimo_visivel {
                            continue;
                        }
                        let w = *l + por_espaco + if resto_esp > 0 { 1 } else { 0 };
                        if resto_esp > 0 {
                            resto_esp -= 1;
                        }
                        let ei = self.cx[*caixa as usize].e.clone();
                        if ei.sublinhado || ei.riscado {
                            let (s, alt) = self.metricas_de(&ei);
                            let (sub, _) = self.deslocamento_v(*caixa, &e);
                            pedacos.push(Pedaco { x: px, y: base - s + sub, largura: w, altura: alt, texto: String::new(), caixa: *caixa });
                        }
                        px += w;
                    }
                    Item::Palavra { texto, largura: l, caixa } => {
                        let ei = self.cx[*caixa as usize].e.clone();
                        let (s, alt) = self.metricas_de(&ei);
                        let (sub, _) = self.deslocamento_v(*caixa, &e);
                        let mut texto = texto.clone();
                        let mut w = *l;
                        if reticencias && px + w > dir {
                            let (t2, w2) = self.encurtar(&texto, (dir - px).max(0), &ei);
                            texto = t2;
                            w = w2;
                        }
                        pedacos.push(Pedaco { x: px, y: base - s + sub, largura: w, altura: alt, texto, caixa: *caixa });
                        px += w;
                    }
                    Item::Atomo(a) => {
                        let a = *a;
                        if self.cx[a as usize].e.fora_do_fluxo() || self.cx[a as usize].e.flutua != 0 {
                            if self.cx[a as usize].e.fora_do_fluxo() {
                                self.cx[a as usize].estatica = (px, y);
                            }
                            continue;
                        }
                        let h = self.externa_h(a);
                        let ea = self.cx[a as usize].e.clone();
                        let topo = match ea.alinha_v {
                            1 => base - sobe_e / 2 - h / 2,
                            2 | 6 => y,
                            3 | 7 => y + altura_l - h,
                            4 => base - h + sobe_e / 3,
                            5 => base - h - sobe_e / 2,
                            _ => {
                                let b = self.cx[a as usize].baseline;
                                let s = if b > 0 { b + self.cx[a as usize].margem[0] } else { h };
                                base - s - ea.desloca_v as i32
                            }
                        };
                        let m = self.cx[a as usize].margem;
                        self.cx[a as usize].x = px + m[3];
                        self.cx[a as usize].y = topo + m[0];
                        px += self.externa_w(a);
                    }
                    Item::Quebra => {}
                }
            }
            // As que continuam abertas na próxima linha.
            for (f, ini) in inicio_caixas {
                self.fundo_inline(f, ini, px - ini, base, &mut fundos);
            }
            y += altura_l;
            i = fim_linha;
            primeira = false;
            if let Some(ex) = extra {
                // O resto da palavra cortada volta para a fila.
                let _ = ex;
            }
        }
        let x = &mut self.cx[c as usize];
        x.pedacos = pedacos;
        x.fundos = fundos;
        x.baseline = ultima_base;
        y
    }

    fn largura_itens(&self, lista: &[Item]) -> i32 {
        let mut t = 0;
        let ultimo = lista.iter().rposition(|x| !matches!(x, Item::Fecha(_) | Item::Abre(_) | Item::Espaco { .. }));
        for (i, x) in lista.iter().enumerate() {
            t += match x {
                Item::Palavra { largura, .. } => *largura,
                Item::Espaco { largura, .. } => {
                    if Some(i) > ultimo {
                        0
                    } else {
                        *largura
                    }
                }
                Item::Atomo(a) => {
                    let e = &self.cx[*a as usize].e;
                    if e.fora_do_fluxo() || e.flutua != 0 { 0 } else { self.externa_w(*a) }
                }
                Item::Abre(f) => self.cx[*f as usize].margem[3] + self.cx[*f as usize].borda[3] + self.cx[*f as usize].preench[3],
                Item::Fecha(f) => self.cx[*f as usize].margem[1] + self.cx[*f as usize].borda[1] + self.cx[*f as usize].preench[1],
                Item::Quebra => 0,
            };
        }
        t
    }

    fn lado_inline(&self, f: u32, lado: usize) -> i32 {
        let x = &self.cx[f as usize];
        x.margem[lado] + x.borda[lado] + x.preench[lado]
    }

    fn deslocamento_v(&mut self, c: u32, _bloco: &Estilo) -> (i32, i32) {
        // sub, super e deslocamentos: somam os das caixas em linha até o bloco.
        let mut total = 0;
        let mut x = c;
        loop {
            let cx = &self.cx[x as usize];
            if !matches!(cx.tipo, Tipo::Texto(_) | Tipo::Inline) {
                break;
            }
            if cx.tipo == Tipo::Inline {
                let f = cx.e.fonte as i32;
                total += match cx.e.alinha_v {
                    4 => f / 3,
                    5 => -(f / 2),
                    _ => -(cx.e.desloca_v as i32),
                };
            }
            x = cx.pai;
            if x == N {
                break;
            }
        }
        (total, 0)
    }

    fn fundo_inline(&mut self, f: u32, x: i32, w: i32, base: i32, fundos: &mut Vec<(u32, i32, i32, i32, i32)>) {
        let e = self.cx[f as usize].e.clone();
        let (s, alt) = self.metricas_de(&e);
        let b = self.cx[f as usize].borda;
        let p = self.cx[f as usize].preench;
        fundos.push((f, x, base - s - b[0] - p[0], w, alt + b[0] + p[0] + b[2] + p[2]));
    }

    fn cortar_palavra(&mut self, itens: &[Item], _l: i32) -> Option<Vec<Item>> {
        let _ = itens;
        None
    }

    fn encurtar(&mut self, t: &str, largura: i32, e: &Estilo) -> (String, i32) {
        let mut s = String::new();
        let mut ultimo = (String::from("…"), self.largura_texto("…", e));
        for c in t.chars() {
            s.push(c);
            let mut cand = s.clone();
            cand.push('…');
            let w = self.largura_texto(&cand, e);
            if w > largura {
                break;
            }
            ultimo = (cand, w);
        }
        ultimo
    }

    fn externa_w(&self, a: u32) -> i32 {
        let x = &self.cx[a as usize];
        x.w + x.margem[1] + x.margem[3] + x.preench[1] + x.preench[3] + x.borda[1] + x.borda[3]
    }

    fn externa_h(&self, a: u32) -> i32 {
        let x = &self.cx[a as usize];
        x.h + x.margem[0] + x.margem[2] + x.preench[0] + x.preench[2] + x.borda[0] + x.borda[2]
    }

    fn borda_h(&self, a: u32) -> i32 {
        let x = &self.cx[a as usize];
        x.h + x.preench[0] + x.preench[2] + x.borda[0] + x.borda[2]
    }

    fn borda_w(&self, a: u32) -> i32 {
        let x = &self.cx[a as usize];
        x.w + x.preench[1] + x.preench[3] + x.borda[1] + x.borda[3]
    }

    /// Mede um inline-block, imagem, controle.
    fn dispor_atomo(&mut self, a: u32, cb: i32) {
        self.medir_caixa(a, cb);
        let e = self.cx[a as usize].e.clone();
        let hor = self.horizontal(a);
        match self.cx[a as usize].tipo {
            Tipo::Imagem(..) | Tipo::Controle | Tipo::Substituido => {
                let (w, h) = self.tamanho_substituido(a, Some(cb), None);
                let x = &mut self.cx[a as usize];
                x.w = w;
                x.h = h;
            }
            _ => {
                let w = match opt(e.largura, Some(cb)) {
                    Some(v) => {
                        if e.caixa_borda { (v - hor).max(0) } else { v }
                    }
                    None => {
                        let (mi, ma) = self.intrinsecas(a);
                        (ma.min((cb - self.margem_h(a)).max(mi)) - hor).max(0)
                    }
                };
                let w = self.limitar_largura(a, w, cb);
                self.novo_contexto(a, w, opt(e.altura, None));
            }
        }
    }

    fn margem_h(&self, a: u32) -> i32 {
        let x = &self.cx[a as usize];
        x.margem[1] + x.margem[3]
    }

    fn limitar_largura(&mut self, c: u32, mut w: i32, cb: i32) -> i32 {
        let e = self.cx[c as usize].e.clone();
        let hor = self.horizontal(c);
        let ajusta = |v: i32| if e.caixa_borda { (v - hor).max(0) } else { v };
        if let Some(m) = opt(e.max_largura, Some(cb)) {
            w = w.min(ajusta(m));
        }
        if let Some(m) = opt(e.min_largura, Some(cb)) {
            w = w.max(ajusta(m));
        }
        w.max(0)
    }

    fn limitar_altura(&mut self, c: u32, mut h: i32, cb_h: Option<i32>) -> i32 {
        let e = self.cx[c as usize].e.clone();
        let ver = self.vertical(c);
        let ajusta = |v: i32| if e.caixa_borda { (v - ver).max(0) } else { v };
        if let Some(m) = opt(e.max_altura, cb_h) {
            h = h.min(ajusta(m));
        }
        if let Some(m) = opt(e.min_altura, cb_h) {
            h = h.max(ajusta(m));
        }
        h.max(0)
    }

    /// Dispõe uma caixa num contexto de floats próprio. `h`: altura do
    /// conteúdo, se já se sabe (a borda de uma altura fixa é tirada aqui).
    fn novo_contexto(&mut self, c: u32, w: i32, h_css: Option<i32>) {
        if let Some((mw, mh, res)) = self.cx[c as usize].memo {
            if mw == w && mh == h_css {
                let x = &mut self.cx[c as usize];
                x.w = w;
                x.h = res;
                return;
            }
        }
        let e = self.cx[c as usize].e.clone();
        let ver = self.vertical(c);
        let h_def = h_css.map(|v| if e.caixa_borda { (v - ver).max(0) } else { v });
        self.cx[c as usize].w = w;
        self.floats.push(Floats { esquerda: Vec::new(), direita: Vec::new() });
        let antes = self.desloc;
        self.desloc = (0, 0);
        let conteudo = self.dispor_conteudo(c, w, h_def);
        let f = self.floats.pop().unwrap();
        self.desloc = antes;
        let fundo_floats = f.esquerda.iter().chain(f.direita.iter()).map(|(_, y, _, h)| y + h).max().unwrap_or(0);
        let auto = h_def.is_none();
        let mut h = h_def.unwrap_or(conteudo.max(fundo_floats));
        h = self.limitar_altura(c, h, None);
        let x = &mut self.cx[c as usize];
        x.h = h;
        if auto {
            x.memo = Some((w, h_css, h));
        }
        self.posicionar_filhas(c);
    }

    fn dispor_conteudo(&mut self, c: u32, w: i32, h: Option<i32>) -> i32 {
        match self.cx[c as usize].tipo {
            Tipo::Flex => self.dispor_flex(c, w, h),
            Tipo::Grid => self.dispor_grid(c, w, h),
            Tipo::Tabela => self.dispor_tabela(c, w),
            _ => {
                if self.cx[c as usize].em_linha {
                    self.dispor_linhas(c, w)
                } else {
                    let (a, _, _) = self.fluxo_blocos(c, w, h, false);
                    a
                }
            }
        }
    }

    /// Os filhos em blocos, um embaixo do outro (com as margens se juntando).
    /// Devolve (altura, margem de cima que passa para o pai, margem de baixo idem).
    fn fluxo_blocos(&mut self, c: u32, w: i32, h_cb: Option<i32>, deixa_passar: bool) -> (i32, i32, i32) {
        let mut y = 0;
        let (mut pos, mut neg) = (0, 0);
        let mut passa_cima = 0;
        let mut primeiro = true;
        let topo_livre = deixa_passar && self.cx[c as usize].preench[0] + self.cx[c as usize].borda[0] == 0;
        let (ox, oy) = self.desloc;
        let base = |x: &Caixa| (x.borda[3] + x.preench[3], x.borda[0] + x.preench[0]);
        let (bx, by) = base(&self.cx[c as usize]);
        for f in self.cx[c as usize].filhos.clone() {
            let ef = self.cx[f as usize].e.clone();
            if ef.fora_do_fluxo() {
                self.cx[f as usize].estatica = (0, y + pos + neg);
                continue;
            }
            if ef.flutua != 0 {
                self.colocar_float(f, w, y + pos + neg);
                continue;
            }
            if matches!(self.cx[f as usize].tipo, Tipo::Texto(_)) {
                continue;
            }
            if ef.limpa != 0 {
                let fundo = self.fundo_floats(ef.limpa) - oy - by;
                if fundo > y + pos + neg {
                    y = fundo;
                    pos = 0;
                    neg = 0;
                    primeiro = false;
                }
            }
            self.medir_caixa(f, w);
            let mf = self.cx[f as usize].margem;
            let (mut mc, mut mb) = (mf[0], mf[2]);
            // O deslocamento desta caixa para os floats, antes de saber as margens finais.
            let y_prov = y + pos.max(mc) + neg.min(mc);
            self.desloc = (ox + bx, oy + by + y_prov);
            let (pm_c, pm_b) = self.dispor_bloco(f, w, h_cb);
            self.desloc = (ox, oy);
            // As margens do primeiro/último filho se juntam às da caixa.
            mc = junta(mc, pm_c);
            mb = junta(mb, pm_b);
            let vazio = self.borda_h(f) == 0;
            if vazio {
                // Uma caixa vazia: as margens de cima e de baixo se juntam.
                let m = junta(mc, mb);
                pos = pos.max(m);
                neg = neg.min(m);
                if primeiro && topo_livre {
                    self.cx[f as usize].y = 0;
                } else {
                    self.cx[f as usize].y = y + pos + neg;
                }
                self.posicionar_x(f, w);
                continue;
            }
            if primeiro && topo_livre {
                passa_cima = junta(pos + neg, mc);
                self.cx[f as usize].y = 0;
            } else {
                pos = pos.max(mc);
                neg = neg.min(mc);
                y += pos + neg;
                self.cx[f as usize].y = y;
            }
            primeiro = false;
            self.posicionar_x(f, w);
            y += self.borda_h(f);
            pos = mb.max(0);
            neg = mb.min(0);
        }
        let baixo_livre = deixa_passar && self.cx[c as usize].preench[2] + self.cx[c as usize].borda[2] == 0 && self.cx[c as usize].e.altura.e_auto();
        if baixo_livre {
            (y, passa_cima, pos + neg)
        } else {
            (y + pos + neg, passa_cima, 0)
        }
    }

    fn fundo_floats(&self, lado: u8) -> i32 {
        let Some(f) = self.floats.last() else { return 0 };
        let mut m = 0;
        if lado & 1 != 0 {
            m = f.esquerda.iter().map(|(_, y, _, h)| y + h).max().unwrap_or(0);
        }
        if lado & 2 != 0 {
            m = m.max(f.direita.iter().map(|(_, y, _, h)| y + h).max().unwrap_or(0));
        }
        m
    }

    /// x de um bloco: margens automáticas centralizam.
    fn posicionar_x(&mut self, f: u32, w: i32) {
        let total = self.borda_w(f);
        let x = &self.cx[f as usize];
        let (ml, mr) = (x.e.margem[3].e_auto(), x.e.margem[1].e_auto());
        // <center> e align="center" centralizam também os blocos de dentro.
        let pai = x.pai;
        let centra = pai != N && {
            let no = self.cx[pai as usize].no;
            let d = self.d;
            no != NADA && (d.e(no, "center") || d.atributo(no, "align").is_some_and(|a| a.eq_ignore_ascii_case("center")))
        };
        let x = &self.cx[f as usize];
        let px = if ml && mr || (centra && !x.e.largura.e_auto()) {
            ((w - total) / 2).max(0)
        } else if ml {
            w - total - x.margem[1]
        } else {
            x.margem[3]
        };
        self.cx[f as usize].x = px;
    }

    /// Dispõe um bloco no fluxo. Devolve as margens que passam dos filhos.
    fn dispor_bloco(&mut self, f: u32, cb: i32, cb_h: Option<i32>) -> (i32, i32) {
        let e = self.cx[f as usize].e.clone();
        let hor = self.horizontal(f);
        let mh = self.margem_h(f);
        let tipo = self.cx[f as usize].tipo.clone();
        if matches!(tipo, Tipo::Imagem(..) | Tipo::Controle | Tipo::Substituido) {
            let (w, h) = self.tamanho_substituido(f, Some(cb), cb_h);
            let x = &mut self.cx[f as usize];
            x.w = w;
            x.h = h;
            return (0, 0);
        }
        let w = match e.largura {
            Tam::MinConteudo => (self.intrinsecas(f).0 - hor).max(0),
            Tam::MaxConteudo => (self.intrinsecas(f).1 - hor).max(0),
            Tam::AjustaConteudo => (self.intrinsecas(f).1.min(cb - mh) - hor).max(0),
            _ => match opt(e.largura, Some(cb)) {
                Some(v) => {
                    if e.caixa_borda { (v - hor).max(0) } else { v }
                }
                None if tipo == Tipo::Tabela => {
                    let (mi, ma) = self.intrinsecas(f);
                    (ma.min((cb - mh).max(mi)) - hor).max(0)
                }
                None => (cb - mh - hor).max(0),
            },
        };
        let w = self.limitar_largura(f, w, cb);
        let h_css = opt(e.altura, cb_h);
        // Quem começa um contexto de floats próprio?
        let contexto = e.recorta || e.overflow_rola || !matches!(tipo, Tipo::Bloco) || e.posicao == Posicao::Absoluta;
        if contexto {
            self.novo_contexto(f, w, h_css);
            return (0, 0);
        }
        self.cx[f as usize].w = w;
        let ver = self.vertical(f);
        let h_def = h_css.map(|v| if e.caixa_borda { (v - ver).max(0) } else { v });
        let (conteudo, pc, pb) = if self.cx[f as usize].em_linha {
            let (a, b) = self.desloc;
            let x = &self.cx[f as usize];
            self.desloc = (a + x.borda[3] + x.preench[3], b + x.borda[0] + x.preench[0]);
            let h = self.dispor_linhas(f, w);
            self.desloc = (a, b);
            (h, 0, 0)
        } else {
            self.fluxo_blocos(f, w, h_def, true)
        };
        let h = self.limitar_altura(f, h_def.unwrap_or(conteudo), cb_h);
        self.cx[f as usize].h = h;
        self.posicionar_filhas(f);
        (pc, pb)
    }

    /// Um float: vai para o lado até onde couber.
    fn colocar_float(&mut self, f: u32, largura: i32, y_atual: i32) {
        self.medir_caixa(f, largura);
        let e = self.cx[f as usize].e.clone();
        let hor = self.horizontal(f);
        let mh = self.margem_h(f);
        let tipo = self.cx[f as usize].tipo.clone();
        if matches!(tipo, Tipo::Imagem(..) | Tipo::Controle | Tipo::Substituido) {
            let (w, h) = self.tamanho_substituido(f, Some(largura), None);
            self.cx[f as usize].w = w;
            self.cx[f as usize].h = h;
        } else {
            let w = match opt(e.largura, Some(largura)) {
                Some(v) => {
                    if e.caixa_borda { (v - hor).max(0) } else { v }
                }
                None => {
                    let (mi, ma) = self.intrinsecas(f);
                    (ma.min((largura - mh).max(mi)) - hor).max(0)
                }
            };
            let w = self.limitar_largura(f, w, largura);
            self.novo_contexto(f, w, opt(e.altura, None));
        }
        let (fw, fh) = (self.externa_w(f), self.externa_h(f));
        let (ox, oy) = self.desloc;
        let mut y = oy + y_atual;
        if e.limpa != 0 {
            y = y.max(self.fundo_floats(e.limpa));
        }
        // Não sobe acima de um float anterior.
        if let Some(fl) = self.floats.last() {
            for &(_, fy, _, _) in fl.esquerda.iter().chain(fl.direita.iter()) {
                y = y.max(fy);
            }
        }
        loop {
            let (a, b) = self.faixa_livre(y, fh.max(1), ox, ox + largura);
            if b - a >= fw || (a == ox && b == ox + largura) {
                let x = if e.flutua == 1 { a } else { b - fw };
                let m = self.cx[f as usize].margem;
                if let Some(fl) = self.floats.last_mut() {
                    let alvo = if e.flutua == 1 { &mut fl.esquerda } else { &mut fl.direita };
                    alvo.push((x, y, fw, fh));
                }
                let cx = &mut self.cx[f as usize];
                cx.x = x - ox + m[3];
                cx.y = y - oy + m[0];
                return;
            }
            // Desce até o próximo float acabar.
            let mut prox = i32::MAX;
            if let Some(fl) = self.floats.last() {
                for &(_, fy, _, fh2) in fl.esquerda.iter().chain(fl.direita.iter()) {
                    if fy + fh2 > y {
                        prox = prox.min(fy + fh2);
                    }
                }
            }
            if prox == i32::MAX {
                let x = if e.flutua == 1 { a } else { b - fw };
                let m = self.cx[f as usize].margem;
                let cx = &mut self.cx[f as usize];
                cx.x = x - ox + m[3];
                cx.y = y - oy + m[0];
                return;
            }
            y = prox;
        }
    }

    // ------------------------------------------------------------ posicionadas

    fn posicionar_filhas(&mut self, c: u32) {
        if self.cx[c as usize].posicionadas.is_empty() || c == self.raiz {
            return;
        }
        let w = self.cx[c as usize].w + self.cx[c as usize].preench[1] + self.cx[c as usize].preench[3];
        let h = self.cx[c as usize].h + self.cx[c as usize].preench[0] + self.cx[c as usize].preench[2];
        for f in self.cx[c as usize].posicionadas.clone() {
            self.dispor_posicionada(f, c, w, h);
        }
    }

    /// Onde a caixa estaria no fluxo, relativo à borda de `cb`.
    fn estatica_em(&self, f: u32, cb: u32) -> (i32, i32) {
        let x = &self.cx[f as usize];
        let (mut sx, mut sy) = x.estatica;
        let mut p = x.pai;
        let mut primeiro = true;
        while p != N && p != cb {
            let q = &self.cx[p as usize];
            if primeiro || q.tipo != Tipo::Inline {
                sx += q.x + q.borda[3] + q.preench[3];
                sy += q.y + q.borda[0] + q.preench[0];
            }
            primeiro = false;
            p = q.pai;
        }
        if p == cb && cb != N {
            let q = &self.cx[cb as usize];
            sx += q.preench[3];
            sy += q.preench[0];
        }
        (sx, sy)
    }

    fn dispor_posicionada(&mut self, f: u32, cb: u32, w_cb: i32, h_cb: i32) {
        self.medir_caixa(f, w_cb);
        let e = self.cx[f as usize].e.clone();
        let hor = self.horizontal(f);
        let ver = self.vertical(f);
        let mh = self.margem_h(f);
        let tipo = self.cx[f as usize].tipo.clone();
        let (esq, dir, topo, baixo) = (opt(e.esquerda, Some(w_cb)), opt(e.direita, Some(w_cb)), opt(e.topo, Some(h_cb)), opt(e.baixo, Some(h_cb)));
        let (m_cima, m_baixo) = (self.cx[f as usize].margem[0], self.cx[f as usize].margem[2]);
        if matches!(tipo, Tipo::Imagem(..) | Tipo::Controle | Tipo::Substituido) {
            let (w, h) = self.tamanho_substituido(f, Some(w_cb), Some(h_cb));
            self.cx[f as usize].w = w;
            self.cx[f as usize].h = h;
        } else {
            let w = match opt(e.largura, Some(w_cb)) {
                Some(v) => {
                    if e.caixa_borda { (v - hor).max(0) } else { v }
                }
                None => match (esq, dir) {
                    (Some(a), Some(b)) => (w_cb - a - b - mh - hor).max(0),
                    _ => {
                        let (mi, ma) = self.intrinsecas(f);
                        (ma.min((w_cb - mh).max(mi)) - hor).max(0)
                    }
                },
            };
            let w = self.limitar_largura(f, w, w_cb);
            let h_css = match opt(e.altura, Some(h_cb)) {
                Some(v) => Some(v),
                None => match (topo, baixo) {
                    (Some(a), Some(b)) if e.altura.e_auto() => {
                        let v = h_cb - a - b - m_cima - m_baixo;
                        Some(if e.caixa_borda { v } else { (v - ver).max(0) })
                    }
                    _ => None,
                },
            };
            self.cx[f as usize].memo = None;
            self.novo_contexto(f, w, h_css);
        }
        let (bw, bh) = (self.borda_w(f), self.borda_h(f));
        let (sx, sy) = self.estatica_em(f, cb);
        let bx = if cb == N { 0 } else { self.cx[cb as usize].borda[3] };
        let by = if cb == N { 0 } else { self.cx[cb as usize].borda[0] };
        let m = self.cx[f as usize].margem;
        let x = match (esq, dir) {
            (Some(a), Some(b)) if e.margem[3].e_auto() && e.margem[1].e_auto() => bx + a + (w_cb - a - b - bw) / 2,
            (Some(a), _) => bx + a + m[3],
            (None, Some(b)) => bx + w_cb - b - bw - m[1],
            _ => sx + m[3],
        };
        let y = match (topo, baixo) {
            (Some(a), Some(b)) if e.margem[0].e_auto() && e.margem[2].e_auto() => by + a + (h_cb - a - b - bh) / 2,
            (Some(a), _) => by + a + m[0],
            (None, Some(b)) => by + h_cb - b - bh - m[2],
            _ => sy + m[0],
        };
        self.cx[f as usize].x = x;
        self.cx[f as usize].y = y;
    }

    // ------------------------------------------------------------ flexbox

    fn dispor_flex(&mut self, c: u32, w: i32, h: Option<i32>) -> i32 {
        let e = self.cx[c as usize].e.clone();
        let linha = e.flex_direcao < 2;
        let reverso = e.flex_direcao & 1 != 0;
        let (gap_p, gap_c) = if linha { (r(e.vao_coluna, w), r(e.vao_linha, h.unwrap_or(0))) } else { (r(e.vao_linha, h.unwrap_or(0)), r(e.vao_coluna, w)) };
        let principal: Option<i32> = if linha { Some(w) } else { h };
        let cruzado: Option<i32> = if linha { h } else { Some(w) };
        let mut itens: Vec<u32> = self.cx[c as usize].filhos.iter().copied().filter(|f| !self.cx[*f as usize].e.fora_do_fluxo()).collect();
        for f in self.cx[c as usize].filhos.clone() {
            if self.cx[f as usize].e.fora_do_fluxo() {
                self.cx[f as usize].estatica = (0, 0);
            }
        }
        itens.sort_by_key(|f| self.cx[*f as usize].e.ordem);
        struct It {
            c: u32,
            base: i32,
            hip: i32,
            min: i32,
            max: i32,
            congelado: bool,
            externo: i32,
            tam: i32,
            cresce: f32,
            encolhe: f32,
        }
        let mut lista: Vec<It> = Vec::with_capacity(itens.len());
        for f in itens {
            self.medir_caixa(f, w);
            let ef = self.cx[f as usize].e.clone();
            let (hor, ver) = (self.horizontal(f), self.vertical(f));
            let x = &self.cx[f as usize];
            let (extra, m_ext) = if linha { (hor, x.margem[1] + x.margem[3]) } else { (ver, x.margem[0] + x.margem[2]) };
            let ajuste = |v: i32| if ef.caixa_borda { (v - extra).max(0) } else { v };
            let tam_css = if linha { ef.largura } else { ef.altura };
            let base_css = if ef.base_flex.e_auto() { tam_css } else { ef.base_flex };
            let base = match opt(base_css, principal) {
                Some(v) if !matches!(base_css, Tam::Pct(_) | Tam::PxPct(..)) || principal.is_some() => ajuste(v),
                _ => {
                    if linha {
                        (self.intrinsecas(f).1 - hor).max(0)
                    } else {
                        let cw = match opt(ef.largura, Some(w)) {
                            Some(v) => {
                                if ef.caixa_borda { (v - hor).max(0) } else { v }
                            }
                            None => (w - self.margem_h(f) - hor).max(0),
                        };
                        self.dispor_item(f, cw, None);
                        self.cx[f as usize].h
                    }
                }
            };
            let (mi_css, ma_css) = if linha { (ef.min_largura, ef.max_largura) } else { (ef.min_altura, ef.max_altura) };
            let min_auto = if linha && !ef.recorta && !ef.overflow_rola {
                let min_conteudo = (self.intrinsecas(f).0 - hor).max(0);
                // O mínimo automático não passa do tamanho pedido.
                match opt(tam_css, principal) {
                    Some(v) => min_conteudo.min(ajuste(v)),
                    None => min_conteudo,
                }
            } else {
                0
            };
            let min = opt(mi_css, principal).map_or(min_auto, ajuste);
            let max = opt(ma_css, principal).map_or(i32::MAX, ajuste);
            let hip = base.clamp(min, max.max(min));
            lista.push(It { c: f, base, hip, min, max, congelado: false, externo: extra + m_ext, tam: hip, cresce: ef.cresce, encolhe: ef.encolhe });
        }
        // As linhas do flex.
        let mut linhas: Vec<Vec<usize>> = Vec::new();
        let quebra = e.flex_quebra != 0;
        let mut atual = Vec::new();
        let mut soma = 0;
        for (i, it) in lista.iter().enumerate() {
            let t = it.hip + it.externo;
            if quebra && !atual.is_empty() && principal.is_some_and(|p| soma + gap_p + t > p) {
                linhas.push(core::mem::take(&mut atual));
                soma = 0;
            }
            soma += if atual.is_empty() { t } else { gap_p + t };
            atual.push(i);
        }
        if !atual.is_empty() {
            linhas.push(atual);
        }
        // Distribui o espaço de cada linha.
        for l in &linhas {
            let Some(p) = principal else {
                for &i in l {
                    lista[i].tam = lista[i].hip;
                }
                continue;
            };
            let gaps = gap_p * (l.len() as i32 - 1).max(0);
            let usado: i32 = l.iter().map(|&i| lista[i].hip + lista[i].externo).sum();
            let crescendo = usado + gaps < p;
            for &i in l {
                let it = &mut lista[i];
                it.tam = it.base;
                let fator = if crescendo { it.cresce } else { it.encolhe };
                it.congelado = fator == 0.0 || (crescendo && it.base > it.hip) || (!crescendo && it.base < it.hip);
                if it.congelado {
                    it.tam = it.hip;
                }
            }
            for _ in 0..8 {
                let (mut livre, mut soma_fatores) = (p - gaps, 0.0f32);
                for &i in l {
                    let it = &lista[i];
                    if it.congelado {
                        livre -= it.tam + it.externo;
                    } else {
                        livre -= it.base + it.externo;
                        soma_fatores += if crescendo { it.cresce } else { it.encolhe * it.base as f32 };
                    }
                }
                if soma_fatores <= 0.0 {
                    break;
                }
                let so_fracao = crescendo && soma_fatores < 1.0;
                let mut violou = 0;
                for &i in l {
                    let it = &mut lista[i];
                    if it.congelado {
                        continue;
                    }
                    let fator = if crescendo { it.cresce } else { it.encolhe * it.base as f32 };
                    let parte = livre as f32 * fator / if so_fracao { 1.0 } else { soma_fatores };
                    let bruto = crate::mat::floor((it.base as f32 + parte) as f64) as i32;
                    it.tam = bruto.clamp(it.min, it.max.max(it.min));
                    violou += it.tam - bruto;
                }
                if violou == 0 {
                    break;
                }
                for &i in l {
                    let it = &mut lista[i];
                    if !it.congelado {
                        let bruto_menor = it.tam == it.min;
                        let bruto_maior = it.tam == it.max;
                        if (violou > 0 && bruto_menor) || (violou < 0 && bruto_maior) {
                            it.congelado = true;
                        }
                    }
                }
            }
        }
        // O tamanho cruzado de cada item, e o de cada linha.
        let mut alturas_linha: Vec<i32> = Vec::with_capacity(linhas.len());
        let mut bases_linha: Vec<i32> = Vec::with_capacity(linhas.len());
        for l in &linhas {
            let mut mx = 0;
            let mut base_max = 0;
            for &i in l {
                let f = lista[i].c;
                let ef = self.cx[f as usize].e.clone();
                let (hor, ver) = (self.horizontal(f), self.vertical(f));
                if linha {
                    let h_css = opt(ef.altura, cruzado);
                    self.dispor_item(f, lista[i].tam, h_css);
                    mx = mx.max(self.externa_h(f));
                    if self.alinhamento(f, &e) == 3 {
                        base_max = base_max.max(self.cx[f as usize].baseline + self.cx[f as usize].margem[0] + self.cx[f as usize].borda[0] + self.cx[f as usize].preench[0]);
                    }
                } else {
                    let cw = match opt(ef.largura, Some(w)) {
                        Some(v) => {
                            if ef.caixa_borda { (v - hor).max(0) } else { v }
                        }
                        None if self.alinhamento(f, &e) == 4 => (w - self.margem_h(f) - hor).max(0),
                        None => {
                            let (mi, ma) = self.intrinsecas(f);
                            (ma.min((w - self.margem_h(f)).max(mi)) - hor).max(0)
                        }
                    };
                    let cw = self.limitar_largura(f, cw, w);
                    let v = lista[i].tam;
                    let forcada = if ef.caixa_borda { v + ver } else { v };
                    self.dispor_item(f, cw, if principal.is_some() || !ef.altura.e_auto() { Some(forcada) } else { None });
                    if principal.is_none() {
                        lista[i].tam = self.cx[f as usize].h;
                    }
                    mx = mx.max(self.externa_w(f));
                }
            }
            alturas_linha.push(mx);
            bases_linha.push(base_max);
        }
        if let (Some(cz), 1) = (cruzado, linhas.len()) {
            alturas_linha[0] = cz;
        }
        let soma_cruz: i32 = alturas_linha.iter().sum::<i32>() + gap_c * (linhas.len() as i32 - 1).max(0);
        // align-content
        let livre_c = cruzado.unwrap_or(soma_cruz) - soma_cruz;
        let n_l = linhas.len() as i32;
        if livre_c > 0 && n_l > 1 && e.alinha_conteudo == 6 {
            let extra = livre_c / n_l;
            for a in alturas_linha.iter_mut() {
                *a += extra;
            }
        }
        let (mut pos_c, entre_c) = distribuir(e.alinha_conteudo, livre_c.max(0), n_l, n_l > 1 && e.alinha_conteudo != 6);
        if e.flex_quebra == 2 {
            alturas_linha.reverse();
            linhas.reverse();
        }
        let mut maior_principal = 0;
        for (nl, l) in linhas.iter().enumerate() {
            let lin_cruz = alturas_linha[nl];
            let gaps = gap_p * (l.len() as i32 - 1).max(0);
            let usado: i32 = l.iter().map(|&i| lista[i].tam + lista[i].externo).sum::<i32>() + gaps;
            let tam_p = principal.unwrap_or(usado);
            maior_principal = maior_principal.max(usado);
            let mut livre = tam_p - usado;
            // Margens automáticas ficam com o que sobrar.
            let mut autos = 0;
            for &i in l {
                let ef = &self.cx[lista[i].c as usize].e;
                let (a, b) = if linha { (ef.margem[3].e_auto(), ef.margem[1].e_auto()) } else { (ef.margem[0].e_auto(), ef.margem[2].e_auto()) };
                autos += a as i32 + b as i32;
            }
            let por_auto = if autos > 0 && livre > 0 { livre / autos } else { 0 };
            if autos > 0 && livre > 0 {
                livre = 0;
            }
            let just = if reverso {
                match e.justifica {
                    0 | 6 => 1,
                    1 => 0,
                    o => o,
                }
            } else {
                e.justifica
            };
            let mut pos = 0;
            let n_itens = l.len() as i32;
            let ordem: Vec<usize> = if reverso { l.iter().rev().copied().collect() } else { l.clone() };
            for (n_ordem, &i) in ordem.iter().enumerate() {
                let antes_item = deslocamento_item(just, livre.max(0), n_itens, n_ordem as i32);
                let f = lista[i].c;
                let tam = lista[i].tam;
                let ef = self.cx[f as usize].e.clone();
                let (hor, ver) = (self.horizontal(f), self.vertical(f));
                let al = self.alinhamento(f, &e);
                if linha {
                    let auto_y = ef.margem[0].e_auto() || ef.margem[2].e_auto();
                    let mg = self.cx[f as usize].margem;
                    // stretch: a altura vem da linha.
                    if al == 4 && ef.altura.e_auto() && !auto_y {
                        let alvo = (lin_cruz - mg[0] - mg[2] - ver).max(0);
                        let alvo = self.limitar_altura(f, alvo, cruzado);
                        if alvo != self.cx[f as usize].h {
                            let forcada = if ef.caixa_borda { alvo + ver } else { alvo };
                            self.dispor_item(f, tam, Some(forcada));
                            self.cx[f as usize].h = alvo;
                        }
                    }
                    let h_ext = self.externa_h(f);
                    let mut y_item = match al {
                        1 => lin_cruz - h_ext,
                        2 => (lin_cruz - h_ext) / 2,
                        3 => bases_linha[nl] - (self.cx[f as usize].baseline + mg[0] + self.cx[f as usize].borda[0] + self.cx[f as usize].preench[0]),
                        _ => 0,
                    };
                    if auto_y {
                        y_item = if ef.margem[0].e_auto() && ef.margem[2].e_auto() { (lin_cruz - h_ext) / 2 } else if ef.margem[0].e_auto() { lin_cruz - h_ext } else { 0 };
                    }
                    let ml = if ef.margem[3].e_auto() { por_auto } else { mg[3] };
                    let mr = if ef.margem[1].e_auto() { por_auto } else { mg[1] };
                    let cx = &mut self.cx[f as usize];
                    cx.w = tam;
                    cx.x = pos + antes_item + ml;
                    cx.y = pos_c + y_item + mg[0];
                    pos += ml + tam + hor + mr + gap_p;
                } else {
                    let mg = self.cx[f as usize].margem;
                    let w_ext = self.externa_w(f);
                    let auto_x = ef.margem[3].e_auto() || ef.margem[1].e_auto();
                    let mut x_item = match al {
                        1 => lin_cruz - w_ext,
                        2 => (lin_cruz - w_ext) / 2,
                        _ => 0,
                    };
                    if auto_x {
                        x_item = if ef.margem[3].e_auto() && ef.margem[1].e_auto() { (lin_cruz - w_ext) / 2 } else if ef.margem[3].e_auto() { lin_cruz - w_ext } else { 0 };
                    }
                    let mc = if ef.margem[0].e_auto() { por_auto } else { mg[0] };
                    let mb = if ef.margem[2].e_auto() { por_auto } else { mg[2] };
                    let cx = &mut self.cx[f as usize];
                    cx.h = tam;
                    cx.x = pos_c + x_item + mg[3];
                    cx.y = pos + antes_item + mc;
                    pos += mc + tam + ver + mb + gap_p;
                }
            }
            pos_c += lin_cruz + gap_c + entre_c;
        }
        if linha { soma_cruz.max(if livre_c > 0 { soma_cruz } else { 0 }) } else { h.unwrap_or(maior_principal) }
    }

    fn alinhamento(&self, f: u32, pai: &Estilo) -> u8 {
        let a = self.cx[f as usize].e.alinha_proprio;
        if a == 5 { pai.alinha_itens } else { a }
    }

    fn dispor_item(&mut self, f: u32, w: i32, h: Option<i32>) {
        let tipo = self.cx[f as usize].tipo.clone();
        if matches!(tipo, Tipo::Imagem(..) | Tipo::Controle | Tipo::Substituido) {
            let (nw, nh) = self.tamanho_substituido(f, Some(w), h);
            let x = &mut self.cx[f as usize];
            x.w = w.max(if nw > 0 && w == 0 { nw } else { w });
            x.h = h.unwrap_or(nh);
            return;
        }
        self.novo_contexto(f, w, h);
        self.cx[f as usize].w = w;
    }

    // ------------------------------------------------------------ grid

    fn resolver_trilhas(&self, lista: &[Trilha], disponivel: Option<i32>, gap: i32, saida: &mut Vec<Trilha>) {
        for t in lista {
            match t {
                Trilha::Repete(n, l) => {
                    for _ in 0..*n {
                        self.resolver_trilhas(l, disponivel, gap, saida);
                    }
                }
                Trilha::RepeteAuto(l) => {
                    // Quantas cabem.
                    let cada: i32 = l
                        .iter()
                        .map(|x| match x {
                            Trilha::T(T::Px(v)) => *v as i32,
                            Trilha::MinMax(T::Px(v), _) => *v as i32,
                            Trilha::MinMax(T::Em(v), _) | Trilha::MinMax(T::Rem(v), _) | Trilha::T(T::Em(v)) | Trilha::T(T::Rem(v)) => (*v * 16.0) as i32,
                            Trilha::T(T::Pct(v)) | Trilha::MinMax(T::Pct(v), _) => (*v * disponivel.unwrap_or(0) as f32 / 100.0) as i32,
                            _ => 1,
                        })
                        .sum::<i32>()
                        .max(1);
                    let n = disponivel.map_or(1, |d| ((d + gap) / (cada + gap * l.len() as i32).max(1)).max(1));
                    for _ in 0..n.min(500) {
                        self.resolver_trilhas(l, disponivel, gap, saida);
                    }
                }
                o => saida.push(o.clone()),
            }
        }
    }

    fn dispor_grid(&mut self, c: u32, w: i32, h: Option<i32>) -> i32 {
        let e = self.cx[c as usize].e.clone();
        let gap_c = r(e.vao_coluna, w);
        let gap_l = r(e.vao_linha, h.unwrap_or(0));
        let mut colunas = Vec::new();
        self.resolver_trilhas(&e.grid_colunas, Some(w), gap_c, &mut colunas);
        let mut linhas_exp = Vec::new();
        self.resolver_trilhas(&e.grid_linhas, h, gap_l, &mut linhas_exp);
        // Colocação dos itens.
        let itens: Vec<u32> = {
            let mut v: Vec<u32> = self.cx[c as usize].filhos.iter().copied().filter(|f| !self.cx[*f as usize].e.fora_do_fluxo()).collect();
            v.sort_by_key(|f| self.cx[*f as usize].e.ordem);
            v
        };
        let n_exp_col = colunas.len().max(e.grid_areas.first().map_or(0, |l| l.len()));
        let mut n_cols = n_exp_col.max(1) as i32;
        // Quem pede uma coluna além das explícitas.
        for f in &itens {
            let ef = &self.cx[*f as usize].e;
            let (a, b) = ef.grid_coluna;
            n_cols = n_cols.max(a.linha - 1 + b.span.max(a.span).max(1)).max(b.linha - 1);
        }
        let mut ocupado: Vec<Vec<bool>> = Vec::new();
        let mut posicoes: Vec<(i32, i32, i32, i32)> = Vec::with_capacity(itens.len());
        let (mut cur_l, mut cur_c) = (0i32, 0i32);
        let marcar = |ocupado: &mut Vec<Vec<bool>>, l: i32, c: i32, ls: i32, cs: i32, n: i32| {
            for i in l..l + ls {
                while ocupado.len() <= i as usize {
                    ocupado.push(vec![false; n.max(1) as usize]);
                }
                for j in c..(c + cs).min(n) {
                    ocupado[i as usize][j as usize] = true;
                }
            }
        };
        let livre = |ocupado: &Vec<Vec<bool>>, l: i32, c: i32, ls: i32, cs: i32| -> bool {
            for i in l..l + ls {
                if let Some(linha) = ocupado.get(i as usize) {
                    for j in c..c + cs {
                        if linha.get(j as usize).copied().unwrap_or(false) {
                            return false;
                        }
                    }
                }
            }
            true
        };
        let n_exp_lin = linhas_exp.len().max(e.grid_areas.len()) as i32;
        let mut pedidas: Vec<(Option<i32>, i32, Option<i32>, i32)> = Vec::with_capacity(itens.len());
        for f in &itens {
            let ef = self.cx[*f as usize].e.clone();
            let lin_col = |g: (crate::css::LinhaGrid, crate::css::LinhaGrid), exp: i32| -> (Option<i32>, i32) {
                let conv = |v: i32| if v < 0 { exp + 1 + v } else { v - 1 };
                let (a, b) = g;
                let span = a.span.max(b.span).max(1);
                if a.linha != 0 && b.linha != 0 {
                    let (x, y) = (conv(a.linha), conv(b.linha));
                    (Some(x.min(y)), (y - x).abs().max(1))
                } else if a.linha != 0 {
                    (Some(conv(a.linha)), span)
                } else if b.linha != 0 {
                    (Some((conv(b.linha) - span).max(0)), span)
                } else {
                    (None, span)
                }
            };
            let (mut cc, mut cs) = lin_col(ef.grid_coluna, n_exp_col as i32);
            let (mut ll, mut ls) = lin_col(ef.grid_linha, n_exp_lin);
            if let Some(nome) = &ef.grid_area {
                'busca: for (i, l) in e.grid_areas.iter().enumerate() {
                    for (j, nm) in l.iter().enumerate() {
                        if nm.as_str() == &**nome {
                            cc = Some(j as i32);
                            ll = Some(i as i32);
                            cs = l.iter().filter(|x| *x == nm).count() as i32;
                            ls = e.grid_areas.iter().filter(|q| q.get(j).is_some_and(|x| x == nm)).count() as i32;
                            break 'busca;
                        }
                    }
                }
            }
            pedidas.push((ll, ls, cc, cs.min(n_cols.max(1))));
        }
        posicoes.resize(itens.len(), (0, 0, 1, 1));
        // Primeiro quem já tem lugar; depois os presos a uma linha; o resto na ordem.
        for (k, p) in pedidas.iter().enumerate() {
            if let (Some(a), Some(b)) = (p.0, p.2) {
                n_cols = n_cols.max(b + p.3);
                marcar(&mut ocupado, a, b, p.1, p.3, n_cols);
                posicoes[k] = (a, b, p.1, p.3);
            }
        }
        for (k, p) in pedidas.iter().enumerate() {
            if let (Some(a), None) = (p.0, p.2) {
                let mut b = 0;
                while !livre(&ocupado, a, b, p.1, p.3) && b + p.3 < n_cols {
                    b += 1;
                }
                marcar(&mut ocupado, a, b, p.1, p.3, n_cols);
                posicoes[k] = (a, b, p.1, p.3);
            }
        }
        for (k, p) in pedidas.iter().enumerate() {
            let (ls, cs) = (p.1, p.3);
            match (p.0, p.2) {
                (Some(_), _) => {}
                (None, Some(b)) => {
                    if b < cur_c {
                        cur_l += 1;
                    }
                    cur_c = b;
                    while !livre(&ocupado, cur_l, b, ls, cs) {
                        cur_l += 1;
                    }
                    n_cols = n_cols.max(b + cs);
                    marcar(&mut ocupado, cur_l, b, ls, cs, n_cols);
                    posicoes[k] = (cur_l, b, ls, cs);
                    cur_c = b + cs;
                }
                (None, None) => {
                    if e.grid_fluxo_coluna {
                        loop {
                            if cur_l + ls > n_exp_lin.max(1) {
                                cur_l = 0;
                                cur_c += 1;
                            }
                            if livre(&ocupado, cur_l, cur_c, ls, cs) {
                                break;
                            }
                            cur_l += 1;
                        }
                        n_cols = n_cols.max(cur_c + cs);
                        marcar(&mut ocupado, cur_l, cur_c, ls, cs, n_cols);
                        posicoes[k] = (cur_l, cur_c, ls, cs);
                        cur_l += ls;
                    } else {
                        loop {
                            if cur_c + cs > n_cols {
                                cur_c = 0;
                                cur_l += 1;
                            }
                            if livre(&ocupado, cur_l, cur_c, ls, cs) {
                                break;
                            }
                            cur_c += 1;
                        }
                        marcar(&mut ocupado, cur_l, cur_c, ls, cs, n_cols);
                        posicoes[k] = (cur_l, cur_c, ls, cs);
                        cur_c += cs;
                    }
                }
            }
        }
        let n_cols = n_cols.max(1) as usize;
        let n_lins = posicoes.iter().map(|p| (p.0 + p.2) as usize).max().unwrap_or(0).max(linhas_exp.len()).max(e.grid_areas.len());
        // Larguras das colunas.
        let auto_col = e.grid_auto_colunas.first().cloned().unwrap_or(Trilha::T(T::Auto));
        let mut defs: Vec<Trilha> = colunas;
        while defs.len() < n_cols {
            defs.push(auto_col.clone());
        }
        let mut larg = vec![0i32; n_cols];
        let mut fr = vec![0.0f32; n_cols];
        let mut estica = vec![false; n_cols];
        let resolver_em = |t: T, base: i32, fonte: f32| -> Option<i32> {
            Some(match t {
                T::Px(v) => v as i32,
                T::Em(v) => (v * fonte) as i32,
                T::Rem(v) => (v * 16.0) as i32,
                T::Pct(v) => (v * base as f32 / 100.0) as i32,
                T::Vw(v) => v as i32,
                T::Vh(v) => v as i32,
                T::Soma(s) => (s[0] + s[1] * fonte + s[2] * 16.0 + s[5] * base as f32 / 100.0) as i32,
                _ => return None,
            })
        };
        let tem_conteudo = |d: &Trilha| !matches!(d, Trilha::T(T::Px(_)) | Trilha::T(T::Em(_)) | Trilha::T(T::Rem(_)) | Trilha::T(T::Pct(_)) | Trilha::T(T::Soma(_)));
        for (i, d) in defs.iter().enumerate() {
            match d {
                Trilha::Fr(x) => fr[i] = *x,
                Trilha::T(t) => match resolver_em(*t, w, e.fonte) {
                    Some(v) => larg[i] = v,
                    None => estica[i] = matches!(t, T::Auto),
                },
                Trilha::MinMax(a, b) => {
                    larg[i] = resolver_em(*a, w, e.fonte).unwrap_or(0);
                    match &**b {
                        Trilha::Fr(x) => fr[i] = *x,
                        Trilha::T(T::Auto) | Trilha::T(T::Max) => estica[i] = true,
                        Trilha::T(t2) => {
                            if let Some(v) = resolver_em(*t2, w, e.fonte) {
                                larg[i] = larg[i].max(v);
                            }
                        }
                        _ => {}
                    }
                }
                _ => {}
            }
        }
        // Colunas pelo conteúdo: só os itens de uma coluna (os outros, depois).
        for (k, f) in itens.iter().enumerate() {
            let (_, pc, _, cs) = posicoes[k];
            let (mi, ma) = self.intrinsecas_externas(*f);
            if cs == 1 {
                let i = pc as usize;
                if tem_conteudo(&defs[i]) && fr[i] == 0.0 {
                    let medida = match &defs[i] {
                        Trilha::T(T::Min) => mi,
                        Trilha::MinMax(T::Min, _) => mi,
                        _ => ma,
                    };
                    if !matches!(defs[i], Trilha::MinMax(T::Px(_), _)) {
                        larg[i] = larg[i].max(medida);
                    }
                }
            }
        }
        let vaos = gap_c * (n_cols as i32 - 1);
        let usado: i32 = larg.iter().sum::<i32>() + vaos;
        let soma_fr: f32 = fr.iter().sum();
        // Muito conteúdo em colunas auto: encolhe até o mínimo.
        if usado > w && soma_fr == 0.0 {
            let mut excesso = usado - w;
            for (k, f) in itens.iter().enumerate() {
                let (_, pc, _, cs) = posicoes[k];
                if cs == 1 && estica[pc as usize] && excesso > 0 {
                    let (mi, _) = self.intrinsecas_externas(*f);
                    let i = pc as usize;
                    let tira = (larg[i] - mi).clamp(0, excesso);
                    larg[i] -= tira;
                    excesso -= tira;
                }
            }
        }
        let usado: i32 = larg.iter().sum::<i32>() + vaos;
        let sobra = (w - usado).max(0);
        if soma_fr > 0.0 {
            let fora_fr: i32 = (0..n_cols).filter(|i| fr[*i] == 0.0).map(|i| larg[i]).sum::<i32>() + vaos;
            let unidade = (w - fora_fr).max(0) as f32 / soma_fr.max(1.0);
            for i in 0..n_cols {
                if fr[i] > 0.0 {
                    larg[i] = larg[i].max((fr[i] * unidade) as i32);
                }
            }
        } else if e.justifica == 6 || e.justifica == 0 {
            // `auto` estica (justify-content: normal).
            let n = estica.iter().filter(|x| **x).count() as i32;
            if n > 0 {
                let cada = sobra / n;
                for i in 0..n_cols {
                    if estica[i] {
                        larg[i] += cada;
                    }
                }
            }
        }
        let posicoes_col: Vec<i32> = {
            let mut v = Vec::with_capacity(n_cols + 1);
            let mut x = 0;
            for l in &larg {
                v.push(x);
                x += l + gap_c;
            }
            v.push(x);
            v
        };
        // Alturas: dispõe cada item com a largura da área.
        let auto_lin = e.grid_auto_linhas.first().cloned().unwrap_or(Trilha::T(T::Auto));
        let mut alt = vec![0i32; n_lins.max(1)];
        let mut fr_l = vec![0.0f32; n_lins.max(1)];
        let mut fixa = vec![false; n_lins.max(1)];
        for i in 0..n_lins {
            let d = linhas_exp.get(i).unwrap_or(&auto_lin);
            match d {
                Trilha::T(t) => {
                    if let Some(v) = resolver_em(*t, h.unwrap_or(0), e.fonte) {
                        alt[i] = v;
                        fixa[i] = true;
                    }
                }
                Trilha::Fr(x) => fr_l[i] = *x,
                Trilha::MinMax(a, _) => alt[i] = resolver_em(*a, h.unwrap_or(0), e.fonte).unwrap_or(0),
                _ => {}
            }
        }
        for (k, f) in itens.iter().enumerate() {
            let (pl, pc, ls, cs) = posicoes[k];
            let area = larg[pc as usize..(pc + cs) as usize].iter().sum::<i32>() + gap_c * (cs - 1);
            self.medir_caixa(*f, area);
            let ef = self.cx[*f as usize].e.clone();
            let (hor, mh) = (self.horizontal(*f), self.margem_h(*f));
            let just = if ef.justifica_proprio == 5 { e.justifica_itens } else { ef.justifica_proprio };
            let cw = match opt(ef.largura, Some(area)) {
                Some(v) => {
                    if ef.caixa_borda { (v - hor).max(0) } else { v }
                }
                None if just == 4 && !ef.margem[3].e_auto() && !ef.margem[1].e_auto() => (area - mh - hor).max(0),
                None => {
                    let (mi, ma) = self.intrinsecas(*f);
                    (ma.min((area - mh).max(mi)) - hor).max(0)
                }
            };
            let cw = self.limitar_largura(*f, cw, area);
            let hc = opt(ef.altura, None);
            self.dispor_item(*f, cw, hc);
            if ls == 1 {
                let i = pl as usize;
                if !fixa[i] && fr_l[i] == 0.0 {
                    alt[i] = alt[i].max(self.externa_h(*f));
                }
            }
        }
        // Itens que atravessam linhas: a última cresce, se faltar.
        for (k, f) in itens.iter().enumerate() {
            let (pl, _, ls, _) = posicoes[k];
            if ls > 1 {
                let soma: i32 = alt[pl as usize..(pl + ls) as usize].iter().sum::<i32>() + gap_l * (ls - 1);
                let falta = self.externa_h(*f) - soma;
                if falta > 0 {
                    alt[(pl + ls - 1) as usize] += falta;
                }
            }
        }
        let vaos_l = gap_l * (n_lins as i32 - 1).max(0);
        if let Some(total) = h {
            let usado: i32 = alt.iter().sum::<i32>() + vaos_l;
            let sobra = (total - usado).max(0);
            let sf: f32 = fr_l.iter().sum();
            if sf > 0.0 {
                let u = sobra as f32 / sf.max(1.0);
                for i in 0..n_lins {
                    if fr_l[i] > 0.0 {
                        alt[i] = alt[i].max((fr_l[i] * u) as i32);
                    }
                }
            } else if e.alinha_conteudo == 6 {
                let n = (0..n_lins).filter(|i| !fixa[*i]).count() as i32;
                if n > 0 {
                    for i in 0..n_lins {
                        if !fixa[i] {
                            alt[i] += sobra / n;
                        }
                    }
                }
            }
        }
        let mut y = 0;
        let mut pos_lin = Vec::with_capacity(n_lins + 1);
        for a in alt.iter().take(n_lins) {
            pos_lin.push(y);
            y += a + gap_l;
        }
        pos_lin.push(y);
        let total_h = (y - gap_l).max(0);
        // Justify-content (com as colunas fixas).
        let total_w = posicoes_col[n_cols] - gap_c;
        let (desl_col, entre_col) = distribuir(e.justifica, (w - total_w).max(0), n_cols as i32, e.justifica != 6);
        for (k, f) in itens.iter().enumerate() {
            let (pl, pc, ls, cs) = posicoes[k];
            let ef = self.cx[*f as usize].e.clone();
            let area_w = larg[pc as usize..(pc + cs) as usize].iter().sum::<i32>() + gap_c * (cs - 1);
            let area_h = alt[pl as usize..(pl + ls).min(n_lins as i32) as usize].iter().sum::<i32>() + gap_l * (ls - 1);
            let al = if ef.alinha_proprio == 5 { e.alinha_itens } else { ef.alinha_proprio };
            let just = if ef.justifica_proprio == 5 { e.justifica_itens } else { ef.justifica_proprio };
            let m = self.cx[*f as usize].margem;
            if al == 4 && ef.altura.e_auto() && !ef.margem[0].e_auto() && !ef.margem[2].e_auto() {
                let ver = self.vertical(*f);
                let alvo = (area_h - m[0] - m[2] - ver).max(0);
                if alvo != self.cx[*f as usize].h {
                    let forcada = if ef.caixa_borda { alvo + ver } else { alvo };
                    let w_at = self.cx[*f as usize].w;
                    self.dispor_item(*f, w_at, Some(forcada));
                    self.cx[*f as usize].h = alvo;
                }
            }
            let (ew, eh) = (self.externa_w(*f), self.externa_h(*f));
            let dx = if ef.margem[3].e_auto() && ef.margem[1].e_auto() {
                (area_w - ew) / 2
            } else {
                match just {
                    1 => area_w - ew,
                    2 => (area_w - ew) / 2,
                    _ => 0,
                }
            };
            let dy = match al {
                1 => area_h - eh,
                2 => (area_h - eh) / 2,
                _ => 0,
            };
            let cx = &mut self.cx[*f as usize];
            cx.x = desl_col + posicoes_col[pc as usize] + pc * entre_col + dx + m[3];
            cx.y = pos_lin[pl as usize] + dy + m[0];
        }
        h.unwrap_or(total_h).max(total_h.min(h.unwrap_or(total_h)))
    }

    // ------------------------------------------------------------ tabelas

    /// As linhas de uma tabela e, em cada uma, as células.
    fn linhas_da_tabela(&self, c: u32) -> Vec<u32> {
        let mut v = Vec::new();
        for f in &self.cx[c as usize].filhos {
            match self.cx[*f as usize].tipo {
                Tipo::Linha => v.push(*f),
                Tipo::GrupoLinhas => {
                    for g in &self.cx[*f as usize].filhos {
                        if self.cx[*g as usize].tipo == Tipo::Linha {
                            v.push(*g);
                        }
                    }
                }
                _ => {}
            }
        }
        v
    }

    fn atributo_int(&self, no: NoId, nome: &str) -> i32 {
        if no == NADA {
            return 1;
        }
        self.d.atributo(no, nome).and_then(|v| v.trim().parse().ok()).unwrap_or(1).clamp(1, 1000)
    }

    /// Onde cada célula cai: (célula, linha, coluna, colspan, rowspan).
    fn grade_da_tabela(&self, c: u32) -> (Vec<(u32, usize, usize, usize, usize)>, usize, Vec<u32>) {
        let linhas = self.linhas_da_tabela(c);
        let mut cheio: Vec<Vec<bool>> = vec![Vec::new(); linhas.len()];
        let mut celulas = Vec::new();
        let mut n_cols = 0;
        for (l, lin) in linhas.iter().enumerate() {
            let mut col = 0;
            for cel in &self.cx[*lin as usize].filhos {
                if self.cx[*cel as usize].tipo != Tipo::Celula {
                    continue;
                }
                while cheio[l].get(col).copied().unwrap_or(false) {
                    col += 1;
                }
                let no = self.cx[*cel as usize].no;
                let cs = self.atributo_int(no, "colspan") as usize;
                let rs = (self.atributo_int(no, "rowspan") as usize).min(linhas.len() - l);
                for i in l..l + rs {
                    if cheio[i].len() < col + cs {
                        cheio[i].resize(col + cs, false);
                    }
                    for j in col..col + cs {
                        cheio[i][j] = true;
                    }
                }
                celulas.push((*cel, l, col, cs, rs));
                col += cs;
                n_cols = n_cols.max(col);
            }
        }
        (celulas, n_cols, linhas)
    }

    fn colunas_intrinsecas(&mut self, c: u32) -> (Vec<i32>, Vec<i32>) {
        let (celulas, n, _) = self.grade_da_tabela(c);
        let e = self.cx[c as usize].e.clone();
        let esp = if e.colapsa_bordas { 0 } else { e.espaco_bordas as i32 };
        let (mut mi, mut ma) = (vec![0; n], vec![0; n]);
        for &(cel, _, col, cs, _) in &celulas {
            if cs == 1 {
                let (a, b) = self.intrinsecas(cel);
                mi[col] = mi[col].max(a);
                ma[col] = ma[col].max(b);
            }
        }
        for &(cel, _, col, cs, _) in &celulas {
            if cs > 1 {
                let (a, b) = self.intrinsecas(cel);
                let vaos = esp * (cs as i32 - 1);
                let soma_mi: i32 = mi[col..col + cs].iter().sum::<i32>() + vaos;
                let soma_ma: i32 = ma[col..col + cs].iter().sum::<i32>() + vaos;
                if a > soma_mi {
                    let cada = (a - soma_mi) / cs as i32;
                    for v in &mut mi[col..col + cs] {
                        *v += cada;
                    }
                }
                if b > soma_ma {
                    let cada = (b - soma_ma) / cs as i32;
                    for v in &mut ma[col..col + cs] {
                        *v += cada;
                    }
                }
            }
        }
        for i in 0..n {
            ma[i] = ma[i].max(mi[i]);
        }
        (mi, ma)
    }

    fn dispor_tabela(&mut self, c: u32, w: i32) -> i32 {
        let e = self.cx[c as usize].e.clone();
        let colapsa = e.colapsa_bordas;
        let esp = if colapsa { 0 } else { e.espaco_bordas as i32 };
        let (celulas, n, linhas) = self.grade_da_tabela(c);
        let mut y = 0;
        // As legendas de cima.
        for f in self.cx[c as usize].filhos.clone() {
            if self.cx[f as usize].tipo == Tipo::Legenda {
                self.medir_caixa(f, w);
                let hor = self.horizontal(f);
                self.novo_contexto(f, (w - hor).max(0), None);
                self.cx[f as usize].x = 0;
                self.cx[f as usize].y = y;
                y += self.borda_h(f);
            }
        }
        if n == 0 {
            return y;
        }
        if colapsa {
            // Bordas juntas: cada célula desenha só as de baixo e da direita.
            for &(cel, l, col, _, _) in &celulas {
                let x = &mut self.cx[cel as usize];
                let mut e2 = (*x.e).clone();
                let mudou = (col > 0 && e2.borda[3] > 0.0) || (l > 0 && e2.borda[0] > 0.0);
                if col > 0 {
                    e2.borda[3] = 0.0;
                }
                if l > 0 {
                    e2.borda[0] = 0.0;
                }
                if mudou {
                    x.e = Rc::new(e2);
                    x.medidas = None;
                }
            }
        }
        let (mi, ma) = self.colunas_intrinsecas(c);
        let dispon = (w - esp * (n as i32 + 1)).max(0);
        let (soma_mi, soma_ma): (i32, i32) = (mi.iter().sum(), ma.iter().sum());
        let mut col_w = vec![0; n];
        // Larguras pedidas nas células da primeira linha: em porcentagem ou px.
        let mut pct = vec![0.0f32; n];
        let mut fixas = vec![false; n];
        for &(cel, _, col, cs, _) in &celulas {
            if cs == 1 {
                match self.cx[cel as usize].e.largura {
                    Tam::Pct(p) => pct[col] = pct[col].max(p),
                    Tam::Px(_) => fixas[col] = true,
                    _ => {}
                }
            }
        }
        if e.tabela_fixa {
            let soma_fixas: i32 = (0..n).filter(|i| fixas[*i]).map(|i| ma[i]).sum();
            let soltas = (0..n).filter(|i| !fixas[*i]).count() as i32;
            let cada = (dispon - soma_fixas).max(0) / soltas.max(1);
            for (i, v) in col_w.iter_mut().enumerate() {
                *v = if fixas[i] { ma[i] } else { cada };
            }
        } else if soma_mi >= dispon {
            col_w = mi.clone();
        } else if soma_ma <= dispon {
            col_w = ma.clone();
            // Largura fixa na tabela: o que sobra vai para as colunas.
            let auto = e.largura.e_auto();
            if !auto || pct.iter().any(|p| *p > 0.0) {
                let sobra = dispon - soma_ma;
                let flexiveis: i32 = (0..n).filter(|i| !fixas[*i]).map(|i| ma[i].max(1)).sum();
                let mut dado = 0;
                for i in 0..n {
                    if !fixas[i] && flexiveis > 0 {
                        let parte = (sobra as i64 * ma[i].max(1) as i64 / flexiveis as i64) as i32;
                        col_w[i] += parte;
                        dado += parte;
                    }
                }
                if flexiveis == 0 {
                    let cada = sobra / n as i32;
                    for v in col_w.iter_mut() {
                        *v += cada;
                    }
                } else if let Some(i) = (0..n).rev().find(|i| !fixas[*i]) {
                    col_w[i] += sobra - dado;
                }
            }
        } else {
            let extra = dispon - soma_mi;
            let dif = (soma_ma - soma_mi).max(1);
            for i in 0..n {
                col_w[i] = mi[i] + (extra as i64 * (ma[i] - mi[i]) as i64 / dif as i64) as i32;
            }
        }
        // Porcentagens pedidas (quando há espaço).
        if !e.tabela_fixa && soma_mi < dispon {
            for i in 0..n {
                if pct[i] > 0.0 {
                    let v = (pct[i] * dispon as f32 / 100.0) as i32;
                    col_w[i] = col_w[i].max(v.max(mi[i]));
                }
            }
            let total: i32 = col_w.iter().sum();
            if total > dispon {
                // Tira das colunas sem porcentagem.
                let mut excesso = total - dispon;
                for i in 0..n {
                    if pct[i] == 0.0 && excesso > 0 {
                        let t = (col_w[i] - mi[i]).clamp(0, excesso);
                        col_w[i] -= t;
                        excesso -= t;
                    }
                }
            }
        }
        // A altura de cada linha.
        let n_l = linhas.len();
        let mut alt = vec![0; n_l];
        for &(cel, l, col, cs, rs) in &celulas {
            let w_cel = col_w[col..col + cs].iter().sum::<i32>() + esp * (cs as i32 - 1);
            self.medir_caixa(cel, w_cel);
            let hor = self.horizontal(cel);
            let ec = self.cx[cel as usize].e.clone();
            self.novo_contexto(cel, (w_cel - hor).max(0), None);
            let ver = self.vertical(cel);
            let mut h = self.cx[cel as usize].h + ver;
            if let Tam::Px(v) = ec.altura {
                h = h.max(if ec.caixa_borda { v as i32 } else { v as i32 + ver });
            }
            if rs == 1 {
                alt[l] = alt[l].max(h);
            }
        }
        // A altura pedida na linha: <tr height=...>
        for (i, lin) in linhas.iter().enumerate() {
            if let Tam::Px(v) = self.cx[*lin as usize].e.altura {
                alt[i] = alt[i].max(v as i32);
            }
        }
        for &(cel, l, _, _, rs) in &celulas {
            if rs > 1 {
                let h = self.borda_h(cel);
                let soma: i32 = alt[l..l + rs].iter().sum::<i32>() + esp * (rs as i32 - 1);
                if h > soma {
                    alt[l + rs - 1] += h - soma;
                }
            }
        }
        // As linhas ficam diretamente na tabela: o grupo é só um invólucro de altura.
        let mut pos_x = Vec::with_capacity(n + 1);
        let mut x = esp;
        for v in &col_w {
            pos_x.push(x);
            x += v + esp;
        }
        pos_x.push(x);
        let mut pos_y = Vec::with_capacity(n_l + 1);
        y += esp;
        for a in &alt {
            pos_y.push(y);
            y += a + esp;
        }
        pos_y.push(y);
        // Grupos: posição zero (as linhas contam da tabela).
        for f in self.cx[c as usize].filhos.clone() {
            if self.cx[f as usize].tipo == Tipo::GrupoLinhas {
                let cx = &mut self.cx[f as usize];
                cx.x = 0;
                cx.y = 0;
                cx.w = 0;
                cx.h = 0;
            }
        }
        let largura_total = x;
        for (i, lin) in linhas.iter().enumerate() {
            let cx = &mut self.cx[*lin as usize];
            cx.x = 0;
            cx.y = pos_y[i];
            cx.w = largura_total;
            cx.h = alt[i];
            cx.margem = [0; 4];
            cx.preench = [0; 4];
            cx.borda = [0; 4];
        }
        for &(cel, l, col, cs, rs) in &celulas {
            let hor = self.horizontal(cel);
            let ver = self.vertical(cel);
            let w_cel = col_w[col..col + cs].iter().sum::<i32>() + esp * (cs as i32 - 1);
            let h_area = alt[l..l + rs].iter().sum::<i32>() + esp * (rs as i32 - 1);
            let conteudo = self.cx[cel as usize].h;
            let ec = self.cx[cel as usize].e.clone();
            let va = match ec.alinha_v {
                2 | 6 => 0,
                3 | 7 => h_area - ver - conteudo,
                0 | 4 | 5 => {
                    if matches!(self.d.nome(self.cx[cel as usize].no), "td" | "th") && ec.alinha_v == 0 && self.d.atributo(self.cx[cel as usize].no, "valign").is_none() {
                        (h_area - ver - conteudo) / 2
                    } else {
                        0
                    }
                }
                _ => (h_area - ver - conteudo) / 2,
            };
            let cx = &mut self.cx[cel as usize];
            cx.x = pos_x[col];
            cx.y = 0;
            cx.w = (w_cel - hor).max(0);
            cx.h = (h_area - ver).max(0);
            cx.extra_y = va.max(0);
        }
        (y - if n_l == 0 { esp } else { 0 }).max(0)
    }

    // ------------------------------------------------------------ o começo

    /// Monta e dispõe a página inteira, e faz a lista de desenho.
    pub fn diagramar(mut self) -> Lista {
        let d = self.d;
        let html = d.html();
        let e_raiz = self.c.estilos.get(&html).cloned().unwrap_or_else(|| Rc::new(Estilo::inicial()));
        let raiz = self.nova(html, e_raiz, Tipo::Bloco, N);
        self.raiz = raiz;
        self.construir_filhos(html, raiz, 0);
        self.arrumar_filhos(raiz);
        self.achar_referencias(raiz, raiz, raiz);
        self.medir_caixa(raiz, self.vw);
        let hor = self.horizontal(raiz);
        let mh = self.margem_h(raiz);
        let w = (self.vw - mh - hor).max(0);
        self.cx[raiz as usize].x = self.cx[raiz as usize].margem[3];
        self.cx[raiz as usize].y = self.cx[raiz as usize].margem[0];
        let h_css = opt(self.cx[raiz as usize].e.altura, Some(self.vh));
        self.novo_contexto(raiz, w, h_css);
        let (vw, vh) = (self.vw, self.vh);
        for f in self.cx[raiz as usize].posicionadas.clone() {
            self.dispor_posicionada(f, raiz, vw, vh.max(self.borda_h(raiz)));
        }
        let mut lista = Lista { recortes: vec![(0, 0, 0, 0)], largura: self.vw, fundo: 0xFFFFFFFF, ..Default::default() };
        // O fundo da página vem do <html> ou, se ele não tem, do <body>.
        let (mut fundo, mut img) = (self.cx[raiz as usize].e.fundo, self.cx[raiz as usize].e.fundo_img.clone());
        let mut fonte_fundo = raiz;
        if fundo >> 24 == 0 && img.is_none() {
            if let Some(b) = self.cx[raiz as usize].filhos.iter().copied().find(|f| d.e(self.cx[*f as usize].no, "body")) {
                fundo = self.cx[b as usize].e.fundo;
                img = self.cx[b as usize].e.fundo_img.clone();
                fonte_fundo = b;
            }
        }
        if fundo >> 24 != 0 {
            lista.fundo = fundo | 0xFF000000;
        }
        let _ = (img, fonte_fundo);
        self.pintar_raiz(raiz, fonte_fundo, &mut lista);
        lista
    }

    // ------------------------------------------------------------ pintura

    fn pintar_raiz(&mut self, raiz: u32, fonte_fundo: u32, l: &mut Lista) {
        let mut p = Pintor { lista: l, opac: 255, fixo: false, recorte: 0, y_max: 0 };
        // O fundo de imagem da página cobre tudo.
        if let Some(img) = self.cx[fonte_fundo as usize].e.fundo_img.clone() {
            let e = self.cx[fonte_fundo as usize].e.clone();
            let (vw, h) = (self.vw, self.borda_h(raiz).max(self.vh));
            self.pintar_imagem_fundo(&mut p, &img, &e, (0, 0, vw, h));
        }
        self.pintar_contexto(raiz, 0, 0, &mut p, fonte_fundo);
        let alt = p.y_max.max(self.borda_h(raiz) + self.cx[raiz as usize].margem[0] + self.cx[raiz as usize].margem[2]);
        l.altura = alt;
    }

    /// Pinta `c` e o que está dentro: um contexto de empilhamento.
    fn pintar_contexto(&mut self, c: u32, ox: i32, oy: i32, p: &mut Pintor, sem_fundo: u32) {
        let mut pendentes: Vec<(i32, u32, i32, i32, u32, bool, u8)> = Vec::new();
        self.pintar_caixa(c, ox, oy, p, &mut pendentes, true, sem_fundo);
        // Os posicionados: z negativo deveria vir antes do fluxo; a pintura em
        // duas fases fica para quando alguém precisar.
        pendentes.sort_by_key(|x| x.0);
        for (_, f, x, y, rec, fixo, op) in pendentes {
            let (ar, af, ao) = (p.recorte, p.fixo, p.opac);
            p.recorte = rec;
            p.fixo = fixo;
            p.opac = op;
            self.pintar_contexto(f, x, y, p, N);
            (p.recorte, p.fixo, p.opac) = (ar, af, ao);
        }
    }

    fn deslocamento_relativo(&self, c: u32) -> (i32, i32) {
        let x = &self.cx[c as usize];
        let e = &x.e;
        let (mut dx, mut dy) = (0, 0);
        let pai = x.pai;
        let (w_p, h_p) = if pai != N { (self.cx[pai as usize].w, self.cx[pai as usize].h) } else { (self.vw, self.vh) };
        if e.posicao == Posicao::Relativa {
            dx = opt(e.esquerda, Some(w_p)).or(opt(e.direita, Some(w_p)).map(|v| -v)).unwrap_or(0);
            dy = opt(e.topo, Some(h_p)).or(opt(e.baixo, Some(h_p)).map(|v| -v)).unwrap_or(0);
        }
        let (bw, bh) = (self.borda_w(c), self.borda_h(c));
        dx += r(e.desloca_x, bw);
        dy += r(e.desloca_y, bh);
        (dx, dy)
    }

    #[allow(clippy::too_many_arguments)]
    fn pintar_caixa(&mut self, c: u32, ox: i32, oy: i32, p: &mut Pintor, pend: &mut Vec<(i32, u32, i32, i32, u32, bool, u8)>, raiz_contexto: bool, sem_fundo: u32) {
        let e = self.cx[c as usize].e.clone();
        let tipo = self.cx[c as usize].tipo.clone();
        if tipo == Tipo::Inline {
            // O fundo e as bordas já vieram com as linhas; só os filhos que
            // não são texto (floats etc.) ficam aqui.
            return;
        }
        let (dx, dy) = self.deslocamento_relativo(c);
        let x = &self.cx[c as usize];
        let (ax, ay) = (ox + x.x + dx, oy + x.y + dy + x.extra_y * 0);
        // Os posicionados e os com z-index vão depois do fluxo.
        let fundo_contexto = e.posicao != Posicao::Estatica || e.opacidade < 1.0 || e.z.is_some();
        if !raiz_contexto && fundo_contexto && e.posicao != Posicao::Grudada {
            let fixo = p.fixo || e.posicao == Posicao::Fixa;
            let opac = ((p.opac as f32) * e.opacidade) as u8;
            // Posicionadas absolutas são relativas à referência delas, não ao pai
            // do fluxo: `x`, `y` já estão nessa referência.
            let (bx, by) = if e.fora_do_fluxo() { self.origem_de_referencia(c) } else { (ox, oy) };
            let recorte = if e.fora_do_fluxo() { 0 } else { p.recorte };
            pend.push((e.z.unwrap_or(0), c, bx, by, recorte, fixo, opac));
            return;
        }
        if raiz_contexto && e.opacidade < 1.0 && c != self.raiz {
            // A opacidade já foi somada ao entrar.
        }
        let (bw, bh) = (self.borda_w(c), self.borda_h(c));
        let visivel = e.visivel;
        // Âncora para eventos e para `getBoundingClientRect`.
        let no = self.cx[c as usize].no;
        if no != NADA {
            p.lista.retangulos.entry(no).or_insert((ax, ay, bw, bh));
            if !e.sem_eventos {
                p.lista.alvos.push(Alvo { x: ax, y: ay, l: bw, a: bh, no, recorte: p.recorte, fixo: p.fixo });
            }
        }
        if !p.fixo {
            p.y_max = p.y_max.max(ay + bh + self.cx[c as usize].margem[2]);
        }
        if visivel && c != sem_fundo {
            self.pintar_fundo_e_borda(c, ax, ay, bw, bh, p, &e);
        } else if visivel {
            self.pintar_so_borda(c, ax, ay, bw, bh, p, &e);
        }
        // O conteúdo.
        let x = &self.cx[c as usize];
        let (ix, iy) = (ax + x.borda[3] + x.preench[3], ay + x.borda[0] + x.preench[0]);
        let recorte_antes = p.recorte;
        if e.recorta {
            let (rx, ry) = (ax + x.borda[3], ay + x.borda[0]);
            let (rw, rh) = (bw - x.borda[1] - x.borda[3], bh - x.borda[0] - x.borda[2]);
            let (rx, ry, rw, rh) = interseccao(p.lista.recortes[p.recorte as usize], p.recorte != 0, (rx, ry, rw, rh));
            p.lista.recortes.push((rx, ry, rw, rh));
            p.recorte = p.lista.recortes.len() as u32 - 1;
        }
        match tipo {
            Tipo::Imagem(i, nat) => {
                if visivel {
                    self.pintar_img(i, nat, ix, iy, x_conteudo(&self.cx[c as usize]), p, &e);
                }
            }
            Tipo::Controle => {
                if visivel {
                    let (w, h) = (self.cx[c as usize].w, self.cx[c as usize].h);
                    p.emitir(Desenho::Controle { x: ix, y: iy, l: w, a: h, no });
                }
            }
            Tipo::Substituido => {
                if visivel {
                    let (w, h) = (self.cx[c as usize].w, self.cx[c as usize].h);
                    if d_nome_iframe(self.d, no) {
                        p.emitir(Desenho::Ret { x: ix, y: iy, l: w, a: h, cor: 0xFFF0F0F0 });
                    }
                }
            }
            Tipo::Celula => {
                let extra = self.cx[c as usize].extra_y;
                self.pintar_filhos(c, ix, iy + extra, p, pend, &e);
            }
            _ => self.pintar_filhos(c, ix, iy, p, pend, &e),
        }
        p.recorte = recorte_antes;
    }

    fn origem_de_referencia(&self, c: u32) -> (i32, i32) {
        // Soma as posições das caixas até a raiz da referência.
        let e = &self.cx[c as usize].e;
        if e.posicao == Posicao::Fixa {
            return (0, 0);
        }
        // A referência é a caixa posicionada mais próxima.
        let mut p = self.cx[c as usize].pai;
        let mut cb = self.raiz;
        while p != N {
            let q = &self.cx[p as usize];
            let tem_transf = !matches!(q.e.desloca_x, Tam::Px(v) if v == 0.0) || !matches!(q.e.desloca_y, Tam::Px(v) if v == 0.0);
            if q.e.posicao != Posicao::Estatica || tem_transf {
                cb = p;
                break;
            }
            p = q.pai;
        }
        self.absoluta(cb)
    }

    /// A posição da caixa em coordenadas da página.
    fn absoluta(&self, c: u32) -> (i32, i32) {
        let mut caminho = Vec::new();
        let mut p = c;
        while p != N {
            caminho.push(p);
            let q = &self.cx[p as usize];
            if q.e.fora_do_fluxo() {
                break;
            }
            p = q.pai;
        }
        let topo = *caminho.last().unwrap();
        let (mut x, mut y) = if self.cx[topo as usize].e.fora_do_fluxo() { self.origem_de_referencia(topo) } else { (0, 0) };
        for &q in caminho.iter().rev() {
            let cx = &self.cx[q as usize];
            let (dx, dy) = self.deslocamento_relativo(q);
            x += cx.x + dx;
            y += cx.y + dy;
            if q != c {
                x += cx.borda[3] + cx.preench[3];
                y += cx.borda[0] + cx.preench[0] + if cx.tipo == Tipo::Celula { cx.extra_y } else { 0 };
                if cx.tipo == Tipo::Linha || cx.tipo == Tipo::GrupoLinhas {
                    // (linhas não têm borda nem preenchimento)
                }
            }
        }
        (x, y)
    }

    fn pintar_filhos(&mut self, c: u32, ix: i32, iy: i32, p: &mut Pintor, pend: &mut Vec<(i32, u32, i32, i32, u32, bool, u8)>, e: &Estilo) {
        // O marcador da lista.
        if let Some(m) = self.cx[c as usize].marcador.clone() {
            if e.visivel {
                let tam = tam_fonte(e.fonte);
                let (sobe, _) = self.metricas_de(e);
                let w = self.m.largura(&m, tam, e.negrito, e.mono);
                // Na altura da primeira linha.
                let (mx, my) = (ix - w - 6, iy);
                let primeira = self.cx[c as usize].filhos.first().copied();
                let lin = self.primeira_base(c, primeira, sobe);
                p.emitir(Desenho::Texto { x: mx, y: my + lin - sobe, texto: m, tam, negrito: e.negrito, italico: false, mono: e.mono, cor: e.cor & 0xFFFFFF });
            }
        }
        if self.cx[c as usize].em_linha || self.cx[c as usize].tipo == Tipo::Inline {
            self.pintar_linhas(c, ix, iy, p, pend);
        }
        for f in self.cx[c as usize].filhos.clone() {
            let ef = self.cx[f as usize].e.clone();
            match self.cx[f as usize].tipo {
                Tipo::Texto(_) | Tipo::Quebra => continue,
                Tipo::Inline => {
                    // Os átomos e floats dentro dela.
                    self.pintar_dentro_de_inline(f, ix, iy, p, pend);
                    continue;
                }
                _ => {}
            }
            let _ = ef;
            self.pintar_caixa(f, ix, iy, p, pend, false, N);
        }
    }

    fn primeira_base(&self, c: u32, _f: Option<u32>, sobe: i32) -> i32 {
        let x = &self.cx[c as usize];
        if let Some(pd) = x.pedacos.first() {
            return pd.y + sobe;
        }
        for f in &x.filhos {
            let q = &self.cx[*f as usize];
            if q.em_linha && !q.pedacos.is_empty() {
                return q.y + q.borda[0] + q.preench[0] + q.pedacos[0].y + sobe;
            }
        }
        sobe
    }

    fn pintar_dentro_de_inline(&mut self, c: u32, ix: i32, iy: i32, p: &mut Pintor, pend: &mut Vec<(i32, u32, i32, i32, u32, bool, u8)>) {
        for f in self.cx[c as usize].filhos.clone() {
            match self.cx[f as usize].tipo {
                Tipo::Texto(_) | Tipo::Quebra => {}
                Tipo::Inline => self.pintar_dentro_de_inline(f, ix, iy, p, pend),
                _ => self.pintar_caixa(f, ix, iy, p, pend, false, N),
            }
        }
    }

    fn pintar_linhas(&mut self, c: u32, ix: i32, iy: i32, p: &mut Pintor, pend: &mut Vec<(i32, u32, i32, i32, u32, bool, u8)>) {
        let _ = pend;
        let fundos = core::mem::take(&mut self.cx[c as usize].fundos);
        for &(f, x, y, w, h) in &fundos {
            let (no, e) = {
                let q = &self.cx[f as usize];
                (q.no, q.e.clone())
            };
            let (dx, dy) = self.deslocamento_relativo(f);
            let (ax, ay) = (ix + x + dx, iy + y + dy);
            if no != NADA && !e.sem_eventos {
                p.lista.alvos.push(Alvo { x: ax, y: ay, l: w, a: h, no, recorte: p.recorte, fixo: p.fixo });
            }
            if no != NADA {
                let en = p.lista.retangulos.entry(no).or_insert((ax, ay, w, h));
                let (x0, y0) = (en.0.min(ax), en.1.min(ay));
                let (x1, y1) = ((en.0 + en.2).max(ax + w), (en.1 + en.3).max(ay + h));
                *en = (x0, y0, x1 - x0, y1 - y0);
            }
            if e.visivel {
                self.pintar_fundo_e_borda(f, ax, ay, w, h, p, &e);
            }
        }
        self.cx[c as usize].fundos = fundos;
        let pedacos = core::mem::take(&mut self.cx[c as usize].pedacos);
        for pd in &pedacos {
            let (no, e) = {
                let q = &self.cx[pd.caixa as usize];
                (q.no, q.e.clone())
            };
            let (mut dx, mut dy) = (0, 0);
            // Relativo de um <span position:relative>.
            let pai = self.cx[pd.caixa as usize].pai;
            if pai != N && self.cx[pai as usize].tipo == Tipo::Inline {
                (dx, dy) = self.deslocamento_relativo(pai);
            }
            let (ax, ay) = (ix + pd.x + dx, iy + pd.y + dy);
            if no != NADA && !e.sem_eventos {
                let alvo_no = if self.d.texto_de_no(no).is_some() { self.d.pai(no) } else { no };
                if alvo_no != NADA {
                    p.lista.alvos.push(Alvo { x: ax, y: ay, l: pd.largura, a: pd.altura, no: alvo_no, recorte: p.recorte, fixo: p.fixo });
                }
            }
            if !p.fixo {
                p.y_max = p.y_max.max(ay + pd.altura);
            }
            if !e.visivel {
                continue;
            }
            let cor = e.cor;
            let tam = tam_fonte(e.fonte);
            if !pd.texto.is_empty() && cor >> 24 != 0 {
                if e.espaco_letras != 0.0 && !e.mono {
                    // Letra a letra.
                    let mut x = ax;
                    for ch in pd.texto.chars() {
                        let mut buf = [0u8; 4];
                        let s = ch.encode_utf8(&mut buf);
                        let w = self.m.largura(s, tam, e.negrito, false);
                        p.emitir_com_alfa(Desenho::Texto { x, y: ay, texto: s.to_string(), tam, negrito: e.negrito, italico: e.italico, mono: false, cor: cor & 0xFFFFFF }, cor >> 24);
                        x += w + e.espaco_letras as i32;
                    }
                } else {
                    p.emitir_com_alfa(Desenho::Texto { x: ax, y: ay, texto: pd.texto.clone(), tam, negrito: e.negrito, italico: e.italico, mono: e.mono, cor: cor & 0xFFFFFF }, cor >> 24);
                }
            }
            let cor_d = e.cor_decoracao.unwrap_or(cor);
            if e.sublinhado {
                p.emitir(Desenho::Ret { x: ax, y: ay + pd.altura - 2, l: pd.largura, a: 1, cor: cor_d });
            }
            if e.riscado {
                p.emitir(Desenho::Ret { x: ax, y: ay + pd.altura / 2, l: pd.largura, a: 1, cor: cor_d });
            }
            if e.sobrelinha {
                p.emitir(Desenho::Ret { x: ax, y: ay + 1, l: pd.largura, a: 1, cor: cor_d });
            }
        }
        self.cx[c as usize].pedacos = pedacos;
    }

    fn pintar_so_borda(&mut self, c: u32, ax: i32, ay: i32, bw: i32, bh: i32, p: &mut Pintor, e: &Estilo) {
        let b = self.cx[c as usize].borda;
        if b.iter().any(|v| *v > 0) {
            let raios = raios_de(e, bw, bh);
            p.emitir(Desenho::Borda { x: ax, y: ay, l: bw, a: bh, raios, larguras: b, cores: e.borda_cor, estilos: e.borda_estilo });
        }
    }

    fn pintar_fundo_e_borda(&mut self, c: u32, ax: i32, ay: i32, bw: i32, bh: i32, p: &mut Pintor, e: &Estilo) {
        if bw <= 0 && bh <= 0 {
            return;
        }
        let raios = raios_de(e, bw, bh);
        if let Some(s) = &e.sombras {
            for sb in s.iter().rev() {
                if sb.dentro {
                    continue;
                }
                let exp = sb.expande as i32;
                p.emitir(Desenho::Sombra { x: ax + sb.x as i32 - exp, y: ay + sb.y as i32 - exp, l: bw + 2 * exp, a: bh + 2 * exp, raios, borrao: sb.borrao as i32, cor: sb.cor });
            }
        }
        if e.fundo >> 24 != 0 && !e.mascara {
            if raios == [0; 4] {
                p.emitir(Desenho::Ret { x: ax, y: ay, l: bw, a: bh, cor: e.fundo });
            } else {
                p.emitir(Desenho::Redondo { x: ax, y: ay, l: bw, a: bh, raios, cor: e.fundo });
            }
        }
        if let Some(img) = e.fundo_img.clone().filter(|_| !e.mascara) {
            let rec_antes = p.recorte;
            if raios != [0; 4] {
                // (sem recorte redondo: as imagens ficam retas)
            }
            self.pintar_imagem_fundo(p, &img, e, (ax, ay, bw, bh));
            p.recorte = rec_antes;
        }
        self.pintar_so_borda(c, ax, ay, bw, bh, p, e);
    }

    fn pintar_imagem_fundo(&mut self, p: &mut Pintor, img: &Imagem, e: &Estilo, (x, y, w, h): (i32, i32, i32, i32)) {
        if w <= 0 || h <= 0 {
            return;
        }
        match img {
            Imagem::Gradiente(ang, paradas) => {
                p.emitir(Desenho::Gradiente { x, y, l: w, a: h, angulo: *ang, paradas: Rc::new(paradas.clone()) });
            }
            Imagem::Url(u) => {
                let Some(url) = (self.base_url)(u) else { return };
                let (indice, tam) = self.h.imagem(&url);
                if indice == usize::MAX {
                    return;
                }
                let Some((iw, ih)) = tam else { return };
                if iw <= 0 || ih <= 0 {
                    return;
                }
                let (tw, th) = match e.fundo_tam.0 {
                    1 => {
                        let k = (w as f32 / iw as f32).max(h as f32 / ih as f32);
                        ((iw as f32 * k) as i32, (ih as f32 * k) as i32)
                    }
                    2 => {
                        let k = (w as f32 / iw as f32).min(h as f32 / ih as f32);
                        ((iw as f32 * k) as i32, (ih as f32 * k) as i32)
                    }
                    _ => {
                        let a = opt(e.fundo_tam.1, Some(w));
                        let b = opt(e.fundo_tam.2, Some(h));
                        match (a, b) {
                            (Some(a), Some(b)) => (a, b),
                            (Some(a), None) => (a, a * ih / iw),
                            (None, Some(b)) => (b * iw / ih, b),
                            _ => (iw, ih),
                        }
                    }
                };
                if tw <= 0 || th <= 0 {
                    return;
                }
                let px = match e.fundo_pos.0 {
                    Tam::Pct(v) => ((w - tw) as f32 * v / 100.0) as i32,
                    o => r(o, w),
                };
                let py = match e.fundo_pos.1 {
                    Tam::Pct(v) => ((h - th) as f32 * v / 100.0) as i32,
                    o => r(o, h),
                };
                // Recorta na caixa.
                let antes = p.recorte;
                let rc = interseccao(p.lista.recortes[p.recorte as usize], p.recorte != 0, (x, y, w, h));
                p.lista.recortes.push(rc);
                p.recorte = p.lista.recortes.len() as u32 - 1;
                let (rx, ry) = (e.fundo_repete == 0 || e.fundo_repete == 1, e.fundo_repete == 0 || e.fundo_repete == 2);
                let (ini_x, fim_x) = if rx { (px.rem_euclid(tw) - tw, w) } else { (px, px + 1) };
                let (ini_y, fim_y) = if ry { (py.rem_euclid(th) - th, h) } else { (py, py + 1) };
                let mut q = ini_y;
                let mut guarda = 0;
                while q < fim_y && guarda < 4000 {
                    let mut o = ini_x;
                    while o < fim_x && guarda < 4000 {
                        p.emitir(Desenho::Imagem { x: x + o, y: y + q, l: tw, a: th, indice, opac: 255 });
                        o += tw;
                        guarda += 1;
                    }
                    q += th;
                }
                p.recorte = antes;
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn pintar_img(&mut self, i: usize, nat: Option<(i32, i32)>, ix: i32, iy: i32, (w, h): (i32, i32), p: &mut Pintor, e: &Estilo) {
        if w <= 0 || h <= 0 {
            return;
        }
        let Some((iw, ih)) = nat else {
            // Ainda não chegou: um retângulo no lugar.
            p.emitir(Desenho::Ret { x: ix, y: iy, l: w, a: h, cor: 0xFFEEEEEE });
            return;
        };
        match e.ajuste_objeto {
            1 | 2 | 3 if iw > 0 && ih > 0 => {
                let (tw, th) = match e.ajuste_objeto {
                    1 => {
                        let k = (w as f32 / iw as f32).min(h as f32 / ih as f32);
                        ((iw as f32 * k) as i32, (ih as f32 * k) as i32)
                    }
                    2 => {
                        let k = (w as f32 / iw as f32).max(h as f32 / ih as f32);
                        ((iw as f32 * k) as i32, (ih as f32 * k) as i32)
                    }
                    _ => (iw, ih),
                };
                let antes = p.recorte;
                let rc = interseccao(p.lista.recortes[p.recorte as usize], p.recorte != 0, (ix, iy, w, h));
                p.lista.recortes.push(rc);
                p.recorte = p.lista.recortes.len() as u32 - 1;
                p.emitir(Desenho::Imagem { x: ix + (w - tw) / 2, y: iy + (h - th) / 2, l: tw, a: th, indice: i, opac: 255 });
                p.recorte = antes;
            }
            _ => p.emitir(Desenho::Imagem { x: ix, y: iy, l: w, a: h, indice: i, opac: 255 }),
        }
    }
}

fn x_conteudo(x: &Caixa) -> (i32, i32) {
    (x.w, x.h)
}

fn d_nome_iframe(d: &Dom, no: NoId) -> bool {
    no != NADA && matches!(d.nome(no), "iframe" | "video" | "object" | "embed")
}

struct Pintor<'a> {
    lista: &'a mut Lista,
    opac: u8,
    fixo: bool,
    recorte: u32,
    y_max: i32,
}

impl Pintor<'_> {
    fn emitir(&mut self, d: Desenho) {
        if self.opac == 0 {
            return;
        }
        self.lista.itens.push(ItemDesenho { d, recorte: self.recorte, fixo: self.fixo, opac: self.opac });
    }

    fn emitir_com_alfa(&mut self, d: Desenho, a: u32) {
        let o = if a >= 255 { self.opac } else { (self.opac as u32 * a / 255) as u8 };
        if o == 0 {
            return;
        }
        self.lista.itens.push(ItemDesenho { d, recorte: self.recorte, fixo: self.fixo, opac: o });
    }
}

fn raios_de(e: &Estilo, bw: i32, bh: i32) -> [i32; 4] {
    let mut r = [0; 4];
    let menor = bw.min(bh);
    for i in 0..4 {
        let v = e.raio[i];
        // Porcentagens vêm negativas (veja `css::aplicar`).
        let px = if v < 0.0 { (-v * menor as f32 / 100.0) as i32 } else { v as i32 };
        r[i] = px.min(menor / 2).max(0);
    }
    r
}

fn interseccao(a: (i32, i32, i32, i32), tem: bool, b: (i32, i32, i32, i32)) -> (i32, i32, i32, i32) {
    if !tem {
        return b;
    }
    let (x0, y0) = (a.0.max(b.0), a.1.max(b.1));
    let (x1, y1) = ((a.0 + a.2).min(b.0 + b.2), (a.1 + a.3).min(b.1 + b.3));
    (x0, y0, (x1 - x0).max(0), (y1 - y0).max(0))
}

fn junta(a: i32, b: i32) -> i32 {
    // Margens que se encostam: a maior positiva mais a mais negativa.
    a.max(0).max(b.max(0)) + a.min(0).min(b.min(0))
}

/// (começo, espaço extra entre os itens) de justify-content.
fn distribuir(modo: u8, livre: i32, n: i32, _usar: bool) -> (i32, i32) {
    if n <= 0 {
        return (0, 0);
    }
    match modo {
        1 => (livre, 0),
        2 => (livre / 2, 0),
        3 => (0, if n > 1 { livre / (n - 1) } else { 0 }),
        4 => (livre / n / 2, livre / n),
        5 => (livre / (n + 1), livre / (n + 1)),
        _ => (0, 0),
    }
}

/// O espaço antes do item `i`, sem acumular erros de arredondamento.
fn deslocamento_item(modo: u8, livre: i32, n: i32, i: i32) -> i32 {
    match modo {
        1 => livre,
        2 => livre / 2,
        3 => {
            if n > 1 { livre * i / (n - 1) } else { 0 }
        }
        4 => livre * (2 * i + 1) / (2 * n).max(1),
        5 => livre * (i + 1) / (n + 1),
        _ => 0,
    }
}

fn expandir_contagem(l: &[Trilha]) -> usize {
    l.iter()
        .map(|t| match t {
            Trilha::Repete(n, x) => *n as usize * expandir_contagem(x),
            Trilha::RepeteAuto(x) => expandir_contagem(x),
            _ => 1,
        })
        .sum()
}

fn romano(mut n: i32) -> String {
    if n <= 0 || n > 3999 {
        return n.to_string();
    }
    let mut s = String::new();
    for (v, r) in [(1000, "M"), (900, "CM"), (500, "D"), (400, "CD"), (100, "C"), (90, "XC"), (50, "L"), (40, "XL"), (10, "X"), (9, "IX"), (5, "V"), (4, "IV"), (1, "I")] {
        while n >= v {
            s.push_str(r);
            n -= v;
        }
    }
    s
}
