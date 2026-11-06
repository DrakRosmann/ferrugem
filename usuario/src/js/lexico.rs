//! O léxico do JavaScript: do texto para uma lista de fichas.
//!
//! O arquivo inteiro vira fichas de uma vez. Assim o leitor pode olhar longe
//! à frente sem voltar atrás (`(a, b) => ...`: basta ver o que vem depois do
//! `)` correspondente, que já está anotado em `Fichas::par`).
//!
//! A dúvida clássica é se `/` começa uma expressão regular ou é uma divisão:
//! depende da ficha anterior (um nome ou um número: divisão).

use super::vm::{Interno, Txt};
use alloc::string::{String, ToString};
use alloc::vec::Vec;

/// As palavras reservadas são os primeiros átomos (veja `Interno::novo`), na
/// ordem desta lista: comparar é só comparar números.
macro_rules! palavras {
    ($($nome:ident = $texto:literal),* $(,)?) => {
        #[allow(non_camel_case_types, dead_code, clippy::upper_case_acronyms)]
        #[repr(u32)]
        enum Indice { $($nome),* }
        $(pub const $nome: Txt = Indice::$nome as u32;)*
        pub const PALAVRAS: &[&str] = &[$($texto),*];
    };
}

palavras! {
    K_BREAK = "break", K_CASE = "case", K_CATCH = "catch", K_CLASS = "class", K_CONST = "const",
    K_CONTINUE = "continue", K_DEBUGGER = "debugger", K_DEFAULT = "default", K_DELETE = "delete",
    K_DO = "do", K_ELSE = "else", K_EXPORT = "export", K_EXTENDS = "extends", K_FALSE = "false",
    K_FINALLY = "finally", K_FOR = "for", K_FUNCTION = "function", K_IF = "if", K_IMPORT = "import",
    K_IN = "in", K_INSTANCEOF = "instanceof", K_NEW = "new", K_NULL = "null", K_RETURN = "return",
    K_SUPER = "super", K_SWITCH = "switch", K_THIS = "this", K_THROW = "throw", K_TRUE = "true",
    K_TRY = "try", K_TYPEOF = "typeof", K_VAR = "var", K_VOID = "void", K_WHILE = "while", K_WITH = "with",
    // Daqui em diante não são reservadas: só aparecem em certos lugares.
    K_LET = "let", K_STATIC = "static", K_YIELD = "yield", K_AWAIT = "await", K_ASYNC = "async",
    K_OF = "of", K_GET = "get", K_SET = "set", K_ARGUMENTS = "arguments", K_EVAL = "eval",
    K_CONSTRUCTOR = "constructor", K_TARGET = "target", K_PROTOTYPE = "prototype", K_LENGTH = "length",
    K_NAME = "name", K_PROTO = "__proto__", K_THIS_VAR = "%this", K_FN_VAR = "%fn", K_NT_VAR = "%nt",
    K_HOME_VAR = "%home", K_VAZIO = "", K_DEFAULT_NOME = "default ",
}

