//! Expressões regulares do JavaScript: compiladas para um pequeno programa e
//! rodadas por uma máquina com retrocesso (backtracking) e pilha explícita.
//! Há um limite de passos: uma expressão patológica falha em vez de travar
//! a janela do navegador.

use alloc::rc::Rc;
use alloc::string::String;
use alloc::vec::Vec;

#[derive(Clone, Debug)]
enum No {
    Vazio,
    Char(u32),
    Qualquer,
    Classe(usize),
    Seq(Vec<No>),
    Alt(Vec<No>),
    Grupo(Box<No>, usize),
    NaoCaptura(Box<No>),
    Repete { dentro: Box<No>, min: u32, max: u32, guloso: bool, grupos: (usize, usize) },
    InicioLinha,
    FimLinha,
    Borda(bool),
    RefVolta(usize),
    RefNome(String),
    Olhar { dentro: Box<No>, negar: bool, atras: bool },
}

use alloc::boxed::Box;

#[derive(Clone, Debug, Default)]
pub struct Classe {
    faixas: Vec<(u32, u32)>,
    negada: bool,
    /// Letra? maiúscula? (das categorias \p{...})
    props: Vec<u8>,
}

const P_LETRA: u8 = 1;
const P_MAIUSC: u8 = 2;
const P_MINUSC: u8 = 3;
const P_NUMERO: u8 = 4;
const P_PONTUACAO: u8 = 5;
const P_ESPACO: u8 = 6;
const P_NAO_LETRA: u8 = 7;
const P_TUDO: u8 = 8;

#[derive(Clone, Copy, Debug)]
enum I {
    Char(u32),
    Qualquer,
    QualquerTudo,
    Classe(u32),
    /// Tenta a primeira; se falhar, a segunda.
    Divide(u32, u32),
    Salto(u32),
    Salvar(u32),
    InicioLinha,
    FimLinha,
    Borda(bool),
    RefVolta(u32),
    /// O programa de dentro vai de pc+1 até `fim` (que tem `Sucesso`).
    Olhar { fim: u32, negar: bool, atras: bool },
    /// Anota a posição num registrador (para não repetir o vazio para sempre).
    Marca(u32),
    /// Falha se não avançou desde a marca.
    ChecaAvanco(u32),
    /// Zera os grupos de `a` até `b` (cada volta de um laço começa limpa).
    ZeraGrupos(u32, u32),
    Sucesso,
}

pub struct Regex {
    prog: Vec<I>,
    classes: Vec<Classe>,
    pub n_grupos: usize,
    n_regs: usize,
    pub nomes: Vec<(String, usize)>,
    pub global: bool,
    pub ignorar: bool,
    pub multilinha: bool,
    pub ponto_tudo: bool,
    pub unicode: bool,
    pub pegajosa: bool,
    pub indices: bool,
    pub fonte: Rc<str>,
    pub flags: Rc<str>,
}

struct Leitor<'a> {
    c: Vec<char>,
    i: usize,
    n_grupos: usize,
    classes: Vec<Classe>,
    nomes: Vec<(String, usize)>,
    unicode: bool,
    _m: core::marker::PhantomData<&'a ()>,
}

type R<T> = Result<T, String>;

fn e_palavra(c: u32) -> bool {
    c < 128 && ((c as u8).is_ascii_alphanumeric() || c == b'_' as u32)
}

fn faixas_palavra() -> Vec<(u32, u32)> {
    alloc::vec![(48, 57), (65, 90), (95, 95), (97, 122)]
}

fn faixas_espaco() -> Vec<(u32, u32)> {
    alloc::vec![(9, 13), (32, 32), (160, 160), (0x1680, 0x1680), (0x2000, 0x200A), (0x2028, 0x2029), (0x202F, 0x202F), (0x205F, 0x205F), (0x3000, 0x3000), (0xFEFF, 0xFEFF)]
}

fn inverter(f: &[(u32, u32)]) -> Vec<(u32, u32)> {
    let mut o = f.to_vec();
    o.sort_unstable();
    let mut r = Vec::new();
    let mut p = 0;
    for (a, b) in o {
        if a > p {
            r.push((p, a - 1));
        }
        p = p.max(b + 1);
    }
    if p <= 0x10FFFF {
        r.push((p, 0x10FFFF));
    }
    r
}

