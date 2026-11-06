//! A máquina virtual do JavaScript: valores, textos, objetos, propriedades,
//! conversões. O laço de execução fica em `interprete.rs`.
//!
//! Os valores são `Copy`: um objeto ou um texto é um índice numa arena. O
//! coletor de lixo (marcar e varrer) só roda em pontos seguros, onde tudo o
//! que está vivo é alcançável da pilha da máquina.

use super::compilador::Funcao;
use super::lexico::*;
use super::regex::Regex;
use crate::mat;
use alloc::boxed::Box;
use alloc::collections::BTreeMap;
use alloc::format;
use alloc::rc::Rc;
use alloc::string::{String, ToString};
use alloc::vec;
use alloc::vec::Vec;

pub type Txt = u32;
pub type ObjId = u32;
pub type Res = Result<Valor, Valor>;
pub type Nativa = fn(&mut Vm, Valor, &[Valor]) -> Res;
pub const NENHUM: u32 = u32::MAX;

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Valor {
    /// Não é um valor do JavaScript: variável não iniciada, buraco de array.
    Vazio,
    Undef,
    Nulo,
    Bool(bool),
    Int(i32),
    Num(f64),
    Texto(Txt),
    Simbolo(Txt),
    Obj(ObjId),
    /// Uma posição da pilha que guarda uma variável capturada.
    Caixa(u32),
}

impl Valor {
    pub fn nulo_ou_undef(self) -> bool {
        matches!(self, Valor::Undef | Valor::Nulo)
    }
    pub fn obj(self) -> Option<ObjId> {
        if let Valor::Obj(o) = self { Some(o) } else { None }
    }
}

/// Um número: como `Int` sempre que der (é muito mais rápido em soft-float).
pub fn num(n: f64) -> Valor {
    let i = n as i32;
    if i as f64 == n && !(i == 0 && n.is_sign_negative()) { Valor::Int(i) } else { Valor::Num(n) }
}

// -----------------------------------------------------------------------------
// Textos e átomos
// -----------------------------------------------------------------------------

enum Conteudo {
    Plano(Rc<str>),
    /// Uma concatenação ainda não feita (montar um texto aos pedaços fica linear).
    Corda(Txt, Txt),
    Livre,
}

const T_ATOMO: u8 = 1;
const T_SIMBOLO: u8 = 2;
const T_ASCII: u8 = 4;
const T_MARCADO: u8 = 8;

struct EntradaTexto {
    c: Conteudo,
    bytes: u32,
    flags: u8,
    /// Átomo: o índice de array que ele representa (ou NENHUM).
    /// Não-átomo: o átomo com o mesmo texto, se já se sabe (ou NENHUM).
    aux: u32,
}

/// A tabela de textos. Um átomo é um texto único: comparar é comparar números.
pub struct Interno {
    textos: Vec<EntradaTexto>,
    livres: Vec<u32>,
    tabela: Vec<u32>,
    n_atomos: usize,
    pub bytes_novos: usize,
    /// Alguém tentou um texto grande demais (o próximo ponto seguro lança o erro).
    pub excedeu: bool,
}

fn fnv(b: &[u8]) -> u32 {
    let mut h = 0x811C9DC5u32;
    for &c in b {
        h = (h ^ c as u32).wrapping_mul(0x01000193);
    }
    h
}

/// "123" → 123, mas "0123" e "4294967295" não são índices de array.
fn indice_de_texto(s: &str) -> u32 {
    let b = s.as_bytes();
    if b.is_empty() || b.len() > 10 || (b[0] == b'0' && b.len() > 1) {
        return NENHUM;
    }
    let mut v: u64 = 0;
    for &c in b {
        if !c.is_ascii_digit() {
            return NENHUM;
        }
        v = v * 10 + (c - b'0') as u64;
    }
    if v < u32::MAX as u64 { v as u32 } else { NENHUM }
}

impl Default for Interno {
    fn default() -> Self {
        Self::novo()
    }
}

impl Interno {
    pub fn novo() -> Interno {
        let mut n = Interno { textos: Vec::with_capacity(4096), livres: Vec::new(), tabela: vec![NENHUM; 8192], n_atomos: 0, bytes_novos: 0, excedeu: false };
        for p in PALAVRAS {
            n.atomo(p);
        }
        n
    }

    pub fn atomo(&mut self, s: &str) -> Txt {
        let mascara = self.tabela.len() - 1;
        let mut i = fnv(s.as_bytes()) as usize & mascara;
        loop {
            let t = self.tabela[i];
            if t == NENHUM {
                break;
            }
            if let Conteudo::Plano(p) = &self.textos[t as usize].c {
                if &**p == s {
                    return t;
                }
            }
            i = (i + 1) & mascara;
        }
        let id = self.alocar(EntradaTexto {
            c: Conteudo::Plano(Rc::from(s)),
            bytes: s.len() as u32,
            flags: T_ATOMO | if s.is_ascii() { T_ASCII } else { 0 },
            aux: indice_de_texto(s),
        });
        self.tabela[i] = id;
        self.n_atomos += 1;
        if self.n_atomos * 2 > self.tabela.len() {
            self.crescer_tabela();
        }
        id
    }

    fn crescer_tabela(&mut self) {
        let novo = self.tabela.len() * 2;
        let velha = core::mem::replace(&mut self.tabela, vec![NENHUM; novo]);
        let mascara = novo - 1;
        for t in velha {
            if t == NENHUM {
                continue;
            }
            let Conteudo::Plano(p) = &self.textos[t as usize].c else { continue };
            let mut i = fnv(p.as_bytes()) as usize & mascara;
            while self.tabela[i] != NENHUM {
                i = (i + 1) & mascara;
            }
            self.tabela[i] = t;
        }
    }

    fn alocar(&mut self, e: EntradaTexto) -> Txt {
        self.bytes_novos += e.bytes as usize + 48;
        if let Some(i) = self.livres.pop() {
            self.textos[i as usize] = e;
            i
        } else {
            self.textos.push(e);
            self.textos.len() as u32 - 1
        }
    }

    pub fn novo_texto(&mut self, s: &str) -> Txt {
        // Textos curtos viram átomos: chaves de objeto e comparações ficam rápidas.
        if s.len() <= 16 {
            return self.atomo(s);
        }
        self.alocar(EntradaTexto { c: Conteudo::Plano(Rc::from(s)), bytes: s.len() as u32, flags: if s.is_ascii() { T_ASCII } else { 0 }, aux: NENHUM })
    }

    pub fn novo_simbolo(&mut self, descricao: &str) -> Txt {
        self.alocar(EntradaTexto { c: Conteudo::Plano(Rc::from(descricao)), bytes: descricao.len() as u32, flags: T_SIMBOLO | T_ATOMO, aux: NENHUM })
    }

    pub fn e_simbolo(&self, t: Txt) -> bool {
        self.textos[t as usize].flags & T_SIMBOLO != 0
    }

    pub fn tamanho_bytes(&self, t: Txt) -> usize {
        self.textos[t as usize].bytes as usize
    }

    pub fn e_ascii(&self, t: Txt) -> bool {
        self.textos[t as usize].flags & T_ASCII != 0
    }

    /// `a + b`. Curtos são copiados; longos viram uma corda.
    pub fn concatenar(&mut self, a: Txt, b: Txt) -> Txt {
        let (ta, tb) = (self.tamanho_bytes(a), self.tamanho_bytes(b));
        if ta == 0 {
            return b;
        }
        if tb == 0 {
            return a;
        }
        if ta + tb > 1 << 28 {
            self.excedeu = true;
            return a;
        }
        if ta + tb < 128 {
            let (x, y) = (self.texto(a), self.texto(b));
            let mut s = String::with_capacity(ta + tb);
            s.push_str(&x);
            s.push_str(&y);
            return self.novo_texto(&s);
        }
        let ascii = self.e_ascii(a) && self.e_ascii(b);
        self.alocar(EntradaTexto { c: Conteudo::Corda(a, b), bytes: (ta + tb) as u32, flags: if ascii { T_ASCII } else { 0 }, aux: NENHUM })
    }

    /// O conteúdo (uma corda é achatada aqui, sem recursão: elas ficam fundas).
    pub fn texto(&mut self, t: Txt) -> Rc<str> {
        if let Conteudo::Plano(p) = &self.textos[t as usize].c {
            return p.clone();
        }
        let mut s = String::with_capacity(self.textos[t as usize].bytes as usize);
        let mut pilha = vec![t];
        while let Some(x) = pilha.pop() {
            match &self.textos[x as usize].c {
                Conteudo::Plano(p) => s.push_str(p),
                Conteudo::Corda(a, b) => {
                    pilha.push(*b);
                    pilha.push(*a);
                }
                Conteudo::Livre => {}
            }
        }
        let p: Rc<str> = Rc::from(s.as_str());
        self.textos[t as usize].c = Conteudo::Plano(p.clone());
        p
    }

    /// O átomo com o mesmo texto.
    pub fn atomo_de(&mut self, t: Txt) -> Txt {
        let e = &self.textos[t as usize];
        if e.flags & T_ATOMO != 0 {
            return t;
        }
        if e.aux != NENHUM {
            return e.aux;
        }
        let s = self.texto(t);
        let a = self.atomo(&s);
        self.textos[t as usize].aux = a;
        a
    }

    /// O índice de array que um átomo representa.
    #[inline]
    pub fn indice(&self, atomo: Txt) -> u32 {
        let e = &self.textos[atomo as usize];
        if e.flags & T_SIMBOLO != 0 { NENHUM } else { e.aux }
    }

    fn marcar(&mut self, t: Txt) {
        let mut pilha = vec![t];
        while let Some(x) = pilha.pop() {
            let e = &mut self.textos[x as usize];
            if e.flags & (T_MARCADO | T_ATOMO) != 0 {
                continue;
            }
            e.flags |= T_MARCADO;
            if let Conteudo::Corda(a, b) = e.c {
                pilha.push(a);
                pilha.push(b);
            }
        }
    }

