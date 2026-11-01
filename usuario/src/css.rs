//! CSS: o leitor de folhas de estilo e a "cascata" (qual declaração vale
//! para cada elemento).
//!
//! ```text
//!   texto ─▶ regras: seletor { propriedade: valor; ... }
//!   para cada elemento: as regras cujo seletor casa com ele, em ordem de
//!   especificidade (#id > .classe > tag) e, no empate, a que vem depois
//! ```
//!
//! Seletores: tag, `*`, `.classe`, `#id`, `[atributo]`, `[atributo=valor]`
//! (e `~=`, `^=`, `$=`, `*=`), `:root`, o descendente (`a b`) e o filho
//! (`a > b`). Um seletor com o que não sabemos (`:hover`, `::before`, `+`,
//! `~`, `:not()`...) simplesmente não casa: melhor não aplicar a regra que
//! aplicar errado. `@media` é avaliado com a largura da janela; variáveis
//! (`var(--x)`) valem as definidas em `:root`.
//!
//! Quem usa (o navegador) decide o que fazer com cada propriedade.

use crate::html::Elemento;
use alloc::collections::BTreeMap;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

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

#[derive(Clone, Debug, Default)]
struct Composto {
    tag: Option<String>,
    id: Option<String>,
    classes: Vec<String>,
    atributos: Vec<(String, Operador, String)>,
    raiz: bool,
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Combinador {
    Descendente,
    Filho,
}

/// Um seletor, da direita para a esquerda: o primeiro composto é o do
/// elemento; o combinador diz como chegar ao seguinte (um ancestral).
#[derive(Clone, Debug)]
struct Seletor {
    partes: Vec<(Composto, Combinador)>,
}

pub struct Declaracao {
    pub nome: String,
    pub valor: String,
    pub importante: bool,
}

struct Regra {
    seletor: Seletor,
    especificidade: u32,
    /// Índices em `Folha::blocos` e `Folha::condicoes` (u16::MAX = sem @media).
    bloco: usize,
    midia: u16,
}

/// Uma folha de estilo já lida.
#[derive(Default)]
pub struct Folha {
    regras: Vec<Regra>,
    blocos: Vec<Vec<Declaracao>>,
    condicoes: Vec<String>,
}

// -----------------------------------------------------------------------------
// O leitor
// -----------------------------------------------------------------------------

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

/// O índice do `}` que fecha o bloco que começa depois de `inicio` (que é o `{`).
fn fim_do_bloco(t: &[u8], inicio: usize) -> usize {
    let mut profundidade = 0;
    let mut aspas = 0u8;
    for (i, &c) in t.iter().enumerate().skip(inicio) {
        match c {
            b'"' | b'\'' if aspas == 0 => aspas = c,
            c if c == aspas => aspas = 0,
            _ if aspas != 0 => {}
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
    t.len()
}

fn ler_composto(texto: &str) -> Option<Composto> {
    let mut c = Composto::default();
    let t = texto.as_bytes();
    let mut i = 0;
    let nome = |i: &mut usize| -> String {
        let inicio = *i;
        while *i < t.len() && (t[*i].is_ascii_alphanumeric() || t[*i] == b'-' || t[*i] == b'_' || t[*i] >= 0x80 || t[*i] == b'\\') {
            *i += if t[*i] == b'\\' { 2 } else { 1 };
        }
        texto.get(inicio..(*i).min(t.len())).unwrap_or("").replace('\\', "")
    };
    if i < t.len() && t[i] == b'*' {
        i += 1;
    } else if i < t.len() && (t[i].is_ascii_alphabetic()) {
        c.tag = Some(nome(&mut i).to_ascii_lowercase());
    }
    while i < t.len() {
        match t[i] {
            b'.' => {
                i += 1;
                c.classes.push(nome(&mut i));
            }
            b'#' => {
                i += 1;
                c.id = Some(nome(&mut i));
            }
            b'[' => {
                let fim = texto[i..].find(']')? + i;
                let dentro = &texto[i + 1..fim];
                let (nome_attr, op, valor) = match dentro.find('=') {
                    None => (dentro.trim(), Operador::Existe, ""),
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
                        let valor = valor.strip_suffix(" i").unwrap_or(valor).trim();
                        (nome_attr.trim(), op, valor.trim_matches(['"', '\'']))
                    }
                };
                c.atributos.push((nome_attr.to_ascii_lowercase(), op, valor.into()));
                i = fim + 1;
            }
            b':' => {
                let resto = &texto[i..];
                if resto.starts_with(":root") && !resto[5..].starts_with(|ch: char| ch.is_alphanumeric() || ch == '-') {
                    c.raiz = true;
                    i += 5;
                } else if resto.starts_with(":link") || resto.starts_with(":any-link") {
                    c.atributos.push(("href".into(), Operador::Existe, String::new()));
                    i += if resto.starts_with(":link") { 5 } else { 9 };
                } else {
                    return None; // :hover, ::before, :not()... não casam
                }
            }
            _ => return None,
        }
    }
    Some(c)
}

fn ler_seletor(texto: &str) -> Option<(Seletor, u32)> {
    // Separa os compostos e os combinadores (espaços em volta do ">" não contam).
    let texto = texto.replace('>', " > ");
    let mut partes = Vec::new();
    let mut combinador = Combinador::Descendente;
    let mut fichas = Vec::new();
    // Espaços dentro de [...] não separam.
    let mut atual = String::new();
    let mut colchetes = 0;
    for ch in texto.chars() {
        match ch {
            '[' => colchetes += 1,
            ']' => colchetes -= 1,
            _ => {}
        }
        if ch.is_whitespace() && colchetes == 0 {
            if !atual.is_empty() {
                fichas.push(core::mem::take(&mut atual));
            }
        } else {
            atual.push(ch);
        }
    }
    if !atual.is_empty() {
        fichas.push(atual);
    }
    for f in fichas {
        match f.as_str() {
            ">" => combinador = Combinador::Filho,
            "+" | "~" => return None,
            _ => {
                partes.push((ler_composto(&f)?, combinador));
                combinador = Combinador::Descendente;
            }
        }
    }
    if partes.is_empty() {
        return None;
    }
    // Da direita para a esquerda; o combinador de cada parte é o que a liga à da esquerda.
    let n = partes.len();
    let mut invertidas: Vec<(Composto, Combinador)> = Vec::with_capacity(n);
    for i in (0..n).rev() {
        let liga = if i > 0 { partes[i].1 } else { Combinador::Descendente };
        invertidas.push((partes[i].0.clone(), liga));
    }
    let (mut a, mut b, mut c) = (0, 0, 0);
    for (p, _) in &invertidas {
        a += p.id.is_some() as u32;
        b += (p.classes.len() + p.atributos.len() + p.raiz as usize) as u32;
        c += p.tag.is_some() as u32;
    }
    Some((Seletor { partes: invertidas }, a.min(255) << 16 | b.min(255) << 8 | c.min(255)))
}

/// As declarações de um bloco (`cor: azul; margem: 0 !important`).
pub fn ler_declaracoes(texto: &str) -> Vec<Declaracao> {
    let mut saida = Vec::new();
    let mut atual = String::new();
    let (mut parenteses, mut aspas) = (0, None);
    let terminar = |d: &str, saida: &mut Vec<Declaracao>| {
        if let Some((nome, valor)) = d.split_once(':') {
            let nome = nome.trim().to_ascii_lowercase();
            let mut valor = valor.trim();
            let importante = valor.to_ascii_lowercase().ends_with("!important");
            if importante {
                valor = valor[..valor.len() - "!important".len()].trim();
            }
            if !nome.is_empty() && !valor.is_empty() {
                let valor = if nome.starts_with("--") { valor.to_string() } else { valor.to_ascii_lowercase() };
                saida.push(Declaracao { nome, valor, importante });
            }
        }
    };
    for ch in texto.chars() {
        match ch {
            '"' | '\'' if aspas.is_none() => aspas = Some(ch),
            c if Some(c) == aspas => aspas = None,
            '(' if aspas.is_none() => parenteses += 1,
            ')' if aspas.is_none() => parenteses -= 1,
            ';' if aspas.is_none() && parenteses <= 0 => {
                terminar(&atual, &mut saida);
                atual.clear();
                continue;
            }
            _ => {}
        }
        atual.push(ch);
    }
    terminar(&atual, &mut saida);
    saida
}

impl Folha {
    pub fn ler(texto: &str) -> Folha {
        let mut f = Folha::default();
        let texto = sem_comentarios(texto);
        f.ler_regras(&texto, u16::MAX);
        f
    }

    pub fn regras(&self) -> usize {
        self.regras.len()
    }

    fn ler_regras(&mut self, texto: &str, midia: u16) {
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
                match (chave, ponto_e_virgula) {
                    (Some(c), Some(p)) if p < c => {
                        i = p + 1;
                        continue;
                    }
                    (None, Some(p)) => {
                        i = p + 1;
                        continue;
                    }
                    (None, None) => break,
                    _ => {}
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
                        self.ler_regras(dentro, n);
                    }
                    "supports" | "layer" | "document" | "container" => self.ler_regras(dentro, midia),
                    _ => {} // @font-face, @keyframes, @page...
                }
                i = fim + 1;
                continue;
            }
            let Some(c) = chave else { break };
            let fim = fim_do_bloco(t, c);
            let seletores = &texto[i..c];
            let corpo = &texto[c + 1..fim.min(texto.len())];
            i = fim + 1;
            if corpo.contains('{') {
                continue; // CSS aninhado: não sabemos
            }
            let declaracoes = ler_declaracoes(corpo);
            if declaracoes.is_empty() {
                continue;
            }
            let bloco = self.blocos.len();
            let mut algum = false;
            for s in dividir_seletores(seletores) {
                if let Some((seletor, especificidade)) = ler_seletor(s.trim()) {
                    self.regras.push(Regra { seletor, especificidade, bloco, midia });
                    algum = true;
                }
            }
            if algum {
                self.blocos.push(declaracoes);
            }
        }
    }
}

