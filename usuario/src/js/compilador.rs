//! O compilador: da árvore para o bytecode (uma máquina de pilha).
//!
//! Variáveis locais moram em posições fixas da pilha (`LerLocal`). As que
//! alguma função interna captura moram numa "caixa" no heap, e a posição
//! guarda a caixa (`LerCaixa`); a função interna leva a caixa consigo
//! (`LerUp`). Quem não é local é global (`LerGlobal`).

use super::lexico::*;
use super::sintaxe::*;
use super::vm::{Interno, Txt};
use alloc::rc::Rc;
use alloc::string::String;
use alloc::vec::Vec;

#[derive(Clone, Copy, Debug)]
pub enum Op {
    Num(u32),
    Int(i32),
    Texto(Txt),
    Undef,
    Nulo,
    Verdadeiro,
    Falso,
    Pop,
    Dup,
    /// a b → a b a b
    Dup2,
    /// a b → b a
    Troca,
    /// a b → a b a
    Sobre,
    /// a b c → c a b
    Rot3,
    /// a b c → b c a
    Rot3Inv,
    /// a b c d → d a b c
    Rot4,

    LerLocal(u16),
    EscLocal(u16),
    LerLocalTdz(u16),
    EscLocalTdz(u16),
    LimparLocal(u16),
    LerCaixa(u16),
    EscCaixa(u16),
    IniCaixa(u16),
    /// Uma caixa nova na posição (vazia: acesso antes de iniciar é erro).
    NovaCaixa(u16),
    NovaCaixaUndef(u16),
    /// O valor da posição vai para uma caixa (um parâmetro capturado).
    Empacotar(u16),
    /// Uma caixa nova com o mesmo valor (uma iteração nova de um `for (let...)`).
    CopiarCaixa(u16),
    LerUp(u16),
    EscUp(u16),
    IniUp(u16),
    LerGlobal(Txt),
    LerGlobalTalvez(Txt),
    EscGlobal(Txt),
    EscGlobalEstrito(Txt),
    DefVarGlobal(Txt),
    /// Cria o `var` global com o valor do topo (funções).
    DefFuncGlobal(Txt),
    DefLexGlobal(Txt, bool),
    ErroConst(Txt),

    ObterProp(Txt),
    /// obj → fn obj
    ObterMetodo(Txt),
    /// obj val → val
    DefProp(Txt),
    /// obj chave → val
    ObterElem,
    /// obj chave → fn obj
    ObterMetodoElem,
    /// obj chave val → val
    DefElem,
    ApagarProp(Txt),
    ApagarElem,
    /// this home → val
    ObterSuper(Txt),
    /// this home chave → val
    ObterSuperElem,
    /// this home val → val
    DefSuper(Txt),
    /// this home chave val → val
    DefSuperElem,

    Soma,
    Sub,
    Mul,
    Div,
    Mod,
    Pot,
    BitE,
    BitOu,
    BitXor,
    Shl,
    Shr,
    Ushr,
    Neg,
    Pos,
    Nao,
    BitNao,
    Typeof,
    Igual,
    Dif,
    Igual3,
    Dif3,
    Menor,
    MenorIgual,
    Maior,
    MaiorIgual,
    Em,
    InstanciaDe,
    /// v → v+1 (já convertido para número)
    Incr,
    Decr,
    ParaNumero,
    ParaChave,

    Salto(u32),
    SaltoFalso(u32),
    SaltoVerdadeiro(u32),
    SaltoFalsoMantem(u32),
    SaltoVerdadeiroMantem(u32),
    SaltoNaoNuloMantem(u32),
    /// Se o topo é null/undefined: tira e salta (a cadeia `?.`).
    SaltoNuloPop(u32),
    /// fn this: se fn é null/undefined, tira os dois e salta (`a.b?.()`).
    SaltoNuloChamada(u32),
    /// Se o topo não é undefined, salta (e fica); senão, tira.
    SaltoNaoUndef(u32),
    Chamar(u16),
    ChamarEsp,
    Novo(u16),
    NovoEsp,
    ChamarSuper(u16),
    ChamarSuperEsp,
    /// this fn → this: os campos da classe.
    IniciarCampos,
    Retornar,
    RetornarUndef,
    Lancar,
    Tentar(u32),
    FimTentar,
    Gosub(u32),
    RetGosub,

    /// obj → iter next
    IterInicio,
    IterInicioAsync,
    /// iter next → iter next valor; ou salta (quando acaba)
    IterProximo(u32),
    /// iter next → iter next valor (undefined depois do fim)
    IterPasso,
    /// iter next → iter next array
    IterResto,
    /// iter next → (chama return() se ainda não acabou)
    IterFechar,
    /// iter next exc → relança, fechando antes
    IterFecharErro,
    /// iter next → iter next resultado (a promessa de um for await)
    IterChamarProximo,
    /// resultado → valor; ou salta se done (tirando o resultado)
    IterDesempacota(u32),
    /// obj → chaves 0
    ForInInicio,
    ChecarObj,

    NovoObjeto,
    NovoArray(u32),
    ArrayPush,
    ArrayEspalhar,
    ArrayBuraco,
    /// obj val → obj
    ObjProp(Txt),
    /// obj chave val → obj
    ObjElem,
    ObjEspalhar,
    /// val k1..kn → val resto
    ObjResto(u16),
    /// alvo chave fn → alvo. bit0-1: 0 método, 1 get, 2 set; bit2: enumerável
    DefMetodo(u8),
    /// ctor chave fn|undef → ctor
    DefCampo,
    /// ctor chave fn|undef → ctor (roda agora); chave vazia = bloco estático
    CampoEstatico,
    /// [pai] → ctor proto
    CriarClasse(u32, bool),
    Closure(u32),
    Regex(Txt, Txt),
    /// n valores → um texto
    Template(u16),
    TemplateObj(u32),
    LerThis,
    LerCallee,
    LerNovoAlvo,
    LerHome,
    Argumentos,
    Resto(u16),
    /// Vira gerador ou função assíncrona: guarda o quadro e devolve.
    Gerador,
    /// v → enviado retorno? (o gerador foi retomado com return?)
    Yield,
    Await,
    /// Como Await, mas só se o valor for uma promessa (os itens de um `for await`).
    AwaitSePromessa,
    /// v → (delegação): iter next recebido → ...
    YieldDelegado,
    /// O próximo passo de um `yield*` (veja `compilar_yield_delegado`).
    DelegadoPasso(u32),
}

pub struct Funcao {
    pub codigo: Vec<Op>,
    pub nums: Vec<f64>,
    pub funcoes: Vec<Rc<Funcao>>,
    pub templates: Vec<Vec<(Txt, Txt)>>,
    pub nome: Txt,
    pub n_params: u16,
    pub n_locais: u16,
    pub comprimento: u16,
    pub ups: Vec<(bool, u16)>,
    pub seta: bool,
    pub assincrona: bool,
    pub gerador: bool,
    pub metodo: bool,
    pub construtor: bool,
    pub derivada: bool,
    pub estrito: bool,
    pub usa_args: bool,
    pub fonte: Rc<str>,
    pub trecho: (u32, u32),
}

enum Ctl {
    Laco { rotulos: Vec<Txt>, quebra: u32, continua: Option<u32> },
    /// Um bloco rotulado: `a: { break a; }`
    Rotulado { rotulo: Txt, quebra: u32 },
    Tentar,
    Finalmente(u32),
    /// Dentro do corpo de um finally (há um endereço de volta na pilha).
    NoFinally,
    /// Um for-of (iter e next na pilha, com um handler ativo).
    Iter { assincrono: bool },
    /// Um for-in (só os dois valores na pilha).
    Valores2,
}

struct EstadoFunc {
    codigo: Vec<Op>,
    nums: Vec<f64>,
    funcoes: Vec<Rc<Funcao>>,
    templates: Vec<Vec<(Txt, Txt)>>,
    ups: Vec<(bool, u16)>,
    chaves_ups: Vec<(u32, usize)>,
    rotulos: Vec<u32>,
    prox_slot: u16,
    max_slot: u16,
    ctl: Vec<Ctl>,
    /// Os rótulos do comando seguinte (`a: for ...`).
    rotulos_pendentes: Vec<Txt>,
    estrito: bool,
    gerador: bool,
    assincrona: bool,
    derivada: bool,
    principal: bool,
    /// Onde resolver `%this` no fim de um construtor derivado.
    escopo: u32,
}