    fn varrer(&mut self) {
        for i in 0..self.textos.len() {
            let e = &mut self.textos[i];
            if e.flags & T_ATOMO != 0 || matches!(e.c, Conteudo::Livre) {
                continue;
            }
            if e.flags & T_MARCADO != 0 {
                e.flags &= !T_MARCADO;
            } else {
                e.c = Conteudo::Livre;
                self.livres.push(i as u32);
            }
        }
        self.bytes_novos = 0;
    }
}

// -----------------------------------------------------------------------------
// Objetos
// -----------------------------------------------------------------------------

pub const P_ESCRITA: u8 = 1;
pub const P_ENUM: u8 = 2;
pub const P_CONFIG: u8 = 4;
pub const P_ACESSOR: u8 = 8;
pub const P_TUDO: u8 = 7;
/// Métodos dos objetos nativos: não aparecem nos laços.
pub const P_OCULTA: u8 = P_ESCRITA | P_CONFIG;

#[derive(Clone, Copy)]
pub struct Prop {
    pub chave: Txt,
    pub flags: u8,
    pub valor: Valor,
}

pub const O_EXTENSIVEL: u8 = 1;
pub const O_MARCADO: u8 = 2;
pub const O_ARGUMENTOS: u8 = 4;
/// Propriedades `name`, `length`, `prototype` já criadas (funções).
pub const O_MATERIAL: u8 = 8;
/// Array congelado (Object.freeze) ou selado.
pub const O_CONGELADO: u8 = 16;

pub struct Closure {
    pub func: Rc<Funcao>,
    pub ups: Rc<[u32]>,
    pub home: Valor,
    /// Os campos da classe: (chave, inicializador).
    pub campos: Vec<(Valor, Valor)>,
}

pub struct DadosNativa {
    pub f: Nativa,
    pub nome: Txt,
    pub comprimento: u8,
    pub construtor: bool,
    /// Valores que a função carrega consigo.
    pub dado: [Valor; 2],
}

pub struct Ligada {
    pub alvo: Valor,
    pub this: Valor,
    pub args: Vec<Valor>,
}

#[derive(Clone, Copy, PartialEq)]
pub enum EstadoGerador {
    Novo,
    Suspenso,
    SuspensoAwait,
    Rodando,
    Fim,
}

pub struct Gerador {
    pub estado: EstadoGerador,
    pub func: Rc<Funcao>,
    pub ups: Rc<[u32]>,
    pub callee: Valor,
    pub this: Valor,
    pub nt: Valor,
    pub args: Vec<Valor>,
    pub ip: usize,
    pub pilha: Vec<Valor>,
    pub handlers: Vec<(u32, u32)>,
    /// A promessa de uma função assíncrona.
    pub promessa: Valor,
    /// Gerador assíncrono: os pedidos (modo, valor, promessa) em fila.
    pub fila: Vec<(u8, Valor, Valor)>,
}

pub struct Reacao {
    pub ok: Valor,
    pub erro: Valor,
    /// A promessa que recebe o resultado (a do `then`).
    pub derivada: Valor,
}

pub struct Promessa {
    /// 0 pendente, 1 cumprida, 2 rejeitada
    pub estado: u8,
    pub valor: Valor,
    pub reacoes: Vec<Reacao>,
    pub tratada: bool,
}

/// Map e Set: a ordem é a de inserção; apagar deixa um buraco.
#[derive(Default)]
pub struct Colecao {
    pub itens: Vec<Option<(Valor, Valor)>>,
    pub indice: BTreeMap<(u8, u64), u32>,
    pub vivos: u32,
}

#[derive(Clone, Copy, PartialEq)]
pub enum TipoTipado {
    I8,
    U8,
    U8C,
    I16,
    U16,
    I32,
    U32,
    F32,
    F64,
}

pub enum Tipo {
    Comum,
    Array(Vec<Valor>),
    Funcao(Box<Closure>),
    Nativa(Box<DadosNativa>),
    Ligada(Box<Ligada>),
    Acessor(Valor, Valor),
    Erro,
    Booleano(bool),
    Numero(f64),
    Texto(Txt),
    Simbolo(Txt),
    Data(f64),
    Regex(Rc<Regex>),
    Mapa(Box<Colecao>),
    Conjunto(Box<Colecao>),
    Promessa(Box<Promessa>),
    Gerador(Box<Gerador>),
    /// tipo: 0 valores, 1 chaves, 2 pares
    IterArray { alvo: Valor, i: u32, tipo: u8 },
    IterColecao { alvo: Valor, i: u32, tipo: u8 },
    IterTexto { alvo: Txt, i: u32 },
    IterRegex { regex: Valor, texto: Txt, global: bool, fim: bool },
    Proxy(Valor, Valor),
    Buffer(Vec<u8>),
    Tipado { buffer: ObjId, tipo: TipoTipado, inicio: u32, n: u32 },
    /// Um objeto do hospedeiro: um nó do DOM etc. (`classe`, `id`)
    Hospede { classe: u32, id: u32 },
    Livre,
}

pub struct Objeto {
    pub proto: u32,
    pub flags: u8,
    pub props: Vec<Prop>,
    pub indice: Option<Box<Vec<u32>>>,
    pub tipo: Tipo,
}

const LIMIAR_INDICE: usize = 12;

fn hash_chave(k: Txt, mascara: usize) -> usize {
    (k.wrapping_mul(2654435761) >> 7) as usize & mascara
}

impl Objeto {
    #[inline]
    pub fn achar(&self, k: Txt) -> Option<usize> {
        if let Some(ind) = &self.indice {
            let m = ind.len() - 1;
            let mut i = hash_chave(k, m);
            loop {
                let e = ind[i];
                if e == 0 {
                    return None;
                }
                if self.props[e as usize - 1].chave == k {
                    return Some(e as usize - 1);
                }
                i = (i + 1) & m;
            }
        }
        self.props.iter().position(|p| p.chave == k)
    }

    pub fn inserir(&mut self, p: Prop) {
        self.props.push(p);
        let n = self.props.len();
        if self.indice.is_some() {
            if self.indice.as_ref().unwrap().len() < n * 2 {
                self.refazer_indice();
            } else {
                let ind = self.indice.as_mut().unwrap();
                let m = ind.len() - 1;
                let mut i = hash_chave(p.chave, m);
                while ind[i] != 0 {
                    i = (i + 1) & m;
                }
                ind[i] = n as u32;
            }
        } else if n > LIMIAR_INDICE {
            self.refazer_indice();
        }
    }

    fn refazer_indice(&mut self) {
        if self.props.len() <= LIMIAR_INDICE {
            self.indice = None;
            return;
        }
        let tam = (self.props.len() * 4).next_power_of_two();
        let m = tam - 1;
        let mut ind = vec![0u32; tam];
        for (n, p) in self.props.iter().enumerate() {
            let mut i = hash_chave(p.chave, m);
            while ind[i] != 0 {
                i = (i + 1) & m;
            }
            ind[i] = n as u32 + 1;
        }
        self.indice = Some(Box::new(ind));
    }

    pub fn remover(&mut self, i: usize) {
        self.props.remove(i);
        if self.indice.is_some() {
            self.refazer_indice();
        }
    }

    pub fn extensivel(&self) -> bool {
        self.flags & O_EXTENSIVEL != 0
    }
}

macro_rules! simbolos {
    ($($c:ident = $t:literal),* $(,)?) => {
        pub struct Simbolos { $(pub $c: Txt),* }
        impl Simbolos {
            fn novo(n: &mut Interno) -> Simbolos { Simbolos { $($c: n.novo_simbolo($t)),* } }
        }
    };
}

simbolos! {
    iterator = "Symbol.iterator", async_iterator = "Symbol.asyncIterator", has_instance = "Symbol.hasInstance",
    to_primitive = "Symbol.toPrimitive", to_string_tag = "Symbol.toStringTag", species = "Symbol.species",
    match_ = "Symbol.match", match_all = "Symbol.matchAll", replace = "Symbol.replace", search = "Symbol.search",
    split = "Symbol.split", is_concat_spreadable = "Symbol.isConcatSpreadable", unscopables = "Symbol.unscopables",
}

macro_rules! atomos {
    ($($c:ident = $t:literal),* $(,)?) => {
        pub struct Atomos { $(pub $c: Txt),* }
        impl Atomos {
            fn novo(n: &mut Interno) -> Atomos { Atomos { $($c: n.atomo($t)),* } }
        }
    };
}

atomos! {
    value = "value", done = "done", next = "next", return_ = "return", throw = "throw", then = "then",
    message = "message", stack = "stack", to_string = "toString", value_of = "valueOf", get = "get", set = "set",
    writable = "writable", enumerable = "enumerable", configurable = "configurable", index = "index", input = "input",
    groups = "groups", last_index = "lastIndex", undefined = "undefined", null = "null", true_ = "true", false_ = "false",
    object = "object", function = "function", number = "number", string = "string", boolean = "boolean", symbol = "symbol",
    has = "has", delete_ = "deleteProperty", own_keys = "ownKeys", apply = "apply", construct = "construct",
    to_json = "toJSON", raw = "raw", cause = "cause", errors = "errors", status = "status", reason = "reason",
    get_own = "getOwnPropertyDescriptor", define_prop = "defineProperty", get_proto = "getPrototypeOf",
    set_proto = "setPrototypeOf", join = "join", callee = "callee", exec = "exec", flags = "flags", source = "source",
}