/// A última reservada (`with`): de `K_LET` para cima, são nomes comuns.
pub fn reservada(a: Txt) -> bool {
    a <= K_WITH
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum P {
    ChaveA, ChaveF, ParenA, ParenF, ColA, ColF, Ponto, Reticencias, PontoVirgula, Virgula, Menor, Maior,
    MenorIgual, MaiorIgual, Igual2, Diferente, Igual3, Diferente3, Mais, Menos, Vezes, Divide, Resto, Pot,
    Mais2, Menos2, Shl, Shr, Ushr, E, Ou, Xor, Nao, Til, E2, Ou2, Nulish, Interrogacao, InterroPonto, DoisPontos,
    Atrib, MaisAtrib, MenosAtrib, VezesAtrib, DivideAtrib, RestoAtrib, PotAtrib, ShlAtrib, ShrAtrib, UshrAtrib,
    EAtrib, OuAtrib, XorAtrib, E2Atrib, Ou2Atrib, NulishAtrib, Seta, Privado,
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Tok {
    Fim,
    /// Um nome ou palavra-chave.
    Nome(Txt),
    /// Um nome privado (`#x`), com o `#` no átomo.
    Privado(Txt),
    Num(f64),
    Str(Txt),
    /// (cru, cozido — ou u32::MAX se o escape é inválido, ainda há mais
    /// pedaços?, é o primeiro pedaço?)
    Tpl(Txt, Txt, bool, bool),
    Regex(Txt, Txt),
    S(P),
}

#[derive(Clone, Copy)]
pub struct Ficha {
    pub tok: Tok,
    /// Havia uma quebra de linha antes (para a inserção de `;`).
    pub nl: bool,
    pub pos: u32,
    pub fim: u32,
}

pub struct Fichas {
    pub lista: Vec<Ficha>,
    /// Para cada `(`, onde está o `)` correspondente.
    pub par: Vec<u32>,
}

struct Lexico<'a> {
    b: &'a [u8],
    s: &'a str,
    i: usize,
}

fn inicio_nome(c: u8) -> bool {
    c.is_ascii_alphabetic() || c == b'_' || c == b'$' || c >= 0x80 || c == b'\\'
}

fn parte_nome(c: u8) -> bool {
    c.is_ascii_alphanumeric() || c == b'_' || c == b'$' || c >= 0x80 || c == b'\\'
}

fn valor_hexa(c: u8) -> Option<u32> {
    (c as char).to_digit(16)
}

/// Dígitos numa base, com `_` entre eles, para f64.
fn ler_base(t: &[u8], base: u32) -> f64 {
    let mut v = 0.0f64;
    for &c in t {
        if c != b'_' {
            v = v * base as f64 + (c as char).to_digit(base).unwrap_or(0) as f64;
        }
    }
    v
}

impl Lexico<'_> {
    fn pular_brancos(&mut self) -> Result<bool, String> {
        let mut nl = false;
        while self.i < self.b.len() {
            let c = self.b[self.i];
            match c {
                b' ' | b'\t' | 0x0B | 0x0C => self.i += 1,
                b'\n' | b'\r' => {
                    nl = true;
                    self.i += 1;
                }
                b'/' if self.b.get(self.i + 1) == Some(&b'/') => {
                    while self.i < self.b.len() && self.b[self.i] != b'\n' && self.b[self.i] != b'\r' {
                        self.i += 1;
                    }
                }
                b'/' if self.b.get(self.i + 1) == Some(&b'*') => {
                    let fim = self.s[self.i + 2..].find("*/").ok_or("comentário sem fim")?;
                    if self.s[self.i..self.i + 2 + fim].contains('\n') {
                        nl = true;
                    }
                    self.i += fim + 4;
                }
                // <!-- e --> no começo da linha: comentários antigos do HTML
                b'<' if self.s[self.i..].starts_with("<!--") => {
                    while self.i < self.b.len() && self.b[self.i] != b'\n' {
                        self.i += 1;
                    }
                }
                0xC2 if self.b.get(self.i + 1) == Some(&0xA0) => self.i += 2,
                0xEF if self.s[self.i..].starts_with('\u{FEFF}') => self.i += 3,
                0xE2 if self.s[self.i..].starts_with(['\u{2028}', '\u{2029}']) => {
                    nl = true;
                    self.i += 3;
                }
                _ => break,
            }
        }
        Ok(nl)
    }

    fn numero(&mut self) -> Result<f64, String> {
        let b = self.b;
        let ini = self.i;
        if b[ini] == b'0' && ini + 1 < b.len() {
            let base = match b[ini + 1] {
                b'x' | b'X' => 16,
                b'o' | b'O' => 8,
                b'b' | b'B' => 2,
                _ => 0,
            };
            if base != 0 {
                self.i += 2;
                let p = self.i;
                while self.i < b.len() && (b[self.i].is_ascii_alphanumeric() || b[self.i] == b'_') && b[self.i] != b'n' {
                    self.i += 1;
                }
                let v = ler_base(&b[p..self.i], base);
                if self.b.get(self.i) == Some(&b'n') {
                    self.i += 1;
                }
                return Ok(v);
            }
            // Octal antigo: 0777 (mas 089 é decimal)
            if b[ini + 1].is_ascii_digit() {
                let mut j = ini + 1;
                while j < b.len() && b[j].is_ascii_digit() {
                    j += 1;
                }
                if b[ini + 1..j].iter().all(|c| *c < b'8') {
                    self.i = j;
                    return Ok(ler_base(&b[ini + 1..j], 8));
                }
            }
        }
        while self.i < b.len() && (b[self.i].is_ascii_digit() || b[self.i] == b'_') {
            self.i += 1;
        }
        if self.b.get(self.i) == Some(&b'n') {
            let v = ler_base(&b[ini..self.i], 10);
            self.i += 1;
            return Ok(v);
        }
        if self.b.get(self.i) == Some(&b'.') {
            self.i += 1;
            while self.i < b.len() && (b[self.i].is_ascii_digit() || b[self.i] == b'_') {
                self.i += 1;
            }
        }
        if matches!(self.b.get(self.i), Some(b'e' | b'E')) {
            let mut j = self.i + 1;
            if matches!(b.get(j), Some(b'+' | b'-')) {
                j += 1;
            }
            if b.get(j).is_some_and(|c| c.is_ascii_digit()) {
                while j < b.len() && b[j].is_ascii_digit() {
                    j += 1;
                }
                self.i = j;
            }
        }
        let t: String = self.s[ini..self.i].chars().filter(|c| *c != '_').collect();
        t.parse::<f64>().map_err(|_| alloc::format!("número inválido: {}", t))
    }

    /// Um escape depois da `\`. `None` = sequência inválida (só importa em template).
    fn escape(&mut self, saida: &mut String) -> Option<()> {
        let c = *self.b.get(self.i)?;
        self.i += 1;
        match c {
            b'n' => saida.push('\n'),
            b't' => saida.push('\t'),
            b'r' => saida.push('\r'),
            b'b' => saida.push('\u{8}'),
            b'f' => saida.push('\u{C}'),
            b'v' => saida.push('\u{B}'),
            b'0'..=b'7' => {
                // \0 ou octal antigo
                let mut v = (c - b'0') as u32;
                let mut n = 1;
                while n < 3 {
                    match self.b.get(self.i) {
                        Some(d @ b'0'..=b'7') if v * 8 + ((*d - b'0') as u32) < 256 => {
                            v = v * 8 + (*d - b'0') as u32;
                            self.i += 1;
                            n += 1;
                        }
                        _ => break,
                    }
                }
                saida.push(char::from_u32(v)?);
            }
            b'x' => {
                let a = valor_hexa(*self.b.get(self.i)?)?;
                let b2 = valor_hexa(*self.b.get(self.i + 1)?)?;
                self.i += 2;
                saida.push(char::from_u32(a * 16 + b2)?);
            }
            b'u' => {
                let v = self.unicode()?;
                // Um par de substitutos escrito em dois escapes.
                if (0xD800..0xDC00).contains(&v) && self.s[self.i..].starts_with("\\u") {
                    let antes = self.i;
                    self.i += 2;
                    let baixo = self.unicode()?;
                    if (0xDC00..0xE000).contains(&baixo) {
                        saida.push(char::from_u32(0x10000 + ((v - 0xD800) << 10) + (baixo - 0xDC00))?);
                    } else {
                        self.i = antes;
                        saida.push('\u{FFFD}');
                    }
                } else {
                    saida.push(char::from_u32(v).unwrap_or('\u{FFFD}'));
                }
            }
            b'\r' => {
                if self.b.get(self.i) == Some(&b'\n') {
                    self.i += 1;
                }
            }
            b'\n' => {}
            c if c < 0x80 => saida.push(c as char),
            _ => {
                // uma letra não-ASCII depois da barra: ela mesma
                self.i -= 1;
                let ch = self.s[self.i..].chars().next()?;
                self.i += ch.len_utf8();
                if ch != '\u{2028}' && ch != '\u{2029}' {
                    saida.push(ch);
                }
            }
        }
        Some(())
    }

    fn unicode(&mut self) -> Option<u32> {
        if self.b.get(self.i) == Some(&b'{') {
            self.i += 1;
            let mut v = 0u32;
            loop {
                let c = *self.b.get(self.i)?;
                self.i += 1;
                if c == b'}' {
                    return Some(v);
                }
                v = v.checked_mul(16)?.checked_add(valor_hexa(c)?)?;
            }
        }
        let mut v = 0;
        for k in 0..4 {
            v = v * 16 + valor_hexa(*self.b.get(self.i + k)?)?;
        }
        self.i += 4;
        Some(v)
    }

    fn texto(&mut self, aspa: u8) -> Result<String, String> {
        self.i += 1;
        let mut saida = String::new();
        loop {
            // Copia de uma vez até a próxima coisa especial.
            let ini = self.i;
            while self.i < self.b.len() {
                let c = self.b[self.i];
                if c == aspa || c == b'\\' || c == b'\n' {
                    break;
                }
                self.i += 1;
            }
            saida.push_str(&self.s[ini..self.i]);
            match self.b.get(self.i) {
                Some(&c) if c == aspa => {
                    self.i += 1;
                    return Ok(saida);
                }
                Some(b'\\') => {
                    self.i += 1;
                    self.escape(&mut saida).ok_or("escape inválido num texto")?;
                }
                _ => return Err(String::from("texto sem fim")),
            }
        }
    }

    /// Um pedaço de template, depois do "`" ou do "}". Para em "`" ou em "${".
    fn pedaco_template(&mut self, n: &mut Interno) -> Result<Tok, String> {
        let ini = self.i;
        let mut cozido = Some(String::new());
        loop {
            let c = *self.b.get(self.i).ok_or("template sem fim")?;
            match c {
                b'`' | b'$' if c == b'`' || self.b.get(self.i + 1) == Some(&b'{') => {
                    let cru: String = self.s[ini..self.i].replace("\r\n", "\n").replace('\r', "\n");
                    let mais = c == b'$';
                    self.i += if mais { 2 } else { 1 };
                    let cozido = cozido.map_or(u32::MAX, |t| n.atomo(&t));
                    return Ok(Tok::Tpl(n.atomo(&cru), cozido, mais, false));
                }
                b'\\' => {
                    self.i += 1;
                    let mut pedaco = String::new();
                    if self.escape(&mut pedaco).is_some() {
                        if let Some(t) = cozido.as_mut() {
                            t.push_str(&pedaco);
                        }
                    } else {
                        cozido = None;
                    }
                }
                b'\r' => {
                    self.i += 1;
                    if self.b.get(self.i) == Some(&b'\n') {
                        self.i += 1;
                    }
                    if let Some(t) = cozido.as_mut() {
                        t.push('\n');
                    }
                }
                _ => {
                    let ch = self.s[self.i..].chars().next().unwrap_or(' ');
                    self.i += ch.len_utf8();
                    if let Some(t) = cozido.as_mut() {
                        t.push(ch);
                    }
                }
            }
        }
    }

    fn regex(&mut self, n: &mut Interno) -> Result<Tok, String> {
        self.i += 1;
        let ini = self.i;
        let mut classe = false;
        loop {
            let c = *self.b.get(self.i).ok_or("expressão regular sem fim")?;
            match c {
                b'\n' => return Err(String::from("expressão regular sem fim")),
                b'\\' => self.i += 1,
                b'[' => classe = true,
                b']' => classe = false,
                b'/' if !classe => break,
                _ => {}
            }
            self.i += 1;
        }
        let corpo = n.atomo(&self.s[ini..self.i]);
        self.i += 1;
        let ini_f = self.i;
        while self.i < self.b.len() && parte_nome(self.b[self.i]) {
            self.i += 1;
        }
        Ok(Tok::Regex(corpo, n.atomo(&self.s[ini_f..self.i])))
    }

    fn nome(&mut self) -> Result<String, String> {
        let ini = self.i;
        let mut escapado = false;
        while self.i < self.b.len() && parte_nome(self.b[self.i]) {
            if self.b[self.i] == b'\\' {
                escapado = true;
                self.i += 1;
            }
            if self.b[self.i] >= 0x80 {
                // U+2028/2029 e o espaço duro separam nomes
                let ch = self.s[self.i..].chars().next().unwrap_or(' ');
                if ch == '\u{2028}' || ch == '\u{2029}' || ch == '\u{A0}' || ch == '\u{FEFF}' {
                    break;
                }
                self.i += ch.len_utf8();
                continue;
            }
            self.i += 1;
        }
        if !escapado {
            return Ok(self.s[ini..self.i].to_string());
        }
        // Tem \uXXXX dentro.
        let fim = self.i;
        self.i = ini;
        let mut saida = String::new();
        while self.i < fim {
            if self.b[self.i] == b'\\' {
                self.i += 2;
                let v = self.unicode().ok_or("escape inválido num nome")?;
                saida.push(char::from_u32(v).ok_or("escape inválido num nome")?);
            } else {
                let ch = self.s[self.i..].chars().next().unwrap_or(' ');
                saida.push(ch);
                self.i += ch.len_utf8();
            }
        }
        Ok(saida)
    }

    fn simbolo(&mut self) -> Option<P> {
        use P::*;
        let r = &self.b[self.i..];
        let t: &[(&[u8], P)] = &[
            (b">>>=", UshrAtrib), (b"...", Reticencias), (b"===", Igual3), (b"!==", Diferente3), (b"**=", PotAtrib),
            (b"<<=", ShlAtrib), (b">>=", ShrAtrib), (b">>>", Ushr), (b"&&=", E2Atrib), (b"||=", Ou2Atrib),
            (b"??=", NulishAtrib), (b"=>", Seta), (b"==", Igual2), (b"!=", Diferente), (b"<=", MenorIgual),
            (b">=", MaiorIgual), (b"&&", E2), (b"||", Ou2), (b"??", Nulish), (b"++", Mais2), (b"--", Menos2),
            (b"+=", MaisAtrib), (b"-=", MenosAtrib), (b"*=", VezesAtrib), (b"/=", DivideAtrib), (b"%=", RestoAtrib),
            (b"&=", EAtrib), (b"|=", OuAtrib), (b"^=", XorAtrib), (b"<<", Shl), (b">>", Shr), (b"**", Pot),
            (b"{", ChaveA), (b"}", ChaveF), (b"(", ParenA), (b")", ParenF), (b"[", ColA), (b"]", ColF), (b".", Ponto),
            (b";", PontoVirgula), (b",", Virgula), (b"<", Menor), (b">", Maior), (b"+", Mais), (b"-", Menos),
            (b"*", Vezes), (b"/", Divide), (b"%", Resto), (b"&", E), (b"|", Ou), (b"^", Xor), (b"!", Nao), (b"~", Til),
            (b"?", Interrogacao), (b":", DoisPontos), (b"=", Atrib),
        ];
        if r.starts_with(b"?.") && !r.get(2).is_some_and(|c| c.is_ascii_digit()) {
            self.i += 2;
            return Some(InterroPonto);
        }
        for (texto, p) in t {
            if r.starts_with(texto) {
                self.i += texto.len();
                return Some(*p);
            }
        }
        None
    }
}

