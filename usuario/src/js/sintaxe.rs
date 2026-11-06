//! A sintaxe do JavaScript: das fichas para a árvore (AST), com os escopos.
//!
//! Enquanto lê, o leitor anota cada declaração no seu escopo e cada uso de
//! nome com o escopo onde apareceu. No fim, `resolver_usos` descobre quais
//! variáveis são capturadas por funções internas: só essas precisam morar
//! no heap (numa "caixa"); as outras ficam na pilha, o que é bem mais rápido.

use super::lexico::*;
use super::vm::{Interno, Txt};
use alloc::boxed::Box;
use alloc::format;
use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum OpU {
    Neg,
    Pos,
    Nao,
    Til,
    Typeof,
    Void,
    Delete,
}

pub enum Chave {
    Nome(Txt),
    Computada(Box<Expr>),
}

pub enum TipoProp {
    Valor(Expr),
    Get(Box<NoFuncao>),
    Set(Box<NoFuncao>),
    Espalhar(Expr),
    /// `{a = 1}`: só vale se virar um padrão de desestruturação.
    Cobre(Txt, u32, Expr),
}

pub struct PropObj {
    pub chave: Chave,
    pub tipo: TipoProp,
}

pub enum Expr {
    Num(f64),
    Str(Txt),
    Bool(bool),
    Nulo,
    Tpl(Vec<Txt>, Vec<Expr>),
    Tagged(Box<Expr>, Vec<(Txt, Txt)>, Vec<Expr>),
    Regex(Txt, Txt),
    Nome(Txt, u32),
    Array(Vec<Expr>),
    Objeto(Vec<PropObj>),
    Funcao(Box<NoFuncao>),
    Classe(Box<NoClasse>),
    Unario(OpU, Box<Expr>),
    Atualiza { soma: bool, prefixo: bool, alvo: Box<Expr> },
    Binario(P, Box<Expr>, Box<Expr>),
    Logico(P, Box<Expr>, Box<Expr>),
    /// `a = b`, `a += b`...; o P é o símbolo da atribuição.
    Atrib(P, Box<Expr>, Box<Expr>),
    /// `[a, b] = c`
    AtribPadrao(Box<Padrao>, Box<Expr>),
    Cond(Box<Expr>, Box<Expr>, Box<Expr>),
    Chamar(Box<Expr>, Vec<Expr>, bool),
    Novo(Box<Expr>, Vec<Expr>),
    Membro(Box<Expr>, Txt, bool),
    Indice(Box<Expr>, Box<Expr>, bool),
    /// O limite de uma cadeia com `?.`: se algo for nulo, o resultado é undefined.
    Cadeia(Box<Expr>),
    Seq(Vec<Expr>),
    Espalhar(Box<Expr>),
    Buraco,
    Yield(Option<Box<Expr>>, bool),
    Await(Box<Expr>),
    SuperMembro(Txt, u32),
    SuperIndice(Box<Expr>, u32),
    SuperChamar(Vec<Expr>, u32),
    NovoAlvo(u32),
}

pub enum Padrao {
    Nome(Txt, u32),
    /// Um alvo qualquer (`a.b`, `a[0]`), só em atribuições.
    Alvo(Expr),
    Array(Vec<Option<(Padrao, Option<Expr>)>>, Option<Box<Padrao>>),
    Objeto(Vec<(Chave, Padrao, Option<Expr>)>, Option<Box<Padrao>>),
}

pub struct Param {
    pub padrao: Padrao,
    pub inicial: Option<Expr>,
    pub resto: bool,
}

pub struct NoFuncao {
    pub nome: Txt,
    pub escopo: u32,
    pub params: Vec<Param>,
    pub corpo: Vec<Stmt>,
    pub seta: bool,
    pub assincrona: bool,
    pub gerador: bool,
    /// Método, getter ou setter: não é construtor nem tem `prototype`.
    pub metodo: bool,
    pub construtor: bool,
    pub derivada: bool,
    pub estrito: bool,
    /// O trecho do fonte (para o `toString`).
    pub fonte: (u32, u32),
}

pub struct MembroClasse {
    pub chave: Chave,
    pub estatico: bool,
    pub tipo: TipoMembro,
}

pub enum TipoMembro {
    Metodo(Box<NoFuncao>),
    Get(Box<NoFuncao>),
    Set(Box<NoFuncao>),
    /// O inicializador vira uma pequena função (o `this` dela é o objeto).
    Campo(Option<Box<NoFuncao>>),
    /// `static { ... }`
    Bloco(Box<NoFuncao>),
}

pub struct NoClasse {
    pub nome: Txt,
    pub escopo: u32,
    pub pai: Option<Expr>,
    pub construtor: Box<NoFuncao>,
    pub membros: Vec<MembroClasse>,
}

pub struct Bloco {
    pub escopo: u32,
    pub stmts: Vec<Stmt>,
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum TipoDecl {
    Var,
    Let,
    Const,
}

pub enum AlvoPara {
    Decl(TipoDecl, Padrao),
    Atrib(Padrao),
}

pub enum Stmt {
    Expr(Expr),
    Decl(TipoDecl, Vec<(Padrao, Option<Expr>)>),
    Funcao(Box<NoFuncao>, u32),
    Classe(Box<NoClasse>, u32),
    Retorno(Option<Expr>),
    Se(Expr, Box<Stmt>, Option<Box<Stmt>>),
    Bloco(Bloco),
    Para { escopo: u32, inicio: Option<Box<Stmt>>, cond: Option<Expr>, passo: Option<Expr>, corpo: Box<Stmt> },
    ParaEm { escopo: u32, alvo: AlvoPara, obj: Expr, corpo: Box<Stmt>, de: bool, espera: bool },
    Enquanto(Expr, Box<Stmt>),
    Faca(Box<Stmt>, Expr),
    Quebra(Option<Txt>),
    Continua(Option<Txt>),
    Lancar(Expr),
    Tentar { bloco: Bloco, param: Option<Padrao>, captura: Option<Bloco>, finalmente: Option<Bloco> },
    Trocar { escopo: u32, valor: Expr, casos: Vec<(Option<Expr>, Vec<Stmt>)> },
    Rotulo(Txt, Box<Stmt>),
    Vazio,
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum TipoVar {
    Var,
    Let,
    Const,
    Param,
    /// O nome de uma função dentro dela mesma (escrever nele não faz nada).
    NomeFuncao,
    /// `this`, `arguments`, a própria função... (existem se alguém usar)
    Pseudo,
}

pub struct DeclVar {
    pub nome: Txt,
    pub tipo: TipoVar,
    pub capturada: bool,
    pub usada: bool,
    pub slot: u16,
}

pub struct Escopo {
    pub pai: u32,
    pub funcao: bool,
    pub seta: bool,
    pub decls: Vec<DeclVar>,
}

pub struct Programa {
    pub corpo: Vec<Stmt>,
    pub globais: Vec<Txt>,
    pub escopos: Vec<Escopo>,
    pub estrito: bool,
    pub fonte: alloc::rc::Rc<str>,
}

pub const ESCOPO_GLOBAL: u32 = 0;
const PROFUNDIDADE_MAXIMA: u32 = 250;

struct Leitor<'a> {
    f: Vec<Ficha>,
    par: Vec<u32>,
    i: usize,
    n: &'a mut Interno,
    fonte: &'a str,
    escopos: Vec<Escopo>,
    usos: Vec<(Txt, u32)>,
    /// Os `var` e as funções do nível de fora (viram propriedades do objeto global).
    globais: Vec<Txt>,
    atual: u32,
    /// O escopo da função em que estamos (onde vão os `var`).
    func: u32,
    gerador: bool,
    assincrona: bool,
    estrito: bool,
    sem_in: bool,
    prof: u32,
    /// Atalho: os átomos de nomes comuns que o leitor olha.
    a_use_strict: Txt,
    a_args: Txt,
}

type R<T> = Result<T, String>;

impl Leitor<'_> {
    fn t(&self) -> Tok {
        self.f[self.i].tok
    }