/// "a, b:not(.c, .d)": vírgulas fora de parênteses separam seletores.
fn dividir_seletores(t: &str) -> Vec<&str> {
    let mut v = Vec::new();
    let (mut inicio, mut prof) = (0, 0);
    for (i, ch) in t.char_indices() {
        match ch {
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

/// Uma condição de `@media` vale para uma tela de `largura` pixels?
/// "screen and (max-width: 720px), print" etc.
pub fn midia_vale(condicao: &str, largura: i32) -> bool {
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
                match nome {
                    "max-width" => largura <= px(),
                    "min-width" => largura >= px(),
                    "orientation" => valor == "landscape",
                    "prefers-color-scheme" => valor == "light",
                    "hover" | "any-hover" => valor == "hover",
                    "pointer" | "any-pointer" => valor == "fine",
                    "prefers-reduced-motion" => valor == "reduce",
                    "color" | "min-resolution" | "max-resolution" | "-webkit-min-device-pixel-ratio" => nome == "color",
                    _ => false,
                }
            } else {
                matches!(parte, "screen" | "all" | "")
            }
        });
        vale != negar
    })
}

/// Um tamanho em pixels: "12px", "1.5em" (com `em` pixels), "2rem", "10pt".
/// Sem unidade, só o zero. Porcentagens e "auto" não são tamanhos aqui.
pub fn pixels(valor: &str, em: f32) -> Option<i32> {
    let v = valor.trim();
    let (numero, unidade) = v.split_at(v.find(|c: char| !(c.is_ascii_digit() || c == '.' || c == '-' || c == '+')).unwrap_or(v.len()));
    let n: f32 = numero.parse().ok()?;
    let px = match unidade.trim() {
        "px" => n,
        "em" => n * em,
        "rem" => n * 16.0,
        "pt" => n * 4.0 / 3.0,
        "ex" | "ch" => n * em / 2.0,
        "" if n == 0.0 => 0.0,
        _ => return None,
    };
    Some(px as i32)
}