impl Leitor<'_> {
    fn ver(&self) -> Option<char> {
        self.c.get(self.i).copied()
    }

    fn aceita(&mut self, c: char) -> bool {
        if self.ver() == Some(c) {
            self.i += 1;
            true
        } else {
            false
        }
    }

    fn alternativa(&mut self) -> R<No> {
        let mut opcoes = alloc::vec![self.sequencia()?];
        while self.aceita('|') {
            opcoes.push(self.sequencia()?);
        }
        Ok(if opcoes.len() == 1 { opcoes.pop().unwrap() } else { No::Alt(opcoes) })
    }

    fn sequencia(&mut self) -> R<No> {
        let mut v = Vec::new();
        while let Some(c) = self.ver() {
            if c == '|' || c == ')' {
                break;
            }
            let grupos_antes = self.n_grupos;
            let at = self.atomo()?;
            let q = self.quantificador(at, grupos_antes)?;
            v.push(q);
        }
        Ok(match v.len() {
            0 => No::Vazio,
            1 => v.pop().unwrap(),
            _ => No::Seq(v),
        })
    }

    fn numero(&mut self) -> Option<u32> {
        let ini = self.i;
        let mut n: u64 = 0;
        while let Some(d) = self.ver().and_then(|c| c.to_digit(10)) {
            n = (n * 10 + d as u64).min(u32::MAX as u64);
            self.i += 1;
        }
        if self.i == ini { None } else { Some(n as u32) }
    }

    fn quantificador(&mut self, at: No, grupos_antes: usize) -> R<No> {
        let (min, max) = match self.ver() {
            Some('*') => {
                self.i += 1;
                (0, u32::MAX)
            }
            Some('+') => {
                self.i += 1;
                (1, u32::MAX)
            }
            Some('?') => {
                self.i += 1;
                (0, 1)
            }
            Some('{') => {
                let antes = self.i;
                self.i += 1;
                let par = (|| {
                    let a = self.numero()?;
                    if self.aceita(',') {
                        if self.ver() == Some('}') {
                            self.i += 1;
                            return Some((a, u32::MAX));
                        }
                        let b = self.numero()?;
                        if !self.aceita('}') {
                            return None;
                        }
                        Some((a, b))
                    } else if self.aceita('}') {
                        Some((a, a))
                    } else {
                        None
                    }
                })();
                match par {
                    Some(p) => p,
                    None => {
                        // `{` que não é quantificador: uma letra comum.
                        self.i = antes;
                        return Ok(at);
                    }
                }
            }
            _ => return Ok(at),
        };
        if max < min {
            return Err(String::from("numbers out of order in {} quantifier"));
        }
        let guloso = !self.aceita('?');
        Ok(No::Repete { dentro: Box::new(at), min, max, guloso, grupos: (grupos_antes + 1, self.n_grupos + 1) })
    }

    fn atomo(&mut self) -> R<No> {
        let c = self.ver().unwrap();
        self.i += 1;
        Ok(match c {
            '^' => No::InicioLinha,
            '$' => No::FimLinha,
            '.' => No::Qualquer,
            '(' => return self.grupo(),
            '[' => return self.classe(),
            '\\' => return self.escape(),
            '*' | '+' | '?' => return Err(String::from("nothing to repeat")),
            c => No::Char(c as u32),
        })
    }

    fn grupo(&mut self) -> R<No> {
        let no = if self.aceita('?') {
            match self.ver() {
                Some(':') => {
                    self.i += 1;
                    let d = self.alternativa()?;
                    No::NaoCaptura(Box::new(d))
                }
                Some('=') | Some('!') => {
                    let negar = self.ver() == Some('!');
                    self.i += 1;
                    let d = self.alternativa()?;
                    No::Olhar { dentro: Box::new(d), negar, atras: false }
                }
                Some('<') => {
                    self.i += 1;
                    if matches!(self.ver(), Some('=' | '!')) {
                        let negar = self.ver() == Some('!');
                        self.i += 1;
                        let d = self.alternativa()?;
                        No::Olhar { dentro: Box::new(d), negar, atras: true }
                    } else {
                        let mut nome = String::new();
                        while let Some(ch) = self.ver() {
                            self.i += 1;
                            if ch == '>' {
                                break;
                            }
                            nome.push(ch);
                        }
                        self.n_grupos += 1;
                        let g = self.n_grupos;
                        self.nomes.push((nome, g));
                        let d = self.alternativa()?;
                        No::Grupo(Box::new(d), g)
                    }
                }
                _ => return Err(String::from("invalid group")),
            }
        } else {
            self.n_grupos += 1;
            let g = self.n_grupos;
            let d = self.alternativa()?;
            No::Grupo(Box::new(d), g)
        };
        if !self.aceita(')') {
            return Err(String::from("unterminated group"));
        }
        Ok(no)
    }

    fn nova_classe(&mut self, c: Classe) -> No {
        self.classes.push(c);
        No::Classe(self.classes.len() - 1)
    }

    fn propriedade(&mut self) -> R<Classe> {
        if !self.aceita('{') {
            return Err(String::from("invalid property name"));
        }
        let mut nome = String::new();
        while let Some(ch) = self.ver() {
            self.i += 1;
            if ch == '}' {
                break;
            }
            nome.push(ch);
        }
        let curto = nome.rsplit('=').next().unwrap_or("");
        let p = match curto {
            "L" | "Letter" | "Alphabetic" | "Alpha" => P_LETRA,
            "Lu" | "Uppercase_Letter" | "Uppercase" => P_MAIUSC,
            "Ll" | "Lowercase_Letter" | "Lowercase" => P_MINUSC,
            "N" | "Nd" | "Number" | "Decimal_Number" => P_NUMERO,
            "P" | "Punctuation" | "S" | "Symbol" => P_PONTUACAO,
            "Z" | "Zs" | "White_Space" | "Separator" => P_ESPACO,
            "M" | "Mn" | "Mark" => P_NAO_LETRA,
            _ => P_TUDO,
        };
        Ok(Classe { faixas: Vec::new(), negada: false, props: alloc::vec![p] })
    }

    fn escape(&mut self) -> R<No> {
        let Some(c) = self.ver() else { return Err(String::from("\\ at end of pattern")) };
        self.i += 1;
        Ok(match c {
            'b' => No::Borda(true),
            'B' => No::Borda(false),
            'd' => self.nova_classe(Classe { faixas: alloc::vec![(48, 57)], ..Default::default() }),
            'D' => self.nova_classe(Classe { faixas: alloc::vec![(48, 57)], negada: true, ..Default::default() }),
            'w' => self.nova_classe(Classe { faixas: faixas_palavra(), ..Default::default() }),
            'W' => self.nova_classe(Classe { faixas: faixas_palavra(), negada: true, ..Default::default() }),
            's' => self.nova_classe(Classe { faixas: faixas_espaco(), ..Default::default() }),
            'S' => self.nova_classe(Classe { faixas: faixas_espaco(), negada: true, ..Default::default() }),
            'p' | 'P' if self.unicode => {
                let mut cl = self.propriedade()?;
                cl.negada = c == 'P';
                self.nova_classe(cl)
            }
            'k' if self.ver() == Some('<') => {
                self.i += 1;
                let mut nome = String::new();
                while let Some(ch) = self.ver() {
                    self.i += 1;
                    if ch == '>' {
                        break;
                    }
                    nome.push(ch);
                }
                No::RefNome(nome)
            }
            '1'..='9' => {
                self.i -= 1;
                let n = self.numero().unwrap_or(1);
                No::RefVolta(n as usize)
            }
            _ => {
                self.i -= 1;
                No::Char(self.char_escapado()?)
            }
        })
    }

    /// Um escape que representa uma letra só.
    fn char_escapado(&mut self) -> R<u32> {
        let Some(c) = self.ver() else { return Err(String::from("\\ at end of pattern")) };
        self.i += 1;
        Ok(match c {
            'n' => 10,
            'r' => 13,
            't' => 9,
            'v' => 11,
            'f' => 12,
            '0' => 0,
            'c' => {
                let l = self.ver().unwrap_or('@');
                self.i += 1;
                (l as u32) % 32
            }
            'x' => {
                let h: Option<u32> = (|| Some(self.c.get(self.i)?.to_digit(16)? * 16 + self.c.get(self.i + 1)?.to_digit(16)?))();
                match h {
                    Some(v) => {
                        self.i += 2;
                        v
                    }
                    None => 'x' as u32,
                }
            }
            'u' => {
                if self.ver() == Some('{') {
                    self.i += 1;
                    let mut v = 0;
                    while let Some(ch) = self.ver() {
                        self.i += 1;
                        if ch == '}' {
                            break;
                        }
                        v = v * 16 + ch.to_digit(16).unwrap_or(0);
                    }
                    v
                } else {
                    let le = |s: &Self, p: usize| -> Option<u32> {
                        let mut v = 0;
                        for k in 0..4 {
                            v = v * 16 + s.c.get(p + k)?.to_digit(16)?;
                        }
                        Some(v)
                    };
                    match le(self, self.i) {
                        Some(v) => {
                            self.i += 4;
                            v
                        }
                        None => 'u' as u32,
                    }
                }
            }
            c => c as u32,
        })
    }

    fn classe(&mut self) -> R<No> {
        let mut cl = Classe { negada: self.aceita('^'), ..Default::default() };
        loop {
            let Some(c) = self.ver() else { return Err(String::from("missing terminating ] for character class")) };
            self.i += 1;
            if c == ']' {
                break;
            }
            let a = match c {
                '\\' => match self.ver() {
                    Some('d') => {
                        self.i += 1;
                        cl.faixas.push((48, 57));
                        continue;
                    }
                    Some('D') => {
                        self.i += 1;
                        cl.faixas.extend(inverter(&[(48, 57)]));
                        continue;
                    }
                    Some('w') => {
                        self.i += 1;
                        cl.faixas.extend(faixas_palavra());
                        continue;
                    }
                    Some('W') => {
                        self.i += 1;
                        cl.faixas.extend(inverter(&faixas_palavra()));
                        continue;
                    }
                    Some('s') => {
                        self.i += 1;
                        cl.faixas.extend(faixas_espaco());
                        continue;
                    }
                    Some('S') => {
                        self.i += 1;
                        cl.faixas.extend(inverter(&faixas_espaco()));
                        continue;
                    }
                    Some('p') if self.unicode => {
                        self.i += 1;
                        let p = self.propriedade()?;
                        cl.props.extend(p.props);
                        continue;
                    }
                    Some('b') => {
                        self.i += 1;
                        8
                    }
                    Some('-') => {
                        self.i += 1;
                        '-' as u32
                    }
                    _ => self.char_escapado()?,
                },
                c => c as u32,
            };
            if self.ver() == Some('-') && self.c.get(self.i + 1).is_some_and(|x| *x != ']') {
                let antes = self.i;
                self.i += 1;
                let d = self.ver().unwrap();
                self.i += 1;
                let b = if d == '\\' {
                    if matches!(self.ver(), Some('d' | 'D' | 'w' | 'W' | 's' | 'S')) {
                        self.i = antes;
                        cl.faixas.push((a, a));
                        continue;
                    }
                    self.char_escapado()?
                } else {
                    d as u32
                };
                if b < a {
                    return Err(String::from("range out of order in character class"));
                }
                cl.faixas.push((a, b));
            } else {
                cl.faixas.push((a, a));
            }
        }
        Ok(self.nova_classe(cl))
    }
}