    fn proximo_tok(&self, n: usize) -> Tok {
        self.f.get(self.i + n).map_or(Tok::Fim, |f| f.tok)
    }

    fn avanca(&mut self) -> Tok {
        let t = self.f[self.i].tok;
        if self.i + 1 < self.f.len() {
            self.i += 1;
        }
        t
    }

    fn e_s(&self, p: P) -> bool {
        self.t() == Tok::S(p)
    }

    fn e_nome(&self, a: Txt) -> bool {
        self.t() == Tok::Nome(a)
    }

    fn aceita(&mut self, p: P) -> bool {
        if self.e_s(p) {
            self.avanca();
            true
        } else {
            false
        }
    }

    fn aceita_nome(&mut self, a: Txt) -> bool {
        if self.e_nome(a) {
            self.avanca();
            true
        } else {
            false
        }
    }

    fn erro<T>(&self, m: &str) -> R<T> {
        let pos = self.f[self.i].pos as usize;
        let linha = self.fonte.as_bytes()[..pos.min(self.fonte.len())].iter().filter(|c| **c == b'\n').count() + 1;
        let trecho: String = self.fonte[pos.min(self.fonte.len())..].chars().take(24).collect();
        Err(format!("{} (linha {}, perto de `{}`)", m, linha, trecho))
    }

    fn espera(&mut self, p: P) -> R<()> {
        if self.aceita(p) {
            Ok(())
        } else {
            self.erro(&format!("esperava {:?}", p))
        }
    }

    /// A inserção automática de `;`.
    fn fim_de_comando(&mut self) -> R<()> {
        if self.aceita(P::PontoVirgula) || self.e_s(P::ChaveF) || self.t() == Tok::Fim || self.f[self.i].nl {
            return Ok(());
        }
        self.erro("esperava ;")
    }

    // ------------------------------------------------------------ escopos

    fn abrir_escopo(&mut self, funcao: bool, seta: bool) -> u32 {
        let id = self.escopos.len() as u32;
        self.escopos.push(Escopo { pai: self.atual, funcao, seta, decls: Vec::new() });
        self.atual = id;
        id
    }

    fn fechar_escopo(&mut self) {
        self.atual = self.escopos[self.atual as usize].pai;
    }

    fn declarar_em(&mut self, escopo: u32, nome: Txt, tipo: TipoVar) {
        let d = &mut self.escopos[escopo as usize].decls;
        if let Some(existente) = d.iter_mut().find(|v| v.nome == nome) {
            // `var x` depois de um parâmetro x é o mesmo x; o nome da própria
            // função perde para qualquer declaração de verdade.
            if existente.tipo == TipoVar::NomeFuncao {
                existente.tipo = tipo;
            }
            return;
        }
        d.push(DeclVar { nome, tipo, capturada: false, usada: false, slot: 0 });
    }

    /// O escopo onde um `var` mora: a função em que estamos. No escopo
    /// global, `var` não é anotado (vira propriedade do objeto global).
    fn declarar_var(&mut self, nome: Txt) {
        // Pode haver um `let` com o nome num bloco intermediário: a
        // especificação diz que é erro; aceitamos e usamos o mais interno.
        if self.func != ESCOPO_GLOBAL {
            self.declarar_em(self.func, nome, TipoVar::Var);
        } else if !self.globais.contains(&nome) {
            self.globais.push(nome);
        }
    }

    fn declarar_lexico(&mut self, nome: Txt, tipo: TipoVar) {
        if self.atual != ESCOPO_GLOBAL {
            self.declarar_em(self.atual, nome, tipo);
        }
    }

    fn uso(&mut self, nome: Txt) -> u32 {
        self.usos.push((nome, self.atual));
        self.atual
    }

    fn nome_ligavel(&mut self) -> R<Txt> {
        match self.t() {
            Tok::Nome(a) if !reservada(a) => {
                self.avanca();
                Ok(a)
            }
            _ => self.erro("esperava um nome"),
        }
    }

    fn declarar_padrao(&mut self, p: &Padrao, tipo: TipoDecl) {
        match p {
            Padrao::Nome(a, _) => match tipo {
                TipoDecl::Var => self.declarar_var(*a),
                TipoDecl::Let => self.declarar_lexico(*a, TipoVar::Let),
                TipoDecl::Const => self.declarar_lexico(*a, TipoVar::Const),
            },
            Padrao::Alvo(_) => {}
            Padrao::Array(itens, resto) => {
                for (p, _) in itens.iter().flatten() {
                    self.declarar_padrao(p, tipo);
                }
                if let Some(r) = resto {
                    self.declarar_padrao(r, tipo);
                }
            }
            Padrao::Objeto(itens, resto) => {
                for (_, p, _) in itens {
                    self.declarar_padrao(p, tipo);
                }
                if let Some(r) = resto {
                    self.declarar_padrao(r, tipo);
                }
            }
        }
    }

    /// Lê um padrão de declaração; os nomes contam como usos no escopo atual.
    fn padrao_decl(&mut self) -> R<Padrao> {
        match self.t() {
            Tok::S(P::ColA) => {
                self.avanca();
                let mut itens = Vec::new();
                let mut resto = None;
                while !self.e_s(P::ColF) {
                    if self.aceita(P::Virgula) {
                        itens.push(None);
                        continue;
                    }
                    if self.aceita(P::Reticencias) {
                        resto = Some(Box::new(self.padrao_decl()?));
                        break;
                    }
                    let p = self.padrao_decl()?;
                    let inicial = if self.aceita(P::Atrib) { Some(self.atribuicao()?) } else { None };
                    itens.push(Some((p, inicial)));
                    if !self.e_s(P::ColF) {
                        self.espera(P::Virgula)?;
                    }
                }
                self.espera(P::ColF)?;
                Ok(Padrao::Array(itens, resto))
            }
            Tok::S(P::ChaveA) => {
                self.avanca();
                let mut itens = Vec::new();
                let mut resto = None;
                while !self.e_s(P::ChaveF) {
                    if self.aceita(P::Reticencias) {
                        resto = Some(Box::new(self.padrao_decl()?));
                        break;
                    }
                    let antes = self.t();
                    let chave = self.chave_prop()?;
                    let p = if self.aceita(P::DoisPontos) {
                        self.padrao_decl()?
                    } else {
                        let Tok::Nome(a) = antes else { return self.erro("padrão inválido") };
                        let e = self.uso(a);
                        Padrao::Nome(a, e)
                    };
                    let inicial = if self.aceita(P::Atrib) { Some(self.atribuicao()?) } else { None };
                    itens.push((chave, p, inicial));
                    if !self.e_s(P::ChaveF) {
                        self.espera(P::Virgula)?;
                    }
                }
                self.espera(P::ChaveF)?;
                Ok(Padrao::Objeto(itens, resto))
            }
            _ => {
                let a = self.nome_ligavel()?;
                let e = self.uso(a);
                Ok(Padrao::Nome(a, e))
            }
        }
    }

    fn expr_para_padrao(&mut self, e: Expr) -> R<Padrao> {
        Ok(match e {
            Expr::Nome(a, s) => Padrao::Nome(a, s),
            e @ (Expr::Membro(..) | Expr::Indice(..) | Expr::SuperMembro(..) | Expr::SuperIndice(..)) => Padrao::Alvo(e),
            Expr::Array(itens) => {
                let mut saida = Vec::new();
                let mut resto = None;
                for item in itens {
                    match item {
                        Expr::Buraco => saida.push(None),
                        Expr::Espalhar(r) => resto = Some(Box::new(self.expr_para_padrao(*r)?)),
                        Expr::Atrib(P::Atrib, alvo, ini) => saida.push(Some((self.expr_para_padrao(*alvo)?, Some(*ini)))),
                        Expr::AtribPadrao(p, ini) => saida.push(Some((*p, Some(*ini)))),
                        outro => saida.push(Some((self.expr_para_padrao(outro)?, None))),
                    }
                }
                Padrao::Array(saida, resto)
            }
            Expr::Objeto(props) => {
                let mut saida = Vec::new();
                let mut resto = None;
                for p in props {
                    match p.tipo {
                        TipoProp::Valor(Expr::Atrib(P::Atrib, alvo, ini)) => saida.push((p.chave, self.expr_para_padrao(*alvo)?, Some(*ini))),
                        TipoProp::Valor(Expr::AtribPadrao(pp, ini)) => saida.push((p.chave, *pp, Some(*ini))),
                        TipoProp::Valor(v) => saida.push((p.chave, self.expr_para_padrao(v)?, None)),
                        TipoProp::Cobre(a, s, ini) => saida.push((p.chave, Padrao::Nome(a, s), Some(ini))),
                        TipoProp::Espalhar(r) => resto = Some(Box::new(self.expr_para_padrao(r)?)),
                        _ => return self.erro("padrão inválido"),
                    }
                }
                Padrao::Objeto(saida, resto)
            }
            _ => return self.erro("alvo de atribuição inválido"),
        })
    }