/// Os protótipos dos objetos embutidos.
#[derive(Default, Clone, Copy)]
pub struct Protos {
    pub objeto: ObjId,
    pub funcao: ObjId,
    pub array: ObjId,
    pub texto: ObjId,
    pub numero: ObjId,
    pub booleano: ObjId,
    pub simbolo: ObjId,
    pub erro: ObjId,
    pub erro_tipo: ObjId,
    pub erro_ref: ObjId,
    pub erro_sintaxe: ObjId,
    pub erro_intervalo: ObjId,
    pub erro_uri: ObjId,
    pub erro_eval: ObjId,
    pub erro_agregado: ObjId,
    pub iterador: ObjId,
    pub iter_array: ObjId,
    pub iter_colecao: ObjId,
    pub iter_texto: ObjId,
    pub iter_regex: ObjId,
    pub gerador: ObjId,
    pub gerador_assinc: ObjId,
    pub func_gerador: ObjId,
    pub func_assinc: ObjId,
    pub promessa: ObjId,
    pub regex: ObjId,
    pub data: ObjId,
    pub mapa: ObjId,
    pub conjunto: ObjId,
    pub buffer: ObjId,
    pub tipado: [ObjId; 9],
}

pub enum Sinal {
    Nada,
    Yield,
    Await,
}

pub struct Quadro {
    pub func: Rc<Funcao>,
    pub ups: Rc<[u32]>,
    pub ip: usize,
    pub base: usize,
    pub callee: Valor,
    pub this: Valor,
    pub nt: Valor,
    pub args: Vec<Valor>,
    pub handlers: Vec<(u32, u32)>,
    pub construindo: bool,
    pub gerador: u32,
}

/// O que o hospedeiro (o navegador) pluga na máquina.
#[derive(Clone, Copy)]
pub struct Ganchos {
    pub console: fn(&mut Vm, u8, &str),
    pub obter: fn(&mut Vm, ObjId, u32, u32, Txt) -> Option<Valor>,
    pub definir: fn(&mut Vm, ObjId, u32, u32, Txt, Valor) -> bool,
    pub apagar: fn(&mut Vm, ObjId, u32, u32, Txt) -> bool,
    pub chaves: fn(&mut Vm, ObjId, u32, u32) -> Vec<Txt>,
    pub agora_ms: fn() -> u64,
    /// Os valores que o hospedeiro guarda fora da máquina (raízes do coletor).
    pub raizes: fn(&mut Vm) -> Vec<Valor>,
}

fn console_padrao(_: &mut Vm, _: u8, _: &str) {}

fn agora_padrao() -> u64 {
    0
}

pub struct Vm {
    pub n: Interno,
    pub objs: Vec<Objeto>,
    pub obj_livres: Vec<u32>,
    pub caixas: Vec<Valor>,
    pub caixa_marcas: Vec<bool>,
    pub caixa_livres: Vec<u32>,
    pub pilha: Vec<Valor>,
    pub quadros: Vec<Quadro>,
    pub global: ObjId,
    /// `let` e `const` do nível de fora dos scripts.
    pub lexico: ObjId,
    pub p: Protos,
    pub s: Simbolos,
    pub a: Atomos,
    pub novo_alvo: Valor,
    pub nativa_atual: ObjId,
    pub prof_nativa: u32,
    pub sinal: Sinal,
    pub microtarefas: alloc::collections::VecDeque<Tarefa>,
    pub alocados: usize,
    pub limite_gc: usize,
    pub vivos_gc: usize,
    pub raizes: Vec<Valor>,
    pub registro_simbolos: BTreeMap<Txt, Txt>,
    pub ganchos: Ganchos,
    /// Para o hospedeiro guardar o estado dele.
    pub hospede: *mut u8,
    /// Passos até olhar o relógio; o prazo; abortando = a execução foi cortada.
    pub orcamento: i32,
    pub prazo_ms: u64,
    pub abortando: bool,
    pub rejeicoes: Vec<Valor>,
    pub semente: u64,
    pub array_iter_original: Valor,
    pub array_iter_next: Valor,
}

/// Uma microtarefa: reação de promessa, ou `queueMicrotask`.
pub enum Tarefa {
    Reacao { manipulador: Valor, valor: Valor, derivada: Valor, rejeitada: bool },
    AdotarThenable { promessa: Valor, thenable: Valor, then: Valor },
    Chamar(Valor),
}

/// Número → texto, como o JavaScript escreve.
pub fn numero_para_texto(n: f64) -> String {
    if n.is_nan() {
        return String::from("NaN");
    }
    if n == 0.0 {
        return String::from("0");
    }
    if n.is_infinite() {
        return String::from(if n > 0.0 { "Infinity" } else { "-Infinity" });
    }
    let i = n as i64;
    if i as f64 == n && mat::abs(n) < 1e15 {
        return i.to_string();
    }
    let t = format!("{:e}", n); // "-1.2345e-7": os dígitos mais curtos
    let (mant, exp) = t.split_once('e').unwrap_or((&t, "0"));
    let exp: i32 = exp.parse().unwrap_or(0);
    let (sinal, mant) = match mant.strip_prefix('-') {
        Some(m) => ("-", m),
        None => ("", mant),
    };
    let digitos: String = mant.chars().filter(|c| *c != '.').collect();
    let k = digitos.len() as i32;
    let n10 = exp + 1;
    let corpo = if k <= n10 && n10 <= 21 {
        let mut s = digitos.clone();
        for _ in 0..(n10 - k) {
            s.push('0');
        }
        s
    } else if 0 < n10 && n10 <= 21 {
        format!("{}.{}", &digitos[..n10 as usize], &digitos[n10 as usize..])
    } else if -6 < n10 && n10 <= 0 {
        let mut s = String::from("0.");
        for _ in 0..(-n10) {
            s.push('0');
        }
        s.push_str(&digitos);
        s
    } else {
        let e = n10 - 1;
        let sinal_e = if e < 0 { '-' } else { '+' };
        if k == 1 {
            format!("{}e{}{}", digitos, sinal_e, e.abs())
        } else {
            format!("{}.{}e{}{}", &digitos[..1], &digitos[1..], sinal_e, e.abs())
        }
    };
    format!("{}{}", sinal, corpo)
}

/// Texto → número (a conversão do `Number("...")`).
pub fn texto_para_numero(s: &str) -> f64 {
    let t = s.trim_matches(|c: char| c.is_whitespace() || c == '\u{FEFF}');
    if t.is_empty() {
        return 0.0;
    }
    let b = t.as_bytes();
    if b.len() > 2 && b[0] == b'0' {
        let base = match b[1] {
            b'x' | b'X' => 16,
            b'o' | b'O' => 8,
            b'b' | b'B' => 2,
            _ => 0,
        };
        if base != 0 {
            let mut v = 0.0;
            for &c in &b[2..] {
                match (c as char).to_digit(base) {
                    Some(d) => v = v * base as f64 + d as f64,
                    None => return f64::NAN,
                }
            }
            return v;
        }
    }
    match t {
        "Infinity" | "+Infinity" => return f64::INFINITY,
        "-Infinity" => return f64::NEG_INFINITY,
        _ => {}
    }
    // O parse do Rust aceita "inf", "nan"...; o JavaScript não.
    if !b.iter().all(|c| c.is_ascii_digit() || matches!(c, b'.' | b'e' | b'E' | b'+' | b'-')) {
        return f64::NAN;
    }
    t.parse::<f64>().unwrap_or(f64::NAN)
}

pub fn para_i32(n: f64) -> i32 {
    if !n.is_finite() {
        return 0;
    }
    if mat::abs(n) < 2147483648.0 {
        return n as i32;
    }
    let m = mat::fmod(mat::trunc(n), 4294967296.0);
    let m = if m < 0.0 { m + 4294967296.0 } else { m };
    m as u32 as i32
}

impl Vm {
    pub fn novo() -> Box<Vm> {
        let mut n = Interno::novo();
        let s = Simbolos::novo(&mut n);
        let a = Atomos::novo(&mut n);
        let mut vm = Box::new(Vm {
            n,
            objs: Vec::with_capacity(2048),
            obj_livres: Vec::new(),
            caixas: Vec::new(),
            caixa_marcas: Vec::new(),
            caixa_livres: Vec::new(),
            pilha: Vec::with_capacity(4096),
            quadros: Vec::with_capacity(64),
            global: 0,
            lexico: 0,
            p: Protos::default(),
            s,
            a,
            novo_alvo: Valor::Undef,
            nativa_atual: 0,
            prof_nativa: 0,
            sinal: Sinal::Nada,
            microtarefas: Default::default(),
            alocados: 0,
            limite_gc: 300_000,
            vivos_gc: 0,
            raizes: Vec::new(),
            registro_simbolos: BTreeMap::new(),
            ganchos: Ganchos {
                console: console_padrao,
                obter: |_, _, _, _, _| None,
                definir: |_, _, _, _, _, _| false,
                apagar: |_, _, _, _, _| false,
                chaves: |_, _, _, _| Vec::new(),
                agora_ms: agora_padrao,
                raizes: |_| Vec::new(),
            },
            hospede: core::ptr::null_mut(),
            orcamento: 20_000,
            prazo_ms: 0,
            abortando: false,
            rejeicoes: Vec::new(),
            semente: 0x2545F4914F6CDD1D,
            array_iter_original: Valor::Undef,
            array_iter_next: Valor::Undef,
        });
        super::nativos::instalar(&mut vm);
        vm
    }

    // ------------------------------------------------------------ alocação

    pub fn novo_obj_com(&mut self, proto: u32, tipo: Tipo) -> ObjId {
        let o = Objeto { proto, flags: O_EXTENSIVEL, props: Vec::new(), indice: None, tipo };
        self.alocados += 1;
        if let Some(i) = self.obj_livres.pop() {
            self.objs[i as usize] = o;
            i
        } else {
            self.objs.push(o);
            self.objs.len() as u32 - 1
        }
    }

    pub fn novo_objeto(&mut self) -> ObjId {
        self.novo_obj_com(self.p.objeto, Tipo::Comum)
    }

    pub fn novo_array(&mut self, v: Vec<Valor>) -> Valor {
        self.alocados += v.len() / 8;
        Valor::Obj(self.novo_obj_com(self.p.array, Tipo::Array(v)))
    }

    pub fn nova_caixa(&mut self, v: Valor) -> u32 {
        self.alocados += 1;
        if let Some(i) = self.caixa_livres.pop() {
            self.caixas[i as usize] = v;
            i
        } else {
            self.caixas.push(v);
            self.caixa_marcas.push(false);
            self.caixas.len() as u32 - 1
        }
    }