struct Compilador {
    p: Vec<I>,
    nomes: Vec<(String, usize)>,
    ignorar: bool,
    ponto_tudo: bool,
    n_regs: usize,
    n_grupos: usize,
}

impl Compilador {
    fn e(&mut self, i: I) -> usize {
        self.p.push(i);
        self.p.len() - 1
    }

    fn no(&mut self, n: &No) -> R<()> {
        if self.p.len() > 200_000 {
            return Err(String::from("regular expression too large"));
        }
        match n {
            No::Vazio => {}
            No::Char(c) => {
                let c = if self.ignorar { dobrar(*c) } else { *c };
                self.e(I::Char(c));
            }
            No::Qualquer => {
                self.e(if self.ponto_tudo { I::QualquerTudo } else { I::Qualquer });
            }
            No::Classe(i) => {
                self.e(I::Classe(*i as u32));
            }
            No::Seq(v) => {
                for x in v {
                    self.no(x)?;
                }
            }
            No::Alt(opcoes) => {
                let mut saltos = Vec::new();
                for (i, o) in opcoes.iter().enumerate() {
                    if i + 1 < opcoes.len() {
                        let d = self.e(I::Divide(0, 0));
                        self.no(o)?;
                        saltos.push(self.e(I::Salto(0)));
                        let prox = self.p.len() as u32;
                        self.p[d] = I::Divide(d as u32 + 1, prox);
                    } else {
                        self.no(o)?;
                    }
                }
                let fim = self.p.len() as u32;
                for s in saltos {
                    self.p[s] = I::Salto(fim);
                }
            }
            No::Grupo(d, g) => {
                self.e(I::Salvar(*g as u32 * 2));
                self.no(d)?;
                self.e(I::Salvar(*g as u32 * 2 + 1));
            }
            No::NaoCaptura(d) => self.no(d)?,
            No::Repete { dentro, min, max, guloso, grupos } => self.repete(dentro, *min, *max, *guloso, *grupos)?,
            No::InicioLinha => {
                self.e(I::InicioLinha);
            }
            No::FimLinha => {
                self.e(I::FimLinha);
            }
            No::Borda(b) => {
                self.e(I::Borda(*b));
            }
            No::RefVolta(g) => {
                // \8 sem o grupo 8: não há referência; casa o vazio (quase nunca aparece).
                if *g <= self.n_grupos {
                    self.e(I::RefVolta(*g as u32));
                }
            }
            No::RefNome(nome) => {
                let Some((_, g)) = self.nomes.iter().find(|(n, _)| n == nome) else {
                    return Err(String::from("invalid named capture referenced"));
                };
                let g = *g as u32;
                self.e(I::RefVolta(g));
            }
            No::Olhar { dentro, negar, atras } => {
                let o = self.e(I::Olhar { fim: 0, negar: *negar, atras: *atras });
                self.no(dentro)?;
                let f = self.e(I::Sucesso) as u32;
                self.p[o] = I::Olhar { fim: f, negar: *negar, atras: *atras };
            }
        }
        Ok(())
    }