    // ------------------------------------------------------------ comandos

    fn comando(&mut self) -> R<Stmt> {
        self.prof += 1;
        if self.prof > PROFUNDIDADE_MAXIMA {
            return self.erro("código aninhado demais");
        }
        let r = self.comando_dentro();
        self.prof -= 1;
        r
    }

    fn comando_dentro(&mut self) -> R<Stmt> {
        match self.t() {
            Tok::S(P::ChaveA) => Ok(Stmt::Bloco(self.bloco()?)),
            Tok::S(P::PontoVirgula) => {
                self.avanca();
                Ok(Stmt::Vazio)
            }
            Tok::Nome(a) => self.comando_nome(a),
            _ => self.comando_expr(),
        }
    }

    fn comando_expr(&mut self) -> R<Stmt> {
        let e = self.expressao()?;
        self.fim_de_comando()?;
        Ok(Stmt::Expr(e))
    }

    fn comando_nome(&mut self, a: Txt) -> R<Stmt> {
        match a {
            K_VAR => {
                self.avanca();
                let d = self.lista_decl(TipoDecl::Var)?;
                self.fim_de_comando()?;
                Ok(d)
            }
            K_CONST => {
                self.avanca();
                let d = self.lista_decl(TipoDecl::Const)?;
                self.fim_de_comando()?;
                Ok(d)
            }
            K_LET if matches!(self.proximo_tok(1), Tok::Nome(b) if !reservada(b) || b == K_LET) || matches!(self.proximo_tok(1), Tok::S(P::ColA | P::ChaveA)) => {
                self.avanca();
                let d = self.lista_decl(TipoDecl::Let)?;
                self.fim_de_comando()?;
                Ok(d)
            }
            K_FUNCTION => self.decl_funcao(false),
            K_ASYNC if self.proximo_tok(1) == Tok::Nome(K_FUNCTION) && !self.f[self.i + 1].nl => {
                self.avanca();
                self.decl_funcao(true)
            }
            K_CLASS => {
                let inicio = self.f[self.i].pos;
                self.avanca();
                let nome = self.nome_ligavel()?;
                self.declarar_lexico(nome, TipoVar::Let);
                let onde = self.uso(nome);
                let c = self.classe(nome, inicio)?;
                Ok(Stmt::Classe(Box::new(c), onde))
            }
            K_IF => {
                self.avanca();
                self.espera(P::ParenA)?;
                let c = self.expressao()?;
                self.espera(P::ParenF)?;
                let entao = Box::new(self.comando()?);
                let senao = if self.aceita_nome(K_ELSE) { Some(Box::new(self.comando()?)) } else { None };
                Ok(Stmt::Se(c, entao, senao))
            }
            K_FOR => self.para(),
            K_WHILE => {
                self.avanca();
                self.espera(P::ParenA)?;
                let c = self.expressao()?;
                self.espera(P::ParenF)?;
                Ok(Stmt::Enquanto(c, Box::new(self.comando()?)))
            }
            K_DO => {
                self.avanca();
                let corpo = Box::new(self.comando()?);
                if !self.aceita_nome(K_WHILE) {
                    return self.erro("esperava while");
                }
                self.espera(P::ParenA)?;
                let c = self.expressao()?;
                self.espera(P::ParenF)?;
                self.aceita(P::PontoVirgula);
                Ok(Stmt::Faca(corpo, c))
            }
            K_RETURN => {
                self.avanca();
                if self.func == ESCOPO_GLOBAL {
                    return self.erro("return fora de uma função");
                }
                let tem = !(self.e_s(P::PontoVirgula) || self.e_s(P::ChaveF) || self.t() == Tok::Fim || self.f[self.i].nl);
                let e = if tem { Some(self.expressao()?) } else { None };
                self.fim_de_comando()?;
                Ok(Stmt::Retorno(e))
            }
            K_BREAK | K_CONTINUE => {
                self.avanca();
                let rotulo = match self.t() {
                    Tok::Nome(r) if !self.f[self.i].nl && !reservada(r) => {
                        self.avanca();
                        Some(r)
                    }
                    _ => None,
                };
                self.fim_de_comando()?;
                Ok(if a == K_BREAK { Stmt::Quebra(rotulo) } else { Stmt::Continua(rotulo) })
            }
            K_THROW => {
                self.avanca();
                let e = self.expressao()?;
                self.fim_de_comando()?;
                Ok(Stmt::Lancar(e))
            }
            K_TRY => self.tentar(),
            K_SWITCH => self.trocar(),
            K_DEBUGGER => {
                self.avanca();
                self.fim_de_comando()?;
                Ok(Stmt::Vazio)
            }
            K_IMPORT if !matches!(self.proximo_tok(1), Tok::S(P::ParenA | P::Ponto)) => self.erro("módulos (import) ainda não são aceitos"),
            K_EXPORT => self.erro("módulos (export) ainda não são aceitos"),
            K_WITH => self.erro("with não é aceito"),
            r if !reservada(r) && self.proximo_tok(1) == Tok::S(P::DoisPontos) => {
                self.avanca();
                self.avanca();
                Ok(Stmt::Rotulo(r, Box::new(self.comando()?)))
            }
            _ => self.comando_expr(),
        }
    }

    fn lista_decl(&mut self, tipo: TipoDecl) -> R<Stmt> {
        let mut itens = Vec::new();
        loop {
            let p = self.padrao_decl()?;
            self.declarar_padrao(&p, tipo);
            let inicial = if self.aceita(P::Atrib) { Some(self.atribuicao()?) } else { None };
            itens.push((p, inicial));
            if !self.aceita(P::Virgula) {
                break;
            }
        }
        Ok(Stmt::Decl(tipo, itens))
    }

    fn decl_funcao(&mut self, assincrona: bool) -> R<Stmt> {
        let inicio = self.f[if assincrona { self.i - 1 } else { self.i }].pos;
        self.avanca(); // function
        let gerador = self.aceita(P::Vezes);
        let nome = self.nome_ligavel()?;
        // Em bloco ou não, a função é anotada na função em volta (como o `var`):
        // o código antigo chama funções declaradas dentro de um `if`.
        self.declarar_var(nome);
        let onde = self.uso(nome);
        let mut f = self.funcao(nome, assincrona, gerador, false, inicio, false)?;
        f.nome = nome;
        Ok(Stmt::Funcao(Box::new(f), onde))
    }

    fn bloco(&mut self) -> R<Bloco> {
        let escopo = self.abrir_escopo(false, false);
        let stmts = self.corpo_bloco()?;
        self.fechar_escopo();
        Ok(Bloco { escopo, stmts })
    }

    fn corpo_bloco(&mut self) -> R<Vec<Stmt>> {
        self.espera(P::ChaveA)?;
        let mut v = Vec::new();
        while !self.e_s(P::ChaveF) {
            if self.t() == Tok::Fim {
                return self.erro("bloco sem fim");
            }
            v.push(self.comando()?);
        }
        self.avanca();
        Ok(v)
    }

