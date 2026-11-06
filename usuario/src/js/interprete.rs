//! O laço de execução: uma máquina de pilha sobre o bytecode de `compilador.rs`.
//!
//! Chamadas de JavaScript para JavaScript não usam a pilha do Rust: é só
//! mais um `Quadro`. Geradores e funções assíncronas guardam o quadro e o
//! pedaço de pilha deles num objeto, e continuam de onde pararam.

use super::compilador::{Funcao, Op};
use super::lexico::*;
use super::vm::*;
use crate::mat;
use alloc::boxed::Box;
use alloc::format;
use alloc::rc::Rc;
use alloc::string::String;
use alloc::vec::Vec;

const LIMITE_QUADROS: usize = 3000;
const LIMITE_NATIVAS: u32 = 800;
const LIMITE_OBJETOS: usize = 1_500_000;

impl Vm {
    /// Compila e roda um script no escopo global. Devolve o valor do último comando.
    pub fn avaliar(&mut self, fonte: &str) -> Res {
        let f = self.compilar_script(fonte)?;
        self.rodar_funcao(f)
    }

    pub fn compilar_script(&mut self, fonte: &str) -> Result<Rc<Funcao>, Valor> {
        let p = match super::sintaxe::ler_programa(fonte, &mut self.n) {
            Ok(p) => p,
            Err(e) => return Err(self.erro_sintaxe(&e)),
        };
        match super::compilador::compilar(p, &mut self.n) {
            Ok(f) => Ok(f),
            Err(e) => Err(self.erro_sintaxe(&e)),
        }
    }

    pub fn rodar_funcao(&mut self, f: Rc<Funcao>) -> Res {
        let c = self.closure_de(f, Rc::from(Vec::new()));
        let this = Valor::Obj(self.global);
        self.chamar(Valor::Obj(c), this, &[])
    }

    pub fn closure_de(&mut self, func: Rc<Funcao>, ups: Rc<[u32]>) -> ObjId {
        let proto = if func.gerador {
            self.p.func_gerador
        } else if func.assincrona {
            self.p.func_assinc
        } else {
            self.p.funcao
        };
        self.novo_obj_com(proto, Tipo::Funcao(Box::new(Closure { func, ups, home: Valor::Undef, campos: Vec::new() })))
    }

    /// Chama uma função qualquer.
    pub fn chamar(&mut self, f: Valor, this: Valor, args: &[Valor]) -> Res {
        let Valor::Obj(id) = f else {
            return Err(self.nao_e_funcao(f));
        };
        match &self.objs[id as usize].tipo {
            Tipo::Funcao(c) => {
                if c.func.construtor {
                    let nome = c.func.nome;
                    let nome = self.n.texto(nome);
                    return Err(self.erro_tipo(&format!("Class constructor {} cannot be invoked without 'new'", nome)));
                }
                self.pilha.push(f);
                self.pilha.push(this);
                self.pilha.extend_from_slice(args);
                let limite = self.quadros.len();
                self.empilhar_quadro(id, this, args.len(), Valor::Undef, false)?;
                self.executar(limite, None)
            }
            Tipo::Nativa(d) => {
                let g = d.f;
                self.chamar_nativa(g, id, this, args, Valor::Undef)
            }
            Tipo::Ligada(l) => {
                let (alvo, t) = (l.alvo, l.this);
                let mut todos = l.args.clone();
                todos.extend_from_slice(args);
                self.chamar(alvo, t, &todos)
            }
            Tipo::Proxy(alvo, man) => {
                let (alvo, man) = (*alvo, *man);
                let armadilha = self.obter(man, self.a.apply)?;
                if armadilha.nulo_ou_undef() {
                    return self.chamar(alvo, this, args);
                }
                let lista = self.novo_array(args.to_vec());
                self.chamar(armadilha, man, &[alvo, this, lista])
            }
            _ => Err(self.nao_e_funcao(f)),
        }
    }

    fn chamar_nativa(&mut self, g: Nativa, id: ObjId, this: Valor, args: &[Valor], nt: Valor) -> Res {
        if self.prof_nativa >= LIMITE_NATIVAS {
            return Err(self.erro_intervalo("Maximum call stack size exceeded"));
        }
        let (antes_nt, antes_at) = (self.novo_alvo, self.nativa_atual);
        self.novo_alvo = nt;
        self.nativa_atual = id;
        self.prof_nativa += 1;
        let r = g(self, this, args);
        self.prof_nativa -= 1;
        self.novo_alvo = antes_nt;
        self.nativa_atual = antes_at;
        r
    }

    fn nao_e_funcao(&mut self, f: Valor) -> Valor {
        let d = match f {
            Valor::Obj(_) => String::from("object"),
            Valor::Texto(t) => format!("\"{}\"", self.n.texto(t)),
            outro => self.texto_de_primitivo(outro),
        };
        self.erro_tipo(&format!("{} is not a function", d))
    }

    /// `new f(...args)`
    pub fn construir(&mut self, f: Valor, args: &[Valor], nt: Valor) -> Res {
        let Valor::Obj(id) = f else {
            let d = self.texto_de_primitivo(f);
            return Err(self.erro_tipo(&format!("{} is not a constructor", d)));
        };
        if !self.e_construtor(f) {
            return Err(self.erro_tipo("object is not a constructor"));
        }
        match &self.objs[id as usize].tipo {
            Tipo::Funcao(c) => {
                let derivada = c.func.derivada;
                let this = if derivada {
                    Valor::Vazio
                } else {
                    let proto = self.proto_de_novo_alvo(nt, self.p.objeto)?;
                    let o = Valor::Obj(self.novo_obj_com(proto, Tipo::Comum));
                    self.pilha.push(o);
                    let r = self.iniciar_campos(o, f);
                    self.pilha.pop();
                    r?;
                    o
                };
                self.pilha.push(f);
                self.pilha.push(this);
                self.pilha.extend_from_slice(args);
                let limite = self.quadros.len();
                self.empilhar_quadro(id, this, args.len(), nt, true)?;
                self.executar(limite, None)
            }
            Tipo::Nativa(d) => {
                let g = d.f;
                self.chamar_nativa(g, id, Valor::Undef, args, nt)
            }
            Tipo::Ligada(l) => {
                let alvo = l.alvo;
                let mut todos = l.args.clone();
                todos.extend_from_slice(args);
                let nt2 = if nt == f { alvo } else { nt };
                self.construir(alvo, &todos, nt2)
            }
            Tipo::Proxy(alvo, man) => {
                let (alvo, man) = (*alvo, *man);
                let armadilha = self.obter(man, self.a.construct)?;
                if armadilha.nulo_ou_undef() {
                    let nt2 = if nt == f { alvo } else { nt };
                    return self.construir(alvo, args, nt2);
                }
                let lista = self.novo_array(args.to_vec());
                self.chamar(armadilha, man, &[alvo, lista, nt])
            }
            _ => Err(self.erro_tipo("object is not a constructor")),
        }
    }

    pub fn proto_de_novo_alvo(&mut self, nt: Valor, padrao: ObjId) -> Result<u32, Valor> {
        if let Valor::Obj(n) = nt {
            let p = self.obter_obj(n, K_PROTOTYPE, nt)?;
            if let Valor::Obj(po) = p {
                return Ok(po);
            }
        }
        Ok(padrao)
    }

    /// Roda os inicializadores de campo da classe `f` sobre `this`.
    fn iniciar_campos(&mut self, this: Valor, f: Valor) -> Result<(), Valor> {
        let Valor::Obj(id) = f else { return Ok(()) };
        let Tipo::Funcao(c) = &self.objs[id as usize].tipo else { return Ok(()) };
        if c.campos.is_empty() {
            return Ok(());
        }
        let campos = c.campos.clone();
        let Valor::Obj(alvo) = this else { return Ok(()) };
        for (k, ini) in campos {
            let v = if ini == Valor::Undef { Valor::Undef } else { self.chamar(ini, this, &[])? };
            let chave = self.para_chave(k)?;
            self.definir_propria(alvo, chave, v, P_TUDO);
        }
        Ok(())
    }

    /// Monta o quadro de uma closure. Na pilha: `f this arg0..argN`.
    fn empilhar_quadro(&mut self, id: ObjId, this: Valor, argc: usize, nt: Valor, construindo: bool) -> Result<(), Valor> {
        if self.quadros.len() >= LIMITE_QUADROS {
            let n = self.pilha.len();
            self.pilha.truncate(n - argc - 2);
            return Err(self.erro_intervalo("Maximum call stack size exceeded"));
        }
        let Tipo::Funcao(c) = &self.objs[id as usize].tipo else { unreachable!() };
        let func = c.func.clone();
        let ups = c.ups.clone();
        let base = self.pilha.len() - argc;
        let n_params = func.n_params as usize;
        let args = if func.usa_args { self.pilha[base..].to_vec() } else { Vec::new() };
        if argc > n_params {
            self.pilha.truncate(base + n_params);
        }
        // Parâmetros que faltam e variáveis locais começam undefined.
        let topo = base + func.n_locais.max(func.n_params) as usize;
        self.pilha.resize(topo, Valor::Undef);
        let this = if !func.estrito && !func.seta && this.nulo_ou_undef() { Valor::Obj(self.global) } else { this };
        self.quadros.push(Quadro { func, ups, ip: 0, base, callee: Valor::Obj(id), this, nt, args, handlers: Vec::new(), construindo, gerador: NENHUM });
        Ok(())
    }