    fn repete(&mut self, d: &No, min: u32, max: u32, guloso: bool, grupos: (usize, usize)) -> R<()> {
        if min > 1000 || (max != u32::MAX && max > 1000) {
            return Err(String::from("regular expression too large"));
        }
        let zera = grupos.1 > grupos.0;
        for _ in 0..min {
            if zera {
                self.e(I::ZeraGrupos(grupos.0 as u32, grupos.1 as u32));
            }
            self.no(d)?;
        }
        if max == u32::MAX {
            // L: divide(corpo, fim); marca; corpo; checa; salto L
            let reg = self.n_regs as u32;
            self.n_regs += 1;
            let l = self.e(I::Divide(0, 0));
            let corpo = self.p.len() as u32;
            self.e(I::Marca(reg));
            if zera {
                self.e(I::ZeraGrupos(grupos.0 as u32, grupos.1 as u32));
            }
            self.no(d)?;
            self.e(I::ChecaAvanco(reg));
            self.e(I::Salto(l as u32));
            let fim = self.p.len() as u32;
            self.p[l] = if guloso { I::Divide(corpo, fim) } else { I::Divide(fim, corpo) };
        } else {
            let mut divisoes = Vec::new();
            for _ in min..max {
                let dv = self.e(I::Divide(0, 0));
                divisoes.push(dv);
                if zera {
                    self.e(I::ZeraGrupos(grupos.0 as u32, grupos.1 as u32));
                }
                self.no(d)?;
            }
            let fim = self.p.len() as u32;
            for dv in divisoes {
                let c = dv as u32 + 1;
                self.p[dv] = if guloso { I::Divide(c, fim) } else { I::Divide(fim, c) };
            }
        }
        Ok(())
    }
}