enum Var {
    Local { slot: u16, tdz: bool, tipo: TipoVar },
    Caixa { slot: u16, tipo: TipoVar },
    Up { idx: u16, tipo: TipoVar },
    Global,
}

pub struct Compilador<'a> {
    escopos: Vec<Escopo>,
    fs: Vec<EstadoFunc>,
    fonte: Rc<str>,
    n: &'a mut Interno,
}

type R<T> = Result<T, String>;

pub fn compilar(p: Programa, n: &mut Interno) -> R<Rc<Funcao>> {
    let mut c = Compilador { escopos: p.escopos, fs: Vec::new(), fonte: p.fonte.clone(), n };
    c.comecar_func(p.estrito, false, false, false, true, ESCOPO_GLOBAL);
    // A posição 0 guarda o valor do último comando (o resultado do eval).
    c.f().prox_slot = 1;
    c.f().max_slot = 1;
    c.entrar_pseudos(ESCOPO_GLOBAL);
    for a in &p.globais {
        c.e(Op::DefVarGlobal(*a));
    }
    c.içar_funcoes(&p.corpo)?;
    for s in &p.corpo {
        c.comando(s)?;
    }
    c.e(Op::LerLocal(0));
    c.e(Op::Retornar);
    let tam = p.fonte.len() as u32;
    let mut f = c.terminar_func(K_VAZIO, 0, 0)?;
    f.trecho = (0, tam);
    Ok(Rc::new(f))
}