    pub fn txt(&mut self, s: &str) -> Valor {
        Valor::Texto(self.n.novo_texto(s))
    }

    pub fn atomo(&mut self, s: &str) -> Txt {
        self.n.atomo(s)
    }

    pub fn nova_nativa(&mut self, nome: &str, comprimento: u8, f: Nativa) -> ObjId {
        let nome = self.n.atomo(nome);
        self.nativa_com(nome, comprimento, f, [Valor::Undef; 2], false)
    }

    pub fn nativa_com(&mut self, nome: Txt, comprimento: u8, f: Nativa, dado: [Valor; 2], construtor: bool) -> ObjId {
        self.novo_obj_com(self.p.funcao, Tipo::Nativa(Box::new(DadosNativa { f, nome, comprimento, construtor, dado })))
    }

    pub fn dado_nativa(&self, i: usize) -> Valor {
        match &self.objs[self.nativa_atual as usize].tipo {
            Tipo::Nativa(d) => d.dado[i],
            _ => Valor::Undef,
        }
    }

    /// Define um método nativo (não enumerável).
    pub fn def(&mut self, obj: ObjId, nome: &str, comprimento: u8, f: Nativa) -> ObjId {
        let id = self.nova_nativa(nome, comprimento, f);
        let k = self.n.atomo(nome);
        self.def_valor(obj, k, Valor::Obj(id), P_OCULTA);
        id
    }

    pub fn def_valor(&mut self, obj: ObjId, k: Txt, v: Valor, flags: u8) {
        let o = &mut self.objs[obj as usize];
        match o.achar(k) {
            Some(i) => {
                o.props[i].valor = v;
                o.props[i].flags = flags;
            }
            None => o.inserir(Prop { chave: k, flags, valor: v }),
        }
    }

    pub fn def_texto(&mut self, obj: ObjId, nome: &str, v: Valor) {
        let k = self.n.atomo(nome);
        self.def_valor(obj, k, v, P_TUDO);
    }

    pub fn def_oculta(&mut self, obj: ObjId, nome: &str, v: Valor) {
        let k = self.n.atomo(nome);
        self.def_valor(obj, k, v, P_OCULTA);
    }

    pub fn def_acessor(&mut self, obj: ObjId, nome: &str, get: Option<Nativa>, set: Option<Nativa>) {
        let g = get.map_or(Valor::Undef, |f| Valor::Obj(self.nova_nativa(nome, 0, f)));
        let s = set.map_or(Valor::Undef, |f| Valor::Obj(self.nova_nativa(nome, 1, f)));
        let k = self.n.atomo(nome);
        self.def_par(obj, k, g, s, P_CONFIG);
    }

    pub fn def_par(&mut self, obj: ObjId, k: Txt, get: Valor, set: Valor, flags: u8) {
        let par = self.novo_obj_com(NENHUM, Tipo::Acessor(get, set));
        self.def_valor(obj, k, Valor::Obj(par), (flags & !P_ESCRITA) | P_ACESSOR);
    }

    // ------------------------------------------------------------ erros

    pub fn novo_erro(&mut self, proto: ObjId, msg: &str) -> Valor {
        let id = self.novo_obj_com(proto, Tipo::Erro);
        let m = self.txt(msg);
        let k = self.a.message;
        self.def_valor(id, k, m, P_OCULTA);
        self.preencher_pilha(id);
        Valor::Obj(id)
    }

    /// `erro.stack`: o nome e a mensagem, mais o nome das funções na pilha.
    pub fn preencher_pilha(&mut self, id: ObjId) {
        let mut s = self.descrever_erro(Valor::Obj(id));
        let nomes: Vec<Txt> = self.quadros.iter().rev().take(10).map(|q| q.func.nome).collect();
        for nome in nomes {
            let t = self.n.texto(nome);
            s.push_str("\n    at ");
            s.push_str(if t.is_empty() { "<anonymous>" } else { &t });
        }
        let t = self.txt(&s);
        let k = self.a.stack;
        self.def_valor(id, k, t, P_OCULTA);
    }

    pub fn erro_tipo(&mut self, msg: &str) -> Valor {
        self.novo_erro(self.p.erro_tipo, msg)
    }

    pub fn erro_ref(&mut self, msg: &str) -> Valor {
        self.novo_erro(self.p.erro_ref, msg)
    }

    pub fn erro_intervalo(&mut self, msg: &str) -> Valor {
        self.novo_erro(self.p.erro_intervalo, msg)
    }

    pub fn erro_sintaxe(&mut self, msg: &str) -> Valor {
        self.novo_erro(self.p.erro_sintaxe, msg)
    }

    /// "TypeError: x" sem chamar código da página.
    pub fn descrever_erro(&mut self, v: Valor) -> String {
        match v {
            Valor::Obj(id) => {
                let nome = self.prop_simples(id, K_NAME);
                let msg = self.prop_simples(id, self.a.message);
                let nome = match nome {
                    Some(Valor::Texto(t)) => self.n.texto(t).to_string(),
                    _ => String::from("Error"),
                };
                match msg {
                    Some(Valor::Texto(t)) => {
                        let m = self.n.texto(t);
                        if m.is_empty() { nome } else { format!("{}: {}", nome, m) }
                    }
                    _ => nome,
                }
            }
            outro => self.texto_de_primitivo(outro),
        }
    }

    /// Uma propriedade de dados (na cadeia), sem rodar getters.
    pub fn prop_simples(&self, id: ObjId, k: Txt) -> Option<Valor> {
        let mut cur = id;
        while cur != NENHUM {
            let o = &self.objs[cur as usize];
            if let Some(i) = o.achar(k) {
                let p = &o.props[i];
                return if p.flags & P_ACESSOR != 0 { None } else { Some(p.valor) };
            }
            cur = o.proto;
        }
        None
    }

    pub fn texto_de_primitivo(&mut self, v: Valor) -> String {
        match v {
            Valor::Vazio | Valor::Undef => String::from("undefined"),
            Valor::Nulo => String::from("null"),
            Valor::Bool(true) => String::from("true"),
            Valor::Bool(false) => String::from("false"),
            Valor::Int(i) => i.to_string(),
            Valor::Num(n) => numero_para_texto(n),
            Valor::Texto(t) => self.n.texto(t).to_string(),
            Valor::Simbolo(t) => format!("Symbol({})", self.n.texto(t)),
            Valor::Obj(_) | Valor::Caixa(_) => String::from("[object]"),
        }
    }

    // ------------------------------------------------------------ conversões

    pub fn para_bool(&mut self, v: Valor) -> bool {
        match v {
            Valor::Vazio | Valor::Undef | Valor::Nulo => false,
            Valor::Bool(b) => b,
            Valor::Int(i) => i != 0,
            Valor::Num(n) => n != 0.0 && !n.is_nan(),
            Valor::Texto(t) => self.n.tamanho_bytes(t) != 0,
            _ => true,
        }
    }

    pub fn para_primitivo(&mut self, v: Valor, dica_texto: Option<bool>) -> Res {
        let Valor::Obj(id) = v else { return Ok(v) };
        let chave = self.s.to_primitive;
        let exotico = self.obter_obj(id, chave, v)?;
        if !exotico.nulo_ou_undef() {
            let d = match dica_texto {
                None => "default",
                Some(true) => "string",
                Some(false) => "number",
            };
            let d = self.txt(d);
            let r = self.chamar(exotico, v, &[d])?;
            if r.obj().is_none() {
                return Ok(r);
            }
            return Err(self.erro_tipo("Cannot convert object to primitive value"));
        }
        let ordem = if dica_texto == Some(true) { [self.a.to_string, self.a.value_of] } else { [self.a.value_of, self.a.to_string] };
        for k in ordem {
            let f = self.obter_obj(id, k, v)?;
            if self.e_funcao(f) {
                let r = self.chamar(f, v, &[])?;
                if r.obj().is_none() {
                    return Ok(r);
                }
            }
        }
        Err(self.erro_tipo("Cannot convert object to primitive value"))
    }

    pub fn para_numero(&mut self, v: Valor) -> Result<f64, Valor> {
        Ok(match v {
            Valor::Int(i) => i as f64,
            Valor::Num(n) => n,
            Valor::Undef | Valor::Vazio => f64::NAN,
            Valor::Nulo | Valor::Bool(false) => 0.0,
            Valor::Bool(true) => 1.0,
            Valor::Texto(t) => {
                let s = self.n.texto(t);
                texto_para_numero(&s)
            }
            Valor::Simbolo(_) => return Err(self.erro_tipo("Cannot convert a Symbol value to a number")),
            Valor::Obj(_) => {
                let p = self.para_primitivo(v, Some(false))?;
                return self.para_numero(p);
            }
            Valor::Caixa(_) => f64::NAN,
        })
    }

    pub fn para_inteiro(&mut self, v: Valor) -> Result<f64, Valor> {
        let n = self.para_numero(v)?;
        Ok(if n.is_nan() { 0.0 } else { mat::trunc(n) })
    }

    pub fn para_i32(&mut self, v: Valor) -> Result<i32, Valor> {
        if let Valor::Int(i) = v {
            return Ok(i);
        }
        Ok(para_i32(self.para_numero(v)?))
    }

    pub fn para_texto_id(&mut self, v: Valor) -> Result<Txt, Valor> {
        Ok(match v {
            Valor::Texto(t) => t,
            Valor::Simbolo(_) => return Err(self.erro_tipo("Cannot convert a Symbol value to a string")),
            Valor::Obj(_) => {
                let p = self.para_primitivo(v, Some(true))?;
                return self.para_texto_id(p);
            }
            outro => {
                let s = self.texto_de_primitivo(outro);
                self.n.novo_texto(&s)
            }
        })
    }

    pub fn para_texto(&mut self, v: Valor) -> Result<Rc<str>, Valor> {
        let t = self.para_texto_id(v)?;
        Ok(self.n.texto(t))
    }