    /// Roda até o quadro de índice `limite` terminar.
    pub fn executar(&mut self, limite: usize, excecao_inicial: Option<Valor>) -> Res {
        self.prof_nativa += 1;
        let r = self.laco(limite, excecao_inicial);
        self.prof_nativa -= 1;
        r
    }

    fn laco(&mut self, limite: usize, excecao_inicial: Option<Valor>) -> Res {
        let mut excecao = excecao_inicial;
        'quadro: loop {
            if let Some(e) = excecao.take() {
                // Desempilha até achar quem trate.
                loop {
                    if !self.abortando {
                        let q = self.quadros.last_mut().unwrap();
                        if let Some((ip, sp)) = q.handlers.pop() {
                            q.ip = ip as usize;
                            let altura = q.base + sp as usize;
                            self.pilha.truncate(altura);
                            self.pilha.push(e);
                            continue 'quadro;
                        }
                    }
                    let q = self.quadros.pop().unwrap();
                    self.pilha.truncate(q.base.saturating_sub(2));
                    if q.gerador != NENHUM {
                        self.terminar_gerador(q.gerador);
                    }
                    if self.quadros.len() == limite {
                        return Err(e);
                    }
                }
            }
            let q = self.quadros.last().unwrap();
            let func = q.func.clone();
            let ups = q.ups.clone();
            let base = q.base;
            let mut ip = q.ip;
            let codigo: &[Op] = &func.codigo;
            macro_rules! t {
                ($e:expr) => {
                    match $e {
                        Ok(v) => v,
                        Err(e) => {
                            excecao = Some(e);
                            continue 'quadro;
                        }
                    }
                };
            }
            macro_rules! pop {
                () => {
                    self.pilha.pop().unwrap_or(Valor::Undef)
                };
            }
            macro_rules! lancar {
                ($e:expr) => {{
                    excecao = Some($e);
                    continue 'quadro;
                }};
            }
            macro_rules! topo {
                () => {
                    *self.pilha.last_mut().unwrap()
                };
            }
            macro_rules! binario_num {
                ($a:ident, $b:ident, $int:expr, $flt:expr) => {{
                    let y_ = pop!();
                    let x_ = topo!();
                    let r = match (x_, y_) {
                        (Valor::Int($a), Valor::Int($b)) => $int,
                        (Valor::Num($a), Valor::Num($b)) => $flt,
                        (Valor::Int(p_), Valor::Num($b)) => {
                            let $a = p_ as f64;
                            $flt
                        }
                        (Valor::Num($a), Valor::Int(q_)) => {
                            let $b = q_ as f64;
                            $flt
                        }
                        _ => {
                            let ($a, $b) = t!(self.numeros_de(x_, y_));
                            $flt
                        }
                    };
                    topo!() = r;
                }};
            }
            macro_rules! bits {
                ($a:ident, $b:ident, $r:expr) => {{
                    let y_ = pop!();
                    let x_ = topo!();
                    let ($a, $b) = match (x_, y_) {
                        (Valor::Int(p_), Valor::Int(q_)) => (p_, q_),
                        _ => {
                            let (p_, q_) = t!(self.numeros_de(x_, y_));
                            (para_i32(p_), para_i32(q_))
                        }
                    };
                    topo!() = $r;
                }};
            }
            macro_rules! comparar {
                ($int:expr, $flt:expr, $trocar:expr, $negar:expr) => {{
                    let b = pop!();
                    let a = topo!();
                    let r = match (a, b) {
                        (Valor::Int(x), Valor::Int(y)) => $int(x, y),
                        (Valor::Num(x), Valor::Num(y)) => $flt(x, y),
                        (Valor::Int(x), Valor::Num(y)) => $flt(x as f64, y),
                        (Valor::Num(x), Valor::Int(y)) => $flt(x, y as f64),
                        _ => t!(self.comparar_lento(a, b, $trocar, $negar)),
                    };
                    topo!() = Valor::Bool(r);
                }};
            }
            loop {
                let op = codigo[ip];
                ip += 1;
                match op {
                    Op::Num(i) => self.pilha.push(Valor::Num(func.nums[i as usize])),
                    Op::Int(i) => self.pilha.push(Valor::Int(i)),
                    Op::Texto(a) => self.pilha.push(Valor::Texto(a)),
                    Op::Undef => self.pilha.push(Valor::Undef),
                    Op::Nulo => self.pilha.push(Valor::Nulo),
                    Op::Verdadeiro => self.pilha.push(Valor::Bool(true)),
                    Op::Falso => self.pilha.push(Valor::Bool(false)),
                    Op::Pop => {
                        self.pilha.pop();
                    }
                    Op::Dup => {
                        let v = topo!();
                        self.pilha.push(v);
                    }
                    Op::Dup2 => {
                        let n = self.pilha.len();
                        let (a, b) = (self.pilha[n - 2], self.pilha[n - 1]);
                        self.pilha.push(a);
                        self.pilha.push(b);
                    }
                    Op::Troca => {
                        let n = self.pilha.len();
                        self.pilha.swap(n - 1, n - 2);
                    }
                    Op::Sobre => {
                        let n = self.pilha.len();
                        let a = self.pilha[n - 2];
                        self.pilha.push(a);
                    }
                    Op::Rot3 => {
                        let n = self.pilha.len();
                        self.pilha[n - 3..].rotate_right(1);
                    }
                    Op::Rot3Inv => {
                        let n = self.pilha.len();
                        self.pilha[n - 3..].rotate_left(1);
                    }
                    Op::Rot4 => {
                        let n = self.pilha.len();
                        self.pilha[n - 4..].rotate_right(1);
                    }

                    Op::LerLocal(s) => {
                        let v = self.pilha[base + s as usize];
                        self.pilha.push(v);
                    }
                    Op::EscLocal(s) => {
                        let v = pop!();
                        self.pilha[base + s as usize] = v;
                    }
                    Op::LerLocalTdz(s) => {
                        let v = self.pilha[base + s as usize];
                        if v == Valor::Vazio {
                            lancar!(self.erro_tdz());
                        }
                        self.pilha.push(v);
                    }
                    Op::EscLocalTdz(s) => {
                        let v = pop!();
                        if self.pilha[base + s as usize] == Valor::Vazio {
                            lancar!(self.erro_tdz());
                        }
                        self.pilha[base + s as usize] = v;
                    }
                    Op::LimparLocal(s) => self.pilha[base + s as usize] = Valor::Vazio,
                    Op::LerCaixa(s) => {
                        let Valor::Caixa(c) = self.pilha[base + s as usize] else { unreachable!() };
                        let v = self.caixas[c as usize];
                        if v == Valor::Vazio {
                            lancar!(self.erro_tdz());
                        }
                        self.pilha.push(v);
                    }
                    Op::EscCaixa(s) => {
                        let v = pop!();
                        let Valor::Caixa(c) = self.pilha[base + s as usize] else { unreachable!() };
                        if self.caixas[c as usize] == Valor::Vazio {
                            lancar!(self.erro_tdz());
                        }
                        self.caixas[c as usize] = v;
                    }
                    Op::IniCaixa(s) => {
                        let v = pop!();
                        let Valor::Caixa(c) = self.pilha[base + s as usize] else { unreachable!() };
                        self.caixas[c as usize] = v;
                    }
                    Op::NovaCaixa(s) => {
                        let c = self.nova_caixa(Valor::Vazio);
                        self.pilha[base + s as usize] = Valor::Caixa(c);
                    }
                    Op::NovaCaixaUndef(s) => {
                        let c = self.nova_caixa(Valor::Undef);
                        self.pilha[base + s as usize] = Valor::Caixa(c);
                    }
                    Op::Empacotar(s) => {
                        let v = self.pilha[base + s as usize];
                        let c = self.nova_caixa(v);
                        self.pilha[base + s as usize] = Valor::Caixa(c);
                    }
                    Op::CopiarCaixa(s) => {
                        let Valor::Caixa(c) = self.pilha[base + s as usize] else { unreachable!() };
                        let v = self.caixas[c as usize];
                        let novo = self.nova_caixa(v);
                        self.pilha[base + s as usize] = Valor::Caixa(novo);
                    }
                    Op::LerUp(i) => {
                        let v = self.caixas[ups[i as usize] as usize];
                        if v == Valor::Vazio {
                            lancar!(self.erro_tdz());
                        }
                        self.pilha.push(v);
                    }
                    Op::EscUp(i) => {
                        let v = pop!();
                        let c = ups[i as usize] as usize;
                        if self.caixas[c] == Valor::Vazio {
                            lancar!(self.erro_tdz());
                        }
                        self.caixas[c] = v;
                    }
                    Op::IniUp(i) => {
                        let v = pop!();
                        self.caixas[ups[i as usize] as usize] = v;
                    }
                    Op::LerGlobal(a) => {
                        let v = t!(self.ler_global(a));
                        self.pilha.push(v);
                    }
                    Op::LerGlobalTalvez(a) => {
                        let v = t!(self.ler_global_talvez(a));
                        self.pilha.push(v);
                    }
                    Op::EscGlobal(a) => {
                        let v = pop!();
                        t!(self.escrever_global(a, v, false));
                    }
                    Op::EscGlobalEstrito(a) => {
                        let v = pop!();
                        t!(self.escrever_global(a, v, true));
                    }
                    Op::DefVarGlobal(a) => {
                        let g = self.global;
                        if self.objs[g as usize].achar(a).is_none() {
                            self.def_valor(g, a, Valor::Undef, P_ESCRITA | P_ENUM);
                        }
                    }
                    Op::DefFuncGlobal(a) => {
                        let v = pop!();
                        let g = self.global;
                        self.def_valor(g, a, v, P_ESCRITA | P_ENUM);
                    }
                    Op::DefLexGlobal(a, constante) => {
                        let v = pop!();
                        let l = self.lexico;
                        self.def_valor(l, a, v, if constante { P_ENUM } else { P_ESCRITA | P_ENUM });
                    }
                    Op::ErroConst(_) => lancar!(self.erro_tipo("Assignment to constant variable.")),

                    Op::ObterProp(a) => {
                        let b = topo!();
                        // O caso mais comum: uma propriedade própria de um objeto.
                        let v = if let Valor::Obj(id) = b {
                            let o = &self.objs[id as usize];
                            match o.achar(a) {
                                Some(i) if o.props[i].flags & P_ACESSOR == 0 => o.props[i].valor,
                                _ => t!(self.obter_obj(id, a, b)),
                            }
                        } else {
                            t!(self.obter(b, a))
                        };
                        topo!() = v;
                    }
                    Op::ObterMetodo(a) => {
                        let b = topo!();
                        let v = t!(self.obter(b, a));
                        if !self.e_funcao(v) && !matches!(codigo[ip], Op::SaltoNuloChamada(_)) {
                            let nome = self.n.texto(a);
                            lancar!(self.erro_tipo(&format!("{} is not a function", nome)));
                        }
                        topo!() = v;
                        self.pilha.push(b);
                    }
                    Op::DefProp(a) => {
                        let v = pop!();
                        let b = topo!();
                        if a == K_PROTO {
                            t!(self.mudar_proto(b, v));
                        } else {
                            t!(self.definir(b, a, v));
                        }
                        topo!() = v;
                    }
                    Op::ObterElem => {
                        let k = pop!();
                        let b = topo!();
                        let v = t!(self.obter_elem(b, k));
                        topo!() = v;
                    }
                    Op::ObterMetodoElem => {
                        let k = pop!();
                        let b = topo!();
                        let v = t!(self.obter_elem(b, k));
                        topo!() = v;
                        self.pilha.push(b);
                    }
                    Op::DefElem => {
                        let v = pop!();
                        let k = pop!();
                        let b = topo!();
                        t!(self.definir_elem(b, k, v));
                        topo!() = v;
                    }
                    Op::ApagarProp(a) => {
                        let b = topo!();
                        let r = match b {
                            Valor::Obj(id) => t!(self.apagar(id, a)),
                            Valor::Undef | Valor::Nulo => lancar!(self.erro_tipo("Cannot convert undefined or null to object")),
                            _ => true,
                        };
                        topo!() = Valor::Bool(r);
                    }
                    Op::ApagarElem => {
                        let k = pop!();
                        let b = topo!();
                        let r = match b {
                            Valor::Obj(id) => {
                                let chave = t!(self.para_chave(k));
                                t!(self.apagar(id, chave))
                            }
                            Valor::Undef | Valor::Nulo => lancar!(self.erro_tipo("Cannot convert undefined or null to object")),
                            _ => true,
                        };
                        topo!() = Valor::Bool(r);
                    }
                    Op::ObterSuper(a) => {
                        let home = pop!();
                        let this = topo!();
                        let v = t!(self.super_obter(home, a, this));
                        topo!() = v;
                    }
                    Op::ObterSuperElem => {
                        let k = pop!();
                        let home = pop!();
                        let this = topo!();
                        let chave = t!(self.para_chave(k));
                        let v = t!(self.super_obter(home, chave, this));
                        topo!() = v;
                    }
                    Op::DefSuper(a) => {
                        let v = pop!();
                        let _home = pop!();
                        let this = topo!();
                        t!(self.definir(this, a, v));
                        topo!() = v;
                    }
                    Op::DefSuperElem => {
                        let v = pop!();
                        let k = pop!();
                        let _home = pop!();
                        let this = topo!();
                        let chave = t!(self.para_chave(k));
                        t!(self.definir(this, chave, v));
                        topo!() = v;
                    }

                    Op::Soma => {
                        let b = pop!();
                        let a = topo!();
                        let r = match (a, b) {
                            (Valor::Int(x), Valor::Int(y)) => match x.checked_add(y) {
                                Some(z) => Valor::Int(z),
                                None => Valor::Num(x as f64 + y as f64),
                            },
                            (Valor::Num(x), Valor::Num(y)) => Valor::Num(x + y),
                            (Valor::Int(x), Valor::Num(y)) => Valor::Num(x as f64 + y),
                            (Valor::Num(x), Valor::Int(y)) => Valor::Num(x + y as f64),
                            (Valor::Texto(x), Valor::Texto(y)) => Valor::Texto(self.n.concatenar(x, y)),
                            _ => t!(self.somar_lento(a, b)),
                        };
                        topo!() = r;
                    }
                    Op::Sub => binario_num!(
                        a,
                        b,
                        match a.checked_sub(b) {
                            Some(z) => Valor::Int(z),
                            None => Valor::Num(a as f64 - b as f64),
                        },
                        Valor::Num(a - b)
                    ),
                    Op::Mul => binario_num!(
                        a,
                        b,
                        match a.checked_mul(b) {
                            Some(z) if z != 0 || (a >= 0 && b >= 0) => Valor::Int(z),
                            _ => Valor::Num(a as f64 * b as f64),
                        },
                        Valor::Num(a * b)
                    ),
                    Op::Div => binario_num!(
                        a,
                        b,
                        if b != 0 && a != 0 && a % b == 0 && !(a == i32::MIN && b == -1) { Valor::Int(a / b) } else { num(a as f64 / b as f64) },
                        num(a / b)
                    ),
                    Op::Mod => binario_num!(
                        a,
                        b,
                        if b > 0 && a >= 0 { Valor::Int(a % b) } else { num(mat::fmod(a as f64, b as f64)) },
                        num(mat::fmod(a, b))
                    ),
                    Op::Pot => {
                        let b = pop!();
                        let a = topo!();
                        let (x, y) = t!(self.numeros_de(a, b));
                        topo!() = num(mat::pow(x, y));
                    }
                    Op::BitE => bits!(a, b, Valor::Int(a & b)),
                    Op::BitOu => bits!(a, b, Valor::Int(a | b)),
                    Op::BitXor => bits!(a, b, Valor::Int(a ^ b)),
                    Op::Shl => bits!(a, b, Valor::Int(a.wrapping_shl(b as u32 & 31))),
                    Op::Shr => bits!(a, b, Valor::Int(a >> (b as u32 & 31))),
                    Op::Ushr => bits!(a, b, num(((a as u32) >> (b as u32 & 31)) as f64)),
                    Op::Neg => {
                        let a = topo!();
                        let r = match a {
                            Valor::Int(i) if i != 0 && i != i32::MIN => Valor::Int(-i),
                            _ => num(-t!(self.para_numero(a))),
                        };
                        topo!() = r;
                    }
                    Op::Pos => {
                        let a = topo!();
                        if !matches!(a, Valor::Int(_) | Valor::Num(_)) {
                            topo!() = num(t!(self.para_numero(a)));
                        }
                    }
                    Op::Nao => {
                        let a = topo!();
                        topo!() = Valor::Bool(!self.para_bool(a));
                    }
                    Op::BitNao => {
                        let a = topo!();
                        let i = t!(self.para_i32(a));
                        topo!() = Valor::Int(!i);
                    }
                    Op::Typeof => {
                        let a = topo!();
                        topo!() = Valor::Texto(self.tipo_de(a));
                    }
                    Op::Igual => {
                        let b = pop!();
                        let a = topo!();
                        let r = t!(self.igual_frouxo(a, b));
                        topo!() = Valor::Bool(r);
                    }
                    Op::Dif => {
                        let b = pop!();
                        let a = topo!();
                        let r = t!(self.igual_frouxo(a, b));
                        topo!() = Valor::Bool(!r);
                    }
                    Op::Igual3 => {
                        let b = pop!();
                        let a = topo!();
                        topo!() = Valor::Bool(self.igual_estrito(a, b));
                    }
                    Op::Dif3 => {
                        let b = pop!();
                        let a = topo!();
                        topo!() = Valor::Bool(!self.igual_estrito(a, b));
                    }
                    Op::Menor => comparar!(|x, y| x < y, |x: f64, y: f64| x < y, false, false),
                    Op::MenorIgual => comparar!(|x, y| x <= y, |x: f64, y: f64| x <= y, true, true),
                    Op::Maior => comparar!(|x, y| x > y, |x: f64, y: f64| x > y, true, false),
                    Op::MaiorIgual => comparar!(|x, y| x >= y, |x: f64, y: f64| x >= y, false, true),
                    Op::Em => {
                        let o = pop!();
                        let k = topo!();
                        let Valor::Obj(id) = o else {
                            lancar!(self.erro_tipo("Cannot use 'in' operator to search for a key in a non-object"));
                        };
                        let chave = t!(self.para_chave(k));
                        topo!() = Valor::Bool(t!(self.tem_prop(id, chave)));
                    }
                    Op::InstanciaDe => {
                        let c = pop!();
                        let v = topo!();
                        topo!() = Valor::Bool(t!(self.instancia_de(v, c)));
                    }
                    Op::Incr => {
                        let a = topo!();
                        topo!() = match a {
                            Valor::Int(i) if i < i32::MAX => Valor::Int(i + 1),
                            Valor::Int(i) => Valor::Num(i as f64 + 1.0),
                            Valor::Num(n) => Valor::Num(n + 1.0),
                            _ => Valor::Num(f64::NAN),
                        };
                    }
                    Op::Decr => {
                        let a = topo!();
                        topo!() = match a {
                            Valor::Int(i) if i > i32::MIN => Valor::Int(i - 1),
                            Valor::Int(i) => Valor::Num(i as f64 - 1.0),
                            Valor::Num(n) => Valor::Num(n - 1.0),
                            _ => Valor::Num(f64::NAN),
                        };
                    }
                    Op::ParaNumero => {
                        let a = topo!();
                        if !matches!(a, Valor::Int(_) | Valor::Num(_)) {
                            topo!() = num(t!(self.para_numero(a)));
                        }
                    }
                    Op::ParaChave => {
                        let a = topo!();
                        if let Valor::Obj(_) = a {
                            let k = t!(self.para_chave(a));
                            topo!() = self.valor_da_chave(k);
                        }
                    }

                    Op::Salto(d) => {
                        let d = d as usize;
                        if d < ip {
                            // Uma volta de laço: um ponto seguro.
                            t!(self.ponto_seguro());
                        }
                        ip = d;
                    }
                    Op::SaltoFalso(d) => {
                        let v = pop!();
                        let b = match v {
                            Valor::Bool(b) => b,
                            _ => self.para_bool(v),
                        };
                        if !b {
                            ip = d as usize;
                        }
                    }
                    Op::SaltoVerdadeiro(d) => {
                        let v = pop!();
                        if self.para_bool(v) {
                            let d = d as usize;
                            if d < ip {
                                t!(self.ponto_seguro());
                            }
                            ip = d;
                        }
                    }
                    Op::SaltoFalsoMantem(d) => {
                        let v = topo!();
                        if !self.para_bool(v) {
                            ip = d as usize;
                        } else {
                            self.pilha.pop();
                        }
                    }
                    Op::SaltoVerdadeiroMantem(d) => {
                        let v = topo!();
                        if self.para_bool(v) {
                            ip = d as usize;
                        } else {
                            self.pilha.pop();
                        }
                    }
                    Op::SaltoNaoNuloMantem(d) => {
                        if !topo!().nulo_ou_undef() {
                            ip = d as usize;
                        } else {
                            self.pilha.pop();
                        }
                    }
                    Op::SaltoNuloPop(d) => {
                        if topo!().nulo_ou_undef() {
                            self.pilha.pop();
                            ip = d as usize;
                        }
                    }
                    Op::SaltoNuloChamada(d) => {
                        let n = self.pilha.len();
                        if self.pilha[n - 2].nulo_ou_undef() {
                            self.pilha.truncate(n - 2);
                            ip = d as usize;
                        }
                    }
                    Op::SaltoNaoUndef(d) => {
                        if topo!() != Valor::Undef {
                            ip = d as usize;
                        } else {
                            self.pilha.pop();
                        }
                    }

                    Op::Chamar(n) => {
                        let n = n as usize;
                        let inicio = self.pilha.len() - n - 2;
                        let f = self.pilha[inicio];
                        if let Valor::Obj(id) = f {
                            if let Tipo::Funcao(c) = &self.objs[id as usize].tipo {
                                if !c.func.construtor {
                                    self.quadros.last_mut().unwrap().ip = ip;
                                    t!(self.ponto_seguro());
                                    let this = self.pilha[inicio + 1];
                                    t!(self.empilhar_quadro(id, this, n, Valor::Undef, false));
                                    continue 'quadro;
                                }
                            }
                        }
                        let this = self.pilha[inicio + 1];
                        let r = self.chamar_da_pilha(f, this, inicio + 2);
                        self.pilha.truncate(inicio);
                        let r = t!(r);
                        self.pilha.push(r);
                    }
                    Op::ChamarEsp => {
                        let lista = pop!();
                        let this = pop!();
                        let f = topo!();
                        let args = t!(self.lista_de_array(lista));
                        let r = t!(self.chamar(f, this, &args));
                        topo!() = r;
                    }
                    Op::Novo(n) => {
                        let n = n as usize;
                        let inicio = self.pilha.len() - n - 1;
                        let f = self.pilha[inicio];
                        let args = self.pilha[inicio + 1..].to_vec();
                        let r = self.construir(f, &args, f);
                        self.pilha.truncate(inicio);
                        let r = t!(r);
                        self.pilha.push(r);
                    }
                    Op::NovoEsp => {
                        let lista = pop!();
                        let f = topo!();
                        let args = t!(self.lista_de_array(lista));
                        let r = t!(self.construir(f, &args, f));
                        topo!() = r;
                    }
                    Op::ChamarSuper(n) => {
                        let n = n as usize;
                        let inicio = self.pilha.len() - n - 2;
                        let (f, nt) = (self.pilha[inicio], self.pilha[inicio + 1]);
                        let args = self.pilha[inicio + 2..].to_vec();
                        let r = self.chamar_super(f, nt, &args);
                        self.pilha.truncate(inicio);
                        let r = t!(r);
                        self.pilha.push(r);
                    }
                    Op::ChamarSuperEsp => {
                        let lista = pop!();
                        let nt = pop!();
                        let f = topo!();
                        let args = t!(self.lista_de_array(lista));
                        let r = t!(self.chamar_super(f, nt, &args));
                        topo!() = r;
                    }
                    Op::IniciarCampos => {
                        let f = pop!();
                        let this = topo!();
                        t!(self.iniciar_campos(this, f));
                    }
                    Op::Retornar | Op::RetornarUndef => {
                        let mut v = if matches!(op, Op::Retornar) { pop!() } else { Valor::Undef };
                        let q = self.quadros.pop().unwrap();
                        self.pilha.truncate(q.base - 2);
                        if q.construindo && v.obj().is_none() {
                            v = q.this;
                        }
                        if q.gerador != NENHUM {
                            self.terminar_gerador(q.gerador);
                        }
                        if self.quadros.len() == limite {
                            return Ok(v);
                        }
                        self.pilha.push(v);
                        continue 'quadro;
                    }
                    Op::Lancar => lancar!(pop!()),
                    Op::Tentar(d) => {
                        let sp = (self.pilha.len() - base) as u32;
                        self.quadros.last_mut().unwrap().handlers.push((d, sp));
                    }
                    Op::FimTentar => {
                        self.quadros.last_mut().unwrap().handlers.pop();
                    }
                    Op::Gosub(d) => {
                        self.pilha.push(Valor::Int(ip as i32));
                        ip = d as usize;
                    }
                    Op::RetGosub => {
                        if let Valor::Int(d) = pop!() {
                            ip = d as usize;
                        }
                    }

                    Op::IterInicio => {
                        let o = pop!();
                        let (it, nx) = t!(self.iter_inicio(o));
                        self.pilha.push(it);
                        self.pilha.push(nx);
                    }
                    Op::IterInicioAsync => {
                        let o = pop!();
                        let (it, nx) = t!(self.iter_inicio_async(o));
                        self.pilha.push(it);
                        self.pilha.push(nx);
                    }
                    Op::IterProximo(d) => {
                        let n = self.pilha.len();
                        let (it, nx) = (self.pilha[n - 2], self.pilha[n - 1]);
                        // Array sem iterador customizado: nx é o índice.
                        if let (Valor::Int(i), Valor::Obj(a)) = (nx, it) {
                            if let Tipo::Array(v) = &self.objs[a as usize].tipo {
                                match v.get(i as usize) {
                                    Some(&x) => {
                                        self.pilha[n - 1] = Valor::Int(i + 1);
                                        self.pilha.push(if x == Valor::Vazio { Valor::Undef } else { x });
                                    }
                                    None => ip = d as usize,
                                }
                                continue;
                            }
                        }
                        match t!(self.iter_passo(it, nx)) {
                            Some(v) => self.pilha.push(v),
                            None => ip = d as usize,
                        }
                    }
                    Op::IterPasso => {
                        let n = self.pilha.len();
                        let (it, nx) = (self.pilha[n - 2], self.pilha[n - 1]);
                        if nx == Valor::Vazio {
                            self.pilha.push(Valor::Undef);
                            continue;
                        }
                        if let (Valor::Int(i), Valor::Obj(a)) = (nx, it) {
                            if let Tipo::Array(v) = &self.objs[a as usize].tipo {
                                let x = v.get(i as usize).copied().unwrap_or(Valor::Undef);
                                self.pilha[n - 1] = Valor::Int(i + 1);
                                self.pilha.push(if x == Valor::Vazio { Valor::Undef } else { x });
                                continue;
                            }
                        }
                        match t!(self.iter_passo(it, nx)) {
                            Some(v) => self.pilha.push(v),
                            None => {
                                self.pilha[n - 1] = Valor::Vazio;
                                self.pilha.push(Valor::Undef);
                            }
                        }
                    }
                    Op::IterResto => {
                        let n = self.pilha.len();
                        let (it, nx) = (self.pilha[n - 2], self.pilha[n - 1]);
                        let mut v = Vec::new();
                        if nx != Valor::Vazio {
                            if let (Valor::Int(i), Valor::Obj(a)) = (nx, it) {
                                if let Tipo::Array(x) = &self.objs[a as usize].tipo {
                                    v.extend(x.iter().skip(i as usize).map(|e| if *e == Valor::Vazio { Valor::Undef } else { *e }));
                                }
                            } else {
                                while let Some(x) = t!(self.iter_passo(it, nx)) {
                                    v.push(x);
                                }
                            }
                            self.pilha[n - 1] = Valor::Vazio;
                        }
                        let arr = self.novo_array(v);
                        self.pilha.push(arr);
                    }
                    Op::IterFechar => {
                        let nx = pop!();
                        let it = pop!();
                        if !matches!(nx, Valor::Int(_) | Valor::Vazio) {
                            t!(self.iter_fechar(it));
                        }
                    }
                    Op::IterFecharErro => {
                        let e = pop!();
                        let nx = pop!();
                        let it = pop!();
                        if !matches!(nx, Valor::Int(_) | Valor::Vazio) {
                            let _ = self.iter_fechar(it);
                        }
                        lancar!(e);
                    }
                    Op::IterChamarProximo => {
                        let n = self.pilha.len();
                        let (it, nx) = (self.pilha[n - 2], self.pilha[n - 1]);
                        let r = t!(self.chamar(nx, it, &[]));
                        self.pilha.push(r);
                    }
                    Op::IterDesempacota(d) => {
                        let r = pop!();
                        let feito = t!(self.obter(r, self.a.done));
                        if self.para_bool(feito) {
                            ip = d as usize;
                        } else {
                            let v = t!(self.obter(r, self.a.value));
                            self.pilha.push(v);
                        }
                    }
                    Op::ForInInicio => {
                        let o = pop!();
                        let chaves = t!(self.chaves_for_in(o));
                        self.pilha.push(chaves);
                        self.pilha.push(Valor::Int(0));
                    }
                    Op::ChecarObj => {
                        if topo!().nulo_ou_undef() {
                            lancar!(self.erro_tipo("Cannot destructure 'undefined' as it is undefined."));
                        }
                    }

                    Op::NovoObjeto => {
                        let o = self.novo_objeto();
                        self.pilha.push(Valor::Obj(o));
                    }
                    Op::NovoArray(n) => {
                        let inicio = self.pilha.len() - n as usize;
                        let v = self.pilha[inicio..].to_vec();
                        self.pilha.truncate(inicio);
                        let a = self.novo_array(v);
                        self.pilha.push(a);
                    }
                    Op::ArrayPush => {
                        let v = pop!();
                        if let Valor::Obj(a) = topo!() {
                            if let Tipo::Array(x) = &mut self.objs[a as usize].tipo {
                                x.push(v);
                            }
                        }
                    }
                    Op::ArrayBuraco => {
                        if let Valor::Obj(a) = topo!() {
                            if let Tipo::Array(x) = &mut self.objs[a as usize].tipo {
                                x.push(Valor::Vazio);
                            }
                        }
                    }
                    Op::ArrayEspalhar => {
                        let o = pop!();
                        let itens = t!(self.iteravel_para_lista(o));
                        if let Valor::Obj(a) = topo!() {
                            if let Tipo::Array(x) = &mut self.objs[a as usize].tipo {
                                x.extend(itens);
                            }
                        }
                    }
                    Op::ObjProp(a) => {
                        let v = pop!();
                        if let Valor::Obj(o) = topo!() {
                            self.def_valor(o, a, v, P_TUDO);
                        }
                    }
                    Op::ObjElem => {
                        let v = pop!();
                        let k = pop!();
                        let chave = t!(self.para_chave(k));
                        if let Valor::Obj(o) = topo!() {
                            self.dar_nome_se_funcao(v, chave);
                            self.definir_propria(o, chave, v, P_TUDO);
                        }
                    }
                    Op::ObjEspalhar => {
                        let fonte = pop!();
                        if let Valor::Obj(o) = topo!() {
                            t!(self.copiar_props(o, fonte, &[]));
                        }
                    }
                    Op::ObjResto(n) => {
                        let inicio = self.pilha.len() - n as usize;
                        let ks = self.pilha[inicio..].to_vec();
                        self.pilha.truncate(inicio);
                        let mut fora = Vec::with_capacity(ks.len());
                        for k in ks {
                            fora.push(t!(self.para_chave(k)));
                        }
                        let fonte = topo!();
                        let o = self.novo_objeto();
                        self.pilha.push(Valor::Obj(o));
                        t!(self.copiar_props(o, fonte, &fora));
                    }
                    Op::DefMetodo(tipo) => {
                        let f = pop!();
                        let k = pop!();
                        let Valor::Obj(alvo) = topo!() else { unreachable!() };
                        let chave = t!(self.para_chave(k));
                        if let Valor::Obj(fid) = f {
                            if let Tipo::Funcao(c) = &mut self.objs[fid as usize].tipo {
                                c.home = Valor::Obj(alvo);
                            }
                            self.dar_nome_se_funcao(f, chave);
                        }
                        let enumeravel = if tipo & 4 != 0 { P_ENUM } else { 0 };
                        match tipo & 3 {
                            0 => self.def_valor(alvo, chave, f, P_ESCRITA | P_CONFIG | enumeravel),
                            t2 => self.def_acessor_metodo(alvo, chave, f, t2 == 1, P_CONFIG | enumeravel),
                        }
                    }
                    Op::DefCampo => {
                        let ini = pop!();
                        let k = pop!();
                        let Valor::Obj(ctor) = topo!() else { unreachable!() };
                        let proto = t!(self.obter_obj(ctor, K_PROTOTYPE, Valor::Obj(ctor)));
                        if let Valor::Obj(fid) = ini {
                            if let Tipo::Funcao(c) = &mut self.objs[fid as usize].tipo {
                                c.home = proto;
                            }
                        }
                        if let Tipo::Funcao(c) = &mut self.objs[ctor as usize].tipo {
                            c.campos.push((k, ini));
                        }
                    }
                    Op::CampoEstatico => {
                        let ini = pop!();
                        let k = pop!();
                        let Valor::Obj(ctor) = topo!() else { unreachable!() };
                        if let Valor::Obj(fid) = ini {
                            if let Tipo::Funcao(c) = &mut self.objs[fid as usize].tipo {
                                c.home = Valor::Obj(ctor);
                            }
                        }
                        let v = if ini == Valor::Undef { Valor::Undef } else { t!(self.chamar(ini, Valor::Obj(ctor), &[])) };
                        if k != Valor::Texto(K_VAZIO) {
                            let chave = t!(self.para_chave(k));
                            self.definir_propria(ctor, chave, v, P_TUDO);
                        }
                    }
                    Op::CriarClasse(i, tem_pai) => {
                        let f = func.funcoes[i as usize].clone();
                        let ups_f = self.capturar(&f, base, &ups);
                        let (proto_pai, ctor_pai) = if tem_pai {
                            let pai = pop!();
                            if pai == Valor::Nulo {
                                (NENHUM, self.p.funcao)
                            } else if !self.e_construtor(pai) {
                                lancar!(self.erro_tipo("Class extends value is not a constructor or null"));
                            } else {
                                let pp = t!(self.obter(pai, K_PROTOTYPE));
                                let pp = match pp {
                                    Valor::Obj(p) => p,
                                    _ => NENHUM,
                                };
                                (pp, pai.obj().unwrap_or(self.p.funcao))
                            }
                        } else {
                            (self.p.objeto, self.p.funcao)
                        };
                        let proto = self.novo_obj_com(proto_pai, Tipo::Comum);
                        let ctor = self.novo_obj_com(ctor_pai, Tipo::Funcao(Box::new(Closure { func: f, ups: ups_f, home: Valor::Obj(proto), campos: Vec::new() })));
                        self.def_valor(ctor, K_PROTOTYPE, Valor::Obj(proto), 0);
                        self.def_valor(proto, K_CONSTRUCTOR, Valor::Obj(ctor), P_OCULTA);
                        self.pilha.push(Valor::Obj(ctor));
                        self.pilha.push(Valor::Obj(proto));
                    }
                    Op::Closure(i) => {
                        let f = func.funcoes[i as usize].clone();
                        let ups_f = self.capturar(&f, base, &ups);
                        let c = self.closure_de(f, ups_f);
                        self.pilha.push(Valor::Obj(c));
                    }
                    Op::Regex(c, f) => {
                        let (fonte, flags) = (self.n.texto(c), self.n.texto(f));
                        let r = t!(super::nativos2::nova_regex(self, &fonte, &flags));
                        self.pilha.push(r);
                    }
                    Op::Template(n) => {
                        let inicio = self.pilha.len() - n as usize;
                        let mut acum = K_VAZIO;
                        for i in 0..n as usize {
                            let v = self.pilha[inicio + i];
                            let id = t!(self.para_texto_id(v));
                            acum = self.n.concatenar(acum, id);
                        }
                        self.pilha.truncate(inicio);
                        self.pilha.push(Valor::Texto(acum));
                    }
                    Op::TemplateObj(i) => {
                        let pedacos = &func.templates[i as usize];
                        let cozidos: Vec<Valor> = pedacos.iter().map(|(_, c)| if *c == u32::MAX { Valor::Undef } else { Valor::Texto(*c) }).collect();
                        let crus: Vec<Valor> = pedacos.iter().map(|(r, _)| Valor::Texto(*r)).collect();
                        let cr = self.novo_array(crus);
                        let co = self.novo_array(cozidos);
                        if let Valor::Obj(id) = co {
                            let k = self.a.raw;
                            self.def_valor(id, k, cr, 0);
                        }
                        self.pilha.push(co);
                    }
                    Op::LerThis => {
                        let v = self.quadros.last().unwrap().this;
                        self.pilha.push(v);
                    }
                    Op::LerCallee => {
                        let v = self.quadros.last().unwrap().callee;
                        self.pilha.push(v);
                    }
                    Op::LerNovoAlvo => {
                        let v = self.quadros.last().unwrap().nt;
                        self.pilha.push(v);
                    }
                    Op::LerHome => {
                        let c = self.quadros.last().unwrap().callee;
                        let h = match c {
                            Valor::Obj(id) => match &self.objs[id as usize].tipo {
                                Tipo::Funcao(f) => f.home,
                                _ => Valor::Undef,
                            },
                            _ => Valor::Undef,
                        };
                        self.pilha.push(h);
                    }
                    Op::Argumentos => {
                        let args = self.quadros.last().unwrap().args.clone();
                        let a = self.novo_array(args);
                        if let Valor::Obj(id) = a {
                            let proto = self.p.objeto;
                            let o = &mut self.objs[id as usize];
                            o.proto = proto;
                            o.flags |= O_ARGUMENTOS;
                            let it = self.array_iter_original;
                            let k = self.s.iterator;
                            self.def_valor(id, k, it, P_OCULTA);
                        }
                        self.pilha.push(a);
                    }
                    Op::Resto(i) => {
                        let args = &self.quadros.last().unwrap().args;
                        let v = args.get(i as usize..).map_or(Vec::new(), |r| r.to_vec());
                        let a = self.novo_array(v);
                        self.pilha.push(a);
                    }
                    Op::Gerador => {
                        // A função vira um objeto; o quadro atual sai da pilha.
                        let mut q = self.quadros.pop().unwrap();
                        let salvo = self.pilha[q.base..].to_vec();
                        self.pilha.truncate(q.base - 2);
                        let eh_gerador = q.func.gerador;
                        let eh_assinc = q.func.assincrona;
                        let proto = if eh_gerador {
                            match self.obter(q.callee, K_PROTOTYPE) {
                                Ok(Valor::Obj(po)) => po,
                                _ => {
                                    if eh_assinc { self.p.gerador_assinc } else { self.p.gerador }
                                }
                            }
                        } else {
                            self.p.objeto
                        };
                        let promessa = if eh_assinc && !eh_gerador { super::nativos2::nova_promessa(self) } else { Valor::Undef };
                        let g = Gerador {
                            estado: EstadoGerador::Novo,
                            func: q.func.clone(),
                            ups: q.ups.clone(),
                            callee: q.callee,
                            this: q.this,
                            nt: q.nt,
                            args: core::mem::take(&mut q.args),
                            ip,
                            pilha: salvo,
                            handlers: Vec::new(),
                            promessa,
                            fila: Vec::new(),
                        };
                        let id = self.novo_obj_com(proto, Tipo::Gerador(Box::new(g)));
                        let resultado = if eh_assinc && !eh_gerador {
                            self.pilha.push(Valor::Obj(id));
                            self.pilha.push(promessa);
                            super::nativos2::avancar_assincrona(self, id, 0, Valor::Undef);
                            self.pilha.pop();
                            self.pilha.pop();
                            promessa
                        } else {
                            Valor::Obj(id)
                        };
                        if self.quadros.len() == limite {
                            return Ok(resultado);
                        }
                        self.pilha.push(resultado);
                        continue 'quadro;
                    }
                    Op::AwaitSePromessa | Op::Yield | Op::Await => {
                        if matches!(op, Op::AwaitSePromessa) && !matches!(topo!(), Valor::Obj(o) if matches!(self.objs[o as usize].tipo, Tipo::Promessa(_))) {
                            continue;
                        }
                        let v = pop!();
                        let q = self.quadros.pop().unwrap();
                        let id = q.gerador;
                        if id == NENHUM {
                            lancar!(self.erro_sintaxe("await/yield fora de um gerador"));
                        }
                        let salvo = self.pilha[q.base..].to_vec();
                        self.pilha.truncate(q.base - 2);
                        let e_yield = matches!(op, Op::Yield);
                        if let Tipo::Gerador(g) = &mut self.objs[id as usize].tipo {
                            g.ip = ip;
                            g.pilha = salvo;
                            g.handlers = q.handlers;
                            g.args = q.args;
                            g.estado = if e_yield { EstadoGerador::Suspenso } else { EstadoGerador::SuspensoAwait };
                        }
                        self.sinal = if e_yield { Sinal::Yield } else { Sinal::Await };
                        return Ok(v);
                    }
                    Op::YieldDelegado => {}
                    Op::DelegadoPasso(d) => {
                        let enviado = pop!();
                        let n = self.pilha.len();
                        let (it, nx) = (self.pilha[n - 2], self.pilha[n - 1]);
                        // Um array no caminho rápido: nx é o índice.
                        if let (Valor::Int(i), Valor::Obj(a)) = (nx, it) {
                            if let Tipo::Array(x) = &self.objs[a as usize].tipo {
                                match x.get(i as usize) {
                                    Some(&e) => {
                                        self.pilha[n - 1] = Valor::Int(i + 1);
                                        self.pilha.push(e);
                                    }
                                    None => {
                                        self.pilha.push(Valor::Undef);
                                        ip = d as usize;
                                    }
                                }
                                continue;
                            }
                        }
                        let r = t!(self.chamar(nx, it, &[enviado]));
                        let feito = t!(self.obter(r, self.a.done));
                        let v = t!(self.obter(r, self.a.value));
                        self.pilha.push(v);
                        if self.para_bool(feito) {
                            ip = d as usize;
                        }
                    }
                }
            }
        }
    }

    #[inline]
    fn ponto_seguro(&mut self) -> Result<(), Valor> {
        if self.n.excedeu {
            self.n.excedeu = false;
            return Err(self.erro_intervalo("Invalid string length"));
        }
        self.orcamento -= 1;
        if self.orcamento <= 0 {
            self.orcamento = 20_000;
            if self.prazo_ms != 0 && (self.ganchos.agora_ms)() > self.prazo_ms {
                self.abortando = true;
                return Err(self.erro_intervalo("script demorou demais e foi interrompido"));
            }
        }
        if self.alocados + self.n.bytes_novos / 64 > self.limite_gc {
            // Só no nível de fora: dentro de uma nativa há valores nas variáveis do Rust.
            if self.prof_nativa == 1 {
                self.coletar_lixo();
            } else if self.objs.len() - self.obj_livres.len() > LIMITE_OBJETOS {
                self.abortando = true;
                return Err(self.erro_intervalo("sem memória para o script"));
            }
        }
        Ok(())
    }

    /// Roda o coletor, se valer a pena, fora de qualquer execução.
    pub fn coletar_se_preciso(&mut self) {
        if self.prof_nativa == 0 && self.alocados + self.n.bytes_novos / 64 > self.limite_gc / 4 {
            self.coletar_lixo();
        }
    }

    fn capturar(&mut self, f: &Rc<Funcao>, base: usize, ups: &Rc<[u32]>) -> Rc<[u32]> {
        if f.ups.is_empty() {
            return Rc::from(Vec::new());
        }
        let mut v = Vec::with_capacity(f.ups.len());
        for &(local, i) in &f.ups {
            if local {
                match self.pilha[base + i as usize] {
                    Valor::Caixa(c) => v.push(c),
                    // A variável ainda não tem caixa (uma função içada antes
                    // da declaração, num caminho raro): cria agora.
                    outro => {
                        let c = self.nova_caixa(outro);
                        self.pilha[base + i as usize] = Valor::Caixa(c);
                        v.push(c);
                    }
                }
            } else {
                v.push(ups[i as usize]);
            }
        }
        Rc::from(v)
    }

    fn erro_tdz(&mut self) -> Valor {
        self.erro_ref("Cannot access variable before initialization")
    }

    pub fn mudar_proto(&mut self, b: Valor, v: Valor) -> Result<(), Valor> {
        if let Valor::Obj(id) = b {
            match v {
                Valor::Obj(p) => self.objs[id as usize].proto = p,
                Valor::Nulo => self.objs[id as usize].proto = NENHUM,
                _ => {}
            }
        }
        Ok(())
    }

    fn chamar_da_pilha(&mut self, f: Valor, this: Valor, inicio_args: usize) -> Res {
        let n = self.pilha.len() - inicio_args;
        if n <= 6 {
            let mut buf = [Valor::Undef; 6];
            buf[..n].copy_from_slice(&self.pilha[inicio_args..]);
            self.chamar(f, this, &buf[..n])
        } else {
            let args = self.pilha[inicio_args..].to_vec();
            self.chamar(f, this, &args)
        }
    }

    fn chamar_super(&mut self, f: Valor, nt: Valor, args: &[Valor]) -> Res {
        let Valor::Obj(id) = f else { return Err(self.erro_sintaxe("super fora de um construtor")) };
        let pai = self.objs[id as usize].proto;
        if pai == NENHUM || !self.e_construtor(Valor::Obj(pai)) {
            // `class A extends null` etc.
            let proto = self.proto_de_novo_alvo(nt, self.p.objeto)?;
            return Ok(Valor::Obj(self.novo_obj_com(proto, Tipo::Comum)));
        }
        self.construir(Valor::Obj(pai), args, nt)
    }

    fn super_obter(&mut self, home: Valor, k: Txt, this: Valor) -> Res {
        let Valor::Obj(h) = home else { return Ok(Valor::Undef) };
        let p = self.objs[h as usize].proto;
        if p == NENHUM {
            return Ok(Valor::Undef);
        }
        self.obter_obj(p, k, this)
    }

    fn dar_nome_se_funcao(&mut self, f: Valor, chave: Txt) {
        // Só métodos com chave computada têm o nome em branco.
        let Valor::Obj(fid) = f else { return };
        let simbolo = self.n.e_simbolo(chave);
        let o = &self.objs[fid as usize];
        if let Tipo::Funcao(c) = &o.tipo {
            if c.func.nome == K_VAZIO && o.flags & O_MATERIAL == 0 {
                let nome = if simbolo {
                    let d = self.n.texto(chave);
                    let t = format!("[{}]", d);
                    self.n.atomo(&t)
                } else {
                    chave
                };
                self.def_valor(fid, K_NAME, Valor::Texto(nome), P_CONFIG);
            }
        }
    }

    pub fn def_acessor_metodo(&mut self, alvo: ObjId, chave: Txt, f: Valor, get: bool, flags: u8) {
        let o = &self.objs[alvo as usize];
        let (mut g, mut s) = (Valor::Undef, Valor::Undef);
        if let Some(i) = o.achar(chave) {
            let p = o.props[i];
            if p.flags & P_ACESSOR != 0 {
                if let Valor::Obj(pid) = p.valor {
                    if let Tipo::Acessor(a, b) = self.objs[pid as usize].tipo {
                        g = a;
                        s = b;
                    }
                }
            }
        }
        if get {
            g = f;
        } else {
            s = f;
        }
        self.def_par(alvo, chave, g, s, flags);
    }

    pub fn copiar_props(&mut self, destino: ObjId, fonte: Valor, fora: &[Txt]) -> Result<(), Valor> {
        match fonte {
            Valor::Obj(f) => {
                let chaves = self.chaves_proprias(f, true, true, true)?;
                for k in chaves {
                    if !fora.contains(&k) {
                        let v = self.obter_obj(f, k, fonte)?;
                        self.definir_propria(destino, k, v, P_TUDO);
                    }
                }
            }
            Valor::Texto(_) => {
                let f = self.para_objeto(fonte)?;
                return self.copiar_props(destino, Valor::Obj(f), fora);
            }
            _ => {}
        }
        Ok(())
    }

    // ------------------------------------------------------------ globais

    fn ler_global(&mut self, a: Txt) -> Res {
        let l = &self.objs[self.lexico as usize];
        if let Some(i) = l.achar(a) {
            return Ok(l.props[i].valor);
        }
        let g = self.global;
        let o = &self.objs[g as usize];
        if let Some(i) = o.achar(a) {
            let p = o.props[i];
            if p.flags & P_ACESSOR == 0 {
                return Ok(p.valor);
            }
            return self.obter_obj(g, a, Valor::Obj(g));
        }
        if a == K_THIS_VAR {
            return Ok(Valor::Obj(g));
        }
        if self.tem_prop(g, a)? {
            return self.obter_obj(g, a, Valor::Obj(g));
        }
        let nome = self.n.texto(a);
        Err(self.erro_ref(&format!("{} is not defined", nome)))
    }

    fn ler_global_talvez(&mut self, a: Txt) -> Res {
        let l = &self.objs[self.lexico as usize];
        if let Some(i) = l.achar(a) {
            return Ok(l.props[i].valor);
        }
        let g = self.global;
        self.obter_obj(g, a, Valor::Obj(g))
    }

    fn escrever_global(&mut self, a: Txt, v: Valor, estrito: bool) -> Result<(), Valor> {
        let l = &mut self.objs[self.lexico as usize];
        if let Some(i) = l.achar(a) {
            if l.props[i].flags & P_ESCRITA == 0 {
                return Err(self.erro_tipo("Assignment to constant variable."));
            }
            l.props[i].valor = v;
            return Ok(());
        }
        let g = self.global;
        if estrito && !self.tem_prop(g, a)? {
            let nome = self.n.texto(a);
            return Err(self.erro_ref(&format!("{} is not defined", nome)));
        }
        self.definir_obj(g, a, v, Valor::Obj(g))
    }

    // ------------------------------------------------------------ operadores

    fn numeros_de(&mut self, a: Valor, b: Valor) -> Result<(f64, f64), Valor> {
        let x = self.para_numero(a)?;
        let y = self.para_numero(b)?;
        Ok((x, y))
    }

    fn somar_lento(&mut self, a: Valor, b: Valor) -> Res {
        let x = self.para_primitivo(a, None)?;
        let y = self.para_primitivo(b, None)?;
        if matches!(x, Valor::Texto(_)) || matches!(y, Valor::Texto(_)) {
            let p = self.para_texto_id(x)?;
            let q = self.para_texto_id(y)?;
            return Ok(Valor::Texto(self.n.concatenar(p, q)));
        }
        let (p, q) = self.numeros_de(x, y)?;
        Ok(num(p + q))
    }

    /// `trocar`: compara b com a. `negar`: o resultado é o contrário. Assim
    /// a<b, a<=b, a>b e a>=b são a mesma conta (e NaN sempre dá false).
    fn comparar_lento(&mut self, a: Valor, b: Valor, trocar: bool, negar: bool) -> Result<bool, Valor> {
        let x = self.para_primitivo(a, Some(false))?;
        let y = self.para_primitivo(b, Some(false))?;
        let (x, y) = if trocar { (y, x) } else { (x, y) };
        if let (Valor::Texto(p), Valor::Texto(q)) = (x, y) {
            let (s, t) = (self.n.texto(p), self.n.texto(q));
            let menor = comparar_utf16(&s, &t) == core::cmp::Ordering::Less;
            return Ok(menor != negar);
        }
        let (p, q) = self.numeros_de(x, y)?;
        if p.is_nan() || q.is_nan() {
            return Ok(false);
        }
        Ok((p < q) != negar)
    }

    pub fn igual_estrito(&mut self, a: Valor, b: Valor) -> bool {
        match (a, b) {
            (Valor::Texto(x), Valor::Texto(y)) => {
                if x == y {
                    return true;
                }
                if self.n.tamanho_bytes(x) != self.n.tamanho_bytes(y) {
                    return false;
                }
                let (s, t) = (self.n.texto(x), self.n.texto(y));
                s == t
            }
            (Valor::Int(x), Valor::Int(y)) => x == y,
            (Valor::Num(x), Valor::Num(y)) => x == y,
            (Valor::Int(x), Valor::Num(y)) | (Valor::Num(y), Valor::Int(x)) => x as f64 == y,
            (Valor::Obj(x), Valor::Obj(y)) => x == y,
            (Valor::Simbolo(x), Valor::Simbolo(y)) => x == y,
            (Valor::Bool(x), Valor::Bool(y)) => x == y,
            (Valor::Undef, Valor::Undef) | (Valor::Nulo, Valor::Nulo) | (Valor::Vazio, Valor::Vazio) => true,
            _ => false,
        }
    }

    pub fn igual_frouxo(&mut self, a: Valor, b: Valor) -> Result<bool, Valor> {
        Ok(match (a, b) {
            (Valor::Undef | Valor::Nulo, Valor::Undef | Valor::Nulo) => true,
            (Valor::Undef | Valor::Nulo, _) | (_, Valor::Undef | Valor::Nulo) => false,
            (Valor::Obj(_), Valor::Obj(_))
            | (Valor::Texto(_), Valor::Texto(_))
            | (Valor::Bool(_), Valor::Bool(_))
            | (Valor::Simbolo(_), Valor::Simbolo(_))
            | (Valor::Int(_) | Valor::Num(_), Valor::Int(_) | Valor::Num(_)) => self.igual_estrito(a, b),
            (Valor::Obj(_), _) => {
                let p = self.para_primitivo(a, None)?;
                return self.igual_frouxo(p, b);
            }
            (_, Valor::Obj(_)) => {
                let p = self.para_primitivo(b, None)?;
                return self.igual_frouxo(a, p);
            }
            (Valor::Simbolo(_), _) | (_, Valor::Simbolo(_)) => false,
            _ => {
                let (x, y) = self.numeros_de(a, b)?;
                x == y
            }
        })
    }

    pub fn instancia_de(&mut self, v: Valor, c: Valor) -> Result<bool, Valor> {
        let Valor::Obj(cid) = c else { return Err(self.erro_tipo("Right-hand side of 'instanceof' is not callable")) };
        let h = self.s.has_instance;
        let m = self.obter_obj(cid, h, c)?;
        if !m.nulo_ou_undef() {
            let r = self.chamar(m, c, &[v])?;
            return Ok(self.para_bool(r));
        }
        if !self.e_funcao(c) {
            return Err(self.erro_tipo("Right-hand side of 'instanceof' is not callable"));
        }
        self.instancia_comum(v, c)
    }

    pub fn instancia_comum(&mut self, v: Valor, c: Valor) -> Result<bool, Valor> {
        let Valor::Obj(cid) = c else { return Ok(false) };
        if let Tipo::Ligada(l) = &self.objs[cid as usize].tipo {
            let a = l.alvo;
            return self.instancia_de(v, a);
        }
        let Valor::Obj(o) = v else { return Ok(false) };
        let p = self.obter_obj(cid, K_PROTOTYPE, c)?;
        let Valor::Obj(p) = p else { return Err(self.erro_tipo("Function has non-object prototype in instanceof check")) };
        let mut cur = self.objs[o as usize].proto;
        while cur != NENHUM {
            if cur == p {
                return Ok(true);
            }
            cur = self.objs[cur as usize].proto;
        }
        Ok(false)
    }

    // ------------------------------------------------------------ iteração

    /// Devolve (iterador, next). Arrays com o iterador original: (array, Int(0)).
    pub fn iter_inicio(&mut self, o: Valor) -> Result<(Valor, Valor), Valor> {
        let k = self.s.iterator;
        if let Valor::Obj(id) = o {
            if matches!(self.objs[id as usize].tipo, Tipo::Array(_)) {
                let m = self.obter_obj(id, k, o)?;
                let nx = self.array_iter_next;
                let proto_iter = self.p.iter_array as usize;
                let next_atual = self.objs[proto_iter].achar(self.a.next).map(|i| self.objs[proto_iter].props[i].valor);
                if m == self.array_iter_original && next_atual == Some(nx) {
                    return Ok((o, Valor::Int(0)));
                }
            }
        }
        let m = if o.nulo_ou_undef() { Valor::Undef } else { self.obter(o, k)? };
        if !self.e_funcao(m) {
            let d = match o {
                Valor::Obj(_) => String::from("object"),
                outro => self.texto_de_primitivo(outro),
            };
            return Err(self.erro_tipo(&format!("{} is not iterable", d)));
        }
        let it = self.chamar(m, o, &[])?;
        if it.obj().is_none() {
            return Err(self.erro_tipo("Result of the Symbol.iterator method is not an object"));
        }
        let nx = self.obter(it, self.a.next)?;
        Ok((it, nx))
    }

    fn iter_inicio_async(&mut self, o: Valor) -> Result<(Valor, Valor), Valor> {
        let k = self.s.async_iterator;
        let m = if o.nulo_ou_undef() { Valor::Undef } else { self.obter(o, k)? };
        if self.e_funcao(m) {
            let it = self.chamar(m, o, &[])?;
            let nx = self.obter(it, self.a.next)?;
            return Ok((it, nx));
        }
        // Um iterador comum serve: o `await` age sobre cada resultado.
        let k = self.s.iterator;
        let m = if o.nulo_ou_undef() { Valor::Undef } else { self.obter(o, k)? };
        if !self.e_funcao(m) {
            return Err(self.erro_tipo("object is not async iterable"));
        }
        let it = self.chamar(m, o, &[])?;
        let nx = self.obter(it, self.a.next)?;
        Ok((it, nx))
    }

    /// Um passo: Some(valor) ou None no fim.
    pub fn iter_passo(&mut self, it: Valor, nx: Valor) -> Result<Option<Valor>, Valor> {
        let r = self.chamar(nx, it, &[])?;
        if r.obj().is_none() {
            return Err(self.erro_tipo("Iterator result is not an object"));
        }
        let feito = self.obter(r, self.a.done)?;
        if self.para_bool(feito) {
            return Ok(None);
        }
        Ok(Some(self.obter(r, self.a.value)?))
    }

    pub fn iter_fechar(&mut self, it: Valor) -> Result<(), Valor> {
        let m = self.obter(it, self.a.return_)?;
        if !m.nulo_ou_undef() {
            self.chamar(m, it, &[])?;
        }
        Ok(())
    }

    pub fn iteravel_para_lista(&mut self, o: Valor) -> Result<Vec<Valor>, Valor> {
        let (it, nx) = self.iter_inicio(o)?;
        if let Valor::Int(_) = nx {
            let Valor::Obj(id) = it else { return Ok(Vec::new()) };
            let Tipo::Array(v) = &self.objs[id as usize].tipo else { return Ok(Vec::new()) };
            return Ok(v.iter().map(|x| if *x == Valor::Vazio { Valor::Undef } else { *x }).collect());
        }
        let mut v = Vec::new();
        while let Some(x) = self.iter_passo(it, nx)? {
            v.push(x);
        }
        Ok(v)
    }

    fn chaves_for_in(&mut self, o: Valor) -> Res {
        let mut saida: Vec<Valor> = Vec::new();
        if o.nulo_ou_undef() {
            return Ok(self.novo_array(saida));
        }
        let id = self.para_objeto(o)?;
        let mut vistas: Vec<Txt> = Vec::new();
        let mut cur = id;
        while cur != NENHUM {
            let enumeraveis = self.chaves_proprias(cur, true, true, false)?;
            for k in enumeraveis {
                if !vistas.contains(&k) {
                    saida.push(Valor::Texto(k));
                }
            }
            let proto = self.objs[cur as usize].proto;
            if proto != NENHUM {
                let todas = self.chaves_proprias(cur, false, true, false)?;
                vistas.extend(todas);
            }
            cur = proto;
        }
        Ok(self.novo_array(saida))
    }

    // ------------------------------------------------------------ geradores

    fn terminar_gerador(&mut self, id: ObjId) {
        if let Tipo::Gerador(g) = &mut self.objs[id as usize].tipo {
            g.estado = EstadoGerador::Fim;
            g.pilha = Vec::new();
            g.handlers = Vec::new();
        }
    }

    /// Continua um gerador. modo: 0 next, 1 throw, 2 return.
    /// Devolve o valor e como parou (`self.sinal` diz se foi yield/await).
    pub fn retomar(&mut self, id: ObjId, modo: u8, valor: Valor) -> Res {
        self.sinal = Sinal::Nada;
        let Tipo::Gerador(g) = &mut self.objs[id as usize].tipo else {
            return Err(self.erro_tipo("not a generator"));
        };
        let estado = g.estado;
        match estado {
            EstadoGerador::Rodando => return Err(self.erro_tipo("Generator is already running")),
            EstadoGerador::Fim => {
                return match modo {
                    1 => Err(valor),
                    2 => Ok(valor),
                    _ => Ok(Valor::Undef),
                };
            }
            EstadoGerador::Novo if modo != 0 => {
                g.estado = EstadoGerador::Fim;
                g.pilha = Vec::new();
                return if modo == 1 { Err(valor) } else { Ok(valor) };
            }
            _ => {}
        }
        g.estado = EstadoGerador::Rodando;
        let pilha = core::mem::take(&mut g.pilha);
        let handlers = core::mem::take(&mut g.handlers);
        let args = core::mem::take(&mut g.args);
        let (func, ups, callee, this, nt, ip) = (g.func.clone(), g.ups.clone(), g.callee, g.this, g.nt, g.ip);
        let limite = self.quadros.len();
        if limite >= LIMITE_QUADROS || self.prof_nativa >= LIMITE_NATIVAS {
            return Err(self.erro_intervalo("Maximum call stack size exceeded"));
        }
        self.pilha.push(callee);
        self.pilha.push(this);
        let base = self.pilha.len();
        self.pilha.extend_from_slice(&pilha);
        self.quadros.push(Quadro { func, ups, ip, base, callee, this, nt, args, handlers, construindo: false, gerador: id });
        let mut excecao = None;
        match estado {
            EstadoGerador::Novo => {}
            EstadoGerador::SuspensoAwait => {
                if modo == 1 {
                    excecao = Some(valor);
                } else {
                    self.pilha.push(valor);
                }
            }
            _ => match modo {
                1 => excecao = Some(valor),
                _ => {
                    self.pilha.push(valor);
                    self.pilha.push(Valor::Bool(modo == 2));
                }
            },
        }
        self.executar(limite, excecao)
    }
}