/// A forma canônica para comparar sem diferenciar maiúsculas.
fn dobrar(c: u32) -> u32 {
    if c < 128 {
        return if (97..=122).contains(&c) { c - 32 } else { c };
    }
    let Some(ch) = char::from_u32(c) else { return c };
    let mut u = ch.to_uppercase();
    match (u.next(), u.next()) {
        (Some(x), None) => x as u32,
        _ => c,
    }
}

/// O texto onde se procura: ASCII direto, ou unidades de UTF-16.
pub enum Entrada<'a> {
    Ascii(&'a [u8]),
    U16(Vec<u16>),
}

impl Entrada<'_> {
    pub fn de(s: &str) -> Entrada<'_> {
        if s.is_ascii() { Entrada::Ascii(s.as_bytes()) } else { Entrada::U16(s.encode_utf16().collect()) }
    }

    #[inline]
    pub fn len(&self) -> usize {
        match self {
            Entrada::Ascii(b) => b.len(),
            Entrada::U16(v) => v.len(),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    #[inline]
    fn em(&self, i: usize) -> u32 {
        match self {
            Entrada::Ascii(b) => b[i] as u32,
            Entrada::U16(v) => v[i] as u32,
        }
    }

    /// Um pedaço como texto.
    pub fn pedaco(&self, a: usize, b: usize) -> String {
        match self {
            Entrada::Ascii(x) => String::from(core::str::from_utf8(&x[a..b]).unwrap_or("")),
            Entrada::U16(v) => String::from_utf16_lossy(&v[a..b]),
        }
    }
}

const LIMITE_PASSOS: u64 = 4_000_000;

enum Ramo {
    Tenta(u32, u32),
    Restaura(u32, i32),
    RestauraReg(u32, i32),
}

impl Regex {
    pub fn compilar(fonte: &str, flags: &str) -> Result<Regex, String> {
        let (mut g, mut i, mut m, mut s, mut u, mut y, mut d) = (false, false, false, false, false, false, false);
        for c in flags.chars() {
            let alvo = match c {
                'g' => &mut g,
                'i' => &mut i,
                'm' => &mut m,
                's' => &mut s,
                'u' | 'v' => &mut u,
                'y' => &mut y,
                'd' => &mut d,
                _ => return Err(alloc::format!("Invalid flags supplied to RegExp constructor '{}'", flags)),
            };
            if *alvo {
                return Err(alloc::format!("Invalid flags supplied to RegExp constructor '{}'", flags));
            }
            *alvo = true;
        }
        let mut l = Leitor { c: fonte.chars().collect(), i: 0, n_grupos: 0, classes: Vec::new(), nomes: Vec::new(), unicode: u, _m: core::marker::PhantomData };
        let arvore = l.alternativa().map_err(|e| alloc::format!("Invalid regular expression: /{}/: {}", fonte, e))?;
        if l.i < l.c.len() {
            return Err(alloc::format!("Invalid regular expression: /{}/: Unmatched ')'", fonte));
        }
        let n_grupos = l.n_grupos;
        let mut c = Compilador { p: Vec::new(), nomes: l.nomes.clone(), ignorar: i, ponto_tudo: s, n_regs: 0, n_grupos };
        c.no(&arvore).map_err(|e| alloc::format!("Invalid regular expression: /{}/: {}", fonte, e))?;
        c.e(I::Sucesso);
        Ok(Regex {
            prog: c.p,
            classes: l.classes,
            n_grupos,
            n_regs: c.n_regs,
            nomes: l.nomes,
            global: g,
            ignorar: i,
            multilinha: m,
            ponto_tudo: s,
            unicode: u,
            pegajosa: y,
            indices: d,
            fonte: Rc::from(fonte),
            flags: Rc::from(flags),
        })
    }

    fn casa_classe(&self, cl: &Classe, c: u32) -> bool {
        let dentro = |c: u32| {
            cl.faixas.iter().any(|&(a, b)| c >= a && c <= b)
                || cl.props.iter().any(|p| {
                    let Some(ch) = char::from_u32(c) else { return false };
                    match *p {
                        P_LETRA => ch.is_alphabetic(),
                        P_MAIUSC => ch.is_uppercase(),
                        P_MINUSC => ch.is_lowercase(),
                        P_NUMERO => ch.is_numeric(),
                        P_PONTUACAO => ch.is_ascii_punctuation() || (0x2010..0x2060).contains(&c) || (0xA1..0xC0).contains(&c),
                        P_ESPACO => ch.is_whitespace(),
                        P_NAO_LETRA => (0x300..0x370).contains(&c),
                        _ => true,
                    }
                })
        };
        let mut r = dentro(c);
        if !r && self.ignorar {
            let m = dobrar(c);
            r = dentro(m);
            if !r {
                if let Some(ch) = char::from_u32(c) {
                    let mut l = ch.to_lowercase();
                    if let (Some(x), None) = (l.next(), l.next()) {
                        r = dentro(x as u32);
                    }
                }
            }
        }
        r != cl.negada
    }

    /// Roda a partir de `inicio`. Devolve os grupos (pares de posições; -1 = não participou).
    pub fn casar(&self, e: &Entrada, inicio: usize) -> Option<Vec<i32>> {
        let mut limite = 0u64;
        let mut pos_inicio = inicio;
        let n = e.len();
        let mut grupos = alloc::vec![-1i32; (self.n_grupos + 1) * 2];
        let mut regs = alloc::vec![-1i32; self.n_regs];
        loop {
            for g in grupos.iter_mut() {
                *g = -1;
            }
            if let Some(fim) = self.rodar(e, 0, pos_inicio, &mut grupos, &mut regs, &mut limite, None) {
                grupos[0] = pos_inicio as i32;
                grupos[1] = fim as i32;
                return Some(grupos);
            }
            if self.pegajosa || pos_inicio >= n || limite > LIMITE_PASSOS {
                return None;
            }
            pos_inicio += 1;
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn rodar(&self, e: &Entrada, pc0: usize, pos0: usize, grupos: &mut Vec<i32>, regs: &mut Vec<i32>, passos: &mut u64, alvo_atras: Option<usize>) -> Option<usize> {
        let n = e.len();
        let mut pilha: Vec<Ramo> = Vec::new();
        let mut pc = pc0;
        let mut pos = pos0;
        loop {
            *passos += 1;
            if *passos > LIMITE_PASSOS {
                return None;
            }
            let mut falhou = false;
            match self.prog[pc] {
                I::Char(c) => {
                    if pos < n {
                        let (x, tam) = self.ler(e, pos);
                        let x = if self.ignorar { dobrar(x) } else { x };
                        if x == c {
                            pos += tam;
                            pc += 1;
                        } else {
                            falhou = true;
                        }
                    } else {
                        falhou = true;
                    }
                }
                I::Qualquer => {
                    if pos < n {
                        let (x, tam) = self.ler(e, pos);
                        if matches!(x, 10 | 13 | 0x2028 | 0x2029) {
                            falhou = true;
                        } else {
                            pos += tam;
                            pc += 1;
                        }
                    } else {
                        falhou = true;
                    }
                }
                I::QualquerTudo => {
                    if pos < n {
                        pos += self.ler(e, pos).1;
                        pc += 1;
                    } else {
                        falhou = true;
                    }
                }
                I::Classe(i) => {
                    if pos < n {
                        let (x, tam) = self.ler(e, pos);
                        if self.casa_classe(&self.classes[i as usize], x) {
                            pos += tam;
                            pc += 1;
                        } else {
                            falhou = true;
                        }
                    } else {
                        falhou = true;
                    }
                }
                I::Divide(a, b) => {
                    pilha.push(Ramo::Tenta(b, pos as u32));
                    pc = a as usize;
                }
                I::Salto(a) => pc = a as usize,
                I::Salvar(s) => {
                    pilha.push(Ramo::Restaura(s, grupos[s as usize]));
                    grupos[s as usize] = pos as i32;
                    pc += 1;
                }
                I::InicioLinha => {
                    if pos == 0 || (self.multilinha && matches!(e.em(pos - 1), 10 | 13 | 0x2028 | 0x2029)) {
                        pc += 1;
                    } else {
                        falhou = true;
                    }
                }
                I::FimLinha => {
                    if pos == n || (self.multilinha && matches!(e.em(pos), 10 | 13 | 0x2028 | 0x2029)) {
                        pc += 1;
                    } else {
                        falhou = true;
                    }
                }
                I::Borda(quer) => {
                    let antes = pos > 0 && e_palavra(e.em(pos - 1));
                    let depois = pos < n && e_palavra(e.em(pos));
                    if (antes != depois) == quer {
                        pc += 1;
                    } else {
                        falhou = true;
                    }
                }
                I::RefVolta(g) => {
                    let (a, b) = (grupos[g as usize * 2], grupos[g as usize * 2 + 1]);
                    if a >= 0 && b >= 0 {
                        let tam = (b - a) as usize;
                        if pos + tam <= n && (0..tam).all(|k| {
                            let (x, y) = (e.em(a as usize + k), e.em(pos + k));
                            x == y || (self.ignorar && dobrar(x) == dobrar(y))
                        }) {
                            pos += tam;
                            pc += 1;
                        } else {
                            falhou = true;
                        }
                    } else {
                        pc += 1;
                    }
                }
                I::Olhar { fim, negar, atras } => {
                    let copia = grupos.clone();
                    let achou = if atras {
                        // Tenta cada começo possível que termine aqui.
                        let mut r = false;
                        for j in (0..=pos).rev() {
                            *grupos = copia.clone();
                            if self.rodar(e, pc + 1, j, grupos, regs, passos, Some(pos)).is_some() {
                                r = true;
                                break;
                            }
                            if *passos > LIMITE_PASSOS {
                                break;
                            }
                        }
                        r
                    } else {
                        self.rodar(e, pc + 1, pos, grupos, regs, passos, None).is_some()
                    };
                    if achou != negar {
                        if negar {
                            *grupos = copia;
                        } else {
                            // Os grupos de dentro valem: guarda como desfazer.
                            for (s, v) in copia.iter().enumerate() {
                                if grupos[s] != *v {
                                    pilha.push(Ramo::Restaura(s as u32, *v));
                                }
                            }
                        }
                        pc = fim as usize + 1;
                    } else {
                        *grupos = copia;
                        falhou = true;
                    }
                }
                I::Marca(r) => {
                    pilha.push(Ramo::RestauraReg(r, regs[r as usize]));
                    regs[r as usize] = pos as i32;
                    pc += 1;
                }
                I::ChecaAvanco(r) => {
                    if regs[r as usize] == pos as i32 {
                        falhou = true;
                    } else {
                        pc += 1;
                    }
                }
                I::ZeraGrupos(a, b) => {
                    for g in a..b {
                        for s in [g * 2, g * 2 + 1] {
                            if grupos[s as usize] != -1 {
                                pilha.push(Ramo::Restaura(s, grupos[s as usize]));
                                grupos[s as usize] = -1;
                            }
                        }
                    }
                    pc += 1;
                }
                I::Sucesso => {
                    if let Some(alvo) = alvo_atras {
                        if pos != alvo {
                            falhou = true;
                        } else {
                            return Some(pos);
                        }
                    } else {
                        return Some(pos);
                    }
                }
            }
            if falhou {
                loop {
                    match pilha.pop() {
                        Some(Ramo::Tenta(p, q)) => {
                            pc = p as usize;
                            pos = q as usize;
                            break;
                        }
                        Some(Ramo::Restaura(s, v)) => grupos[s as usize] = v,
                        Some(Ramo::RestauraReg(r, v)) => regs[r as usize] = v,
                        None => return None,
                    }
                }
            }
        }
    }

    /// A letra em `pos`: com a flag `u`, um par de substitutos vale por uma.
    #[inline]
    fn ler(&self, e: &Entrada, pos: usize) -> (u32, usize) {
        let c = e.em(pos);
        if self.unicode && (0xD800..0xDC00).contains(&c) && pos + 1 < e.len() {
            let d = e.em(pos + 1);
            if (0xDC00..0xE000).contains(&d) {
                return (0x10000 + ((c - 0xD800) << 10) + (d - 0xDC00), 2);
            }
        }
        (c, 1)
    }
}