    pub fn para_objeto(&mut self, v: Valor) -> Result<ObjId, Valor> {
        Ok(match v {
            Valor::Obj(o) => o,
            Valor::Texto(t) => self.novo_obj_com(self.p.texto, Tipo::Texto(t)),
            Valor::Int(i) => self.novo_obj_com(self.p.numero, Tipo::Numero(i as f64)),
            Valor::Num(n) => self.novo_obj_com(self.p.numero, Tipo::Numero(n)),
            Valor::Bool(b) => self.novo_obj_com(self.p.booleano, Tipo::Booleano(b)),
            Valor::Simbolo(s) => self.novo_obj_com(self.p.simbolo, Tipo::Simbolo(s)),
            _ => return Err(self.erro_tipo("Cannot convert undefined or null to object")),
        })
    }

    /// Um valor como chave de propriedade (um átomo ou um símbolo).
    pub fn para_chave(&mut self, v: Valor) -> Result<Txt, Valor> {
        Ok(match v {
            Valor::Texto(t) => self.n.atomo_de(t),
            Valor::Simbolo(s) => s,
            Valor::Int(i) => {
                let mut buf = [0u8; 12];
                let s = escrever_int(i, &mut buf);
                self.n.atomo(s)
            }
            Valor::Obj(_) => {
                let p = self.para_primitivo(v, Some(true))?;
                return self.para_chave(p);
            }
            outro => {
                let s = self.texto_de_primitivo(outro);
                self.n.atomo(&s)
            }
        })
    }

    /// A chave de propriedade como valor.
    pub fn valor_da_chave(&self, k: Txt) -> Valor {
        if self.n.e_simbolo(k) { Valor::Simbolo(k) } else { Valor::Texto(k) }
    }

    pub fn e_funcao(&self, v: Valor) -> bool {
        match v {
            Valor::Obj(o) => match &self.objs[o as usize].tipo {
                Tipo::Funcao(_) | Tipo::Nativa(_) | Tipo::Ligada(_) => true,
                Tipo::Proxy(a, _) => self.e_funcao(*a),
                _ => false,
            },
            _ => false,
        }
    }

    pub fn e_construtor(&self, v: Valor) -> bool {
        match v {
            Valor::Obj(o) => match &self.objs[o as usize].tipo {
                Tipo::Funcao(c) => !c.func.seta && !c.func.metodo && !c.func.gerador && !c.func.assincrona,
                Tipo::Nativa(d) => d.construtor,
                Tipo::Ligada(l) => self.e_construtor(l.alvo),
                Tipo::Proxy(a, _) => self.e_construtor(*a),
                _ => false,
            },
            _ => false,
        }
    }

    pub fn e_array(&self, v: Valor) -> bool {
        match v {
            Valor::Obj(o) => {
                let ob = &self.objs[o as usize];
                match &ob.tipo {
                    Tipo::Array(_) => ob.flags & O_ARGUMENTOS == 0,
                    Tipo::Proxy(a, _) => self.e_array(*a),
                    _ => false,
                }
            }
            _ => false,
        }
    }

    pub fn tipo_de(&mut self, v: Valor) -> Txt {
        match v {
            Valor::Vazio | Valor::Undef | Valor::Caixa(_) => self.a.undefined,
            Valor::Nulo => self.a.object,
            Valor::Bool(_) => self.a.boolean,
            Valor::Int(_) | Valor::Num(_) => self.a.number,
            Valor::Texto(_) => self.a.string,
            Valor::Simbolo(_) => self.a.symbol,
            Valor::Obj(_) => {
                if self.e_funcao(v) { self.a.function } else { self.a.object }
            }
        }
    }

    // ------------------------------------------------------------ propriedades

    /// `name`, `length` e `prototype` das funções só existem quando alguém olha.
    pub fn materializar(&mut self, id: ObjId) {
        let o = &mut self.objs[id as usize];
        if o.flags & O_MATERIAL != 0 {
            return;
        }
        o.flags |= O_MATERIAL;
        let (nome, comp, proto) = match &o.tipo {
            Tipo::Funcao(c) => {
                let f = &c.func;
                let tem_proto = !f.seta && !f.metodo && (!f.assincrona || f.gerador);
                (f.nome, f.comprimento as i32, if tem_proto { 1 + f.gerador as u8 + (f.assincrona as u8) } else { 0 })
            }
            Tipo::Nativa(d) => (d.nome, d.comprimento as i32, 0),
            _ => return,
        };
        // As que o programa já definiu (um `static name`) têm prioridade.
        if self.objs[id as usize].achar(K_NAME).is_none() {
            self.def_valor(id, K_NAME, Valor::Texto(nome), P_CONFIG);
        }
        if self.objs[id as usize].achar(K_LENGTH).is_none() {
            self.def_valor(id, K_LENGTH, Valor::Int(comp), P_CONFIG);
        }
        if proto != 0 && self.objs[id as usize].achar(K_PROTOTYPE).is_none() {
            let base = match proto {
                1 => self.p.objeto,
                2 => self.p.gerador,
                _ => self.p.gerador_assinc,
            };
            let po = self.novo_obj_com(base, Tipo::Comum);
            if proto == 1 {
                self.def_valor(po, K_CONSTRUCTOR, Valor::Obj(id), P_OCULTA);
            }
            self.def_valor(id, K_PROTOTYPE, Valor::Obj(po), P_ESCRITA);
        }
    }

    fn funcao_nao_material(&self, id: ObjId) -> bool {
        let o = &self.objs[id as usize];
        o.flags & O_MATERIAL == 0 && matches!(o.tipo, Tipo::Funcao(_) | Tipo::Nativa(_))
    }

    /// `o[k]` com `k` já átomo/símbolo.
    pub fn obter_obj(&mut self, id: ObjId, k: Txt, receptor: Valor) -> Res {
        let mut cur = id;
        let mut passos = 0;
        while cur != NENHUM {
            passos += 1;
            if passos > 4096 {
                return Err(self.erro_intervalo("prototype chain too long"));
            }
            if matches!(k, K_NAME | K_LENGTH | K_PROTOTYPE) && self.funcao_nao_material(cur) {
                self.materializar(cur);
            }
            let o = &self.objs[cur as usize];
            if let Some(i) = o.achar(k) {
                let p = o.props[i];
                if p.flags & P_ACESSOR != 0 {
                    return self.chamar_getter(p.valor, receptor);
                }
                return Ok(p.valor);
            }
            match &o.tipo {
                Tipo::Comum | Tipo::Funcao(_) | Tipo::Nativa(_) => {}
                Tipo::Array(v) => {
                    if k == K_LENGTH {
                        return Ok(num(v.len() as f64));
                    }
                    let i = self.n.indice(k);
                    if i != NENHUM {
                        if let Some(&x) = v.get(i as usize) {
                            if x != Valor::Vazio {
                                return Ok(x);
                            }
                        }
                    }
                }
                Tipo::Texto(t) => {
                    let t = *t;
                    if k == K_LENGTH {
                        return Ok(Valor::Int(self.comprimento_texto(t) as i32));
                    }
                    let i = self.n.indice(k);
                    if i != NENHUM {
                        if let Some(c) = self.char_em(t, i as usize) {
                            return Ok(c);
                        }
                    }
                }
                Tipo::Tipado { n, .. } => {
                    let n = *n;
                    let i = self.n.indice(k);
                    if i != NENHUM {
                        return Ok(self.tipado_ler(cur, i as usize));
                    }
                    if k == K_LENGTH {
                        return Ok(num(n as f64));
                    }
                }
                Tipo::Proxy(alvo, man) => {
                    let (alvo, man) = (*alvo, *man);
                    return self.proxy_obter(alvo, man, k, receptor);
                }
                Tipo::Hospede { classe, id: hid } => {
                    let (c, h) = (*classe, *hid);
                    if let Some(v) = (self.ganchos.obter)(self, cur, c, h, k) {
                        return Ok(v);
                    }
                }
                _ => {}
            }
            cur = self.objs[cur as usize].proto;
        }
        Ok(Valor::Undef)
    }

    fn chamar_getter(&mut self, par: Valor, receptor: Valor) -> Res {
        let Valor::Obj(pid) = par else { return Ok(Valor::Undef) };
        if let Tipo::Acessor(g, _) = self.objs[pid as usize].tipo {
            if g == Valor::Undef {
                return Ok(Valor::Undef);
            }
            return self.chamar(g, receptor, &[]);
        }
        Ok(Valor::Undef)
    }

    fn proxy_obter(&mut self, alvo: Valor, man: Valor, k: Txt, receptor: Valor) -> Res {
        let armadilha = self.obter(man, self.a.get)?;
        if armadilha.nulo_ou_undef() {
            return match alvo {
                Valor::Obj(a) => self.obter_obj(a, k, receptor),
                _ => Ok(Valor::Undef),
            };
        }
        let chave = self.valor_da_chave(k);
        self.chamar(armadilha, man, &[alvo, chave, receptor])
    }

    /// `base.k`
    pub fn obter(&mut self, base: Valor, k: Txt) -> Res {
        match base {
            Valor::Obj(id) => self.obter_obj(id, k, base),
            Valor::Texto(t) => {
                if k == K_LENGTH {
                    return Ok(Valor::Int(self.comprimento_texto(t) as i32));
                }
                let i = self.n.indice(k);
                if i != NENHUM {
                    return Ok(self.char_em(t, i as usize).unwrap_or(Valor::Undef));
                }
                self.obter_obj(self.p.texto, k, base)
            }
            Valor::Int(_) | Valor::Num(_) => self.obter_obj(self.p.numero, k, base),
            Valor::Bool(_) => self.obter_obj(self.p.booleano, k, base),
            Valor::Simbolo(_) => self.obter_obj(self.p.simbolo, k, base),
            _ => {
                let nome = self.n.texto(k);
                let tipo = if base == Valor::Nulo { "null" } else { "undefined" };
                Err(self.erro_tipo(&format!("Cannot read properties of {} (reading '{}')", tipo, nome)))
            }
        }
    }