// -----------------------------------------------------------------------------
// A cascata
// -----------------------------------------------------------------------------

impl Composto {
    fn casa(&self, e: &Elemento, raiz: bool) -> bool {
        if self.raiz && !raiz {
            return false;
        }
        if let Some(t) = &self.tag {
            if *t != e.nome {
                return false;
            }
        }
        if let Some(id) = &self.id {
            if e.atributo("id") != Some(id.as_str()) {
                return false;
            }
        }
        if !self.classes.is_empty() {
            let classes = e.atributo("class").unwrap_or("");
            if !self.classes.iter().all(|c| classes.split_ascii_whitespace().any(|x| x == c)) {
                return false;
            }
        }
        self.atributos.iter().all(|(nome, op, valor)| {
            let Some(v) = e.atributo(nome) else { return false };
            match op {
                Operador::Existe => true,
                Operador::Igual => v == valor,
                Operador::Palavra => v.split_ascii_whitespace().any(|x| x == valor),
                Operador::Comeca => !valor.is_empty() && v.starts_with(valor.as_str()),
                Operador::Termina => !valor.is_empty() && v.ends_with(valor.as_str()),
                Operador::Contem => !valor.is_empty() && v.contains(valor.as_str()),
                Operador::Hifen => v == valor || v.starts_with(&format!("{}-", valor)),
            }
        })
    }
}