    fn para(&mut self) -> R<Stmt> {
        self.avanca();
        let espera = self.aceita_nome(K_AWAIT);
        self.espera(P::ParenA)?;
        let escopo = self.abrir_escopo(false, false);
        let mut inicio = None;
        let decl = match self.t() {
            Tok::Nome(K_VAR) => Some(TipoDecl::Var),
            Tok::Nome(K_CONST) => Some(TipoDecl::Const),
            Tok::Nome(K_LET) if !matches!(self.proximo_tok(1), Tok::S(P::Atrib | P::Ponto | P::PontoVirgula) | Tok::Nome(K_IN)) => Some(TipoDecl::Let),
            _ => None,
        };
        let r;
        if let Some(tipo) = decl {
            self.avanca();
            let antes = self.sem_in;
            self.sem_in = true;
            let p = self.padrao_decl()?;
            self.declarar_padrao(&p, tipo);
            if self.e_nome(K_OF) || self.e_nome(K_IN) {
                self.sem_in = antes;
                r = self.resto_para_em(escopo, AlvoPara::Decl(tipo, p), espera);
                self.fechar_escopo();
                return r;
            }
            let ini = if self.aceita(P::Atrib) { Some(self.atribuicao()?) } else { None };
            let mut itens = vec![(p, ini)];
            while self.aceita(P::Virgula) {
                let p = self.padrao_decl()?;
                self.declarar_padrao(&p, tipo);
                let ini = if self.aceita(P::Atrib) { Some(self.atribuicao()?) } else { None };
                itens.push((p, ini));
            }
            self.sem_in = antes;
            inicio = Some(Box::new(Stmt::Decl(tipo, itens)));
        } else if !self.e_s(P::PontoVirgula) {
            let antes = self.sem_in;
            self.sem_in = true;
            let e = self.expressao()?;
            self.sem_in = antes;
            if self.e_nome(K_OF) || self.e_nome(K_IN) {
                let p = self.expr_para_padrao(e)?;
                r = self.resto_para_em(escopo, AlvoPara::Atrib(p), espera);
                self.fechar_escopo();
                return r;
            }
            inicio = Some(Box::new(Stmt::Expr(e)));
        }
        self.espera(P::PontoVirgula)?;
        let cond = if self.e_s(P::PontoVirgula) { None } else { Some(self.expressao()?) };
        self.espera(P::PontoVirgula)?;
        let passo = if self.e_s(P::ParenF) { None } else { Some(self.expressao()?) };
        self.espera(P::ParenF)?;
        let corpo = Box::new(self.comando()?);
        self.fechar_escopo();
        Ok(Stmt::Para { escopo, inicio, cond, passo, corpo })
    }

    fn resto_para_em(&mut self, escopo: u32, alvo: AlvoPara, espera: bool) -> R<Stmt> {
        let de = self.e_nome(K_OF);
        self.avanca();
        let obj = if de { self.atribuicao()? } else { self.expressao()? };
        self.espera(P::ParenF)?;
        let corpo = Box::new(self.comando()?);
        Ok(Stmt::ParaEm { escopo, alvo, obj, corpo, de, espera })
    }

    fn tentar(&mut self) -> R<Stmt> {
        self.avanca();
        let bloco = self.bloco()?;
        let (mut param, mut captura, mut finalmente) = (None, None, None);
        if self.aceita_nome(K_CATCH) {
            let escopo = self.abrir_escopo(false, false);
            if self.aceita(P::ParenA) {
                let p = self.padrao_decl()?;
                self.declarar_padrao(&p, TipoDecl::Let);
                self.espera(P::ParenF)?;
                param = Some(p);
            }
            let stmts = self.corpo_bloco()?;
            self.fechar_escopo();
            captura = Some(Bloco { escopo, stmts });
        }
        if self.aceita_nome(K_FINALLY) {
            finalmente = Some(self.bloco()?);
        }
        if captura.is_none() && finalmente.is_none() {
            return self.erro("try sem catch nem finally");
        }
        Ok(Stmt::Tentar { bloco, param, captura, finalmente })
    }

    fn trocar(&mut self) -> R<Stmt> {
        self.avanca();
        self.espera(P::ParenA)?;
        let valor = self.expressao()?;
        self.espera(P::ParenF)?;
        self.espera(P::ChaveA)?;
        let escopo = self.abrir_escopo(false, false);
        let mut casos = Vec::new();
        while !self.aceita(P::ChaveF) {
            let teste = if self.aceita_nome(K_DEFAULT) {
                None
            } else if self.aceita_nome(K_CASE) {
                Some(self.expressao()?)
            } else {
                return self.erro("esperava case");
            };
            self.espera(P::DoisPontos)?;
            let mut corpo = Vec::new();
            while !matches!(self.t(), Tok::Nome(K_CASE | K_DEFAULT) | Tok::S(P::ChaveF)) {
                if self.t() == Tok::Fim {
                    return self.erro("switch sem fim");
                }
                corpo.push(self.comando()?);
            }
            casos.push((teste, corpo));
        }
        self.fechar_escopo();
        Ok(Stmt::Trocar { escopo, valor, casos })
    }

    // ------------------------------------------------------------ funções

    fn parametros(&mut self) -> R<Vec<Param>> {
        self.espera(P::ParenA)?;
        let mut params = Vec::new();
        while !self.e_s(P::ParenF) {
            let resto = self.aceita(P::Reticencias);
            let p = self.padrao_decl()?;
            self.declarar_params(&p);
            let inicial = if self.aceita(P::Atrib) { Some(self.atribuicao()?) } else { None };
            params.push(Param { padrao: p, inicial, resto });
            if !self.e_s(P::ParenF) {
                self.espera(P::Virgula)?;
            }
        }
        self.avanca();
        Ok(params)
    }

    fn declarar_params(&mut self, p: &Padrao) {
        match p {
            Padrao::Nome(a, _) => {
                let e = self.atual;
                self.declarar_em(e, *a, TipoVar::Param);
            }
            Padrao::Alvo(_) => {}
            Padrao::Array(itens, resto) => {
                for (p, _) in itens.iter().flatten() {
                    self.declarar_params(p);
                }
                if let Some(r) = resto {
                    self.declarar_params(r);
                }
            }
            Padrao::Objeto(itens, resto) => {
                for (_, p, _) in itens {
                    self.declarar_params(p);
                }
                if let Some(r) = resto {
                    self.declarar_params(r);
                }
            }
        }
    }

    fn com_funcao<T>(&mut self, seta: bool, assincrona: bool, gerador: bool, f: impl FnOnce(&mut Self, u32) -> R<T>) -> R<T> {
        let antes = (self.func, self.gerador, self.assincrona, self.sem_in, self.estrito);
        let escopo = self.abrir_escopo(true, seta);
        self.func = escopo;
        self.gerador = gerador;
        self.assincrona = assincrona;
        self.sem_in = false;
        let r = f(self, escopo);
        self.fechar_escopo();
        (self.func, self.gerador, self.assincrona, self.sem_in, self.estrito) = antes;
        r
    }

    /// Lê a diretiva `"use strict"` no começo de um corpo.
    fn diretivas(&mut self) {
        if let Tok::Str(s) = self.t() {
            if s == self.a_use_strict {
                self.estrito = true;
            }
        }
    }

    /// `(params) { corpo }`, com o cursor no `(`.
    fn funcao(&mut self, nome: Txt, assincrona: bool, gerador: bool, metodo: bool, inicio: u32, nome_proprio: bool) -> R<NoFuncao> {
        self.com_funcao(false, assincrona, gerador, |s, escopo| {
            if nome_proprio {
                s.declarar_em(escopo, nome, TipoVar::NomeFuncao);
            }
            let params = s.parametros()?;
            if !s.e_s(P::ChaveA) {
                return s.erro("esperava {");
            }
            s.avanca();
            s.diretivas();
            let mut corpo = Vec::new();
            while !s.e_s(P::ChaveF) {
                if s.t() == Tok::Fim {
                    return s.erro("função sem fim");
                }
                corpo.push(s.comando()?);
            }
            let fim = s.f[s.i].fim;
            s.avanca();
            Ok(NoFuncao {
                nome,
                escopo,
                params,
                corpo,
                seta: false,
                assincrona,
                gerador,
                metodo,
                construtor: false,
                derivada: false,
                estrito: s.estrito,
                fonte: (inicio, fim),
            })
        })
    }