    pub fn obter_nome(&mut self, base: Valor, nome: &str) -> Res {
        let k = self.n.atomo(nome);
        self.obter(base, k)
    }

    /// `base[chave]`, com caminhos rápidos para arrays e textos.
    pub fn obter_elem(&mut self, base: Valor, chave: Valor) -> Res {
        let i = match chave {
            Valor::Int(i) if i >= 0 => i as usize,
            Valor::Num(n) if n >= 0.0 && n < 4294967295.0 && mat::trunc(n) == n => n as usize,
            _ => usize::MAX,
        };
        if i != usize::MAX {
            match base {
                Valor::Obj(id) => match &self.objs[id as usize].tipo {
                    Tipo::Array(v) => {
                        if let Some(&x) = v.get(i) {
                            if x != Valor::Vazio {
                                return Ok(x);
                            }
                        }
                        let proto = self.objs[id as usize].proto;
                        if proto == self.p.array || proto == self.p.objeto {
                            // Sem índices nos protótipos (o caso de sempre).
                            return Ok(Valor::Undef);
                        }
                    }
                    Tipo::Tipado { .. } => return Ok(self.tipado_ler(id, i)),
                    _ => {}
                },
                Valor::Texto(t) => return Ok(self.char_em(t, i).unwrap_or(Valor::Undef)),
                _ => {}
            }
        }
        if base.nulo_ou_undef() {
            let s = match chave {
                Valor::Obj(_) => String::from("object"),
                outro => self.texto_de_primitivo(outro),
            };
            let tipo = if base == Valor::Nulo { "null" } else { "undefined" };
            return Err(self.erro_tipo(&format!("Cannot read properties of {} (reading '{}')", tipo, s)));
        }
        let k = self.para_chave(chave)?;
        self.obter(base, k)
    }

    pub fn definir_nome(&mut self, base: Valor, nome: &str, v: Valor) -> Result<(), Valor> {
        let k = self.n.atomo(nome);
        self.definir(base, k, v)
    }

    /// `base.k = v`
    pub fn definir(&mut self, base: Valor, k: Txt, v: Valor) -> Result<(), Valor> {
        let id = match base {
            Valor::Obj(o) => o,
            Valor::Undef | Valor::Nulo | Valor::Vazio => {
                let nome = self.n.texto(k);
                let tipo = if base == Valor::Nulo { "null" } else { "undefined" };
                return Err(self.erro_tipo(&format!("Cannot set properties of {} (setting '{}')", tipo, nome)));
            }
            _ => return Ok(()), // primitivos: a escrita some
        };
        self.definir_obj(id, k, v, base)
    }

    pub fn definir_obj(&mut self, id: ObjId, k: Txt, v: Valor, receptor: Valor) -> Result<(), Valor> {
        if matches!(k, K_NAME | K_LENGTH | K_PROTOTYPE) && self.funcao_nao_material(id) {
            self.materializar(id);
        }
        // A própria: o caso comum.
        let o = &mut self.objs[id as usize];
        if let Some(i) = o.achar(k) {
            let p = &mut o.props[i];
            if p.flags & P_ACESSOR != 0 {
                let par = p.valor;
                return self.chamar_setter(par, receptor, v);
            }
            if p.flags & P_ESCRITA != 0 {
                p.valor = v;
            }
            return Ok(());
        }
        // Os exóticos.
        match &mut o.tipo {
            Tipo::Array(_) => {
                if k == K_LENGTH {
                    return self.mudar_tamanho_array(id, v);
                }
                let i = self.n.indice(k);
                if i != NENHUM && self.definir_indice(id, i as usize, v) {
                    return Ok(());
                }
            }
            Tipo::Tipado { .. } => {
                let i = self.n.indice(k);
                if i != NENHUM {
                    let n = self.para_numero(v)?;
                    self.tipado_escrever(id, i as usize, n);
                    return Ok(());
                }
            }
            Tipo::Proxy(alvo, man) => {
                let (alvo, man) = (*alvo, *man);
                let armadilha = self.obter(man, self.a.set)?;
                if armadilha.nulo_ou_undef() {
                    return self.definir(alvo, k, v);
                }
                let chave = self.valor_da_chave(k);
                self.chamar(armadilha, man, &[alvo, chave, v, receptor])?;
                return Ok(());
            }
            Tipo::Hospede { classe, id: hid } => {
                let (c, h) = (*classe, *hid);
                if (self.ganchos.definir)(self, id, c, h, k, v) {
                    return Ok(());
                }
            }
            _ => {}
        }
        // Na cadeia: um setter, ou uma propriedade só de leitura.
        let mut cur = self.objs[id as usize].proto;
        while cur != NENHUM {
            let o = &self.objs[cur as usize];
            if let Some(i) = o.achar(k) {
                let p = o.props[i];
                if p.flags & P_ACESSOR != 0 {
                    return self.chamar_setter(p.valor, receptor, v);
                }
                if p.flags & P_ESCRITA == 0 {
                    return Ok(());
                }
                break;
            }
            if let Tipo::Proxy(alvo, _) = o.tipo {
                return self.definir(alvo, k, v);
            }
            cur = o.proto;
        }
        let o = &mut self.objs[id as usize];
        if o.extensivel() {
            o.inserir(Prop { chave: k, flags: P_TUDO, valor: v });
        }
        Ok(())
    }

    fn chamar_setter(&mut self, par: Valor, receptor: Valor, v: Valor) -> Result<(), Valor> {
        let Valor::Obj(pid) = par else { return Ok(()) };
        if let Tipo::Acessor(_, s) = self.objs[pid as usize].tipo {
            if s != Valor::Undef {
                self.chamar(s, receptor, &[v])?;
            }
        }
        Ok(())
    }

    fn mudar_tamanho_array(&mut self, id: ObjId, v: Valor) -> Result<(), Valor> {
        let n = self.para_numero(v)?;
        if n < 0.0 || mat::trunc(n) != n || n > 4294967295.0 {
            return Err(self.erro_intervalo("Invalid array length"));
        }
        if n > 8_000_000.0 {
            return Err(self.erro_intervalo("Array too large"));
        }
        if self.objs[id as usize].flags & O_CONGELADO != 0 {
            return Ok(());
        }
        if let Tipo::Array(a) = &mut self.objs[id as usize].tipo {
            a.resize(n as usize, Valor::Vazio);
        }
        Ok(())
    }

    /// Escreve `arr[i]`. Devolve false se o índice é longe demais: vira uma
    /// propriedade comum (um array "esparso").
    fn definir_indice(&mut self, id: ObjId, i: usize, v: Valor) -> bool {
        let o = &mut self.objs[id as usize];
        if o.flags & O_CONGELADO != 0 {
            return true;
        }
        let ext = o.flags & O_EXTENSIVEL != 0;
        let Tipo::Array(a) = &mut o.tipo else { return false };
        if i < a.len() {
            a[i] = v;
            return true;
        }
        if !ext {
            return true;
        }
        if i > a.len() + (1 << 20) {
            return false;
        }
        if i > a.len() {
            a.resize(i, Valor::Vazio);
        }
        a.push(v);
        true
    }

    /// `base[chave] = v`
    pub fn definir_elem(&mut self, base: Valor, chave: Valor, v: Valor) -> Result<(), Valor> {
        if let Valor::Obj(id) = base {
            let i = match chave {
                Valor::Int(i) if i >= 0 => i as usize,
                Valor::Num(n) if n >= 0.0 && n < 4294967295.0 && mat::trunc(n) == n => n as usize,
                _ => usize::MAX,
            };
            if i != usize::MAX {
                match &self.objs[id as usize].tipo {
                    Tipo::Array(_) => {
                        if self.definir_indice(id, i, v) {
                            return Ok(());
                        }
                    }
                    Tipo::Tipado { .. } => {
                        let n = self.para_numero(v)?;
                        self.tipado_escrever(id, i, n);
                        return Ok(());
                    }
                    _ => {}
                }
            }
        }
        if base.nulo_ou_undef() {
            let s = self.texto_de_primitivo(chave);
            let tipo = if base == Valor::Nulo { "null" } else { "undefined" };
            return Err(self.erro_tipo(&format!("Cannot set properties of {} (setting '{}')", tipo, s)));
        }
        let k = self.para_chave(chave)?;
        self.definir(base, k, v)
    }

    /// Cria uma propriedade (não passa por setters).
    pub fn definir_propria(&mut self, id: ObjId, k: Txt, v: Valor, flags: u8) {
        if let Tipo::Array(_) = self.objs[id as usize].tipo {
            let i = self.n.indice(k);
            if i != NENHUM && self.definir_indice(id, i as usize, v) {
                return;
            }
        }
        self.def_valor(id, k, v, flags);
    }

    pub fn tem_propria(&mut self, id: ObjId, k: Txt) -> bool {
        if matches!(k, K_NAME | K_LENGTH | K_PROTOTYPE) && self.funcao_nao_material(id) {
            self.materializar(id);
        }
        let o = &self.objs[id as usize];
        if o.achar(k).is_some() {
            return true;
        }
        match &o.tipo {
            Tipo::Array(v) => {
                if k == K_LENGTH {
                    return true;
                }
                let i = self.n.indice(k);
                i != NENHUM && v.get(i as usize).is_some_and(|x| *x != Valor::Vazio)
            }
            Tipo::Texto(t) => {
                let t = *t;
                if k == K_LENGTH {
                    return true;
                }
                let i = self.n.indice(k);
                i != NENHUM && (i as usize) < self.comprimento_texto(t)
            }
            Tipo::Tipado { n, .. } => {
                let n = *n;
                let i = self.n.indice(k);
                i != NENHUM && i < n
            }
            Tipo::Hospede { classe, id: h } => {
                let (c, h) = (*classe, *h);
                (self.ganchos.obter)(self, id, c, h, k).is_some()
            }
            _ => false,
        }
    }