impl Compilador<'_> {
    fn f(&mut self) -> &mut EstadoFunc {
        self.fs.last_mut().unwrap()
    }

    fn e(&mut self, op: Op) {
        self.f().codigo.push(op);
    }

    fn rotulo(&mut self) -> u32 {
        let f = self.f();
        f.rotulos.push(u32::MAX);
        f.rotulos.len() as u32 - 1
    }

    fn fixar(&mut self, r: u32) {
        let f = self.f();
        f.rotulos[r as usize] = f.codigo.len() as u32;
    }

    fn comecar_func(&mut self, estrito: bool, gerador: bool, assincrona: bool, derivada: bool, principal: bool, escopo: u32) {
        self.fs.push(EstadoFunc {
            codigo: Vec::new(),
            nums: Vec::new(),
            funcoes: Vec::new(),
            templates: Vec::new(),
            ups: Vec::new(),
            chaves_ups: Vec::new(),
            rotulos: Vec::new(),
            prox_slot: 0,
            max_slot: 0,
            ctl: Vec::new(),
            rotulos_pendentes: Vec::new(),
            estrito,
            gerador,
            assincrona,
            derivada,
            principal,
            escopo,
        });
    }

    fn terminar_func(&mut self, nome: Txt, n_params: u16, comprimento: u16) -> R<Funcao> {
        let mut f = self.fs.pop().unwrap();
        let rot = core::mem::take(&mut f.rotulos);
        for op in f.codigo.iter_mut() {
            match op {
                Op::Salto(r) | Op::SaltoFalso(r) | Op::SaltoVerdadeiro(r) | Op::SaltoFalsoMantem(r) | Op::SaltoVerdadeiroMantem(r)
                | Op::SaltoNaoNuloMantem(r) | Op::SaltoNuloPop(r) | Op::SaltoNuloChamada(r) | Op::SaltoNaoUndef(r) | Op::Tentar(r)
                | Op::Gosub(r) | Op::IterProximo(r) | Op::IterDesempacota(r) | Op::DelegadoPasso(r) => *r = rot[*r as usize],
                _ => {}
            }
        }
        Ok(Funcao {
            codigo: f.codigo,
            nums: f.nums,
            funcoes: f.funcoes,
            templates: f.templates,
            nome,
            n_params,
            n_locais: f.max_slot,
            comprimento,
            ups: f.ups,
            seta: false,
            assincrona: f.assincrona,
            gerador: f.gerador,
            metodo: false,
            construtor: false,
            derivada: f.derivada,
            estrito: f.estrito,
            usa_args: false,
            fonte: self.fonte.clone(),
            trecho: (0, 0),
        })
    }

    // ------------------------------------------------------------ variáveis

    fn novo_slot(&mut self) -> R<u16> {
        let f = self.f();
        let s = f.prox_slot;
        if s == u16::MAX {
            return Err(String::from("variáveis demais numa função"));
        }
        f.prox_slot += 1;
        f.max_slot = f.max_slot.max(f.prox_slot);
        Ok(s)
    }

    /// Dá posições às declarações de um escopo e prepara as caixas.
    fn entrar_escopo(&mut self, e: u32) -> R<u16> {
        let antes = self.f().prox_slot;
        if e == ESCOPO_GLOBAL {
            return Ok(antes);
        }
        for i in 0..self.escopos[e as usize].decls.len() {
            let slot = self.novo_slot()?;
            let d = &mut self.escopos[e as usize].decls[i];
            d.slot = slot;
            let (cap, tipo) = (d.capturada, d.tipo);
            match (cap, tipo) {
                (true, TipoVar::Var) => self.e(Op::NovaCaixaUndef(slot)),
                (true, _) => self.e(Op::NovaCaixa(slot)),
                (false, TipoVar::Let | TipoVar::Const) => self.e(Op::LimparLocal(slot)),
                _ => {}
            }
        }
        Ok(antes)
    }

    fn sair_escopo(&mut self, antes: u16) {
        self.f().prox_slot = antes;
    }

    fn resolver(&mut self, nome: Txt, onde: u32) -> Var {
        let mut s = onde;
        let mut cruzadas = 0usize;
        loop {
            let esc = &self.escopos[s as usize];
            if let Some(i) = esc.decls.iter().position(|d| d.nome == nome) {
                let d = &esc.decls[i];
                let (slot, tipo, capt) = (d.slot, d.tipo, d.capturada);
                if cruzadas == 0 {
                    let tdz = matches!(tipo, TipoVar::Let | TipoVar::Const) || nome == K_THIS_VAR;
                    return if capt { Var::Caixa { slot, tipo } } else { Var::Local { slot, tdz, tipo } };
                }
                let nivel = self.fs.len() - 1;
                let idx = self.up_em(nivel, cruzadas, s, i);
                return Var::Up { idx, tipo };
            }
            if s == ESCOPO_GLOBAL {
                return Var::Global;
            }
            if esc.funcao {
                cruzadas += 1;
            }
            s = esc.pai;
        }
    }

    fn up_em(&mut self, nivel: usize, cruzadas: usize, escopo: u32, decl: usize) -> u16 {
        if let Some(i) = self.fs[nivel].chaves_ups.iter().position(|k| *k == (escopo, decl)) {
            return i as u16;
        }
        let entrada = if cruzadas == 1 {
            (true, self.escopos[escopo as usize].decls[decl].slot)
        } else {
            (false, self.up_em(nivel - 1, cruzadas - 1, escopo, decl))
        };
        let f = &mut self.fs[nivel];
        f.ups.push(entrada);
        f.chaves_ups.push((escopo, decl));
        f.ups.len() as u16 - 1
    }

    fn ler_var(&mut self, nome: Txt, onde: u32) {
        let op = match self.resolver(nome, onde) {
            Var::Local { slot, tdz: true, .. } => Op::LerLocalTdz(slot),
            Var::Local { slot, .. } => Op::LerLocal(slot),
            Var::Caixa { slot, .. } => Op::LerCaixa(slot),
            Var::Up { idx, .. } => Op::LerUp(idx),
            Var::Global => Op::LerGlobal(nome),
        };
        self.e(op);
    }

    /// Escreve o topo da pilha na variável (e tira).
    fn esc_var(&mut self, nome: Txt, onde: u32) {
        let estrito = self.f().estrito;
        match self.resolver(nome, onde) {
            Var::Local { tipo: TipoVar::Const, .. } | Var::Caixa { tipo: TipoVar::Const, .. } | Var::Up { tipo: TipoVar::Const, .. } => self.e(Op::ErroConst(nome)),
            Var::Local { tipo: TipoVar::NomeFuncao, .. } | Var::Caixa { tipo: TipoVar::NomeFuncao, .. } | Var::Up { tipo: TipoVar::NomeFuncao, .. } => self.e(Op::Pop),
            Var::Local { slot, tdz: true, .. } => self.e(Op::EscLocalTdz(slot)),
            Var::Local { slot, .. } => self.e(Op::EscLocal(slot)),
            Var::Caixa { slot, .. } => self.e(Op::EscCaixa(slot)),
            Var::Up { idx, .. } => self.e(Op::EscUp(idx)),
            Var::Global => self.e(if estrito { Op::EscGlobalEstrito(nome) } else { Op::EscGlobal(nome) }),
        }
    }

    /// A primeira escrita (`let x = 1`): pode, mesmo em `const`.
    fn ini_var(&mut self, nome: Txt, onde: u32, global: Option<bool>) {
        match self.resolver(nome, onde) {
            Var::Local { slot, .. } => self.e(Op::EscLocal(slot)),
            Var::Caixa { slot, .. } => self.e(Op::IniCaixa(slot)),
            Var::Up { idx, .. } => self.e(Op::IniUp(idx)),
            Var::Global => match global {
                Some(constante) => self.e(Op::DefLexGlobal(nome, constante)),
                None => self.e(Op::EscGlobal(nome)),
            },
        }
    }

    fn entrar_pseudos(&mut self, e: u32) {
        let mut slots = Vec::new();
        for i in 0..self.escopos[e as usize].decls.len() {
            let d = &self.escopos[e as usize].decls[i];
            if d.tipo == TipoVar::Pseudo {
                slots.push(i);
            }
        }
        for i in slots {
            let slot = self.novo_slot().unwrap_or(0);
            let d = &mut self.escopos[e as usize].decls[i];
            d.slot = slot;
            let (nome, capt) = (d.nome, d.capturada);
            if capt {
                self.e(Op::NovaCaixa(slot));
            }
            let origem = match nome {
                K_THIS_VAR => {
                    if self.f().derivada {
                        // `this` só existe depois do super().
                        if !capt {
                            self.e(Op::LimparLocal(slot));
                        }
                        continue;
                    }
                    Op::LerThis
                }
                K_FN_VAR => Op::LerCallee,
                K_NT_VAR => Op::LerNovoAlvo,
                K_HOME_VAR => Op::LerHome,
                _ => Op::Argumentos,
            };
            self.e(origem);
            self.e(if capt { Op::IniCaixa(slot) } else { Op::EscLocal(slot) });
        }
    }

    // ------------------------------------------------------------ funções

    fn funcao(&mut self, no: &NoFuncao) -> R<u32> {
        let escopo = no.escopo;
        self.comecar_func(no.estrito, no.gerador, no.assincrona, no.derivada, false, escopo);
        // Os parâmetros primeiro: posições 0..n, na ordem.
        let n_params = no.params.iter().filter(|p| !p.resto).count() as u16;
        for _ in 0..no.params.len() {
            self.novo_slot()?;
        }
        let mut comprimento = 0;
        let mut contando = true;
        for p in &no.params {
            if p.inicial.is_some() || p.resto {
                contando = false;
            }
            if contando {
                comprimento += 1;
            }
        }
        // Os parâmetros que são nomes simples ficam na posição deles.
        for (i, p) in no.params.iter().enumerate() {
            if let Padrao::Nome(a, _) = p.padrao {
                if let Some(d) = self.escopos[escopo as usize].decls.iter_mut().find(|d| d.nome == a) {
                    d.slot = i as u16;
                }
            }
        }
        // As outras declarações do escopo da função.
        let simples: Vec<Txt> = no.params.iter().filter_map(|p| if let Padrao::Nome(a, _) = p.padrao { Some(a) } else { None }).collect();
        let mut usa_args = no.params.iter().any(|p| p.resto);
        for i in 0..self.escopos[escopo as usize].decls.len() {
            let (nome, tipo, capt, usada) = {
                let d = &self.escopos[escopo as usize].decls[i];
                (d.nome, d.tipo, d.capturada, d.usada)
            };
            if tipo == TipoVar::Pseudo {
                if nome == K_ARGUMENTS {
                    usa_args = true;
                }
                continue;
            }
            if simples.contains(&nome) {
                if capt {
                    let slot = self.escopos[escopo as usize].decls[i].slot;
                    self.e(Op::Empacotar(slot));
                }
                continue;
            }
            let slot = self.novo_slot()?;
            self.escopos[escopo as usize].decls[i].slot = slot;
            match (capt, tipo) {
                (true, TipoVar::Var | TipoVar::Param) => self.e(Op::NovaCaixaUndef(slot)),
                (true, _) => self.e(Op::NovaCaixa(slot)),
                (false, TipoVar::Let | TipoVar::Const) => self.e(Op::LimparLocal(slot)),
                _ => {}
            }
            if tipo == TipoVar::NomeFuncao && usada {
                self.e(Op::LerCallee);
                self.e(if capt { Op::IniCaixa(slot) } else { Op::EscLocal(slot) });
            }
        }
        self.entrar_pseudos(escopo);
        // Valores iniciais, desestruturação e resto.
        for (i, p) in no.params.iter().enumerate() {
            let i16_ = i as u16;
            if p.resto {
                self.e(Op::Resto(i16_));
            } else if let (Padrao::Nome(a, onde), Some(ini)) = (&p.padrao, &p.inicial) {
                let l = self.rotulo();
                self.ler_var(*a, *onde);
                self.e(Op::SaltoNaoUndef(l));
                self.expr(ini)?;
                self.fixar(l);
                self.ini_var(*a, *onde, None);
                continue;
            } else if matches!(p.padrao, Padrao::Nome(..)) {
                continue;
            } else {
                self.e(Op::LerLocal(i16_));
                if let Some(ini) = &p.inicial {
                    let l = self.rotulo();
                    self.e(Op::SaltoNaoUndef(l));
                    self.expr(ini)?;
                    self.fixar(l);
                }
            }
            self.ligar_padrao(&p.padrao, Liga::Inicia(None))?;
        }
        if no.gerador || no.assincrona {
            self.e(Op::Gerador);
        }
        self.içar_funcoes(&no.corpo)?;
        for s in &no.corpo {
            self.comando(s)?;
        }
        if no.derivada {
            self.ler_var(K_THIS_VAR, escopo);
            self.e(Op::Retornar);
        } else {
            self.e(Op::RetornarUndef);
        }
        let mut f = self.terminar_func(no.nome, n_params, comprimento)?;
        f.seta = no.seta;
        f.metodo = no.metodo;
        f.construtor = no.construtor;
        f.usa_args = usa_args;
        f.trecho = no.fonte;
        let pf = self.f();
        pf.funcoes.push(Rc::new(f));
        Ok(pf.funcoes.len() as u32 - 1)
    }

    /// As funções declaradas num bloco existem desde o começo dele.
    fn içar_funcoes(&mut self, stmts: &[Stmt]) -> R<()> {
        for s in stmts {
            let mut s = s;
            while let Stmt::Rotulo(_, dentro) = s {
                s = dentro;
            }
            if let Stmt::Funcao(no, onde) = s {
                let i = self.funcao(no)?;
                self.e(Op::Closure(i));
                match self.resolver(no.nome, *onde) {
                    Var::Global => self.e(Op::DefFuncGlobal(no.nome)),
                    _ => self.ini_var(no.nome, *onde, None),
                }
            }
        }
        Ok(())
    }

    // ------------------------------------------------------------ comandos

    fn bloco(&mut self, b: &Bloco) -> R<()> {
        let antes = self.entrar_escopo(b.escopo)?;
        self.içar_funcoes(&b.stmts)?;
        for s in &b.stmts {
            self.comando(s)?;
        }
        self.sair_escopo(antes);
        Ok(())
    }

    fn comando(&mut self, s: &Stmt) -> R<()> {
        match s {
            Stmt::Expr(e) => {
                self.expr(e)?;
                if self.f().principal && self.f().ctl.is_empty() {
                    self.e(Op::EscLocal(0));
                } else {
                    self.e(Op::Pop);
                }
            }
            Stmt::Decl(tipo, itens) => {
                for (p, ini) in itens {
                    match ini {
                        Some(i) => self.expr(i)?,
                        None if *tipo == TipoDecl::Var => continue,
                        None => self.e(Op::Undef),
                    }
                    let liga = match tipo {
                        TipoDecl::Var => Liga::Atribui,
                        TipoDecl::Let => Liga::Inicia(Some(false)),
                        TipoDecl::Const => Liga::Inicia(Some(true)),
                    };
                    self.ligar_padrao(p, liga)?;
                }
            }
            Stmt::Funcao(..) | Stmt::Vazio => {}
            Stmt::Classe(c, onde) => {
                self.classe(c)?;
                self.ini_var(c.nome, *onde, Some(false));
            }
            Stmt::Retorno(e) => {
                match e {
                    Some(e) => self.expr(e)?,
                    None => self.e(Op::Undef),
                }
                if self.f().assincrona && self.f().gerador {
                    self.e(Op::Await);
                }
                self.retorno()?;
            }
            Stmt::Se(c, a, b) => {
                let senao = self.rotulo();
                self.expr(c)?;
                self.e(Op::SaltoFalso(senao));
                self.comando(a)?;
                if let Some(b) = b {
                    let fim = self.rotulo();
                    self.e(Op::Salto(fim));
                    self.fixar(senao);
                    self.comando(b)?;
                    self.fixar(fim);
                } else {
                    self.fixar(senao);
                }
            }
            Stmt::Bloco(b) => self.bloco(b)?,
            Stmt::Para { escopo, inicio, cond, passo, corpo } => self.para(*escopo, inicio.as_deref(), cond.as_ref(), passo.as_ref(), corpo)?,
            Stmt::ParaEm { escopo, alvo, obj, corpo, de, espera } => self.para_em(*escopo, alvo, obj, corpo, *de, *espera)?,
            Stmt::Enquanto(c, corpo) => {
                let (inicio, fim) = (self.rotulo(), self.rotulo());
                self.fixar(inicio);
                self.expr(c)?;
                self.e(Op::SaltoFalso(fim));
                self.empilhar_laco(fim, Some(inicio));
                self.comando(corpo)?;
                self.f().ctl.pop();
                self.e(Op::Salto(inicio));
                self.fixar(fim);
            }
            Stmt::Faca(corpo, c) => {
                let (inicio, teste, fim) = (self.rotulo(), self.rotulo(), self.rotulo());
                self.fixar(inicio);
                self.empilhar_laco(fim, Some(teste));
                self.comando(corpo)?;
                self.f().ctl.pop();
                self.fixar(teste);
                self.expr(c)?;
                self.e(Op::SaltoVerdadeiro(inicio));
                self.fixar(fim);
            }
            Stmt::Quebra(r) => self.saltar(*r, true)?,
            Stmt::Continua(r) => self.saltar(*r, false)?,
            Stmt::Lancar(e) => {
                self.expr(e)?;
                self.e(Op::Lancar);
            }
            Stmt::Tentar { bloco, param, captura, finalmente } => self.tentar(bloco, param.as_ref(), captura.as_ref(), finalmente.as_ref())?,
            Stmt::Trocar { escopo, valor, casos } => self.trocar(*escopo, valor, casos)?,
            Stmt::Rotulo(r, dentro) => {
                if matches!(**dentro, Stmt::Para { .. } | Stmt::ParaEm { .. } | Stmt::Enquanto(..) | Stmt::Faca(..) | Stmt::Rotulo(..)) {
                    self.f().rotulos_pendentes.push(*r);
                    self.comando(dentro)?;
                } else {
                    let fim = self.rotulo();
                    self.f().ctl.push(Ctl::Rotulado { rotulo: *r, quebra: fim });
                    self.comando(dentro)?;
                    self.f().ctl.pop();
                    self.fixar(fim);
                }
            }
        }
        Ok(())
    }

    fn empilhar_laco(&mut self, quebra: u32, continua: Option<u32>) {
        let rotulos = core::mem::take(&mut self.f().rotulos_pendentes);
        self.f().ctl.push(Ctl::Laco { rotulos, quebra, continua });
    }

    /// Emite o que precisa ser desfeito ao sair das `n` entradas de cima de
    /// `ctl` (fechar iteradores, rodar os finally...).
    fn desfazer_ctl(&mut self, ate: usize) {
        let n = self.f().ctl.len();
        let mut i = n;
        while i > ate {
            i -= 1;
            enum A {
                Nada,
                Tentar,
                Fin(u32),
                NoFin,
                Iter(bool),
                V2,
            }
            let a = match &self.f().ctl[i] {
                Ctl::Tentar => A::Tentar,
                Ctl::Finalmente(l) => A::Fin(*l),
                Ctl::NoFinally => A::NoFin,
                Ctl::Iter { assincrono } => A::Iter(*assincrono),
                Ctl::Valores2 => A::V2,
                _ => A::Nada,
            };
            match a {
                A::Tentar => self.e(Op::FimTentar),
                A::Fin(l) => {
                    self.e(Op::FimTentar);
                    self.e(Op::Gosub(l));
                }
                A::NoFin => self.e(Op::Pop),
                A::Iter(_) => {
                    self.e(Op::FimTentar);
                    self.e(Op::IterFechar);
                }
                A::V2 => {
                    self.e(Op::Pop);
                    self.e(Op::Pop);
                }
                A::Nada => {}
            }
        }
    }

    /// Um `return` com o valor no topo: passa pelos finally e iteradores. O
    /// valor fica embaixo do que eles usam: guardamos num local temporário.
    fn retorno(&mut self) -> R<()> {
        let f = self.f();
        let precisa = f.ctl.iter().any(|c| !matches!(c, Ctl::Laco { .. } | Ctl::Rotulado { .. }));
        if !precisa {
            if self.f().derivada {
                self.retorno_derivado();
            }
            self.e(Op::Retornar);
            return Ok(());
        }
        let antes = self.f().prox_slot;
        let tmp = self.novo_slot()?;
        self.e(Op::EscLocal(tmp));
        self.desfazer_ctl(0);
        self.e(Op::LerLocal(tmp));
        self.f().prox_slot = antes;
        if self.f().derivada {
            self.retorno_derivado();
        }
        self.e(Op::Retornar);
        Ok(())
    }

    fn retorno_derivado(&mut self) {
        // `return x` num construtor derivado: se x não é objeto, vale o this.
        // Só undefined é comum: `return;`.
        let l = self.rotulo();
        self.e(Op::SaltoNaoUndef(l));
        let e = self.f().escopo;
        self.ler_var(K_THIS_VAR, e);
        self.fixar(l);
    }

    fn saltar(&mut self, rotulo: Option<Txt>, quebra: bool) -> R<()> {
        let n = self.f().ctl.len();
        let mut i = n;
        let mut achou = None;
        while i > 0 {
            i -= 1;
            match &self.f().ctl[i] {
                Ctl::Laco { rotulos, quebra: q, continua } => {
                    let casa = rotulo.is_none_or(|r| rotulos.contains(&r));
                    if casa {
                        if quebra {
                            achou = Some((i, *q));
                            break;
                        } else if let Some(c) = continua {
                            achou = Some((i, *c));
                            break;
                        }
                    }
                }
                Ctl::Rotulado { rotulo: r, quebra: q } if quebra && Some(*r) == rotulo => {
                    achou = Some((i, *q));
                    break;
                }
                _ => {}
            }
        }
        let Some((nivel, destino)) = achou else {
            return Err(String::from(if quebra { "break fora de um laço" } else { "continue fora de um laço" }));
        };
        self.desfazer_ctl(nivel + 1);
        self.e(Op::Salto(destino));
        Ok(())
    }

    fn para(&mut self, escopo: u32, inicio: Option<&Stmt>, cond: Option<&Expr>, passo: Option<&Expr>, corpo: &Stmt) -> R<()> {
        let antes = self.entrar_escopo(escopo)?;
        if let Some(i) = inicio {
            match i {
                Stmt::Expr(e) => {
                    self.expr(e)?;
                    self.e(Op::Pop);
                }
                outro => self.comando(outro)?,
            }
        }
        // As variáveis capturadas ganham uma caixa nova a cada volta.
        let caixas: Vec<u16> = self.escopos[escopo as usize].decls.iter().filter(|d| d.capturada && d.tipo != TipoVar::Var).map(|d| d.slot).collect();
        let (topo, cont, fim) = (self.rotulo(), self.rotulo(), self.rotulo());
        self.fixar(topo);
        if let Some(c) = cond {
            self.expr(c)?;
            self.e(Op::SaltoFalso(fim));
        }
        self.empilhar_laco(fim, Some(cont));
        self.comando(corpo)?;
        self.f().ctl.pop();
        self.fixar(cont);
        for s in caixas {
            self.e(Op::CopiarCaixa(s));
        }
        if let Some(p) = passo {
            self.expr(p)?;
            self.e(Op::Pop);
        }
        self.e(Op::Salto(topo));
        self.fixar(fim);
        self.sair_escopo(antes);
        Ok(())
    }

    fn para_em(&mut self, escopo: u32, alvo: &AlvoPara, obj: &Expr, corpo: &Stmt, de: bool, espera: bool) -> R<()> {
        let antes = self.entrar_escopo(escopo)?;
        self.expr(obj)?;
        let (topo, fim, erro) = (self.rotulo(), self.rotulo(), self.rotulo());
        if !de {
            self.e(Op::ForInInicio);
        } else if espera {
            self.e(Op::IterInicioAsync);
        } else {
            self.e(Op::IterInicio);
        }
        if de {
            self.e(Op::Tentar(erro));
        }
        self.fixar(topo);
        if espera {
            self.e(Op::IterChamarProximo);
            self.e(Op::Await);
            self.e(Op::IterDesempacota(fim));
            self.e(Op::AwaitSePromessa);
        } else {
            self.e(Op::IterProximo(fim));
        }
        // Uma caixa nova por volta para as variáveis capturadas.
        let decls: Vec<(u16, bool)> = self.escopos[escopo as usize].decls.iter().filter(|d| d.tipo != TipoVar::Var).map(|d| (d.slot, d.capturada)).collect();
        for (slot, capt) in decls {
            self.e(if capt { Op::NovaCaixa(slot) } else { Op::LimparLocal(slot) });
        }
        match alvo {
            AlvoPara::Decl(TipoDecl::Var, p) | AlvoPara::Atrib(p) => self.ligar_padrao(p, Liga::Atribui)?,
            AlvoPara::Decl(_, p) => self.ligar_padrao(p, Liga::Inicia(None))?,
        }
        let cont = self.rotulo();
        let quebra = self.rotulo();
        let fim_laco = self.rotulo();
        if de {
            self.f().ctl.push(Ctl::Iter { assincrono: espera });
        } else {
            self.f().ctl.push(Ctl::Valores2);
        }
        self.empilhar_laco(quebra, Some(cont));
        self.comando(corpo)?;
        self.f().ctl.pop();
        self.f().ctl.pop();
        self.fixar(cont);
        self.e(Op::Salto(topo));
        if de {
            // Um `break`: fecha o iterador.
            self.fixar(quebra);
            self.e(Op::FimTentar);
            self.e(Op::IterFechar);
            self.e(Op::Salto(fim_laco));
            // O corpo lançou: fecha o iterador e relança.
            self.fixar(erro);
            self.e(Op::IterFecharErro);
            // O iterador acabou sozinho.
            self.fixar(fim);
            self.e(Op::FimTentar);
            self.e(Op::Pop);
            self.e(Op::Pop);
        } else {
            self.fixar(quebra);
            self.fixar(fim);
            self.e(Op::Pop);
            self.e(Op::Pop);
        }
        self.fixar(fim_laco);
        self.sair_escopo(antes);
        Ok(())
    }

    fn tentar(&mut self, bloco: &Bloco, param: Option<&Padrao>, captura: Option<&Bloco>, finalmente: Option<&Bloco>) -> R<()> {
        let rot_fin = finalmente.map(|_| self.rotulo());
        let rot_exc = self.rotulo();
        if let Some(l) = rot_fin {
            self.e(Op::Tentar(rot_exc));
            self.f().ctl.push(Ctl::Finalmente(l));
        }
        if let Some(c) = captura {
            let (rot_catch, rot_fim) = (self.rotulo(), self.rotulo());
            self.e(Op::Tentar(rot_catch));
            self.f().ctl.push(Ctl::Tentar);
            self.bloco(bloco)?;
            self.f().ctl.pop();
            self.e(Op::FimTentar);
            self.e(Op::Salto(rot_fim));
            self.fixar(rot_catch);
            let antes = self.entrar_escopo(c.escopo)?;
            match param {
                Some(p) => self.ligar_padrao(p, Liga::Inicia(None))?,
                None => self.e(Op::Pop),
            }
            self.içar_funcoes(&c.stmts)?;
            for s in &c.stmts {
                self.comando(s)?;
            }
            self.sair_escopo(antes);
            self.fixar(rot_fim);
        } else {
            self.bloco(bloco)?;
        }
        if let (Some(l), Some(fin)) = (rot_fin, finalmente) {
            self.f().ctl.pop();
            let depois = self.rotulo();
            self.e(Op::FimTentar);
            self.e(Op::Gosub(l));
            self.e(Op::Salto(depois));
            self.fixar(rot_exc);
            self.e(Op::Gosub(l));
            self.e(Op::Lancar);
            self.fixar(l);
            self.f().ctl.push(Ctl::NoFinally);
            self.bloco(fin)?;
            self.f().ctl.pop();
            self.e(Op::RetGosub);
            self.fixar(depois);
        }
        Ok(())
    }

    fn trocar(&mut self, escopo: u32, valor: &Expr, casos: &[(Option<Expr>, Vec<Stmt>)]) -> R<()> {
        self.expr(valor)?;
        let antes = self.entrar_escopo(escopo)?;
        for (_, corpo) in casos {
            self.içar_funcoes(corpo)?;
        }
        let fim = self.rotulo();
        let corpos: Vec<u32> = casos.iter().map(|_| 0).collect();
        let corpos: Vec<u32> = corpos.iter().map(|_| self.rotulo()).collect();
        for (i, (teste, _)) in casos.iter().enumerate() {
            if let Some(t) = teste {
                let prox = self.rotulo();
                self.e(Op::Dup);
                self.expr(t)?;
                self.e(Op::Igual3);
                self.e(Op::SaltoFalso(prox));
                self.e(Op::Pop);
                self.e(Op::Salto(corpos[i]));
                self.fixar(prox);
            }
        }
        self.e(Op::Pop);
        match casos.iter().position(|(t, _)| t.is_none()) {
            Some(i) => self.e(Op::Salto(corpos[i])),
            None => self.e(Op::Salto(fim)),
        }
        // `break` num switch; `continue` procura o laço de fora.
        self.empilhar_laco(fim, None);
        for (i, (_, corpo)) in casos.iter().enumerate() {
            self.fixar(corpos[i]);
            for s in corpo {
                self.comando(s)?;
            }
        }
        self.f().ctl.pop();
        self.fixar(fim);
        self.sair_escopo(antes);
        Ok(())
    }

    // ------------------------------------------------------------ desestruturação

    /// Consome o topo da pilha, ligando-o ao padrão.
    fn ligar_padrao(&mut self, p: &Padrao, modo: Liga) -> R<()> {
        match p {
            Padrao::Nome(a, onde) => match modo {
                Liga::Atribui => self.esc_var(*a, *onde),
                Liga::Inicia(g) => self.ini_var(*a, *onde, g),
            },
            Padrao::Alvo(e) => match e {
                Expr::Membro(obj, nome, _) => {
                    self.expr(obj)?;
                    self.e(Op::Troca);
                    self.e(Op::DefProp(*nome));
                    self.e(Op::Pop);
                }
                Expr::Indice(obj, chave, _) => {
                    self.expr(obj)?;
                    self.expr(chave)?;
                    self.e(Op::Rot3Inv);
                    self.e(Op::DefElem);
                    self.e(Op::Pop);
                }
                _ => return Err(String::from("alvo de desestruturação inválido")),
            },
            Padrao::Array(itens, resto) => {
                self.e(Op::IterInicio);
                for item in itens {
                    self.e(Op::IterPasso);
                    match item {
                        None => self.e(Op::Pop),
                        Some((p, ini)) => {
                            if let Some(i) = ini {
                                let l = self.rotulo();
                                self.e(Op::SaltoNaoUndef(l));
                                self.expr(i)?;
                                self.fixar(l);
                            }
                            self.ligar_padrao(p, modo)?;
                        }
                    }
                }
                if let Some(r) = resto {
                    self.e(Op::IterResto);
                    self.ligar_padrao(r, modo)?;
                }
                self.e(Op::IterFechar);
            }
            Padrao::Objeto(itens, resto) => {
                self.e(Op::ChecarObj);
                for (chave, p, ini) in itens {
                    self.e(Op::Dup);
                    match chave {
                        Chave::Nome(a) => self.e(Op::ObterProp(*a)),
                        Chave::Computada(c) => {
                            self.expr(c)?;
                            self.e(Op::ObterElem);
                        }
                    }
                    if let Some(i) = ini {
                        let l = self.rotulo();
                        self.e(Op::SaltoNaoUndef(l));
                        self.expr(i)?;
                        self.fixar(l);
                    }
                    self.ligar_padrao(p, modo)?;
                }
                if let Some(r) = resto {
                    let mut n = 0u16;
                    for (chave, _, _) in itens {
                        match chave {
                            Chave::Nome(a) => self.e(Op::Texto(*a)),
                            Chave::Computada(c) => self.expr(c)?,
                        }
                        n += 1;
                    }
                    self.e(Op::ObjResto(n));
                    self.ligar_padrao(r, modo)?;
                }
                self.e(Op::Pop);
            }
        }
        Ok(())
    }

    // ------------------------------------------------------------ classes

    fn chave(&mut self, c: &Chave) -> R<()> {
        match c {
            Chave::Nome(a) => self.e(Op::Texto(*a)),
            Chave::Computada(e) => {
                self.expr(e)?;
                self.e(Op::ParaChave);
            }
        }
        Ok(())
    }

    fn classe(&mut self, c: &NoClasse) -> R<()> {
        let antes = self.entrar_escopo(c.escopo)?;
        if let Some(p) = &c.pai {
            self.expr(p)?;
        }
        let ctor = self.funcao(&c.construtor)?;
        self.e(Op::CriarClasse(ctor, c.pai.is_some()));
        if c.nome != K_VAZIO && self.escopos[c.escopo as usize].decls.iter().any(|d| d.nome == c.nome) {
            self.e(Op::Sobre);
            self.ini_var(c.nome, c.escopo, None);
        }
        for m in &c.membros {
            match &m.tipo {
                TipoMembro::Metodo(f) | TipoMembro::Get(f) | TipoMembro::Set(f) => {
                    if m.estatico {
                        self.e(Op::Troca);
                    }
                    self.chave(&m.chave)?;
                    let i = self.funcao(f)?;
                    self.e(Op::Closure(i));
                    let tipo = match m.tipo {
                        TipoMembro::Get(_) => 1,
                        TipoMembro::Set(_) => 2,
                        _ => 0,
                    };
                    self.e(Op::DefMetodo(tipo));
                    if m.estatico {
                        self.e(Op::Troca);
                    }
                }
                TipoMembro::Campo(ini) => {
                    self.e(Op::Troca);
                    self.chave(&m.chave)?;
                    match ini {
                        Some(f) => {
                            let i = self.funcao(f)?;
                            self.e(Op::Closure(i));
                        }
                        None => self.e(Op::Undef),
                    }
                    self.e(if m.estatico { Op::CampoEstatico } else { Op::DefCampo });
                    self.e(Op::Troca);
                }
                TipoMembro::Bloco(f) => {
                    self.e(Op::Troca);
                    self.e(Op::Texto(K_VAZIO));
                    let i = self.funcao(f)?;
                    self.e(Op::Closure(i));
                    self.e(Op::CampoEstatico);
                    self.e(Op::Troca);
                }
            }
        }
        self.e(Op::Pop);
        self.sair_escopo(antes);
        Ok(())
    }

    // ------------------------------------------------------------ expressões

    fn num(&mut self, n: f64) {
        let i = n as i32;
        if i as f64 == n && !(n == 0.0 && n.is_sign_negative()) {
            self.e(Op::Int(i));
        } else {
            let f = self.f();
            f.nums.push(n);
            let idx = f.nums.len() as u32 - 1;
            self.e(Op::Num(idx));
        }
    }

    fn args_tem_espalhar(args: &[Expr]) -> bool {
        args.iter().any(|a| matches!(a, Expr::Espalhar(_)))
    }

    /// Junta os argumentos (ou itens de array) com espalhamento num array.
    fn montar_array(&mut self, itens: &[Expr]) -> R<()> {
        // A parte inicial simples vai de uma vez.
        let simples = itens.iter().take_while(|a| !matches!(a, Expr::Espalhar(_) | Expr::Buraco)).count();
        for a in &itens[..simples] {
            self.expr(a)?;
        }
        self.e(Op::NovoArray(simples as u32));
        for a in &itens[simples..] {
            match a {
                Expr::Espalhar(x) => {
                    self.expr(x)?;
                    self.e(Op::ArrayEspalhar);
                }
                Expr::Buraco => self.e(Op::ArrayBuraco),
                x => {
                    self.expr(x)?;
                    self.e(Op::ArrayPush);
                }
            }
        }
        Ok(())
    }

    fn args(&mut self, args: &[Expr]) -> R<Option<u16>> {
        if Self::args_tem_espalhar(args) {
            self.montar_array(args)?;
            return Ok(None);
        }
        for a in args {
            self.expr(a)?;
        }
        Ok(Some(args.len() as u16))
    }

    /// Empilha `fn this` para uma chamada.
    fn callee(&mut self, c: &Expr, rot_nulo: Option<u32>) -> R<()> {
        match c {
            Expr::Membro(obj, nome, opcional) => {
                self.expr_cadeia(obj, rot_nulo)?;
                if *opcional {
                    self.e(Op::SaltoNuloPop(rot_nulo.unwrap_or(0)));
                }
                self.e(Op::ObterMetodo(*nome));
            }
            Expr::Indice(obj, chave, opcional) => {
                self.expr_cadeia(obj, rot_nulo)?;
                if *opcional {
                    self.e(Op::SaltoNuloPop(rot_nulo.unwrap_or(0)));
                }
                self.expr(chave)?;
                self.e(Op::ObterMetodoElem);
            }
            Expr::SuperMembro(nome, esc) => {
                self.ler_var(K_THIS_VAR, *esc);
                self.ler_var(K_HOME_VAR, *esc);
                self.e(Op::ObterSuper(*nome));
                self.ler_var(K_THIS_VAR, *esc);
            }
            Expr::SuperIndice(chave, esc) => {
                self.ler_var(K_THIS_VAR, *esc);
                self.ler_var(K_HOME_VAR, *esc);
                self.expr(chave)?;
                self.e(Op::ObterSuperElem);
                self.ler_var(K_THIS_VAR, *esc);
            }
            Expr::Chamar(..) if rot_nulo.is_some() => {
                self.expr_cadeia(c, rot_nulo)?;
                self.e(Op::Undef);
            }
            outro => {
                self.expr(outro)?;
                self.e(Op::Undef);
            }
        }
        Ok(())
    }

    /// Dentro de uma cadeia `?.`: os membros e chamadas saltam para `rot_nulo`.
    fn expr_cadeia(&mut self, e: &Expr, rot_nulo: Option<u32>) -> R<()> {
        let Some(r) = rot_nulo else { return self.expr(e) };
        match e {
            Expr::Membro(obj, nome, opcional) => {
                self.expr_cadeia(obj, rot_nulo)?;
                if *opcional {
                    self.e(Op::SaltoNuloPop(r));
                }
                self.e(Op::ObterProp(*nome));
            }
            Expr::Indice(obj, chave, opcional) => {
                self.expr_cadeia(obj, rot_nulo)?;
                if *opcional {
                    self.e(Op::SaltoNuloPop(r));
                }
                self.expr(chave)?;
                self.e(Op::ObterElem);
            }
            Expr::Chamar(c, args, opcional) => {
                self.callee(c, rot_nulo)?;
                if *opcional {
                    self.e(Op::SaltoNuloChamada(r));
                }
                match self.args(args)? {
                    Some(n) => self.e(Op::Chamar(n)),
                    None => self.e(Op::ChamarEsp),
                }
            }
            outro => self.expr(outro)?,
        }
        Ok(())
    }

    fn expr(&mut self, e: &Expr) -> R<()> {
        match e {
            Expr::Num(n) => self.num(*n),
            Expr::Str(a) => self.e(Op::Texto(*a)),
            Expr::Bool(true) => self.e(Op::Verdadeiro),
            Expr::Bool(false) => self.e(Op::Falso),
            Expr::Nulo => self.e(Op::Nulo),
            Expr::Tpl(pedacos, exprs) => {
                let mut n = 0u16;
                for (i, p) in pedacos.iter().enumerate() {
                    if *p != K_VAZIO {
                        self.e(Op::Texto(*p));
                        n += 1;
                    }
                    if let Some(x) = exprs.get(i) {
                        self.expr(x)?;
                        n += 1;
                    }
                }
                self.e(Op::Template(n));
            }
            Expr::Tagged(c, pedacos, exprs) => {
                self.callee(c, None)?;
                let f = self.f();
                f.templates.push(pedacos.clone());
                let i = f.templates.len() as u32 - 1;
                self.e(Op::TemplateObj(i));
                for x in exprs {
                    self.expr(x)?;
                }
                self.e(Op::Chamar(exprs.len() as u16 + 1));
            }
            Expr::Regex(c, f) => self.e(Op::Regex(*c, *f)),
            Expr::Nome(a, onde) => self.ler_var(*a, *onde),
            Expr::Array(itens) => {
                if itens.iter().any(|i| matches!(i, Expr::Espalhar(_) | Expr::Buraco)) {
                    self.montar_array(itens)?;
                } else if itens.len() > 64 {
                    // Arrays enormes não ocupam a pilha toda de uma vez.
                    self.e(Op::NovoArray(0));
                    for x in itens {
                        self.expr(x)?;
                        self.e(Op::ArrayPush);
                    }
                } else {
                    for x in itens {
                        self.expr(x)?;
                    }
                    self.e(Op::NovoArray(itens.len() as u32));
                }
            }
            Expr::Objeto(props) => self.objeto(props)?,
            Expr::Funcao(f) => {
                let i = self.funcao(f)?;
                self.e(Op::Closure(i));
            }
            Expr::Classe(c) => self.classe(c)?,
            Expr::Unario(op, x) => self.unario(*op, x)?,
            Expr::Atualiza { soma, prefixo, alvo } => self.atualiza(*soma, *prefixo, alvo)?,
            Expr::Binario(p, a, b) => {
                self.expr(a)?;
                self.expr(b)?;
                self.e(op_binario(*p));
            }
            Expr::Logico(p, a, b) => {
                let fim = self.rotulo();
                self.expr(a)?;
                self.e(match p {
                    P::E2 => Op::SaltoFalsoMantem(fim),
                    P::Ou2 => Op::SaltoVerdadeiroMantem(fim),
                    _ => Op::SaltoNaoNuloMantem(fim),
                });
                self.expr(b)?;
                self.fixar(fim);
            }
            Expr::Atrib(p, alvo, v) => self.atribuicao(*p, alvo, v)?,
            Expr::AtribPadrao(p, v) => {
                self.expr(v)?;
                self.e(Op::Dup);
                self.ligar_padrao(p, Liga::Atribui)?;
            }
            Expr::Cond(c, a, b) => {
                let (senao, fim) = (self.rotulo(), self.rotulo());
                self.expr(c)?;
                self.e(Op::SaltoFalso(senao));
                self.expr(a)?;
                self.e(Op::Salto(fim));
                self.fixar(senao);
                self.expr(b)?;
                self.fixar(fim);
            }
            Expr::Chamar(c, args, _) => {
                self.callee(c, None)?;
                match self.args(args)? {
                    Some(n) => self.e(Op::Chamar(n)),
                    None => self.e(Op::ChamarEsp),
                }
            }
            Expr::Novo(c, args) => {
                self.expr(c)?;
                match self.args(args)? {
                    Some(n) => self.e(Op::Novo(n)),
                    None => self.e(Op::NovoEsp),
                }
            }
            Expr::Membro(obj, nome, _) => {
                self.expr(obj)?;
                self.e(Op::ObterProp(*nome));
            }
            Expr::Indice(obj, chave, _) => {
                self.expr(obj)?;
                self.expr(chave)?;
                self.e(Op::ObterElem);
            }
            Expr::Cadeia(x) => {
                let (nulo, fim) = (self.rotulo(), self.rotulo());
                self.expr_cadeia(x, Some(nulo))?;
                self.e(Op::Salto(fim));
                self.fixar(nulo);
                self.e(Op::Undef);
                self.fixar(fim);
            }
            Expr::Seq(v) => {
                for (i, x) in v.iter().enumerate() {
                    self.expr(x)?;
                    if i + 1 < v.len() {
                        self.e(Op::Pop);
                    }
                }
            }
            Expr::Espalhar(_) => return Err(String::from("... fora de lugar")),
            Expr::Buraco => self.e(Op::Undef),
            Expr::Yield(v, delegado) => {
                match v {
                    Some(x) => self.expr(x)?,
                    None => self.e(Op::Undef),
                }
                if *delegado {
                    self.yield_delegado()?;
                } else {
                    if self.f().assincrona {
                        self.e(Op::Await);
                    }
                    self.yield_simples()?;
                }
            }
            Expr::Await(x) => {
                self.expr(x)?;
                self.e(Op::Await);
            }
            Expr::SuperMembro(nome, esc) => {
                self.ler_var(K_THIS_VAR, *esc);
                self.ler_var(K_HOME_VAR, *esc);
                self.e(Op::ObterSuper(*nome));
            }
            Expr::SuperIndice(chave, esc) => {
                self.ler_var(K_THIS_VAR, *esc);
                self.ler_var(K_HOME_VAR, *esc);
                self.expr(chave)?;
                self.e(Op::ObterSuperElem);
            }
            Expr::SuperChamar(args, esc) => {
                self.ler_var(K_FN_VAR, *esc);
                self.ler_var(K_NT_VAR, *esc);
                match self.args(args)? {
                    Some(n) => self.e(Op::ChamarSuper(n)),
                    None => self.e(Op::ChamarSuperEsp),
                }
                self.e(Op::Dup);
                self.ini_var(K_THIS_VAR, *esc, None);
                self.ler_var(K_FN_VAR, *esc);
                self.e(Op::IniciarCampos);
            }
            Expr::NovoAlvo(esc) => self.ler_var(K_NT_VAR, *esc),
        }
        Ok(())
    }

    /// `Yield` deixa [valor, é_retorno]: se pediram return(), sai por aqui,
    /// passando pelos finally.
    fn yield_simples(&mut self) -> R<()> {
        self.e(Op::Yield);
        let segue = self.rotulo();
        self.e(Op::SaltoFalso(segue));
        self.retorno()?;
        self.fixar(segue);
        Ok(())
    }

    /// `yield* x`: repete até o iterador interno acabar.
    fn yield_delegado(&mut self) -> R<()> {
        self.e(Op::IterInicio);
        // [iter next] enviado (começa undefined)
        self.e(Op::Undef);
        let (topo, fim) = (self.rotulo(), self.rotulo());
        self.fixar(topo);
        // iter next enviado → iter next valor; ou salta com iter next retorno
        self.e(Op::DelegadoPasso(fim));
        self.yield_simples()?;
        self.e(Op::Salto(topo));
        self.fixar(fim);
        // iter next retorno → retorno
        self.e(Op::Rot3);
        self.e(Op::Pop);
        self.e(Op::Pop);
        Ok(())
    }

    fn unario(&mut self, op: OpU, x: &Expr) -> R<()> {
        match op {
            OpU::Typeof => {
                if let Expr::Nome(a, onde) = x {
                    if matches!(self.resolver(*a, *onde), Var::Global) {
                        self.e(Op::LerGlobalTalvez(*a));
                        self.e(Op::Typeof);
                        return Ok(());
                    }
                }
                self.expr(x)?;
                self.e(Op::Typeof);
            }
            OpU::Void => {
                self.expr(x)?;
                self.e(Op::Pop);
                self.e(Op::Undef);
            }
            OpU::Delete => match x {
                Expr::Membro(o, nome, _) => {
                    self.expr(o)?;
                    self.e(Op::ApagarProp(*nome));
                }
                Expr::Indice(o, c, _) => {
                    self.expr(o)?;
                    self.expr(c)?;
                    self.e(Op::ApagarElem);
                }
                Expr::Nome(a, onde) if matches!(self.resolver(*a, *onde), Var::Global) => {
                    self.e(Op::LerGlobal(K_THIS_VAR));
                    self.e(Op::ApagarProp(*a));
                }
                outro => {
                    self.expr(outro)?;
                    self.e(Op::Pop);
                    self.e(Op::Verdadeiro);
                }
            },
            OpU::Neg => {
                self.expr(x)?;
                self.e(Op::Neg);
            }
            OpU::Pos => {
                self.expr(x)?;
                self.e(Op::Pos);
            }
            OpU::Nao => {
                self.expr(x)?;
                self.e(Op::Nao);
            }
            OpU::Til => {
                self.expr(x)?;
                self.e(Op::BitNao);
            }
        }
        Ok(())
    }

    fn atualiza(&mut self, soma: bool, prefixo: bool, alvo: &Expr) -> R<()> {
        let op = if soma { Op::Incr } else { Op::Decr };
        match alvo {
            Expr::Nome(a, onde) => {
                self.ler_var(*a, *onde);
                self.e(Op::ParaNumero);
                if !prefixo {
                    self.e(Op::Dup);
                }
                self.e(op);
                if prefixo {
                    self.e(Op::Dup);
                }
                self.esc_var(*a, *onde);
            }
            Expr::Membro(o, nome, _) => {
                self.expr(o)?;
                self.e(Op::Dup);
                self.e(Op::ObterProp(*nome));
                self.e(Op::ParaNumero);
                if prefixo {
                    self.e(op);
                    self.e(Op::DefProp(*nome));
                } else {
                    // obj v → v obj v+1
                    self.e(Op::Dup);
                    self.e(Op::Rot3);
                    self.e(op);
                    self.e(Op::DefProp(*nome));
                    self.e(Op::Pop);
                }
            }
            Expr::Indice(o, c, _) => {
                self.expr(o)?;
                self.expr(c)?;
                self.e(Op::ParaChave);
                self.e(Op::Dup2);
                self.e(Op::ObterElem);
                self.e(Op::ParaNumero);
                if prefixo {
                    self.e(op);
                    self.e(Op::DefElem);
                } else {
                    // obj k v → v obj k v+1
                    self.e(Op::Dup);
                    self.e(Op::Rot4);
                    self.e(op);
                    self.e(Op::DefElem);
                    self.e(Op::Pop);
                }
            }
            _ => return Err(String::from("alvo inválido para ++/--")),
        }
        Ok(())
    }

    fn atribuicao(&mut self, p: P, alvo: &Expr, v: &Expr) -> R<()> {
        // a &&= b, a ||= b, a ??= b: só escreve se precisar.
        let logico = matches!(p, P::E2Atrib | P::Ou2Atrib | P::NulishAtrib);
        let salto = |fim: u32| match p {
            P::E2Atrib => Op::SaltoFalsoMantem(fim),
            P::Ou2Atrib => Op::SaltoVerdadeiroMantem(fim),
            _ => Op::SaltoNaoNuloMantem(fim),
        };
        match alvo {
            Expr::Nome(a, onde) => {
                if p == P::Atrib {
                    self.expr(v)?;
                    self.e(Op::Dup);
                    self.esc_var(*a, *onde);
                } else if logico {
                    let fim = self.rotulo();
                    self.ler_var(*a, *onde);
                    self.e(salto(fim));
                    self.expr(v)?;
                    self.e(Op::Dup);
                    self.esc_var(*a, *onde);
                    self.fixar(fim);
                } else {
                    self.ler_var(*a, *onde);
                    self.expr(v)?;
                    self.e(op_composta(p));
                    self.e(Op::Dup);
                    self.esc_var(*a, *onde);
                }
            }
            Expr::Membro(o, nome, _) => {
                self.expr(o)?;
                if p == P::Atrib {
                    self.expr(v)?;
                } else if logico {
                    let (fim, curto) = (self.rotulo(), self.rotulo());
                    self.e(Op::Dup);
                    self.e(Op::ObterProp(*nome));
                    self.e(salto(curto));
                    self.expr(v)?;
                    self.e(Op::DefProp(*nome));
                    self.e(Op::Salto(fim));
                    self.fixar(curto);
                    self.e(Op::Troca);
                    self.e(Op::Pop);
                    self.fixar(fim);
                    return Ok(());
                } else {
                    self.e(Op::Dup);
                    self.e(Op::ObterProp(*nome));
                    self.expr(v)?;
                    self.e(op_composta(p));
                }
                self.e(Op::DefProp(*nome));
            }
            Expr::Indice(o, c, _) => {
                self.expr(o)?;
                self.expr(c)?;
                if p == P::Atrib {
                    self.expr(v)?;
                } else if logico {
                    let (fim, curto) = (self.rotulo(), self.rotulo());
                    self.e(Op::ParaChave);
                    self.e(Op::Dup2);
                    self.e(Op::ObterElem);
                    self.e(salto(curto));
                    self.expr(v)?;
                    self.e(Op::DefElem);
                    self.e(Op::Salto(fim));
                    self.fixar(curto);
                    // obj k r → r
                    self.e(Op::Rot3);
                    self.e(Op::Pop);
                    self.e(Op::Pop);
                    self.fixar(fim);
                    return Ok(());
                } else {
                    self.e(Op::ParaChave);
                    self.e(Op::Dup2);
                    self.e(Op::ObterElem);
                    self.expr(v)?;
                    self.e(op_composta(p));
                }
                self.e(Op::DefElem);
            }
            Expr::SuperMembro(nome, esc) => {
                self.ler_var(K_THIS_VAR, *esc);
                self.ler_var(K_HOME_VAR, *esc);
                self.expr(v)?;
                self.e(Op::DefSuper(*nome));
            }
            Expr::SuperIndice(c, esc) => {
                self.ler_var(K_THIS_VAR, *esc);
                self.ler_var(K_HOME_VAR, *esc);
                self.expr(c)?;
                self.expr(v)?;
                self.e(Op::DefSuperElem);
            }
            _ => return Err(String::from("alvo de atribuição inválido")),
        }
        Ok(())
    }

    fn objeto(&mut self, props: &[PropObj]) -> R<()> {
        self.e(Op::NovoObjeto);
        for p in props {
            match &p.tipo {
                TipoProp::Valor(v) => match &p.chave {
                    Chave::Nome(a) if *a != K_PROTO => {
                        if let Expr::Funcao(f) = v {
                            if f.metodo {
                                self.e(Op::Texto(*a));
                                let i = self.funcao(f)?;
                                self.e(Op::Closure(i));
                                self.e(Op::DefMetodo(4));
                                continue;
                            }
                        }
                        self.expr(v)?;
                        self.e(Op::ObjProp(*a));
                    }
                    Chave::Nome(a) => {
                        // `__proto__: x` muda o protótipo.
                        self.e(Op::Dup);
                        self.expr(v)?;
                        self.e(Op::DefProp(*a));
                        self.e(Op::Pop);
                    }
                    Chave::Computada(c) => {
                        self.expr(c)?;
                        self.e(Op::ParaChave);
                        if let Expr::Funcao(f) = v {
                            if f.metodo {
                                let i = self.funcao(f)?;
                                self.e(Op::Closure(i));
                                self.e(Op::DefMetodo(4));
                                continue;
                            }
                        }
                        self.expr(v)?;
                        self.e(Op::ObjElem);
                    }
                },
                TipoProp::Get(f) => {
                    self.chave(&p.chave)?;
                    let i = self.funcao(f)?;
                    self.e(Op::Closure(i));
                    self.e(Op::DefMetodo(5));
                }
                TipoProp::Set(f) => {
                    self.chave(&p.chave)?;
                    let i = self.funcao(f)?;
                    self.e(Op::Closure(i));
                    self.e(Op::DefMetodo(6));
                }
                TipoProp::Espalhar(x) => {
                    self.expr(x)?;
                    self.e(Op::ObjEspalhar);
                }
                TipoProp::Cobre(..) => return Err(String::from("`{a = 1}` só vale em desestruturação")),
            }
        }
        Ok(())
    }
}