impl Seletor {
    fn casa(&self, e: &Elemento, ancestrais: &[&Elemento]) -> bool {
        casa_partes(&self.partes, e, ancestrais)
    }
}

fn casa_partes(partes: &[(Composto, Combinador)], e: &Elemento, ancestrais: &[&Elemento]) -> bool {
    let raiz = e.nome == "html";
    if !partes[0].0.casa(e, raiz) {
        return false;
    }
    if partes.len() == 1 {
        return true;
    }
    match partes[0].1 {
        Combinador::Filho => match ancestrais.split_last() {
            Some((pai, resto)) => casa_partes(&partes[1..], pai, resto),
            None => false,
        },
        Combinador::Descendente => (0..ancestrais.len()).rev().any(|i| casa_partes(&partes[1..], ancestrais[i], &ancestrais[..i])),
    }
}

/// As regras de todas as folhas, indexadas pelo composto da direita (o
/// id, uma classe, ou a tag): para cada elemento, só as candidatas.
struct Indice<'a> {
    por_id: BTreeMap<&'a str, Vec<(usize, usize)>>,
    por_classe: BTreeMap<&'a str, Vec<(usize, usize)>>,
    por_tag: BTreeMap<&'a str, Vec<(usize, usize)>>,
    outras: Vec<(usize, usize)>,
}

/// O estilo calculado de cada elemento (pelo endereço dele na árvore):
/// as declarações que valem, já na ordem da cascata (a última vence).
pub type Calculado = BTreeMap<usize, Vec<(String, String)>>;

/// Aplica as folhas (na ordem do documento) à árvore, para uma tela de `largura` pixels.
pub fn calcular(raiz: &Elemento, folhas: &[&Folha], largura: i32) -> Calculado {
    let mut indice = Indice { por_id: BTreeMap::new(), por_classe: BTreeMap::new(), por_tag: BTreeMap::new(), outras: Vec::new() };
    for (f, folha) in folhas.iter().enumerate() {
        let condicoes: Vec<bool> = folha.condicoes.iter().map(|c| midia_vale(c, largura)).collect();
        for (r, regra) in folha.regras.iter().enumerate() {
            if regra.midia != u16::MAX && !condicoes[regra.midia as usize] {
                continue;
            }
            let direita = &regra.seletor.partes[0].0;
            if let Some(id) = &direita.id {
                indice.por_id.entry(id.as_str()).or_default().push((f, r));
            } else if let Some(c) = direita.classes.first() {
                indice.por_classe.entry(c.as_str()).or_default().push((f, r));
            } else if let Some(t) = &direita.tag {
                indice.por_tag.entry(t.as_str()).or_default().push((f, r));
            } else {
                indice.outras.push((f, r));
            }
        }
    }
    let mut saida = Calculado::new();
    let mut variaveis = BTreeMap::new();
    let mut pilha = Vec::new();
    calcular_em(raiz, folhas, &indice, &mut pilha, &mut saida, &mut variaveis);
    // var(--x, reserva): com as variáveis da raiz.
    if !variaveis.is_empty() {
        for declaracoes in saida.values_mut() {
            for (_, valor) in declaracoes.iter_mut() {
                if valor.contains("var(") {
                    *valor = trocar_variaveis(valor, &variaveis);
                }
            }
        }
    }
    saida
}