    fn seta(&mut self, assincrona: bool, inicio: u32) -> R<Expr> {
        let gerador_fora = self.gerador;
        let no = self.com_funcao(true, assincrona, false, |s, escopo| {
            let _ = gerador_fora;
            let params = if s.e_s(P::ParenA) {
                s.parametros()?
            } else {
                let a = s.nome_ligavel()?;
                s.declarar_em(escopo, a, TipoVar::Param);
                s.usos.push((a, escopo));
                vec![Param { padrao: Padrao::Nome(a, escopo), inicial: None, resto: false }]
            };
            if !s.aceita(P::Seta) {
                return s.erro("esperava =>");
            }
            let corpo = if s.e_s(P::ChaveA) {
                s.avanca();
                s.diretivas();
                let mut v = Vec::new();
                while !s.e_s(P::ChaveF) {
                    if s.t() == Tok::Fim {
                        return s.erro("função sem fim");
                    }
                    v.push(s.comando()?);
                }
                s.avanca();
                v
            } else {
                vec![Stmt::Retorno(Some(s.atribuicao()?))]
            };
            let fim = s.f[s.i - 1].fim;
            Ok(NoFuncao {
                nome: K_VAZIO,
                escopo,
                params,
                corpo,
                seta: true,
                assincrona,
                gerador: false,
                metodo: false,
                construtor: false,
                derivada: false,
                estrito: s.estrito,
                fonte: (inicio, fim),
            })
        })?;
        Ok(Expr::Funcao(Box::new(no)))
    }

    /// A chave de uma propriedade: nome, texto, número ou `[expr]`.
    fn chave_prop(&mut self) -> R<Chave> {
        match self.avanca() {
            Tok::Nome(a) | Tok::Str(a) | Tok::Privado(a) => Ok(Chave::Nome(a)),
            Tok::Num(n) => {
                let t = super::vm::numero_para_texto(n);
                Ok(Chave::Nome(self.n.atomo(&t)))
            }
            Tok::S(P::ColA) => {
                let antes = self.sem_in;
                self.sem_in = false;
                let e = self.atribuicao()?;
                self.sem_in = antes;
                self.espera(P::ColF)?;
                Ok(Chave::Computada(Box::new(e)))
            }
            _ => {
                self.i -= 1;
                self.erro("esperava o nome de uma propriedade")
            }
        }
    }

    fn fim_chave(&self) -> bool {
        matches!(self.t(), Tok::S(P::Virgula | P::ChaveF | P::DoisPontos | P::ParenA | P::Atrib | P::PontoVirgula))
    }

    fn objeto(&mut self) -> R<Expr> {
        self.avanca();
        let antes = self.sem_in;
        self.sem_in = false;
        let mut props = Vec::new();
        while !self.e_s(P::ChaveF) {
            let p = self.prop_objeto()?;
            props.push(p);
            if !self.e_s(P::ChaveF) {
                self.espera(P::Virgula)?;
            }
        }
        self.avanca();
        self.sem_in = antes;
        Ok(Expr::Objeto(props))
    }

    fn prop_objeto(&mut self) -> R<PropObj> {
        if self.aceita(P::Reticencias) {
            let e = self.atribuicao()?;
            return Ok(PropObj { chave: Chave::Nome(K_VAZIO), tipo: TipoProp::Espalhar(e) });
        }
        let inicio = self.f[self.i].pos;
        // get/set/async/* antes de uma chave
        let mut assincrona = false;
        let mut gerador = false;
        if let Tok::Nome(a @ (K_GET | K_SET)) = self.t() {
            self.avanca();
            if !self.fim_chave() {
                let chave = self.chave_prop()?;
                let nome = nome_da_chave(&chave);
                let f = Box::new(self.funcao(nome, false, false, true, inicio, false)?);
                let tipo = if a == K_GET { TipoProp::Get(f) } else { TipoProp::Set(f) };
                return Ok(PropObj { chave, tipo });
            }
            self.i -= 1;
        }
        if self.e_nome(K_ASYNC) && !matches!(self.proximo_tok(1), Tok::S(P::Virgula | P::ChaveF | P::DoisPontos | P::ParenA | P::Atrib)) && !self.f[self.i + 1].nl {
            self.avanca();
            assincrona = true;
        }
        if self.aceita(P::Vezes) {
            gerador = true;
        }
        let antes = self.t();
        let chave = self.chave_prop()?;
        if self.e_s(P::ParenA) {
            let nome = nome_da_chave(&chave);
            let f = self.funcao(nome, assincrona, gerador, true, inicio, false)?;
            return Ok(PropObj { chave, tipo: TipoProp::Valor(Expr::Funcao(Box::new(f))) });
        }
        if self.aceita(P::DoisPontos) {
            let mut v = self.atribuicao()?;
            dar_nome(&mut v, &chave);
            return Ok(PropObj { chave, tipo: TipoProp::Valor(v) });
        }
        // {a} ou {a = 1}
        let Tok::Nome(a) = antes else { return self.erro("objeto inválido") };
        if reservada(a) {
            return self.erro("objeto inválido");
        }
        let e = self.uso(a);
        if self.aceita(P::Atrib) {
            let ini = self.atribuicao()?;
            return Ok(PropObj { chave, tipo: TipoProp::Cobre(a, e, ini) });
        }
        Ok(PropObj { chave, tipo: TipoProp::Valor(Expr::Nome(a, e)) })
    }