/// O erro vem com o número da linha.
pub fn ler_fichas(fonte: &str, n: &mut Interno) -> Result<Fichas, String> {
    let mut l = Lexico { b: fonte.as_bytes(), s: fonte, i: 0 };
    if fonte.starts_with("#!") {
        l.i = fonte.find('\n').unwrap_or(fonte.len());
    }
    let mut lista: Vec<Ficha> = Vec::with_capacity(fonte.len() / 5 + 8);
    let mut par: Vec<u32> = Vec::with_capacity(fonte.len() / 5 + 8);
    // Os parênteses abertos: (índice da ficha, vinha depois de if/while/for/with?)
    let mut parenteses: Vec<(u32, bool)> = Vec::new();
    // As chaves abertas: true = é o `${` de um template.
    let mut chaves: Vec<bool> = Vec::new();
    // O `)` anterior fechava um if/while/for?
    let mut paren_de_controle = false;
    let linha = |i: usize| fonte.as_bytes()[..i.min(fonte.len())].iter().filter(|c| **c == b'\n').count() + 1;
    let erro = |m: String, i: usize| alloc::format!("{} (linha {})", m, linha(i));
    loop {
        let nl = l.pular_brancos().map_err(|e| erro(e, l.i))?;
        let pos = l.i;
        if l.i >= l.b.len() {
            lista.push(Ficha { tok: Tok::Fim, nl, pos: pos as u32, fim: pos as u32 });
            par.push(0);
            break;
        }
        let c = l.b[l.i];
        let tok = if c.is_ascii_digit() || (c == b'.' && l.b.get(l.i + 1).is_some_and(|d| d.is_ascii_digit())) {
            Tok::Num(l.numero().map_err(|e| erro(e, pos))?)
        } else if inicio_nome(c) {
            let t = l.nome().map_err(|e| erro(e, pos))?;
            Tok::Nome(n.atomo(&t))
        } else if c == b'"' || c == b'\'' {
            let t = l.texto(c).map_err(|e| erro(e, pos))?;
            Tok::Str(n.atomo(&t))
        } else if c == b'`' {
            l.i += 1;
            let t = l.pedaco_template(n).map_err(|e| erro(e, pos))?;
            let Tok::Tpl(cru, cozido, mais, _) = t else { unreachable!() };
            if mais {
                chaves.push(true);
            }
            Tok::Tpl(cru, cozido, mais, true)
        } else if c == b'}' && chaves.last() == Some(&true) {
            chaves.pop();
            l.i += 1;
            let t = l.pedaco_template(n).map_err(|e| erro(e, pos))?;
            if matches!(t, Tok::Tpl(_, _, true, _)) {
                chaves.push(true);
            }
            t
        } else if c == b'#' {
            l.i += 1;
            let t = l.nome().map_err(|e| erro(e, pos))?;
            let mut nome = String::from("#");
            nome.push_str(&t);
            Tok::Privado(n.atomo(&nome))
        } else if c == b'/' {
            let regex_aqui = match lista.last().map(|f| f.tok) {
                None => true,
                Some(Tok::S(P::ParenF)) => paren_de_controle,
                Some(Tok::S(P::ColF | P::Mais2 | P::Menos2)) => false,
                Some(Tok::S(_)) => true,
                Some(Tok::Nome(a)) => {
                    reservada(a) && !matches!(a, K_THIS | K_SUPER | K_NULL | K_TRUE | K_FALSE) || a == K_YIELD || a == K_AWAIT || a == K_OF
                }
                Some(_) => false,
            };
            if regex_aqui {
                l.regex(n).map_err(|e| erro(e, pos))?
            } else {
                Tok::S(l.simbolo().ok_or_else(|| erro(String::from("símbolo inválido"), pos))?)
            }
        } else {
            match l.simbolo() {
                Some(s) => Tok::S(s),
                None => return Err(erro(alloc::format!("caractere inesperado {:?}", fonte[pos..].chars().next().unwrap_or(' ')), pos)),
            }
        };
        let indice = lista.len() as u32;
        par.push(0);
        match tok {
            Tok::S(P::ParenA) => {
                let controle = matches!(lista.last().map(|f| f.tok), Some(Tok::Nome(K_IF | K_WHILE | K_FOR | K_WITH)));
                parenteses.push((indice, controle));
            }
            Tok::S(P::ParenF) => {
                if let Some((abre, controle)) = parenteses.pop() {
                    par[abre as usize] = indice;
                    paren_de_controle = controle;
                }
            }
            Tok::S(P::ChaveA) => chaves.push(false),
            Tok::S(P::ChaveF) => {
                chaves.pop();
            }
            _ => {}
        }
        lista.push(Ficha { tok, nl, pos: pos as u32, fim: l.i as u32 });
    }
    Ok(Fichas { lista, par })
}