fn trocar_variaveis(valor: &str, variaveis: &BTreeMap<String, String>) -> String {
    let mut v = String::from(valor);
    for _ in 0..4 {
        let Some(i) = v.find("var(") else { break };
        // o ")" que fecha este var(
        let mut prof = 0;
        let mut fim = v.len();
        for (j, ch) in v[i..].char_indices() {
            match ch {
                '(' => prof += 1,
                ')' => {
                    prof -= 1;
                    if prof == 0 {
                        fim = i + j;
                        break;
                    }
                }
                _ => {}
            }
        }
        let dentro = &v[i + 4..fim.min(v.len())];
        let (nome, reserva) = dentro.split_once(',').map_or((dentro.trim(), ""), |(n, r)| (n.trim(), r.trim()));
        let troca = variaveis.get(nome).map(|s| s.to_ascii_lowercase()).unwrap_or_else(|| reserva.to_string());
        v = format!("{}{}{}", &v[..i], troca, v.get(fim + 1..).unwrap_or(""));
    }
    v
}

fn calcular_em<'a>(
    e: &'a Elemento,
    folhas: &[&Folha],
    indice: &Indice,
    pilha: &mut Vec<&'a Elemento>,
    saida: &mut Calculado,
    variaveis: &mut BTreeMap<String, String>,
) {
    let mut candidatas: Vec<(usize, usize)> = Vec::new();
    if let Some(id) = e.atributo("id") {
        if let Some(v) = indice.por_id.get(id) {
            candidatas.extend_from_slice(v);
        }
    }
    for c in e.atributo("class").unwrap_or("").split_ascii_whitespace() {
        if let Some(v) = indice.por_classe.get(c) {
            candidatas.extend_from_slice(v);
        }
    }
    if let Some(v) = indice.por_tag.get(e.nome.as_str()) {
        candidatas.extend_from_slice(v);
    }
    candidatas.extend_from_slice(&indice.outras);
    let mut valem: Vec<(bool, u32, usize, usize, usize)> = Vec::new(); // (importante, especificidade, folha, regra, declaração)
    candidatas.sort_unstable();
    candidatas.dedup();
    for (f, r) in candidatas {
        let regra = &folhas[f].regras[r];
        if regra.seletor.casa(e, pilha) {
            for (d, decl) in folhas[f].blocos[regra.bloco].iter().enumerate() {
                valem.push((decl.importante, regra.especificidade, f, r, d));
            }
        }
    }
    if !valem.is_empty() {
        valem.sort_unstable();
        let mut lista = Vec::with_capacity(valem.len());
        for (_, _, f, r, d) in valem {
            let decl = &folhas[f].blocos[folhas[f].regras[r].bloco][d];
            if decl.nome.starts_with("--") {
                if e.nome == "html" || e.nome == "body" {
                    variaveis.insert(decl.nome.clone(), decl.valor.clone());
                }
                continue;
            }
            lista.push((decl.nome.clone(), decl.valor.clone()));
        }
        if !lista.is_empty() {
            saida.insert(e as *const Elemento as usize, lista);
        }
    }
    pilha.push(e);
    for f in &e.filhos {
        if let crate::html::No::Elemento(filho) = f {
            calcular_em(filho, folhas, indice, pilha, saida, variaveis);
        }
    }
    pilha.pop();
}

use alloc::format;