#[derive(Clone, Copy)]
enum Liga {
    Atribui,
    /// Some(constante): no nível global, cria a variável.
    Inicia(Option<bool>),
}

fn op_binario(p: P) -> Op {
    match p {
        P::Mais => Op::Soma,
        P::Menos => Op::Sub,
        P::Vezes => Op::Mul,
        P::Divide => Op::Div,
        P::Resto => Op::Mod,
        P::Pot => Op::Pot,
        P::E => Op::BitE,
        P::Ou => Op::BitOu,
        P::Xor => Op::BitXor,
        P::Shl => Op::Shl,
        P::Shr => Op::Shr,
        P::Ushr => Op::Ushr,
        P::Igual2 => Op::Igual,
        P::Diferente => Op::Dif,
        P::Igual3 => Op::Igual3,
        P::Diferente3 => Op::Dif3,
        P::Menor => Op::Menor,
        P::MenorIgual => Op::MenorIgual,
        P::Maior => Op::Maior,
        P::MaiorIgual => Op::MaiorIgual,
        P::Privado => Op::InstanciaDe,
        _ => Op::Em,
    }
}

fn op_composta(p: P) -> Op {
    op_binario(match p {
        P::MaisAtrib => P::Mais,
        P::MenosAtrib => P::Menos,
        P::VezesAtrib => P::Vezes,
        P::DivideAtrib => P::Divide,
        P::RestoAtrib => P::Resto,
        P::PotAtrib => P::Pot,
        P::ShlAtrib => P::Shl,
        P::ShrAtrib => P::Shr,
        P::UshrAtrib => P::Ushr,
        P::EAtrib => P::E,
        P::OuAtrib => P::Ou,
        _ => P::Xor,
    })
}