    pub fn tem_prop(&mut self, id: ObjId, k: Txt) -> Result<bool, Valor> {
        let mut cur = id;
        while cur != NENHUM {
            if let Tipo::Proxy(alvo, man) = self.objs[cur as usize].tipo {
                let armadilha = self.obter(man, self.a.has)?;
                if armadilha.nulo_ou_undef() {
                    let Valor::Obj(a) = alvo else { return Ok(false) };
                    return self.tem_prop(a, k);
                }
                let chave = self.valor_da_chave(k);
                let r = self.chamar(armadilha, man, &[alvo, chave])?;
                return Ok(self.para_bool(r));
            }
            if self.tem_propria(cur, k) {
                return Ok(true);
            }
            cur = self.objs[cur as usize].proto;
        }
        Ok(false)
    }

    pub fn apagar(&mut self, id: ObjId, k: Txt) -> Result<bool, Valor> {
        if matches!(k, K_NAME | K_LENGTH | K_PROTOTYPE) && self.funcao_nao_material(id) {
            self.materializar(id);
        }
        if let Tipo::Proxy(alvo, man) = self.objs[id as usize].tipo {
            let armadilha = self.obter(man, self.a.delete_)?;
            if armadilha.nulo_ou_undef() {
                let Valor::Obj(a) = alvo else { return Ok(true) };
                return self.apagar(a, k);
            }
            let chave = self.valor_da_chave(k);
            let r = self.chamar(armadilha, man, &[alvo, chave])?;
            return Ok(self.para_bool(r));
        }
        if let Tipo::Hospede { classe, id: h } = self.objs[id as usize].tipo {
            if (self.ganchos.apagar)(self, id, classe, h, k) {
                return Ok(true);
            }
        }
        let i = self.n.indice(k);
        let o = &mut self.objs[id as usize];
        if o.flags & O_CONGELADO == 0 {
            if let Tipo::Array(v) = &mut o.tipo {
                if i != NENHUM && (i as usize) < v.len() {
                    v[i as usize] = Valor::Vazio;
                    return Ok(true);
                }
            }
        }
        if let Some(p) = o.achar(k) {
            if o.props[p].flags & P_CONFIG == 0 {
                return Ok(false);
            }
            o.remover(p);
        }
        Ok(true)
    }

    /// As chaves próprias na ordem da especificação: índices, textos, símbolos.
    pub fn chaves_proprias(&mut self, id: ObjId, so_enumeraveis: bool, com_textos: bool, com_simbolos: bool) -> Result<Vec<Txt>, Valor> {
        if let Tipo::Proxy(alvo, man) = self.objs[id as usize].tipo {
            let armadilha = self.obter(man, self.a.own_keys)?;
            if !armadilha.nulo_ou_undef() {
                let r = self.chamar(armadilha, man, &[alvo])?;
                let itens = self.lista_de_array(r)?;
                let mut v = Vec::new();
                for i in itens {
                    let k = self.para_chave(i)?;
                    if if self.n.e_simbolo(k) { com_simbolos } else { com_textos } {
                        v.push(k);
                    }
                }
                return Ok(v);
            }
            let Valor::Obj(a) = alvo else { return Ok(Vec::new()) };
            return self.chaves_proprias(a, so_enumeraveis, com_textos, com_simbolos);
        }
        if self.funcao_nao_material(id) {
            self.materializar(id);
        }
        let mut saida = Vec::new();
        let mut numericos: Vec<(u32, Txt)> = Vec::new();
        let extra: Option<Vec<usize>> = match &self.objs[id as usize].tipo {
            Tipo::Array(v) => Some(v.iter().enumerate().filter(|(_, x)| **x != Valor::Vazio).map(|(i, _)| i).collect()),
            Tipo::Texto(t) => {
                let t = *t;
                Some((0..self.comprimento_texto(t)).collect())
            }
            Tipo::Tipado { n, .. } => Some((0..*n as usize).collect()),
            _ => None,
        };
        if com_textos {
            if let Some(ids) = extra {
                let mut buf = [0u8; 12];
                for i in ids {
                    let k = self.n.atomo(escrever_int(i as i32, &mut buf));
                    saida.push(k);
                }
            }
            if let Tipo::Hospede { classe, id: h } = self.objs[id as usize].tipo {
                let mais = (self.ganchos.chaves)(self, id, classe, h);
                saida.extend(mais);
            }
        }
        let mut textos = Vec::new();
        let mut simbolos = Vec::new();
        let props = core::mem::take(&mut self.objs[id as usize].props);
        for p in &props {
            if so_enumeraveis && p.flags & P_ENUM == 0 {
                continue;
            }
            if self.n.e_simbolo(p.chave) {
                simbolos.push(p.chave);
            } else {
                let i = self.n.indice(p.chave);
                if i != NENHUM {
                    numericos.push((i, p.chave));
                } else if !self.e_privada(p.chave) {
                    textos.push(p.chave);
                }
            }
        }
        self.objs[id as usize].props = props;
        if com_textos {
            numericos.sort_unstable();
            saida.extend(numericos.iter().map(|x| x.1));
            if !so_enumeraveis && matches!(self.objs[id as usize].tipo, Tipo::Array(_) | Tipo::Texto(_)) {
                saida.push(K_LENGTH);
            }
            saida.extend(textos);
        }
        if com_simbolos {
            saida.extend(simbolos);
        }
        Ok(saida)
    }

    /// Os nomes privados `#x` nunca aparecem nas listas de chaves.
    fn e_privada(&mut self, k: Txt) -> bool {
        self.n.tamanho_bytes(k) > 1 && self.n.texto(k).starts_with('#')
    }

    /// Um array (ou parecido) para um Vec.
    pub fn lista_de_array(&mut self, v: Valor) -> Result<Vec<Valor>, Valor> {
        if let Valor::Obj(id) = v {
            if let Tipo::Array(a) = &self.objs[id as usize].tipo {
                return Ok(a.iter().map(|x| if *x == Valor::Vazio { Valor::Undef } else { *x }).collect());
            }
        } else if v.nulo_ou_undef() {
            return Err(self.erro_tipo("CreateListFromArrayLike called on non-object"));
        }
        let n = self.obter(v, K_LENGTH)?;
        let n = self.para_inteiro(n)?.max(0.0) as usize;
        let mut saida = Vec::with_capacity(n.min(1 << 16));
        for i in 0..n {
            saida.push(self.obter_elem(v, Valor::Int(i as i32))?);
        }
        Ok(saida)
    }

    // ------------------------------------------------------------ textos em UTF-16

    /// O `length` do JavaScript: unidades de UTF-16.
    pub fn comprimento_texto(&mut self, t: Txt) -> usize {
        if self.n.e_ascii(t) {
            return self.n.tamanho_bytes(t);
        }
        let s = self.n.texto(t);
        s.chars().map(char::len_utf16).sum()
    }

    /// `texto[i]`: um texto de uma unidade.
    pub fn char_em(&mut self, t: Txt, i: usize) -> Option<Valor> {
        if self.n.e_ascii(t) {
            if i >= self.n.tamanho_bytes(t) {
                return None;
            }
            let s = self.n.texto(t);
            let mut buf = [0u8; 4];
            let c = s.as_bytes()[i] as char;
            let a = self.n.atomo(c.encode_utf8(&mut buf));
            return Some(Valor::Texto(a));
        }
        let s = self.n.texto(t);
        let mut pos = 0;
        for c in s.chars() {
            let n = c.len_utf16();
            if i < pos + n {
                let mut buf = [0u8; 4];
                let e = if n == 2 { "\u{FFFD}" } else { c.encode_utf8(&mut buf) };
                let a = self.n.atomo(e);
                return Some(Valor::Texto(a));
            }
            pos += n;
        }
        None
    }

    // ------------------------------------------------------------ arrays tipados

    pub fn tipado_ler(&self, id: ObjId, i: usize) -> Valor {
        let Tipo::Tipado { buffer, tipo, inicio, n } = self.objs[id as usize].tipo else { return Valor::Undef };
        if i >= n as usize {
            return Valor::Undef;
        }
        let Tipo::Buffer(b) = &self.objs[buffer as usize].tipo else { return Valor::Undef };
        let tam = tamanho_tipado(tipo);
        let p = inicio as usize + i * tam;
        if p + tam > b.len() {
            return Valor::Undef;
        }
        let d = &b[p..p + tam];
        match tipo {
            TipoTipado::I8 => Valor::Int(d[0] as i8 as i32),
            TipoTipado::U8 | TipoTipado::U8C => Valor::Int(d[0] as i32),
            TipoTipado::I16 => Valor::Int(i16::from_le_bytes([d[0], d[1]]) as i32),
            TipoTipado::U16 => Valor::Int(u16::from_le_bytes([d[0], d[1]]) as i32),
            TipoTipado::I32 => Valor::Int(i32::from_le_bytes([d[0], d[1], d[2], d[3]])),
            TipoTipado::U32 => num(u32::from_le_bytes([d[0], d[1], d[2], d[3]]) as f64),
            TipoTipado::F32 => num(f32::from_le_bytes([d[0], d[1], d[2], d[3]]) as f64),
            TipoTipado::F64 => num(f64::from_le_bytes([d[0], d[1], d[2], d[3], d[4], d[5], d[6], d[7]])),
        }
    }

    pub fn tipado_escrever(&mut self, id: ObjId, i: usize, x: f64) {
        let Tipo::Tipado { buffer, tipo, inicio, n } = self.objs[id as usize].tipo else { return };
        if i >= n as usize {
            return;
        }
        let Tipo::Buffer(b) = &mut self.objs[buffer as usize].tipo else { return };
        let tam = tamanho_tipado(tipo);
        let p = inicio as usize + i * tam;
        if p + tam > b.len() {
            return;
        }
        let d = &mut b[p..p + tam];
        let int = para_i32(x);
        match tipo {
            TipoTipado::I8 | TipoTipado::U8 => d[0] = int as u8,
            TipoTipado::U8C => d[0] = if x.is_nan() { 0 } else { mat::round(x).clamp(0.0, 255.0) as u8 },
            TipoTipado::I16 | TipoTipado::U16 => d.copy_from_slice(&(int as u16).to_le_bytes()),
            TipoTipado::I32 | TipoTipado::U32 => d.copy_from_slice(&(int as u32).to_le_bytes()),
            TipoTipado::F32 => d.copy_from_slice(&(x as f32).to_le_bytes()),
            TipoTipado::F64 => d.copy_from_slice(&x.to_le_bytes()),
        }
    }