    fn classe(&mut self, nome: Txt, inicio: u32) -> R<NoClasse> {
        let estrito_antes = self.estrito;
        self.estrito = true;
        let escopo = self.abrir_escopo(false, false);
        if nome != K_VAZIO {
            self.declarar_em(escopo, nome, TipoVar::Const);
        }
        let pai = if self.aceita_nome(K_EXTENDS) { Some(self.nivel_chamada()?) } else { None };
        self.espera(P::ChaveA)?;
        let mut construtor = None;
        let mut membros = Vec::new();
        while !self.aceita(P::ChaveF) {
            if self.aceita(P::PontoVirgula) {
                continue;
            }
            if self.t() == Tok::Fim {
                return self.erro("classe sem fim");
            }
            let ini_m = self.f[self.i].pos;
            let mut estatico = false;
            if self.e_nome(K_STATIC) && !matches!(self.proximo_tok(1), Tok::S(P::ParenA | P::Atrib | P::PontoVirgula | P::ChaveF)) {
                self.avanca();
                estatico = true;
                if self.e_s(P::ChaveA) {
                    let f = self.com_funcao(false, false, false, |s, esc| {
                        let corpo = s.corpo_bloco()?;
                        Ok(no_simples(esc, corpo, true))
                    })?;
                    membros.push(MembroClasse { chave: Chave::Nome(K_VAZIO), estatico: true, tipo: TipoMembro::Bloco(Box::new(f)) });
                    continue;
                }
            }
            let mut assincrona = false;
            let mut gerador = false;
            let mut acesso = None;
            if let Tok::Nome(a @ (K_GET | K_SET)) = self.t() {
                if !matches!(self.proximo_tok(1), Tok::S(P::ParenA | P::Atrib | P::PontoVirgula | P::ChaveF)) && !self.f[self.i + 1].nl {
                    self.avanca();
                    acesso = Some(a);
                }
            }
            if self.e_nome(K_ASYNC) && !matches!(self.proximo_tok(1), Tok::S(P::ParenA | P::Atrib | P::PontoVirgula | P::ChaveF)) && !self.f[self.i + 1].nl {
                self.avanca();
                assincrona = true;
            }
            if self.aceita(P::Vezes) {
                gerador = true;
            }
            let chave = self.chave_prop()?;
            let nome_m = nome_da_chave(&chave);
            if self.e_s(P::ParenA) {
                let mut f = self.funcao(nome_m, assincrona, gerador, true, ini_m, false)?;
                if !estatico && acesso.is_none() && nome_m == K_CONSTRUCTOR {
                    f.construtor = true;
                    f.metodo = false;
                    f.derivada = pai.is_some();
                    f.nome = nome;
                    f.fonte = (inicio, 0);
                    construtor = Some(Box::new(f));
                    continue;
                }
                let f = Box::new(f);
                let tipo = match acesso {
                    Some(K_GET) => TipoMembro::Get(f),
                    Some(_) => TipoMembro::Set(f),
                    None => TipoMembro::Metodo(f),
                };
                membros.push(MembroClasse { chave, estatico, tipo });
                continue;
            }
            // Um campo: `x = 1;`
            let inicial = if self.aceita(P::Atrib) {
                let f = self.com_funcao(false, false, false, |s, esc| {
                    let mut e = s.atribuicao()?;
                    dar_nome(&mut e, &Chave::Nome(nome_m));
                    Ok(no_simples(esc, vec![Stmt::Retorno(Some(e))], true))
                })?;
                Some(Box::new(f))
            } else {
                None
            };
            self.fim_de_comando()?;
            membros.push(MembroClasse { chave, estatico, tipo: TipoMembro::Campo(inicial) });
        }
        let fim = self.f[self.i - 1].fim;
        let derivada = pai.is_some();
        let mut construtor = match construtor {
            Some(c) => c,
            None => Box::new(self.com_funcao(false, false, false, |s, esc| {
                let mut f = no_simples(esc, Vec::new(), false);
                if derivada {
                    // constructor(...args) { super(...args); }
                    let args = s.a_args;
                    s.declarar_em(esc, args, TipoVar::Param);
                    s.usos.push((args, esc));
                    f.params.push(Param { padrao: Padrao::Nome(args, esc), inicial: None, resto: true });
                    for u in [K_FN_VAR, K_NT_VAR, K_THIS_VAR] {
                        s.usos.push((u, esc));
                    }
                    let a = Expr::Espalhar(Box::new(Expr::Nome(args, esc)));
                    f.corpo.push(Stmt::Expr(Expr::SuperChamar(vec![a], esc)));
                }
                Ok(f)
            })?),
        };
        construtor.construtor = true;
        construtor.metodo = false;
        construtor.derivada = derivada;
        construtor.nome = nome;
        construtor.fonte = (inicio, fim);
        construtor.estrito = true;
        self.fechar_escopo();
        self.estrito = estrito_antes;
        Ok(NoClasse { nome, escopo, pai, construtor, membros })
    }

    // ------------------------------------------------------------ expressões

    fn expressao(&mut self) -> R<Expr> {
        let e = self.atribuicao()?;
        if !self.e_s(P::Virgula) {
            return Ok(e);
        }
        let mut v = vec![e];
        while self.aceita(P::Virgula) {
            v.push(self.atribuicao()?);
        }
        Ok(Expr::Seq(v))
    }

    fn e_seta_a_frente(&self) -> bool {
        match self.t() {
            Tok::S(P::ParenA) => {
                let f = self.par[self.i] as usize;
                f != 0 && matches!(self.f.get(f + 1), Some(x) if x.tok == Tok::S(P::Seta))
            }
            Tok::Nome(a) if !reservada(a) => self.proximo_tok(1) == Tok::S(P::Seta),
            _ => false,
        }
    }

    fn atribuicao(&mut self) -> R<Expr> {
        self.prof += 1;
        if self.prof > PROFUNDIDADE_MAXIMA {
            return self.erro("expressão aninhada demais");
        }
        let r = self.atribuicao_dentro();
        self.prof -= 1;
        r
    }

    fn atribuicao_dentro(&mut self) -> R<Expr> {
        let inicio = self.f[self.i].pos;
        if self.e_nome(K_ASYNC) && !self.f[self.i + 1].nl {
            let fim_seta = match self.proximo_tok(1) {
                Tok::S(P::ParenA) => {
                    let f = self.par[self.i + 1] as usize;
                    f != 0 && matches!(self.f.get(f + 1), Some(x) if x.tok == Tok::S(P::Seta))
                }
                Tok::Nome(a) if !reservada(a) => self.proximo_tok(2) == Tok::S(P::Seta),
                _ => false,
            };
            if fim_seta {
                self.avanca();
                return self.seta(true, inicio);
            }
        }
        if self.e_seta_a_frente() {
            return self.seta(false, inicio);
        }
        if self.gerador && self.e_nome(K_YIELD) {
            return self.expr_yield();
        }
        let e = self.condicional()?;
        let Tok::S(p) = self.t() else { return Ok(e) };
        match p {
            P::Atrib => {
                self.avanca();
                if matches!(e, Expr::Array(_) | Expr::Objeto(_)) {
                    let padrao = self.expr_para_padrao(e)?;
                    let v = self.atribuicao()?;
                    return Ok(Expr::AtribPadrao(Box::new(padrao), Box::new(v)));
                }
                if !matches!(e, Expr::Nome(..) | Expr::Membro(..) | Expr::Indice(..) | Expr::SuperMembro(..) | Expr::SuperIndice(..)) {
                    return self.erro("alvo de atribuição inválido");
                }
                let mut v = self.atribuicao()?;
                if let Expr::Nome(a, _) = e {
                    dar_nome(&mut v, &Chave::Nome(a));
                }
                Ok(Expr::Atrib(p, Box::new(e), Box::new(v)))
            }
            P::MaisAtrib | P::MenosAtrib | P::VezesAtrib | P::DivideAtrib | P::RestoAtrib | P::PotAtrib | P::ShlAtrib | P::ShrAtrib
            | P::UshrAtrib | P::EAtrib | P::OuAtrib | P::XorAtrib | P::E2Atrib | P::Ou2Atrib | P::NulishAtrib => {
                if !matches!(e, Expr::Nome(..) | Expr::Membro(..) | Expr::Indice(..) | Expr::SuperMembro(..) | Expr::SuperIndice(..)) {
                    return self.erro("alvo de atribuição inválido");
                }
                self.avanca();
                let v = self.atribuicao()?;
                Ok(Expr::Atrib(p, Box::new(e), Box::new(v)))
            }
            _ => Ok(e),
        }
    }

    fn expr_yield(&mut self) -> R<Expr> {
        self.avanca();
        if self.f[self.i].nl || matches!(self.t(), Tok::S(P::ParenF | P::ColF | P::ChaveF | P::Virgula | P::PontoVirgula | P::DoisPontos) | Tok::Fim) {
            return Ok(Expr::Yield(None, false));
        }
        let delegado = self.aceita(P::Vezes);
        let e = self.atribuicao()?;
        Ok(Expr::Yield(Some(Box::new(e)), delegado))
    }

    fn condicional(&mut self) -> R<Expr> {
        let c = self.binario(0)?;
        if !self.aceita(P::Interrogacao) {
            return Ok(c);
        }
        let antes = self.sem_in;
        self.sem_in = false;
        let a = self.atribuicao()?;
        self.sem_in = antes;
        self.espera(P::DoisPontos)?;
        let b = self.atribuicao()?;
        Ok(Expr::Cond(Box::new(c), Box::new(a), Box::new(b)))
    }

    fn precedencia(&self) -> Option<(u8, P)> {
        let p = match self.t() {
            Tok::S(p) => p,
            Tok::Nome(K_INSTANCEOF) => return Some((8, P::Menor)),
            Tok::Nome(K_IN) if !self.sem_in => return Some((8, P::Maior)),
            _ => return None,
        };
        let n = match p {
            P::Nulish => 1,
            P::Ou2 => 2,
            P::E2 => 3,
            P::Ou => 4,
            P::Xor => 5,
            P::E => 6,
            P::Igual2 | P::Diferente | P::Igual3 | P::Diferente3 => 7,
            P::Menor | P::Maior | P::MenorIgual | P::MaiorIgual => 8,
            P::Shl | P::Shr | P::Ushr => 9,
            P::Mais | P::Menos => 10,
            P::Vezes | P::Divide | P::Resto => 11,
            P::Pot => 12,
            _ => return None,
        };
        Some((n, p))
    }