/// Compara como o JavaScript: unidade a unidade de UTF-16. Só difere da ordem
/// dos bytes de UTF-8 para letras fora do plano básico.
pub fn comparar_utf16(a: &str, b: &str) -> core::cmp::Ordering {
    if a.is_ascii() && b.is_ascii() {
        return a.cmp(b);
    }
    a.encode_utf16().cmp(b.encode_utf16())
}

/// As tarefas da fila: reações de promessas e `queueMicrotask`.
pub fn rodar_microtarefas(vm: &mut Vm) {
    while let Some(t) = vm.microtarefas.pop_front() {
        if vm.abortando {
            continue;
        }
        match t {
            Tarefa::Chamar(f) => {
                vm.pilha.push(f);
                let r = vm.chamar(f, Valor::Undef, &[]);
                vm.pilha.pop();
                if let Err(e) = r {
                    relatar_erro(vm, e);
                }
            }
            Tarefa::Reacao { manipulador, valor, derivada, rejeitada } => {
                super::nativos2::rodar_reacao(vm, manipulador, valor, derivada, rejeitada);
            }
            Tarefa::AdotarThenable { promessa, thenable, then } => {
                super::nativos2::adotar(vm, promessa, thenable, then);
            }
        }
    }
    // Rejeições que ninguém tratou.
    for p in core::mem::take(&mut vm.rejeicoes) {
        if let Valor::Obj(id) = p {
            if let Tipo::Promessa(pr) = &vm.objs[id as usize].tipo {
                if !pr.tratada {
                    let v = pr.valor;
                    let d = vm.descrever_erro(v);
                    let m = format!("Uncaught (in promise) {}", d);
                    (vm.ganchos.console)(vm, 3, &m);
                }
            }
        }
    }
}

/// Mostra um erro não tratado no console.
pub fn relatar_erro(vm: &mut Vm, e: Valor) {
    vm.abortando = false;
    let d = vm.descrever_erro(e);
    let m = format!("Uncaught {}", d);
    (vm.ganchos.console)(vm, 3, &m);
}