    // ------------------------------------------------------------ coletor de lixo

    pub fn coletar_lixo(&mut self) {
        let mut pend: Vec<ObjId> = Vec::with_capacity(1024);
        let pilha = core::mem::take(&mut self.pilha);
        for v in &pilha {
            self.marcar_valor(*v, &mut pend);
        }
        self.pilha = pilha;
        let quadros = core::mem::take(&mut self.quadros);
        for q in &quadros {
            self.marcar_valor(q.callee, &mut pend);
            self.marcar_valor(q.this, &mut pend);
            self.marcar_valor(q.nt, &mut pend);
            for a in &q.args {
                self.marcar_valor(*a, &mut pend);
            }
            for c in q.ups.iter() {
                self.marcar_caixa(*c, &mut pend);
            }
            if q.gerador != NENHUM {
                self.marcar_valor(Valor::Obj(q.gerador), &mut pend);
            }
        }
        self.quadros = quadros;
        let raizes = core::mem::take(&mut self.raizes);
        for v in &raizes {
            self.marcar_valor(*v, &mut pend);
        }
        self.raizes = raizes;
        let do_hospede = (self.ganchos.raizes)(self);
        for v in do_hospede {
            self.marcar_valor(v, &mut pend);
        }
        let rej = core::mem::take(&mut self.rejeicoes);
        for v in &rej {
            self.marcar_valor(*v, &mut pend);
        }
        self.rejeicoes = rej;
        let tarefas = core::mem::take(&mut self.microtarefas);
        for t in &tarefas {
            match t {
                Tarefa::Reacao { manipulador, valor, derivada, .. } => {
                    self.marcar_valor(*manipulador, &mut pend);
                    self.marcar_valor(*valor, &mut pend);
                    self.marcar_valor(*derivada, &mut pend);
                }
                Tarefa::AdotarThenable { promessa, thenable, then } => {
                    self.marcar_valor(*promessa, &mut pend);
                    self.marcar_valor(*thenable, &mut pend);
                    self.marcar_valor(*then, &mut pend);
                }
                Tarefa::Chamar(f) => self.marcar_valor(*f, &mut pend),
            }
        }
        self.microtarefas = tarefas;
        for v in [Valor::Obj(self.global), Valor::Obj(self.lexico), self.novo_alvo, self.array_iter_original, self.array_iter_next] {
            self.marcar_valor(v, &mut pend);
        }
        let p = self.p;
        for o in [
            p.objeto, p.funcao, p.array, p.texto, p.numero, p.booleano, p.simbolo, p.erro, p.erro_tipo, p.erro_ref, p.erro_sintaxe, p.erro_intervalo,
            p.erro_uri, p.erro_eval, p.erro_agregado, p.iterador, p.iter_array, p.iter_colecao, p.iter_texto, p.iter_regex, p.gerador, p.gerador_assinc,
            p.func_gerador, p.func_assinc, p.promessa, p.regex, p.data, p.mapa, p.conjunto, p.buffer,
        ] {
            self.marcar_valor(Valor::Obj(o), &mut pend);
        }
        for o in p.tipado {
            self.marcar_valor(Valor::Obj(o), &mut pend);
        }
        self.marcar_valor(Valor::Obj(self.nativa_atual), &mut pend);
        while let Some(id) = pend.pop() {
            self.marcar_filhos(id, &mut pend);
        }
        // A varredura.
        let mut vivos = 0;
        for i in 0..self.objs.len() {
            let o = &mut self.objs[i];
            if matches!(o.tipo, Tipo::Livre) {
                continue;
            }
            if o.flags & O_MARCADO != 0 {
                o.flags &= !O_MARCADO;
                vivos += 1;
            } else {
                *o = Objeto { proto: NENHUM, flags: 0, props: Vec::new(), indice: None, tipo: Tipo::Livre };
                self.obj_livres.push(i as u32);
            }
        }
        for i in 0..self.caixas.len() {
            if self.caixa_marcas[i] {
                self.caixa_marcas[i] = false;
                vivos += 1;
            } else if self.caixas[i] != Valor::Caixa(NENHUM) {
                self.caixas[i] = Valor::Caixa(NENHUM);
                self.caixa_livres.push(i as u32);
            }
        }
        self.n.varrer();
        self.alocados = 0;
        self.vivos_gc = vivos;
        self.limite_gc = (vivos / 2).max(300_000);
    }

    fn marcar_caixa(&mut self, c: u32, pend: &mut Vec<ObjId>) {
        if c as usize >= self.caixas.len() || self.caixa_marcas[c as usize] {
            return;
        }
        self.caixa_marcas[c as usize] = true;
        let v = self.caixas[c as usize];
        self.marcar_valor(v, pend);
    }

    pub fn marcar_valor(&mut self, v: Valor, pend: &mut Vec<ObjId>) {
        match v {
            Valor::Texto(t) | Valor::Simbolo(t) => self.n.marcar(t),
            Valor::Obj(o) => {
                let ob = &mut self.objs[o as usize];
                if ob.flags & O_MARCADO == 0 {
                    ob.flags |= O_MARCADO;
                    pend.push(o);
                }
            }
            Valor::Caixa(c) => self.marcar_caixa(c, pend),
            _ => {}
        }
    }

    fn marcar_filhos(&mut self, id: ObjId, pend: &mut Vec<ObjId>) {
        let proto = self.objs[id as usize].proto;
        if proto != NENHUM {
            self.marcar_valor(Valor::Obj(proto), pend);
        }
        let tipo = core::mem::replace(&mut self.objs[id as usize].tipo, Tipo::Comum);
        let props = core::mem::take(&mut self.objs[id as usize].props);
        for p in &props {
            self.n.marcar(p.chave);
            self.marcar_valor(p.valor, pend);
        }
        self.objs[id as usize].props = props;
        match &tipo {
            Tipo::Array(v) => {
                for x in v {
                    self.marcar_valor(*x, pend);
                }
            }
            Tipo::Funcao(c) => {
                for u in c.ups.iter() {
                    self.marcar_caixa(*u, pend);
                }
                self.marcar_valor(c.home, pend);
                for (k, f) in &c.campos {
                    self.marcar_valor(*k, pend);
                    self.marcar_valor(*f, pend);
                }
            }
            Tipo::Nativa(d) => {
                self.marcar_valor(d.dado[0], pend);
                self.marcar_valor(d.dado[1], pend);
            }
            Tipo::Ligada(l) => {
                self.marcar_valor(l.alvo, pend);
                self.marcar_valor(l.this, pend);
                for a in &l.args {
                    self.marcar_valor(*a, pend);
                }
            }
            Tipo::Acessor(g, s) | Tipo::Proxy(g, s) => {
                self.marcar_valor(*g, pend);
                self.marcar_valor(*s, pend);
            }
            Tipo::Texto(t) | Tipo::Simbolo(t) => self.n.marcar(*t),
            Tipo::Mapa(c) | Tipo::Conjunto(c) => {
                for (k, v) in c.itens.iter().flatten() {
                    self.marcar_valor(*k, pend);
                    self.marcar_valor(*v, pend);
                }
            }
            Tipo::Promessa(p) => {
                self.marcar_valor(p.valor, pend);
                for r in &p.reacoes {
                    self.marcar_valor(r.ok, pend);
                    self.marcar_valor(r.erro, pend);
                    self.marcar_valor(r.derivada, pend);
                }
            }
            Tipo::Gerador(g) => {
                for v in [g.callee, g.this, g.nt, g.promessa] {
                    self.marcar_valor(v, pend);
                }
                for a in g.args.iter().chain(g.pilha.iter()) {
                    self.marcar_valor(*a, pend);
                }
                for c in g.ups.iter() {
                    self.marcar_caixa(*c, pend);
                }
                for (_, v, p) in &g.fila {
                    self.marcar_valor(*v, pend);
                    self.marcar_valor(*p, pend);
                }
            }
            Tipo::IterArray { alvo, .. } | Tipo::IterColecao { alvo, .. } => self.marcar_valor(*alvo, pend),
            Tipo::IterTexto { alvo, .. } => self.n.marcar(*alvo),
            Tipo::IterRegex { regex, texto, .. } => {
                self.marcar_valor(*regex, pend);
                self.n.marcar(*texto);
            }
            Tipo::Tipado { buffer, .. } => self.marcar_valor(Valor::Obj(*buffer), pend),
            _ => {}
        }
        self.objs[id as usize].tipo = tipo;
    }
}

pub fn tamanho_tipado(t: TipoTipado) -> usize {
    match t {
        TipoTipado::I8 | TipoTipado::U8 | TipoTipado::U8C => 1,
        TipoTipado::I16 | TipoTipado::U16 => 2,
        TipoTipado::I32 | TipoTipado::U32 | TipoTipado::F32 => 4,
        TipoTipado::F64 => 8,
    }
}

/// Escreve um inteiro sem alocar (a conversão mais comum de todas).
pub fn escrever_int(i: i32, buf: &mut [u8; 12]) -> &str {
    let mut p = buf.len();
    let negativo = i < 0;
    let mut n = (i as i64).unsigned_abs();
    loop {
        p -= 1;
        buf[p] = b'0' + (n % 10) as u8;
        n /= 10;
        if n == 0 {
            break;
        }
    }
    if negativo {
        p -= 1;
        buf[p] = b'-';
    }
    core::str::from_utf8(&buf[p..]).unwrap_or("0")
}