    fn binario(&mut self, minimo: u8) -> R<Expr> {
        let mut e = self.unario()?;
        while let Some((prec, p)) = self.precedencia() {
            if prec < minimo.max(1) {
                break;
            }
            let palavra = self.t();
            self.avanca();
            // ** é associativo à direita
            let d = self.binario(if p == P::Pot { prec } else { prec + 1 })?;
            e = match (palavra, p) {
                (Tok::Nome(K_INSTANCEOF), _) => Expr::Binario(P::Privado, Box::new(e), Box::new(d)),
                (Tok::Nome(K_IN), _) => Expr::Binario(P::Seta, Box::new(e), Box::new(d)),
                (_, P::E2 | P::Ou2 | P::Nulish) => Expr::Logico(p, Box::new(e), Box::new(d)),
                _ => Expr::Binario(p, Box::new(e), Box::new(d)),
            };
        }
        Ok(e)
    }

    fn unario(&mut self) -> R<Expr> {
        let op = match self.t() {
            Tok::S(P::Menos) => OpU::Neg,
            Tok::S(P::Mais) => OpU::Pos,
            Tok::S(P::Nao) => OpU::Nao,
            Tok::S(P::Til) => OpU::Til,
            Tok::Nome(K_TYPEOF) => OpU::Typeof,
            Tok::Nome(K_VOID) => OpU::Void,
            Tok::Nome(K_DELETE) => OpU::Delete,
            Tok::S(p @ (P::Mais2 | P::Menos2)) => {
                self.avanca();
                let alvo = self.unario()?;
                if !matches!(alvo, Expr::Nome(..) | Expr::Membro(..) | Expr::Indice(..)) {
                    return self.erro("alvo inválido para ++/--");
                }
                return Ok(Expr::Atualiza { soma: p == P::Mais2, prefixo: true, alvo: Box::new(alvo) });
            }
            Tok::Nome(K_AWAIT) if self.assincrona => {
                self.avanca();
                let e = self.unario()?;
                return Ok(Expr::Await(Box::new(e)));
            }
            _ => return self.pos_fixo(),
        };
        self.avanca();
        self.prof += 1;
        if self.prof > PROFUNDIDADE_MAXIMA {
            return self.erro("expressão aninhada demais");
        }
        let e = self.unario()?;
        self.prof -= 1;
        Ok(Expr::Unario(op, Box::new(e)))
    }

    fn pos_fixo(&mut self) -> R<Expr> {
        let e = self.nivel_chamada()?;
        if let Tok::S(p @ (P::Mais2 | P::Menos2)) = self.t() {
            if !self.f[self.i].nl && matches!(e, Expr::Nome(..) | Expr::Membro(..) | Expr::Indice(..)) {
                self.avanca();
                return Ok(Expr::Atualiza { soma: p == P::Mais2, prefixo: false, alvo: Box::new(e) });
            }
        }
        Ok(e)
    }

    fn argumentos(&mut self) -> R<Vec<Expr>> {
        self.espera(P::ParenA)?;
        let antes = self.sem_in;
        self.sem_in = false;
        let mut v = Vec::new();
        while !self.e_s(P::ParenF) {
            if self.aceita(P::Reticencias) {
                v.push(Expr::Espalhar(Box::new(self.atribuicao()?)));
            } else {
                v.push(self.atribuicao()?);
            }
            if !self.e_s(P::ParenF) {
                self.espera(P::Virgula)?;
            }
        }
        self.avanca();
        self.sem_in = antes;
        Ok(v)
    }

    fn novo(&mut self) -> R<Expr> {
        self.avanca(); // new
        if self.aceita(P::Ponto) {
            if !self.aceita_nome(K_TARGET) {
                return self.erro("esperava new.target");
            }
            let e = self.uso(K_NT_VAR);
            return Ok(Expr::NovoAlvo(e));
        }
        // O alvo do `new` não inclui chamadas: `new a.b(c).d` = `(new a.b(c)).d`
        let mut alvo = if self.e_nome(K_NEW) { self.novo()? } else { self.primario()? };
        loop {
            match self.t() {
                Tok::S(P::Ponto) => {
                    self.avanca();
                    let nome = self.nome_de_propriedade()?;
                    alvo = Expr::Membro(Box::new(alvo), nome, false);
                }
                Tok::S(P::ColA) => {
                    self.avanca();
                    let antes = self.sem_in;
                    self.sem_in = false;
                    let i = self.expressao()?;
                    self.sem_in = antes;
                    self.espera(P::ColF)?;
                    alvo = Expr::Indice(Box::new(alvo), Box::new(i), false);
                }
                _ => break,
            }
        }
        let args = if self.e_s(P::ParenA) { self.argumentos()? } else { Vec::new() };
        Ok(Expr::Novo(Box::new(alvo), args))
    }

    fn nome_de_propriedade(&mut self) -> R<Txt> {
        match self.avanca() {
            Tok::Nome(a) | Tok::Privado(a) => Ok(a),
            _ => {
                self.i -= 1;
                self.erro("esperava o nome de uma propriedade")
            }
        }
    }

    fn nivel_chamada(&mut self) -> R<Expr> {
        let mut e = if self.e_nome(K_NEW) {
            self.novo()?
        } else if self.e_nome(K_SUPER) {
            self.super_()?
        } else {
            self.primario()?
        };
        let mut cadeia = false;
        loop {
            match self.t() {
                Tok::S(P::Ponto) => {
                    self.avanca();
                    let nome = self.nome_de_propriedade()?;
                    e = Expr::Membro(Box::new(e), nome, false);
                }
                Tok::S(P::InterroPonto) => {
                    self.avanca();
                    cadeia = true;
                    match self.t() {
                        Tok::S(P::ParenA) => {
                            let a = self.argumentos()?;
                            e = Expr::Chamar(Box::new(e), a, true);
                        }
                        Tok::S(P::ColA) => {
                            self.avanca();
                            let i = self.expressao()?;
                            self.espera(P::ColF)?;
                            e = Expr::Indice(Box::new(e), Box::new(i), true);
                        }
                        _ => {
                            let nome = self.nome_de_propriedade()?;
                            e = Expr::Membro(Box::new(e), nome, true);
                        }
                    }
                }
                Tok::S(P::ColA) => {
                    self.avanca();
                    let antes = self.sem_in;
                    self.sem_in = false;
                    let i = self.expressao()?;
                    self.sem_in = antes;
                    self.espera(P::ColF)?;
                    e = Expr::Indice(Box::new(e), Box::new(i), false);
                }
                Tok::S(P::ParenA) => {
                    let a = self.argumentos()?;
                    e = Expr::Chamar(Box::new(e), a, false);
                }
                Tok::Tpl(_, _, _, true) => {
                    let (crus, exprs) = self.template_marcado()?;
                    e = Expr::Tagged(Box::new(e), crus, exprs);
                }
                _ => break,
            }
        }
        Ok(if cadeia { Expr::Cadeia(Box::new(e)) } else { e })
    }

    fn super_(&mut self) -> R<Expr> {
        self.avanca();
        let esc = self.atual;
        match self.t() {
            Tok::S(P::ParenA) => {
                for u in [K_FN_VAR, K_NT_VAR, K_THIS_VAR] {
                    self.uso(u);
                }
                let a = self.argumentos()?;
                Ok(Expr::SuperChamar(a, esc))
            }
            Tok::S(P::Ponto) => {
                self.avanca();
                self.uso(K_THIS_VAR);
                self.uso(K_HOME_VAR);
                let nome = self.nome_de_propriedade()?;
                Ok(Expr::SuperMembro(nome, esc))
            }
            Tok::S(P::ColA) => {
                self.avanca();
                self.uso(K_THIS_VAR);
                self.uso(K_HOME_VAR);
                let i = self.expressao()?;
                self.espera(P::ColF)?;
                Ok(Expr::SuperIndice(Box::new(i), esc))
            }
            _ => self.erro("super inválido"),
        }
    }

    fn template_marcado(&mut self) -> R<(Vec<(Txt, Txt)>, Vec<Expr>)> {
        let mut pedacos = Vec::new();
        let mut exprs = Vec::new();
        loop {
            let Tok::Tpl(cru, cozido, mais, _) = self.t() else { return self.erro("template inválido") };
            self.avanca();
            pedacos.push((cru, cozido));
            if !mais {
                return Ok((pedacos, exprs));
            }
            exprs.push(self.expressao()?);
        }
    }

    fn primario(&mut self) -> R<Expr> {
        let inicio = self.f[self.i].pos;
        match self.t() {
            Tok::Num(n) => {
                self.avanca();
                Ok(Expr::Num(n))
            }
            Tok::Str(a) => {
                self.avanca();
                Ok(Expr::Str(a))
            }
            Tok::Regex(c, f) => {
                self.avanca();
                Ok(Expr::Regex(c, f))
            }
            Tok::Tpl(..) => {
                let antes = self.sem_in;
                self.sem_in = false;
                let (pedacos, exprs) = self.template_marcado()?;
                self.sem_in = antes;
                if pedacos.iter().any(|(_, c)| *c == u32::MAX) {
                    return self.erro("escape inválido num template");
                }
                Ok(Expr::Tpl(pedacos.into_iter().map(|(_, c)| c).collect(), exprs))
            }
            Tok::Privado(a) => {
                // `#x in obj`
                self.avanca();
                Ok(Expr::Str(a))
            }
            Tok::S(P::ParenA) => {
                self.avanca();
                let antes = self.sem_in;
                self.sem_in = false;
                let e = self.expressao()?;
                self.sem_in = antes;
                self.espera(P::ParenF)?;
                // (a?.b).c: a cadeia acaba aqui. `(a, b) = 1` etc. são erros que nem olhamos.
                Ok(e)
            }
            Tok::S(P::ColA) => {
                self.avanca();
                let antes = self.sem_in;
                self.sem_in = false;
                let mut v = Vec::new();
                while !self.e_s(P::ColF) {
                    if self.e_s(P::Virgula) {
                        self.avanca();
                        v.push(Expr::Buraco);
                        continue;
                    }
                    if self.aceita(P::Reticencias) {
                        v.push(Expr::Espalhar(Box::new(self.atribuicao()?)));
                    } else {
                        v.push(self.atribuicao()?);
                    }
                    if !self.e_s(P::ColF) {
                        self.espera(P::Virgula)?;
                    }
                }
                self.avanca();
                self.sem_in = antes;
                Ok(Expr::Array(v))
            }
            Tok::S(P::ChaveA) => self.objeto(),
            Tok::Nome(a) => match a {
                K_FUNCTION => self.expr_funcao(false, inicio),
                K_ASYNC if self.proximo_tok(1) == Tok::Nome(K_FUNCTION) && !self.f[self.i + 1].nl => {
                    self.avanca();
                    self.expr_funcao(true, inicio)
                }
                K_CLASS => {
                    self.avanca();
                    let nome = match self.t() {
                        Tok::Nome(n) if !reservada(n) => {
                            self.avanca();
                            n
                        }
                        _ => K_VAZIO,
                    };
                    Ok(Expr::Classe(Box::new(self.classe(nome, inicio)?)))
                }
                K_THIS => {
                    self.avanca();
                    let e = self.uso(K_THIS_VAR);
                    Ok(Expr::Nome(K_THIS_VAR, e))
                }
                K_NULL => {
                    self.avanca();
                    Ok(Expr::Nulo)
                }
                K_TRUE => {
                    self.avanca();
                    Ok(Expr::Bool(true))
                }
                K_FALSE => {
                    self.avanca();
                    Ok(Expr::Bool(false))
                }
                K_NEW => self.novo(),
                K_IMPORT => self.erro("módulos (import) ainda não são aceitos"),
                _ if reservada(a) => self.erro("palavra reservada inesperada"),
                _ => {
                    self.avanca();
                    let e = self.uso(a);
                    Ok(Expr::Nome(a, e))
                }
            },
            _ => self.erro("expressão inesperada"),
        }
    }

    fn expr_funcao(&mut self, assincrona: bool, inicio: u32) -> R<Expr> {
        self.avanca();
        let gerador = self.aceita(P::Vezes);
        let (nome, proprio) = match self.t() {
            Tok::Nome(n) if !reservada(n) => {
                self.avanca();
                (n, true)
            }
            _ => (K_VAZIO, false),
        };
        Ok(Expr::Funcao(Box::new(self.funcao(nome, assincrona, gerador, false, inicio, proprio)?)))
    }
}

fn no_simples(escopo: u32, corpo: Vec<Stmt>, metodo: bool) -> NoFuncao {
    NoFuncao {
        nome: K_VAZIO,
        escopo,
        params: Vec::new(),
        corpo,
        seta: false,
        assincrona: false,
        gerador: false,
        metodo,
        construtor: false,
        derivada: false,
        estrito: true,
        fonte: (0, 0),
    }
}

fn nome_da_chave(c: &Chave) -> Txt {
    match c {
        Chave::Nome(a) => *a,
        Chave::Computada(_) => K_VAZIO,
    }
}

/// `x = function() {}` dá o nome "x" à função.
fn dar_nome(v: &mut Expr, c: &Chave) {
    let Chave::Nome(a) = c else { return };
    match v {
        Expr::Funcao(f) if f.nome == K_VAZIO => f.nome = *a,
        Expr::Classe(c) if c.nome == K_VAZIO => {
            c.nome = *a;
            c.construtor.nome = *a;
        }
        _ => {}
    }
}

/// Depois de ler tudo: quem é capturado por quem, e quais pseudovariáveis existem.
fn resolver_usos(escopos: &mut [Escopo], usos: &[(Txt, u32)]) {
    for &(nome, onde) in usos {
        let mut s = onde;
        let mut cruzou = false;
        loop {
            let esc = &mut escopos[s as usize];
            if let Some(d) = esc.decls.iter_mut().find(|d| d.nome == nome) {
                d.usada = true;
                d.capturada |= cruzou;
                break;
            }
            let pseudo = matches!(nome, K_THIS_VAR | K_FN_VAR | K_NT_VAR | K_HOME_VAR) || (nome == K_ARGUMENTS && s != ESCOPO_GLOBAL);
            if pseudo && esc.funcao && !esc.seta {
                esc.decls.push(DeclVar { nome, tipo: TipoVar::Pseudo, capturada: cruzou, usada: true, slot: 0 });
                break;
            }
            if s == ESCOPO_GLOBAL {
                break;
            }
            if esc.funcao {
                cruzou = true;
            }
            s = esc.pai;
        }
    }
}

pub fn ler_programa(fonte: &str, n: &mut Interno) -> Result<Programa, String> {
    let fichas = ler_fichas(fonte, n)?;
    let a_use_strict = n.atomo("use strict");
    let a_args = n.atomo("%args");
    let mut l = Leitor {
        f: fichas.lista,
        par: fichas.par,
        i: 0,
        n,
        fonte,
        escopos: vec![Escopo { pai: 0, funcao: true, seta: false, decls: Vec::new() }],
        usos: Vec::new(),
        globais: Vec::new(),
        atual: ESCOPO_GLOBAL,
        func: ESCOPO_GLOBAL,
        gerador: false,
        assincrona: false,
        estrito: false,
        sem_in: false,
        prof: 0,
        a_use_strict,
        a_args,
    };
    l.diretivas();
    let mut corpo = Vec::new();
    while l.t() != Tok::Fim {
        corpo.push(l.comando()?);
    }
    let estrito = l.estrito;
    let globais = core::mem::take(&mut l.globais);
    let usos = core::mem::take(&mut l.usos);
    let mut escopos = core::mem::take(&mut l.escopos);
    resolver_usos(&mut escopos, &usos);
    Ok(Programa { corpo, globais, escopos, estrito, fonte: alloc::rc::Rc::from(fonte) })
}
