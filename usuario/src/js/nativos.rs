//! Os objetos embutidos, parte 1: Object, Function, Array, Error, Symbol,
//! Boolean, Reflect, iteradores, geradores, as funções globais e o console.
//! (A parte 2 está em `nativos2.rs`.)

use super::interprete::comparar_utf16;
use super::lexico::*;
use super::vm::*;
use crate::mat;
use alloc::boxed::Box;
use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec;
use alloc::vec::Vec;

pub fn arg(a: &[Valor], i: usize) -> Valor {
    a.get(i).copied().unwrap_or(Valor::Undef)
}

/// Cria um construtor nativo ligado ao protótipo, e o publica no objeto global.
pub fn construtor(vm: &mut Vm, nome: &str, comprimento: u8, f: Nativa, proto: ObjId) -> ObjId {
    let k = vm.n.atomo(nome);
    let c = vm.nativa_com(k, comprimento, f, [Valor::Undef; 2], true);
    vm.materializar(c);
    vm.def_valor(c, K_PROTOTYPE, Valor::Obj(proto), 0);
    vm.def_valor(proto, K_CONSTRUCTOR, Valor::Obj(c), P_OCULTA);
    let g = vm.global;
    vm.def_valor(g, k, Valor::Obj(c), P_OCULTA);
    c
}

pub fn def_marca(vm: &mut Vm, proto: ObjId, marca: &str) {
    let t = vm.txt(marca);
    let k = vm.s.to_string_tag;
    vm.def_valor(proto, k, t, P_CONFIG);
}

pub fn resultado_iter(vm: &mut Vm, v: Valor, feito: bool) -> Valor {
    let o = vm.novo_objeto();
    let (kv, kd) = (vm.a.value, vm.a.done);
    vm.def_valor(o, kv, v, P_TUDO);
    vm.def_valor(o, kd, Valor::Bool(feito), P_TUDO);
    Valor::Obj(o)
}

/// O índice relativo de `slice`, `splice`, `at`...: negativo conta do fim.
pub fn indice_relativo(vm: &mut Vm, v: Valor, n: usize, padrao: usize) -> Result<usize, Valor> {
    if v == Valor::Undef {
        return Ok(padrao);
    }
    let i = vm.para_inteiro(v)?;
    Ok(if i < 0.0 { (n as f64 + i).max(0.0) as usize } else { i.min(n as f64) as usize })
}

fn comprimento(vm: &mut Vm, o: Valor) -> Result<usize, Valor> {
    if let Valor::Obj(id) = o {
        if let Tipo::Array(v) = &vm.objs[id as usize].tipo {
            return Ok(v.len());
        }
    }
    let n = vm.obter(o, K_LENGTH)?;
    let n = vm.para_inteiro(n)?;
    Ok(n.clamp(0.0, 9007199254740991.0) as usize)
}

/// O item `i` de um array ou parecido; None se é um buraco.
fn item(vm: &mut Vm, o: Valor, i: usize) -> Result<Option<Valor>, Valor> {
    if let Valor::Obj(id) = o {
        if let Tipo::Array(v) = &vm.objs[id as usize].tipo {
            return match v.get(i) {
                Some(&x) if x != Valor::Vazio => Ok(Some(x)),
                _ => Ok(None),
            };
        }
        let k = vm.para_chave(num(i as f64))?;
        if !vm.tem_prop(id, k)? {
            return Ok(None);
        }
        return Ok(Some(vm.obter_obj(id, k, o)?));
    }
    Ok(Some(vm.obter_elem(o, num(i as f64))?))
}

fn funcao_arg(vm: &mut Vm, f: Valor) -> Result<(), Valor> {
    if vm.e_funcao(f) {
        return Ok(());
    }
    let d = match f {
        Valor::Obj(_) => String::from("object"),
        o => vm.texto_de_primitivo(o),
    };
    Err(vm.erro_tipo(&format!("{} is not a function", d)))
}

pub fn instalar(vm: &mut Vm) {
    // Os protótipos fundamentais primeiro: os outros dependem deles.
    let po = vm.novo_obj_com(NENHUM, Tipo::Comum);
    vm.p.objeto = po;
    vm.p.funcao = vm.novo_obj_com(po, Tipo::Nativa(Box::new(DadosNativa {
        f: |_, _, _| Ok(Valor::Undef),
        nome: K_VAZIO,
        comprimento: 0,
        construtor: false,
        dado: [Valor::Undef; 2],
    })));
    vm.global = vm.novo_obj_com(po, Tipo::Comum);
    vm.lexico = vm.novo_obj_com(NENHUM, Tipo::Comum);
    vm.p.array = vm.novo_obj_com(po, Tipo::Array(Vec::new()));
    vm.p.texto = vm.novo_obj_com(po, Tipo::Texto(K_VAZIO));
    vm.p.numero = vm.novo_obj_com(po, Tipo::Numero(0.0));
    vm.p.booleano = vm.novo_obj_com(po, Tipo::Booleano(false));
    vm.p.simbolo = vm.novo_obj_com(po, Tipo::Comum);
    vm.p.erro = vm.novo_obj_com(po, Tipo::Comum);
    vm.p.iterador = vm.novo_obj_com(po, Tipo::Comum);
    let it = vm.p.iterador;
    vm.p.iter_array = vm.novo_obj_com(it, Tipo::Comum);
    vm.p.iter_colecao = vm.novo_obj_com(it, Tipo::Comum);
    vm.p.iter_texto = vm.novo_obj_com(it, Tipo::Comum);
    vm.p.iter_regex = vm.novo_obj_com(it, Tipo::Comum);
    vm.p.gerador = vm.novo_obj_com(it, Tipo::Comum);
    vm.p.gerador_assinc = vm.novo_obj_com(po, Tipo::Comum);
    let pf = vm.p.funcao;
    vm.p.func_gerador = vm.novo_obj_com(pf, Tipo::Comum);
    vm.p.func_assinc = vm.novo_obj_com(pf, Tipo::Comum);
    vm.p.promessa = vm.novo_obj_com(po, Tipo::Comum);
    vm.p.regex = vm.novo_obj_com(po, Tipo::Comum);
    vm.p.data = vm.novo_obj_com(po, Tipo::Comum);
    vm.p.mapa = vm.novo_obj_com(po, Tipo::Comum);
    vm.p.conjunto = vm.novo_obj_com(po, Tipo::Comum);
    vm.p.buffer = vm.novo_obj_com(po, Tipo::Comum);

    objeto(vm);
    funcao(vm);
    erros(vm);
    array(vm);
    simbolo(vm);
    booleano(vm);
    iteradores(vm);
    reflect(vm);
    globais(vm);
    console(vm);
    super::nativos2::instalar(vm);

    let g = vm.global;
    vm.def_oculta(g, "globalThis", Valor::Obj(g));
}

// -----------------------------------------------------------------------------
// Object
// -----------------------------------------------------------------------------

fn de_descritor(vm: &mut Vm, id: ObjId, k: Txt) -> Res {
    if !vm.tem_propria(id, k) {
        return Ok(Valor::Undef);
    }
    let r = vm.novo_objeto();
    let o = &vm.objs[id as usize];
    let (valor, flags) = match o.achar(k) {
        Some(i) => (o.props[i].valor, o.props[i].flags),
        None => {
            // Item de array, letra de um texto ou `length`.
            let congelado = o.flags & O_CONGELADO != 0;
            let v = vm.obter_obj(id, k, Valor::Obj(id))?;
            let fixo = k == K_LENGTH || matches!(vm.objs[id as usize].tipo, Tipo::Texto(_));
            (v, if fixo { if k == K_LENGTH && !congelado && matches!(vm.objs[id as usize].tipo, Tipo::Array(_)) { P_ESCRITA } else { 0 } } else if congelado { P_ENUM } else { P_TUDO })
        }
    };
    if flags & P_ACESSOR != 0 {
        if let Valor::Obj(p) = valor {
            if let Tipo::Acessor(g, s) = vm.objs[p as usize].tipo {
                let (kg, ks) = (vm.a.get, vm.a.set);
                vm.def_valor(r, kg, g, P_TUDO);
                vm.def_valor(r, ks, s, P_TUDO);
            }
        }
    } else {
        let (kv, kw) = (vm.a.value, vm.a.writable);
        vm.def_valor(r, kv, valor, P_TUDO);
        vm.def_valor(r, kw, Valor::Bool(flags & P_ESCRITA != 0), P_TUDO);
    }
    let (ke, kc) = (vm.a.enumerable, vm.a.configurable);
    vm.def_valor(r, ke, Valor::Bool(flags & P_ENUM != 0), P_TUDO);
    vm.def_valor(r, kc, Valor::Bool(flags & P_CONFIG != 0), P_TUDO);
    Ok(Valor::Obj(r))
}

/// `Object.defineProperty`
pub fn definir_com_descritor(vm: &mut Vm, id: ObjId, k: Txt, d: Valor) -> Result<(), Valor> {
    if d.obj().is_none() {
        return Err(vm.erro_tipo("Property description must be an object"));
    }
    if vm.objs[id as usize].flags & O_MATERIAL == 0 && matches!(vm.objs[id as usize].tipo, Tipo::Funcao(_) | Tipo::Nativa(_)) {
        vm.materializar(id);
    }
    let (kg, ks, kv, kw, ke, kc) = (vm.a.get, vm.a.set, vm.a.value, vm.a.writable, vm.a.enumerable, vm.a.configurable);
    let tem = |vm: &mut Vm, c: Txt| -> Result<bool, Valor> { vm.tem_prop(d.obj().unwrap(), c) };
    let (tg, ts, tv, tw, te, tc) = (tem(vm, kg)?, tem(vm, ks)?, tem(vm, kv)?, tem(vm, kw)?, tem(vm, ke)?, tem(vm, kc)?);
    if (tg || ts) && (tv || tw) {
        return Err(vm.erro_tipo("Invalid property descriptor. Cannot both specify accessors and a value or writable attribute"));
    }
    let antigo = {
        let o = &vm.objs[id as usize];
        o.achar(k).map(|i| o.props[i])
    };
    if let Some(p) = antigo {
        if p.flags & P_CONFIG == 0 {
            let so_valor = tv && p.flags & P_ESCRITA != 0 && !tg && !ts;
            if !so_valor {
                let nome = vm.n.texto(k);
                return Err(vm.erro_tipo(&format!("Cannot redefine property: {}", nome)));
            }
        }
    } else if !vm.objs[id as usize].extensivel() {
        let nome = vm.n.texto(k);
        return Err(vm.erro_tipo(&format!("Cannot define property {}, object is not extensible", nome)));
    }
    let mut flags = antigo.map_or(0, |p| p.flags & (P_ENUM | P_CONFIG | P_ESCRITA));
    let dv = d;
    if te {
        let v = vm.obter(dv, ke)?;
        flags = if vm.para_bool(v) { flags | P_ENUM } else { flags & !P_ENUM };
    }
    if tc {
        let v = vm.obter(dv, kc)?;
        flags = if vm.para_bool(v) { flags | P_CONFIG } else { flags & !P_CONFIG };
    }
    if tw {
        let v = vm.obter(dv, kw)?;
        flags = if vm.para_bool(v) { flags | P_ESCRITA } else { flags & !P_ESCRITA };
    }
    if tg || ts {
        let (mut g, mut s) = (Valor::Undef, Valor::Undef);
        if let Some(p) = antigo {
            if p.flags & P_ACESSOR != 0 {
                if let Valor::Obj(pid) = p.valor {
                    if let Tipo::Acessor(a, b) = vm.objs[pid as usize].tipo {
                        g = a;
                        s = b;
                    }
                }
            }
        }
        if tg {
            g = vm.obter(dv, kg)?;
        }
        if ts {
            s = vm.obter(dv, ks)?;
        }
        vm.def_par(id, k, g, s, flags);
        return Ok(());
    }
    let valor = if tv {
        vm.obter(dv, kv)?
    } else {
        match antigo {
            Some(p) if p.flags & P_ACESSOR == 0 => p.valor,
            _ => Valor::Undef,
        }
    };
    // Item de array: `length` e índices continuam no vetor, se forem comuns.
    if let Tipo::Array(_) = vm.objs[id as usize].tipo {
        if k == K_LENGTH {
            if tv {
                vm.definir_obj(id, k, valor, Valor::Obj(id))?;
            }
            return Ok(());
        }
        if flags == P_TUDO {
            vm.definir_propria(id, k, valor, flags);
            return Ok(());
        }
    }
    vm.def_valor(id, k, valor, flags);
    Ok(())
}

fn congelar(vm: &mut Vm, id: ObjId, tudo: bool) {
    let o = &mut vm.objs[id as usize];
    o.flags &= !O_EXTENSIVEL;
    if matches!(o.tipo, Tipo::Array(_)) && tudo {
        o.flags |= O_CONGELADO;
    }
    for p in o.props.iter_mut() {
        p.flags &= !P_CONFIG;
        if tudo && p.flags & P_ACESSOR == 0 {
            p.flags &= !P_ESCRITA;
        }
    }
}

fn entradas(vm: &mut Vm, o: Valor, modo: u8) -> Res {
    let id = vm.para_objeto(o)?;
    let chaves = vm.chaves_proprias(id, true, true, false)?;
    let mut saida = Vec::with_capacity(chaves.len());
    for k in chaves {
        match modo {
            0 => saida.push(Valor::Texto(k)),
            1 => saida.push(vm.obter_obj(id, k, Valor::Obj(id))?),
            _ => {
                let v = vm.obter_obj(id, k, Valor::Obj(id))?;
                let par = vm.novo_array(vec![Valor::Texto(k), v]);
                saida.push(par);
            }
        }
    }
    Ok(vm.novo_array(saida))
}

fn objeto(vm: &mut Vm) {
    let p = vm.p.objeto;
    let c = construtor(vm, "Object", 1, |vm, _, a| {
        let v = arg(a, 0);
        if vm.novo_alvo != Valor::Undef {
            let nt = vm.novo_alvo;
            let proto = vm.proto_de_novo_alvo(nt, vm.p.objeto)?;
            if v.nulo_ou_undef() {
                return Ok(Valor::Obj(vm.novo_obj_com(proto, Tipo::Comum)));
            }
        }
        if v.nulo_ou_undef() {
            return Ok(Valor::Obj(vm.novo_objeto()));
        }
        Ok(Valor::Obj(vm.para_objeto(v)?))
    }, p);
    vm.def(c, "keys", 1, |vm, _, a| entradas(vm, arg(a, 0), 0));
    vm.def(c, "values", 1, |vm, _, a| entradas(vm, arg(a, 0), 1));
    vm.def(c, "entries", 1, |vm, _, a| entradas(vm, arg(a, 0), 2));
    vm.def(c, "assign", 2, |vm, _, a| {
        let alvo = vm.para_objeto(arg(a, 0))?;
        for f in a.iter().skip(1) {
            if let Valor::Obj(fid) = *f {
                let chaves = vm.chaves_proprias(fid, true, true, true)?;
                for k in chaves {
                    let v = vm.obter_obj(fid, k, *f)?;
                    vm.definir_obj(alvo, k, v, Valor::Obj(alvo))?;
                }
            } else if let Valor::Texto(_) = *f {
                let fid = vm.para_objeto(*f)?;
                let chaves = vm.chaves_proprias(fid, true, true, false)?;
                for k in chaves {
                    let v = vm.obter_obj(fid, k, Valor::Obj(fid))?;
                    vm.definir_obj(alvo, k, v, Valor::Obj(alvo))?;
                }
            }
        }
        Ok(Valor::Obj(alvo))
    });
    vm.def(c, "create", 2, |vm, _, a| {
        let proto = match arg(a, 0) {
            Valor::Obj(p) => p,
            Valor::Nulo => NENHUM,
            _ => return Err(vm.erro_tipo("Object prototype may only be an Object or null")),
        };
        let o = vm.novo_obj_com(proto, Tipo::Comum);
        if let Valor::Obj(ds) = arg(a, 1) {
            let chaves = vm.chaves_proprias(ds, true, true, true)?;
            for k in chaves {
                let d = vm.obter_obj(ds, k, Valor::Obj(ds))?;
                definir_com_descritor(vm, o, k, d)?;
            }
        }
        Ok(Valor::Obj(o))
    });
    vm.def(c, "getPrototypeOf", 1, |vm, _, a| {
        let id = vm.para_objeto(arg(a, 0))?;
        let p = vm.objs[id as usize].proto;
        Ok(if p == NENHUM { Valor::Nulo } else { Valor::Obj(p) })
    });
    vm.def(c, "setPrototypeOf", 2, |vm, _, a| {
        vm.mudar_proto(arg(a, 0), arg(a, 1))?;
        Ok(arg(a, 0))
    });
    vm.def(c, "defineProperty", 3, |vm, _, a| {
        let Valor::Obj(id) = arg(a, 0) else { return Err(vm.erro_tipo("Object.defineProperty called on non-object")) };
        let k = vm.para_chave(arg(a, 1))?;
        definir_com_descritor(vm, id, k, arg(a, 2))?;
        Ok(arg(a, 0))
    });
    vm.def(c, "defineProperties", 2, |vm, _, a| {
        let Valor::Obj(id) = arg(a, 0) else { return Err(vm.erro_tipo("Object.defineProperties called on non-object")) };
        let ds = vm.para_objeto(arg(a, 1))?;
        let chaves = vm.chaves_proprias(ds, true, true, true)?;
        for k in chaves {
            let d = vm.obter_obj(ds, k, Valor::Obj(ds))?;
            definir_com_descritor(vm, id, k, d)?;
        }
        Ok(arg(a, 0))
    });
    vm.def(c, "getOwnPropertyDescriptor", 2, |vm, _, a| {
        let id = vm.para_objeto(arg(a, 0))?;
        let k = vm.para_chave(arg(a, 1))?;
        de_descritor(vm, id, k)
    });
    vm.def(c, "getOwnPropertyDescriptors", 1, |vm, _, a| {
        let id = vm.para_objeto(arg(a, 0))?;
        let r = vm.novo_objeto();
        let chaves = vm.chaves_proprias(id, false, true, true)?;
        for k in chaves {
            let d = de_descritor(vm, id, k)?;
            vm.def_valor(r, k, d, P_TUDO);
        }
        Ok(Valor::Obj(r))
    });
    vm.def(c, "getOwnPropertyNames", 1, |vm, _, a| {
        let id = vm.para_objeto(arg(a, 0))?;
        let chaves = vm.chaves_proprias(id, false, true, false)?;
        let v = chaves.into_iter().map(Valor::Texto).collect();
        Ok(vm.novo_array(v))
    });
    vm.def(c, "getOwnPropertySymbols", 1, |vm, _, a| {
        let id = vm.para_objeto(arg(a, 0))?;
        let chaves = vm.chaves_proprias(id, false, false, true)?;
        let v = chaves.into_iter().map(Valor::Simbolo).collect();
        Ok(vm.novo_array(v))
    });
    vm.def(c, "freeze", 1, |vm, _, a| {
        if let Valor::Obj(id) = arg(a, 0) {
            congelar(vm, id, true);
        }
        Ok(arg(a, 0))
    });
    vm.def(c, "seal", 1, |vm, _, a| {
        if let Valor::Obj(id) = arg(a, 0) {
            congelar(vm, id, false);
        }
        Ok(arg(a, 0))
    });
    vm.def(c, "preventExtensions", 1, |vm, _, a| {
        if let Valor::Obj(id) = arg(a, 0) {
            vm.objs[id as usize].flags &= !O_EXTENSIVEL;
        }
        Ok(arg(a, 0))
    });
    vm.def(c, "isFrozen", 1, |vm, _, a| {
        let Valor::Obj(id) = arg(a, 0) else { return Ok(Valor::Bool(true)) };
        let o = &vm.objs[id as usize];
        let vazio_ou_cong = match &o.tipo {
            Tipo::Array(v) => v.is_empty() || o.flags & O_CONGELADO != 0,
            _ => true,
        };
        Ok(Valor::Bool(!o.extensivel() && vazio_ou_cong && o.props.iter().all(|p| p.flags & P_CONFIG == 0 && (p.flags & P_ACESSOR != 0 || p.flags & P_ESCRITA == 0))))
    });
    vm.def(c, "isSealed", 1, |vm, _, a| {
        let Valor::Obj(id) = arg(a, 0) else { return Ok(Valor::Bool(true)) };
        let o = &vm.objs[id as usize];
        Ok(Valor::Bool(!o.extensivel() && o.props.iter().all(|p| p.flags & P_CONFIG == 0)))
    });
    vm.def(c, "isExtensible", 1, |vm, _, a| {
        let Valor::Obj(id) = arg(a, 0) else { return Ok(Valor::Bool(false)) };
        Ok(Valor::Bool(vm.objs[id as usize].extensivel()))
    });
    vm.def(c, "is", 2, |vm, _, a| {
        let (x, y) = (arg(a, 0), arg(a, 1));
        Ok(Valor::Bool(mesmo_valor(vm, x, y, false)))
    });
    vm.def(c, "fromEntries", 1, |vm, _, a| {
        let itens = vm.iteravel_para_lista(arg(a, 0))?;
        let o = vm.novo_objeto();
        for par in itens {
            let k = vm.obter_elem(par, Valor::Int(0))?;
            let v = vm.obter_elem(par, Valor::Int(1))?;
            let chave = vm.para_chave(k)?;
            vm.definir_propria(o, chave, v, P_TUDO);
        }
        Ok(Valor::Obj(o))
    });
    vm.def(c, "hasOwn", 2, |vm, _, a| {
        let id = vm.para_objeto(arg(a, 0))?;
        let k = vm.para_chave(arg(a, 1))?;
        Ok(Valor::Bool(vm.tem_propria(id, k)))
    });
    vm.def(c, "groupBy", 2, |vm, _, a| {
        let itens = vm.iteravel_para_lista(arg(a, 0))?;
        let o = vm.novo_obj_com(NENHUM, Tipo::Comum);
        for (i, x) in itens.into_iter().enumerate() {
            let g = vm.chamar(arg(a, 1), Valor::Undef, &[x, num(i as f64)])?;
            let k = vm.para_chave(g)?;
            let atual = vm.obter_obj(o, k, Valor::Obj(o))?;
            match atual {
                Valor::Obj(l) => {
                    if let Tipo::Array(v) = &mut vm.objs[l as usize].tipo {
                        v.push(x);
                    }
                }
                _ => {
                    let l = vm.novo_array(vec![x]);
                    vm.def_valor(o, k, l, P_TUDO);
                }
            }
        }
        Ok(Valor::Obj(o))
    });

    vm.def(p, "hasOwnProperty", 1, |vm, this, a| {
        let k = vm.para_chave(arg(a, 0))?;
        let id = vm.para_objeto(this)?;
        Ok(Valor::Bool(vm.tem_propria(id, k)))
    });
    vm.def(p, "isPrototypeOf", 1, |vm, this, a| {
        let Valor::Obj(o) = arg(a, 0) else { return Ok(Valor::Bool(false)) };
        let alvo = vm.para_objeto(this)?;
        let mut cur = vm.objs[o as usize].proto;
        while cur != NENHUM {
            if cur == alvo {
                return Ok(Valor::Bool(true));
            }
            cur = vm.objs[cur as usize].proto;
        }
        Ok(Valor::Bool(false))
    });
    vm.def(p, "propertyIsEnumerable", 1, |vm, this, a| {
        let k = vm.para_chave(arg(a, 0))?;
        let id = vm.para_objeto(this)?;
        let chaves = vm.chaves_proprias(id, true, true, true)?;
        Ok(Valor::Bool(chaves.contains(&k)))
    });
    vm.def(p, "valueOf", 0, |vm, this, _| Ok(Valor::Obj(vm.para_objeto(this)?)));
    vm.def(p, "toLocaleString", 0, |vm, this, _| {
        let k = vm.a.to_string;
        let f = vm.obter(this, k)?;
        vm.chamar(f, this, &[])
    });
    vm.def(p, "toString", 0, |vm, this, _| {
        let t = match this {
            Valor::Undef | Valor::Vazio => String::from("[object Undefined]"),
            Valor::Nulo => String::from("[object Null]"),
            _ => {
                let id = vm.para_objeto(this)?;
                let k = vm.s.to_string_tag;
                let marca = vm.obter_obj(id, k, Valor::Obj(id))?;
                if let Valor::Texto(m) = marca {
                    format!("[object {}]", vm.n.texto(m))
                } else {
                    let nome = match &vm.objs[id as usize].tipo {
                        Tipo::Array(_) if vm.objs[id as usize].flags & O_ARGUMENTOS != 0 => "Arguments",
                        Tipo::Array(_) => "Array",
                        Tipo::Funcao(_) | Tipo::Nativa(_) | Tipo::Ligada(_) => "Function",
                        Tipo::Erro => "Error",
                        Tipo::Booleano(_) => "Boolean",
                        Tipo::Numero(_) => "Number",
                        Tipo::Texto(_) => "String",
                        Tipo::Data(_) => "Date",
                        Tipo::Regex(_) => "RegExp",
                        _ => "Object",
                    };
                    format!("[object {}]", nome)
                }
            }
        };
        Ok(vm.txt(&t))
    });
    vm.def_acessor(
        p,
        "__proto__",
        Some(|vm, this, _| {
            let id = vm.para_objeto(this)?;
            let p = vm.objs[id as usize].proto;
            Ok(if p == NENHUM { Valor::Nulo } else { Valor::Obj(p) })
        }),
        Some(|vm, this, a| {
            vm.mudar_proto(this, arg(a, 0))?;
            Ok(Valor::Undef)
        }),
    );
    vm.def(p, "__defineGetter__", 2, |vm, this, a| {
        if let Valor::Obj(id) = this {
            let k = vm.para_chave(arg(a, 0))?;
            vm.def_acessor_metodo(id, k, arg(a, 1), true, P_ENUM | P_CONFIG);
        }
        Ok(Valor::Undef)
    });
    vm.def(p, "__defineSetter__", 2, |vm, this, a| {
        if let Valor::Obj(id) = this {
            let k = vm.para_chave(arg(a, 0))?;
            vm.def_acessor_metodo(id, k, arg(a, 1), false, P_ENUM | P_CONFIG);
        }
        Ok(Valor::Undef)
    });
}

/// SameValue (`zero`: SameValueZero, -0 igual a +0).
pub fn mesmo_valor(vm: &mut Vm, x: Valor, y: Valor, zero: bool) -> bool {
    let n = |v: Valor| match v {
        Valor::Int(i) => Some(i as f64),
        Valor::Num(f) => Some(f),
        _ => None,
    };
    if let (Some(a), Some(b)) = (n(x), n(y)) {
        if a.is_nan() && b.is_nan() {
            return true;
        }
        if a == 0.0 && b == 0.0 && !zero {
            return a.is_sign_negative() == b.is_sign_negative();
        }
        return a == b;
    }
    vm.igual_estrito(x, y)
}

// -----------------------------------------------------------------------------
// Function
// -----------------------------------------------------------------------------

fn funcao(vm: &mut Vm) {
    let p = vm.p.funcao;
    construtor(vm, "Function", 1, |vm, _, a| {
        // new Function("a", "b", "return a + b")
        let n = a.len();
        let mut params = Vec::new();
        for x in a.iter().take(n.saturating_sub(1)) {
            params.push(vm.para_texto(*x)?.to_string());
        }
        let corpo = if n > 0 { vm.para_texto(a[n - 1])?.to_string() } else { String::new() };
        let fonte = format!("(function anonymous({}\n) {{\n{}\n}})", params.join(","), corpo);
        vm.avaliar(&fonte)
    }, p);
    vm.def(p, "call", 1, |vm, this, a| {
        let t = arg(a, 0);
        vm.chamar(this, t, a.get(1..).unwrap_or(&[]))
    });
    vm.def(p, "apply", 2, |vm, this, a| {
        let t = arg(a, 0);
        let lista = arg(a, 1);
        let args = if lista.nulo_ou_undef() { Vec::new() } else { vm.lista_de_array(lista)? };
        vm.chamar(this, t, &args)
    });
    vm.def(p, "bind", 1, |vm, this, a| {
        funcao_arg(vm, this)?;
        let proto = this.obj().map_or(vm.p.funcao, |o| vm.objs[o as usize].proto);
        let nome = vm.obter(this, K_NAME)?;
        let comp = vm.obter(this, K_LENGTH)?;
        let comp = vm.para_inteiro(comp)?;
        let extras = a.len().saturating_sub(1) as f64;
        let id = vm.novo_obj_com(proto, Tipo::Ligada(Box::new(Ligada { alvo: this, this: arg(a, 0), args: a.get(1..).unwrap_or(&[]).to_vec() })));
        let n = vm.para_texto(nome)?;
        let t = vm.txt(&format!("bound {}", n));
        vm.def_valor(id, K_NAME, t, P_CONFIG);
        vm.def_valor(id, K_LENGTH, num((comp - extras).max(0.0)), P_CONFIG);
        Ok(Valor::Obj(id))
    });
    vm.def(p, "toString", 0, |vm, this, _| {
        let Valor::Obj(id) = this else { return Err(vm.erro_tipo("Function.prototype.toString requires that 'this' be a Function")) };
        let t = match &vm.objs[id as usize].tipo {
            Tipo::Funcao(c) => {
                let (a, b) = c.func.trecho;
                let fonte = c.func.fonte.clone();
                String::from(fonte.get(a as usize..b as usize).unwrap_or("function () { [native code] }"))
            }
            Tipo::Nativa(d) => {
                let nome = d.nome;
                format!("function {}() {{ [native code] }}", vm.n.texto(nome))
            }
            _ => String::from("function () { [native code] }"),
        };
        Ok(vm.txt(&t))
    });
    let k = vm.s.has_instance;
    let f = vm.nova_nativa("[Symbol.hasInstance]", 1, |vm, this, a| Ok(Valor::Bool(vm.instancia_comum(arg(a, 0), this)?)));
    // Sem este valor na cadeia: instanceof já faz o caminho comum quando ela não existe.
    let _ = (k, f);
}

// -----------------------------------------------------------------------------
// Erros
// -----------------------------------------------------------------------------

fn construir_erro(vm: &mut Vm, padrao: ObjId, a: &[Valor]) -> Res {
    let nt = vm.novo_alvo;
    let proto = vm.proto_de_novo_alvo(nt, padrao)?;
    let id = vm.novo_obj_com(proto, Tipo::Erro);
    let m = arg(a, 0);
    if m != Valor::Undef {
        let s = vm.para_texto_id(m)?;
        let k = vm.a.message;
        vm.def_valor(id, k, Valor::Texto(s), P_OCULTA);
    }
    if let Valor::Obj(op) = arg(a, 1) {
        let kc = vm.a.cause;
        if vm.tem_prop(op, kc)? {
            let c = vm.obter_obj(op, kc, arg(a, 1))?;
            vm.def_valor(id, kc, c, P_OCULTA);
        }
    }
    vm.preencher_pilha(id);
    Ok(Valor::Obj(id))
}

fn novo_tipo_erro(vm: &mut Vm, nome: &str, proto_pai: ObjId, ctor_pai: Option<ObjId>, f: Nativa) -> ObjId {
    let p = vm.novo_obj_com(proto_pai, Tipo::Comum);
    let c = construtor(vm, nome, 1, f, p);
    if let Some(pai) = ctor_pai {
        vm.objs[c as usize].proto = pai;
    }
    let t = vm.txt(nome);
    vm.def_valor(p, K_NAME, t, P_OCULTA);
    let k = vm.a.message;
    let vazio = Valor::Texto(K_VAZIO);
    vm.def_valor(p, k, vazio, P_OCULTA);
    p
}

fn erros(vm: &mut Vm) {
    let pe = vm.p.erro;
    let ce = construtor(vm, "Error", 1, |vm, _, a| {
        let p = vm.p.erro;
        construir_erro(vm, p, a)
    }, pe);
    vm.def(ce, "captureStackTrace", 1, |vm, _, a| {
        if let Valor::Obj(id) = arg(a, 0) {
            vm.preencher_pilha(id);
        }
        Ok(Valor::Undef)
    });
    let t = vm.txt("Error");
    vm.def_valor(pe, K_NAME, t, P_OCULTA);
    let k = vm.a.message;
    vm.def_valor(pe, k, Valor::Texto(K_VAZIO), P_OCULTA);
    vm.def(pe, "toString", 0, |vm, this, _| {
        let n = vm.obter(this, K_NAME)?;
        let n = if n == Valor::Undef { String::from("Error") } else { vm.para_texto(n)?.to_string() };
        let k = vm.a.message;
        let m = vm.obter(this, k)?;
        let m = if m == Valor::Undef { String::new() } else { vm.para_texto(m)?.to_string() };
        let t = if m.is_empty() { n } else if n.is_empty() { m } else { format!("{}: {}", n, m) };
        Ok(vm.txt(&t))
    });
    vm.p.erro_tipo = novo_tipo_erro(vm, "TypeError", pe, Some(ce), |vm, _, a| { let p = vm.p.erro_tipo; construir_erro(vm, p, a) });
    vm.p.erro_ref = novo_tipo_erro(vm, "ReferenceError", pe, Some(ce), |vm, _, a| { let p = vm.p.erro_ref; construir_erro(vm, p, a) });
    vm.p.erro_sintaxe = novo_tipo_erro(vm, "SyntaxError", pe, Some(ce), |vm, _, a| { let p = vm.p.erro_sintaxe; construir_erro(vm, p, a) });
    vm.p.erro_intervalo = novo_tipo_erro(vm, "RangeError", pe, Some(ce), |vm, _, a| { let p = vm.p.erro_intervalo; construir_erro(vm, p, a) });
    vm.p.erro_uri = novo_tipo_erro(vm, "URIError", pe, Some(ce), |vm, _, a| { let p = vm.p.erro_uri; construir_erro(vm, p, a) });
    vm.p.erro_eval = novo_tipo_erro(vm, "EvalError", pe, Some(ce), |vm, _, a| { let p = vm.p.erro_eval; construir_erro(vm, p, a) });
    vm.p.erro_agregado = novo_tipo_erro(vm, "AggregateError", pe, Some(ce), |vm, _, a| {
        let p = vm.p.erro_agregado;
        let e = construir_erro(vm, p, a.get(1..).unwrap_or(&[]))?;
        let lista = vm.iteravel_para_lista(arg(a, 0))?;
        let l = vm.novo_array(lista);
        let k = vm.a.errors;
        if let Valor::Obj(id) = e {
            vm.def_valor(id, k, l, P_OCULTA);
        }
        Ok(e)
    });
}

// -----------------------------------------------------------------------------
// Array
// -----------------------------------------------------------------------------

/// Ordena com comparação que pode lançar: merge sort (estável).
pub fn ordenar(vm: &mut Vm, v: Vec<Valor>, cmp: &mut dyn FnMut(&mut Vm, Valor, Valor) -> Result<bool, Valor>) -> Result<Vec<Valor>, Valor> {
    let n = v.len();
    let mut a = v;
    let mut b = a.clone();
    let mut tam = 1;
    while tam < n {
        let mut i = 0;
        while i < n {
            let meio = (i + tam).min(n);
            let fim = (i + tam * 2).min(n);
            let (mut p, mut q, mut k) = (i, meio, i);
            while p < meio && q < fim {
                // `cmp(x, y)`: y vem antes de x?
                if cmp(vm, a[p], a[q])? {
                    b[k] = a[q];
                    q += 1;
                } else {
                    b[k] = a[p];
                    p += 1;
                }
                k += 1;
            }
            while p < meio {
                b[k] = a[p];
                p += 1;
                k += 1;
            }
            while q < fim {
                b[k] = a[q];
                q += 1;
                k += 1;
            }
            i += tam * 2;
        }
        core::mem::swap(&mut a, &mut b);
        tam *= 2;
    }
    Ok(a)
}

fn ordenar_js(vm: &mut Vm, itens: Vec<Valor>, f: Valor) -> Result<Vec<Valor>, Valor> {
    if f != Valor::Undef {
        funcao_arg(vm, f)?;
    }
    // undefined vai para o fim, sempre; os buracos, depois dele.
    let total = itens.len();
    let vazios = itens.iter().filter(|x| **x == Valor::Vazio).count();
    let indefinidos = itens.iter().filter(|x| **x == Valor::Undef).count();
    let v: Vec<Valor> = itens.into_iter().filter(|x| !matches!(x, Valor::Undef | Valor::Vazio)).collect();
    let mut r = if f == Valor::Undef {
        let mut chaves = Vec::with_capacity(v.len());
        for x in &v {
            chaves.push(vm.para_texto(*x)?);
        }
        let mut ordem: Vec<usize> = (0..v.len()).collect();
        ordem.sort_by(|&a, &b| comparar_utf16(&chaves[a], &chaves[b]));
        ordem.iter().map(|&i| v[i]).collect()
    } else {
        ordenar(vm, v, &mut |vm, x, y| {
            let c = vm.chamar(f, Valor::Undef, &[x, y])?;
            let n = vm.para_numero(c)?;
            Ok(n > 0.0)
        })?
    };
    for _ in 0..indefinidos {
        r.push(Valor::Undef);
    }
    for _ in 0..vazios {
        r.push(Valor::Vazio);
    }
    debug_assert_eq!(r.len(), total);
    Ok(r)
}

fn achatar(vm: &mut Vm, origem: Valor, prof: f64, saida: &mut Vec<Valor>) -> Result<(), Valor> {
    let n = comprimento(vm, origem)?;
    for i in 0..n {
        if let Some(x) = item(vm, origem, i)? {
            if prof >= 1.0 && vm.e_array(x) {
                achatar(vm, x, prof - 1.0, saida)?;
            } else {
                saida.push(x);
            }
        }
    }
    Ok(())
}

/// `this` como Vec, para os métodos que só leem.
fn vetor_de(vm: &mut Vm, this: Valor) -> Result<Vec<Valor>, Valor> {
    if this.nulo_ou_undef() {
        return Err(vm.erro_tipo("Array.prototype method called on null or undefined"));
    }
    let o = Valor::Obj(vm.para_objeto(this)?);
    vm.lista_de_array(o)
}

/// Grava o resultado em `this` (o `sort`, o `reverse`...).
fn gravar_vetor(vm: &mut Vm, this: Valor, v: Vec<Valor>) -> Result<(), Valor> {
    if let Valor::Obj(id) = this {
        if vm.objs[id as usize].flags & O_CONGELADO != 0 {
            return Err(vm.erro_tipo("Cannot assign to read only property of frozen array"));
        }
        if let Tipo::Array(a) = &mut vm.objs[id as usize].tipo {
            *a = v;
            return Ok(());
        }
        let n = v.len();
        for (i, x) in v.into_iter().enumerate() {
            vm.definir_elem(this, num(i as f64), x)?;
        }
        vm.definir(this, K_LENGTH, num(n as f64))?;
    }
    Ok(())
}

fn iter_novo(vm: &mut Vm, alvo: Valor, tipo: u8) -> Res {
    let p = vm.p.iter_array;
    Ok(Valor::Obj(vm.novo_obj_com(p, Tipo::IterArray { alvo, i: 0, tipo })))
}

fn laco_array(vm: &mut Vm, this: Valor, a: &[Valor], modo: u8) -> Res {
    let f = arg(a, 0);
    funcao_arg(vm, f)?;
    let t = arg(a, 1);
    let o = Valor::Obj(vm.para_objeto(this)?);
    let n = comprimento(vm, o)?;
    let mut saida = Vec::new();
    if modo == 1 {
        saida.resize(n, Valor::Vazio);
    }
    for i in 0..n {
        let Some(x) = item(vm, o, i)? else {
            continue;
        };
        let r = vm.chamar(f, t, &[x, num(i as f64), o])?;
        match modo {
            0 => {}
            1 => saida[i] = r,
            2 => {
                if vm.para_bool(r) {
                    saida.push(x);
                }
            }
            3 => {
                if vm.para_bool(r) {
                    return Ok(Valor::Bool(true));
                }
            }
            _ => {
                if !vm.para_bool(r) {
                    return Ok(Valor::Bool(false));
                }
            }
        }
    }
    Ok(match modo {
        0 => Valor::Undef,
        1 | 2 => vm.novo_array(saida),
        3 => Valor::Bool(false),
        _ => Valor::Bool(true),
    })
}

fn achar_array(vm: &mut Vm, this: Valor, a: &[Valor], de_tras: bool, indice: bool) -> Res {
    let f = arg(a, 0);
    funcao_arg(vm, f)?;
    let t = arg(a, 1);
    let o = Valor::Obj(vm.para_objeto(this)?);
    let n = comprimento(vm, o)?;
    for j in 0..n {
        let i = if de_tras { n - 1 - j } else { j };
        let x = vm.obter_elem(o, num(i as f64))?;
        let r = vm.chamar(f, t, &[x, num(i as f64), o])?;
        if vm.para_bool(r) {
            return Ok(if indice { num(i as f64) } else { x });
        }
    }
    Ok(if indice { Valor::Int(-1) } else { Valor::Undef })
}

fn reduzir(vm: &mut Vm, this: Valor, a: &[Valor], direita: bool) -> Res {
    let f = arg(a, 0);
    funcao_arg(vm, f)?;
    let o = Valor::Obj(vm.para_objeto(this)?);
    let n = comprimento(vm, o)?;
    let mut acc = if a.len() > 1 { Some(a[1]) } else { None };
    for j in 0..n {
        let i = if direita { n - 1 - j } else { j };
        let Some(x) = item(vm, o, i)? else { continue };
        acc = Some(match acc {
            None => x,
            Some(c) => vm.chamar(f, Valor::Undef, &[c, x, num(i as f64), o])?,
        });
    }
    match acc {
        Some(c) => Ok(c),
        None => Err(vm.erro_tipo("Reduce of empty array with no initial value")),
    }
}

pub fn juntar(vm: &mut Vm, this: Valor, sep: &str) -> Res {
    // Um array que contém a si mesmo vira texto vazio ali dentro.
    let id = vm.para_objeto(this)?;
    let k = vm.atomo("%juntando");
    if vm.objs[id as usize].achar(k).is_some() {
        return Ok(Valor::Texto(K_VAZIO));
    }
    vm.def_valor(id, k, Valor::Bool(true), P_CONFIG);
    let r = (|| {
        let o = Valor::Obj(id);
        let n = comprimento(vm, o)?;
        let mut s = String::new();
        for i in 0..n {
            if i > 0 {
                s.push_str(sep);
            }
            let x = vm.obter_elem(o, num(i as f64))?;
            if !x.nulo_ou_undef() {
                let t = vm.para_texto(x)?;
                s.push_str(&t);
            }
        }
        Ok(s)
    })();
    let _ = vm.apagar(id, k);
    let s: String = r?;
    Ok(vm.txt(&s))
}

fn array(vm: &mut Vm) {
    let p = vm.p.array;
    let c = construtor(vm, "Array", 1, |vm, _, a| {
        let nt = vm.novo_alvo;
        let proto = vm.proto_de_novo_alvo(nt, vm.p.array)?;
        let v = if a.len() == 1 {
            match a[0] {
                Valor::Int(_) | Valor::Num(_) => {
                    let n = vm.para_numero(a[0])?;
                    if n < 0.0 || mat::trunc(n) != n || n > 4294967295.0 {
                        return Err(vm.erro_intervalo("Invalid array length"));
                    }
                    if n > 8_000_000.0 {
                        return Err(vm.erro_intervalo("Array too large"));
                    }
                    vec![Valor::Vazio; n as usize]
                }
                x => vec![x],
            }
        } else {
            a.to_vec()
        };
        let id = vm.novo_array(v);
        if let Valor::Obj(i) = id {
            vm.objs[i as usize].proto = proto;
        }
        Ok(id)
    }, p);
    vm.def(c, "isArray", 1, |vm, _, a| Ok(Valor::Bool(vm.e_array(arg(a, 0)))));
    vm.def(c, "of", 0, |vm, _, a| Ok(vm.novo_array(a.to_vec())));
    vm.def(c, "from", 1, |vm, _, a| {
        let fonte = arg(a, 0);
        let f = arg(a, 1);
        if fonte.nulo_ou_undef() {
            return Err(vm.erro_tipo("undefined is not iterable"));
        }
        let k = vm.s.iterator;
        let m = vm.obter(fonte, k)?;
        let itens = if !m.nulo_ou_undef() {
            vm.iteravel_para_lista(fonte)?
        } else {
            let o = Valor::Obj(vm.para_objeto(fonte)?);
            vm.lista_de_array(o)?
        };
        if f == Valor::Undef {
            return Ok(vm.novo_array(itens));
        }
        let t = arg(a, 2);
        let mut saida = Vec::with_capacity(itens.len());
        for (i, x) in itens.into_iter().enumerate() {
            saida.push(vm.chamar(f, t, &[x, num(i as f64)])?);
        }
        Ok(vm.novo_array(saida))
    });

    vm.def(p, "push", 1, |vm, this, a| {
        if let Valor::Obj(id) = this {
            let o = &mut vm.objs[id as usize];
            if o.flags & (O_CONGELADO) == 0 && o.flags & O_EXTENSIVEL != 0 {
                if let Tipo::Array(v) = &mut o.tipo {
                    v.extend_from_slice(a);
                    return Ok(num(v.len() as f64));
                }
            } else if matches!(o.tipo, Tipo::Array(_)) {
                return Err(vm.erro_tipo("Cannot add property, object is not extensible"));
            }
        }
        let mut n = comprimento(vm, this)?;
        for x in a {
            vm.definir_elem(this, num(n as f64), *x)?;
            n += 1;
        }
        vm.definir(this, K_LENGTH, num(n as f64))?;
        Ok(num(n as f64))
    });
    vm.def(p, "pop", 0, |vm, this, _| {
        if let Valor::Obj(id) = this {
            if vm.objs[id as usize].flags & O_CONGELADO != 0 {
                return Err(vm.erro_tipo("Cannot delete property of frozen array"));
            }
            if let Tipo::Array(v) = &mut vm.objs[id as usize].tipo {
                return Ok(match v.pop() {
                    Some(Valor::Vazio) | None => Valor::Undef,
                    Some(x) => x,
                });
            }
        }
        let n = comprimento(vm, this)?;
        if n == 0 {
            vm.definir(this, K_LENGTH, Valor::Int(0))?;
            return Ok(Valor::Undef);
        }
        let x = vm.obter_elem(this, num((n - 1) as f64))?;
        vm.definir(this, K_LENGTH, num((n - 1) as f64))?;
        Ok(x)
    });
    vm.def(p, "shift", 0, |vm, this, _| {
        let mut v = vetor_de(vm, this)?;
        if v.is_empty() {
            return Ok(Valor::Undef);
        }
        let x = v.remove(0);
        gravar_vetor(vm, this, v)?;
        Ok(x)
    });
    vm.def(p, "unshift", 1, |vm, this, a| {
        let v = vetor_de(vm, this)?;
        let mut novo = a.to_vec();
        novo.extend(v);
        let n = novo.len();
        gravar_vetor(vm, this, novo)?;
        Ok(num(n as f64))
    });
    vm.def(p, "slice", 2, |vm, this, a| {
        let o = Valor::Obj(vm.para_objeto(this)?);
        let n = comprimento(vm, o)?;
        let i = indice_relativo(vm, arg(a, 0), n, 0)?;
        let f = indice_relativo(vm, arg(a, 1), n, n)?;
        let mut saida = Vec::with_capacity(f.saturating_sub(i));
        if let Valor::Obj(id) = o {
            if let Tipo::Array(v) = &vm.objs[id as usize].tipo {
                if i < f {
                    saida.extend_from_slice(&v[i..f]);
                }
                return Ok(vm.novo_array(saida));
            }
        }
        for k in i..f.max(i) {
            saida.push(vm.obter_elem(o, num(k as f64))?);
        }
        Ok(vm.novo_array(saida))
    });
    vm.def(p, "splice", 2, |vm, this, a| {
        let mut v = vetor_de(vm, this)?;
        let n = v.len();
        let i = indice_relativo(vm, arg(a, 0), n, 0)?;
        let tirar = if a.is_empty() {
            0
        } else if a.len() == 1 {
            n - i
        } else {
            vm.para_inteiro(a[1])?.clamp(0.0, (n - i) as f64) as usize
        };
        let novos = a.get(2..).unwrap_or(&[]).to_vec();
        let removidos: Vec<Valor> = v.splice(i..i + tirar, novos).collect();
        gravar_vetor(vm, this, v)?;
        Ok(vm.novo_array(removidos))
    });
    vm.def(p, "concat", 1, |vm, this, a| {
        let mut saida = Vec::new();
        let o = Valor::Obj(vm.para_objeto(this)?);
        let k = vm.s.is_concat_spreadable;
        let mut juntar_um = |vm: &mut Vm, x: Valor| -> Result<(), Valor> {
            let espalha = if let Valor::Obj(_) = x {
                let e = vm.obter(x, k)?;
                if e != Valor::Undef { vm.para_bool(e) } else { vm.e_array(x) }
            } else {
                false
            };
            if espalha {
                if let Valor::Obj(id) = x {
                    if let Tipo::Array(v) = &vm.objs[id as usize].tipo {
                        saida.extend_from_slice(v);
                        return Ok(());
                    }
                }
                let itens = vm.lista_de_array(x)?;
                saida.extend(itens);
            } else {
                saida.push(x);
            }
            Ok(())
        };
        juntar_um(vm, o)?;
        for x in a {
            juntar_um(vm, *x)?;
        }
        Ok(vm.novo_array(saida))
    });
    vm.def(p, "join", 1, |vm, this, a| {
        let sep = if arg(a, 0) == Valor::Undef { alloc::rc::Rc::from(",") } else { vm.para_texto(a[0])? };
        juntar(vm, this, &sep)
    });
    vm.def(p, "toString", 0, |vm, this, _| {
        let k = vm.a.join;
        let f = vm.obter(this, k)?;
        if vm.e_funcao(f) {
            return vm.chamar(f, this, &[]);
        }
        Ok(vm.txt("[object Object]"))
    });
    vm.def(p, "toLocaleString", 0, |vm, this, _| juntar(vm, this, ","));
    vm.def(p, "reverse", 0, |vm, this, _| {
        let mut v = vetor_de(vm, this)?;
        v.reverse();
        gravar_vetor(vm, this, v)?;
        Ok(this)
    });
    vm.def(p, "toReversed", 0, |vm, this, _| {
        let mut v = vetor_de(vm, this)?;
        v.reverse();
        Ok(vm.novo_array(v))
    });
    vm.def(p, "indexOf", 1, |vm, this, a| {
        let o = Valor::Obj(vm.para_objeto(this)?);
        let n = comprimento(vm, o)?;
        let i = indice_relativo(vm, arg(a, 1), n, 0)?;
        let alvo = arg(a, 0);
        for k in i..n {
            if let Some(x) = item(vm, o, k)? {
                if vm.igual_estrito(x, alvo) {
                    return Ok(num(k as f64));
                }
            }
        }
        Ok(Valor::Int(-1))
    });
    vm.def(p, "lastIndexOf", 1, |vm, this, a| {
        let o = Valor::Obj(vm.para_objeto(this)?);
        let n = comprimento(vm, o)?;
        if n == 0 {
            return Ok(Valor::Int(-1));
        }
        let inicio = if a.len() > 1 {
            let i = vm.para_inteiro(a[1])?;
            if i < 0.0 { n as f64 + i } else { i.min((n - 1) as f64) }
        } else {
            (n - 1) as f64
        };
        if inicio < 0.0 {
            return Ok(Valor::Int(-1));
        }
        let alvo = arg(a, 0);
        let mut k = inicio as usize + 1;
        while k > 0 {
            k -= 1;
            if let Some(x) = item(vm, o, k)? {
                if vm.igual_estrito(x, alvo) {
                    return Ok(num(k as f64));
                }
            }
        }
        Ok(Valor::Int(-1))
    });
    vm.def(p, "includes", 1, |vm, this, a| {
        let o = Valor::Obj(vm.para_objeto(this)?);
        let n = comprimento(vm, o)?;
        let i = indice_relativo(vm, arg(a, 1), n, 0)?;
        let alvo = arg(a, 0);
        for k in i..n {
            let x = vm.obter_elem(o, num(k as f64))?;
            if mesmo_valor(vm, x, alvo, true) {
                return Ok(Valor::Bool(true));
            }
        }
        Ok(Valor::Bool(false))
    });
    vm.def(p, "forEach", 1, |vm, this, a| laco_array(vm, this, a, 0));
    vm.def(p, "map", 1, |vm, this, a| laco_array(vm, this, a, 1));
    vm.def(p, "filter", 1, |vm, this, a| laco_array(vm, this, a, 2));
    vm.def(p, "some", 1, |vm, this, a| laco_array(vm, this, a, 3));
    vm.def(p, "every", 1, |vm, this, a| laco_array(vm, this, a, 4));
    vm.def(p, "find", 1, |vm, this, a| achar_array(vm, this, a, false, false));
    vm.def(p, "findIndex", 1, |vm, this, a| achar_array(vm, this, a, false, true));
    vm.def(p, "findLast", 1, |vm, this, a| achar_array(vm, this, a, true, false));
    vm.def(p, "findLastIndex", 1, |vm, this, a| achar_array(vm, this, a, true, true));
    vm.def(p, "reduce", 1, |vm, this, a| reduzir(vm, this, a, false));
    vm.def(p, "reduceRight", 1, |vm, this, a| reduzir(vm, this, a, true));
    vm.def(p, "sort", 1, |vm, this, a| {
        let o = Valor::Obj(vm.para_objeto(this)?);
        let itens = if let Valor::Obj(id) = o {
            if let Tipo::Array(v) = &vm.objs[id as usize].tipo { v.clone() } else { vm.lista_de_array(o)? }
        } else {
            Vec::new()
        };
        let r = ordenar_js(vm, itens, arg(a, 0))?;
        gravar_vetor(vm, o, r)?;
        Ok(o)
    });
    vm.def(p, "toSorted", 1, |vm, this, a| {
        let itens = vetor_de(vm, this)?;
        let r = ordenar_js(vm, itens, arg(a, 0))?;
        Ok(vm.novo_array(r))
    });
    vm.def(p, "toSpliced", 2, |vm, this, a| {
        let mut v = vetor_de(vm, this)?;
        let n = v.len();
        let i = indice_relativo(vm, arg(a, 0), n, 0)?;
        let tirar = if a.len() < 2 { if a.is_empty() { 0 } else { n - i } } else { vm.para_inteiro(a[1])?.clamp(0.0, (n - i) as f64) as usize };
        v.splice(i..i + tirar, a.get(2..).unwrap_or(&[]).to_vec());
        Ok(vm.novo_array(v))
    });
    vm.def(p, "with", 2, |vm, this, a| {
        let mut v = vetor_de(vm, this)?;
        let i = vm.para_inteiro(arg(a, 0))?;
        let i = if i < 0.0 { v.len() as f64 + i } else { i };
        if i < 0.0 || i >= v.len() as f64 {
            return Err(vm.erro_intervalo("Invalid index"));
        }
        v[i as usize] = arg(a, 1);
        Ok(vm.novo_array(v))
    });
    vm.def(p, "at", 1, |vm, this, a| {
        let o = Valor::Obj(vm.para_objeto(this)?);
        let n = comprimento(vm, o)?;
        let i = vm.para_inteiro(arg(a, 0))?;
        let i = if i < 0.0 { n as f64 + i } else { i };
        if i < 0.0 || i >= n as f64 {
            return Ok(Valor::Undef);
        }
        vm.obter_elem(o, num(i))
    });
    vm.def(p, "fill", 1, |vm, this, a| {
        let o = Valor::Obj(vm.para_objeto(this)?);
        let n = comprimento(vm, o)?;
        let i = indice_relativo(vm, arg(a, 1), n, 0)?;
        let f = indice_relativo(vm, arg(a, 2), n, n)?;
        for k in i..f.max(i) {
            vm.definir_elem(o, num(k as f64), arg(a, 0))?;
        }
        Ok(o)
    });
    vm.def(p, "copyWithin", 2, |vm, this, a| {
        let mut v = vetor_de(vm, this)?;
        let n = v.len();
        let alvo = indice_relativo(vm, arg(a, 0), n, 0)?;
        let i = indice_relativo(vm, arg(a, 1), n, 0)?;
        let f = indice_relativo(vm, arg(a, 2), n, n)?;
        let pedaco: Vec<Valor> = v[i..f.max(i)].to_vec();
        for (k, x) in pedaco.into_iter().enumerate() {
            if alvo + k < n {
                v[alvo + k] = x;
            }
        }
        gravar_vetor(vm, this, v)?;
        Ok(this)
    });
    vm.def(p, "flat", 0, |vm, this, a| {
        let prof = if arg(a, 0) == Valor::Undef { 1.0 } else { vm.para_inteiro(a[0])? };
        let mut saida = Vec::new();
        achatar(vm, this, prof, &mut saida)?;
        Ok(vm.novo_array(saida))
    });
    vm.def(p, "flatMap", 1, |vm, this, a| {
        let r = laco_array(vm, this, a, 1)?;
        let mut saida = Vec::new();
        achatar(vm, r, 1.0, &mut saida)?;
        Ok(vm.novo_array(saida))
    });
    vm.def(p, "keys", 0, |vm, this, _| iter_novo(vm, this, 1));
    vm.def(p, "entries", 0, |vm, this, _| iter_novo(vm, this, 2));
    let valores = vm.def(p, "values", 0, |vm, this, _| iter_novo(vm, this, 0));
    let k = vm.s.iterator;
    vm.def_valor(p, k, Valor::Obj(valores), P_OCULTA);
    vm.array_iter_original = Valor::Obj(valores);
}

// -----------------------------------------------------------------------------
// Iteradores e geradores
// -----------------------------------------------------------------------------

fn iteradores(vm: &mut Vm) {
    let pi = vm.p.iterador;
    let f = vm.nova_nativa("[Symbol.iterator]", 0, |_, this, _| Ok(this));
    let k = vm.s.iterator;
    vm.def_valor(pi, k, Valor::Obj(f), P_OCULTA);

    let pa = vm.p.iter_array;
    def_marca(vm, pa, "Array Iterator");
    let nx = vm.def(pa, "next", 0, |vm, this, _| {
        let Valor::Obj(id) = this else { return Err(vm.erro_tipo("next method called on incompatible receiver")) };
        let Tipo::IterArray { alvo, i, tipo } = vm.objs[id as usize].tipo else {
            return Err(vm.erro_tipo("next method called on incompatible receiver"));
        };
        if alvo == Valor::Undef {
            return Ok(resultado_iter(vm, Valor::Undef, true));
        }
        let n = comprimento(vm, alvo)?;
        if i as usize >= n {
            if let Tipo::IterArray { alvo, .. } = &mut vm.objs[id as usize].tipo {
                *alvo = Valor::Undef;
            }
            return Ok(resultado_iter(vm, Valor::Undef, true));
        }
        if let Tipo::IterArray { i: ii, .. } = &mut vm.objs[id as usize].tipo {
            *ii += 1;
        }
        let v = match tipo {
            1 => num(i as f64),
            0 => vm.obter_elem(alvo, num(i as f64))?,
            _ => {
                let x = vm.obter_elem(alvo, num(i as f64))?;
                vm.novo_array(vec![num(i as f64), x])
            }
        };
        Ok(resultado_iter(vm, v, false))
    });
    vm.array_iter_next = Valor::Obj(nx);

    let pg = vm.p.gerador;
    def_marca(vm, pg, "Generator");
    let fg = vm.p.func_gerador;
    def_marca(vm, fg, "GeneratorFunction");
    vm.def_valor(fg, K_PROTOTYPE, Valor::Obj(pg), P_CONFIG);
    let fa = vm.p.func_assinc;
    def_marca(vm, fa, "AsyncFunction");
    // Os construtores não são globais: só se chega neles pelo protótipo.
    for (proto, nome) in [(fa, "AsyncFunction"), (fg, "GeneratorFunction")] {
        let k = vm.n.atomo(nome);
        let c = vm.nativa_com(k, 1, |vm, _, _| Err(vm.erro_tipo("not supported")), [Valor::Undef; 2], true);
        vm.def_valor(proto, K_CONSTRUCTOR, Valor::Obj(c), P_CONFIG);
    }
    fn passo(vm: &mut Vm, this: Valor, modo: u8, v: Valor) -> Res {
        let Valor::Obj(id) = this else { return Err(vm.erro_tipo("next method called on incompatible receiver")) };
        if !matches!(vm.objs[id as usize].tipo, Tipo::Gerador(_)) {
            return Err(vm.erro_tipo("next method called on incompatible receiver"));
        }
        let r = vm.retomar(id, modo, v)?;
        let suspenso = matches!(vm.sinal, Sinal::Yield);
        vm.sinal = Sinal::Nada;
        Ok(resultado_iter(vm, r, !suspenso))
    }
    vm.def(pg, "next", 1, |vm, this, a| passo(vm, this, 0, arg(a, 0)));
    vm.def(pg, "throw", 1, |vm, this, a| passo(vm, this, 1, arg(a, 0)));
    vm.def(pg, "return", 1, |vm, this, a| passo(vm, this, 2, arg(a, 0)));
}

// -----------------------------------------------------------------------------
// Symbol, Boolean, Reflect
// -----------------------------------------------------------------------------

fn simbolo(vm: &mut Vm) {
    let p = vm.p.simbolo;
    let c = construtor(vm, "Symbol", 0, |vm, _, a| {
        if vm.novo_alvo != Valor::Undef {
            return Err(vm.erro_tipo("Symbol is not a constructor"));
        }
        let d = if arg(a, 0) == Valor::Undef { alloc::rc::Rc::from("") } else { vm.para_texto(a[0])? };
        Ok(Valor::Simbolo(vm.n.novo_simbolo(&d)))
    }, p);
    let bem = vm.s.iterator;
    let lista = [
        ("iterator", bem),
        ("asyncIterator", vm.s.async_iterator),
        ("hasInstance", vm.s.has_instance),
        ("toPrimitive", vm.s.to_primitive),
        ("toStringTag", vm.s.to_string_tag),
        ("species", vm.s.species),
        ("match", vm.s.match_),
        ("matchAll", vm.s.match_all),
        ("replace", vm.s.replace),
        ("search", vm.s.search),
        ("split", vm.s.split),
        ("isConcatSpreadable", vm.s.is_concat_spreadable),
        ("unscopables", vm.s.unscopables),
    ];
    for (n, s) in lista {
        let k = vm.n.atomo(n);
        vm.def_valor(c, k, Valor::Simbolo(s), 0);
    }
    vm.def(c, "for", 1, |vm, _, a| {
        let t = vm.para_texto_id(arg(a, 0))?;
        let k = vm.n.atomo_de(t);
        if let Some(s) = vm.registro_simbolos.get(&k) {
            return Ok(Valor::Simbolo(*s));
        }
        let d = vm.n.texto(k);
        let s = vm.n.novo_simbolo(&d);
        vm.registro_simbolos.insert(k, s);
        Ok(Valor::Simbolo(s))
    });
    vm.def(c, "keyFor", 1, |vm, _, a| {
        let Valor::Simbolo(s) = arg(a, 0) else { return Err(vm.erro_tipo("not a symbol")) };
        Ok(vm.registro_simbolos.iter().find(|(_, v)| **v == s).map_or(Valor::Undef, |(k, _)| Valor::Texto(*k)))
    });
    fn valor(vm: &mut Vm, this: Valor) -> Result<Txt, Valor> {
        match this {
            Valor::Simbolo(s) => Ok(s),
            Valor::Obj(id) => match vm.objs[id as usize].tipo {
                Tipo::Simbolo(s) => Ok(s),
                _ => Err(vm.erro_tipo("not a symbol")),
            },
            _ => Err(vm.erro_tipo("not a symbol")),
        }
    }
    vm.def(p, "toString", 0, |vm, this, _| {
        let s = valor(vm, this)?;
        let d = vm.n.texto(s);
        Ok(vm.txt(&format!("Symbol({})", d)))
    });
    vm.def(p, "valueOf", 0, |vm, this, _| Ok(Valor::Simbolo(valor(vm, this)?)));
    vm.def_acessor(p, "description", Some(|vm, this, _| {
        let s = valor(vm, this)?;
        let d = vm.n.texto(s);
        Ok(vm.txt(&d))
    }), None);
    def_marca(vm, p, "Symbol");
}

fn booleano(vm: &mut Vm) {
    let p = vm.p.booleano;
    construtor(vm, "Boolean", 1, |vm, _, a| {
        let b = vm.para_bool(arg(a, 0));
        if vm.novo_alvo != Valor::Undef {
            let nt = vm.novo_alvo;
            let proto = vm.proto_de_novo_alvo(nt, vm.p.booleano)?;
            return Ok(Valor::Obj(vm.novo_obj_com(proto, Tipo::Booleano(b))));
        }
        Ok(Valor::Bool(b))
    }, p);
    fn valor(vm: &mut Vm, this: Valor) -> Result<bool, Valor> {
        match this {
            Valor::Bool(b) => Ok(b),
            Valor::Obj(id) => match vm.objs[id as usize].tipo {
                Tipo::Booleano(b) => Ok(b),
                _ => Err(vm.erro_tipo("not a boolean")),
            },
            _ => Err(vm.erro_tipo("not a boolean")),
        }
    }
    vm.def(p, "toString", 0, |vm, this, _| {
        let b = valor(vm, this)?;
        Ok(vm.txt(if b { "true" } else { "false" }))
    });
    vm.def(p, "valueOf", 0, |vm, this, _| Ok(Valor::Bool(valor(vm, this)?)));
}

fn reflect(vm: &mut Vm) {
    let r = vm.novo_objeto();
    let g = vm.global;
    vm.def_oculta(g, "Reflect", Valor::Obj(r));
    def_marca(vm, r, "Reflect");
    fn alvo(vm: &mut Vm, a: &[Valor]) -> Result<ObjId, Valor> {
        match arg(a, 0) {
            Valor::Obj(o) => Ok(o),
            _ => Err(vm.erro_tipo("Reflect method called on non-object")),
        }
    }
    vm.def(r, "get", 2, |vm, _, a| {
        let o = alvo(vm, a)?;
        let k = vm.para_chave(arg(a, 1))?;
        let rec = if a.len() > 2 { a[2] } else { a[0] };
        vm.obter_obj(o, k, rec)
    });
    vm.def(r, "set", 3, |vm, _, a| {
        let o = alvo(vm, a)?;
        let k = vm.para_chave(arg(a, 1))?;
        let rec = if a.len() > 3 { a[3] } else { a[0] };
        vm.definir_obj(o, k, arg(a, 2), rec)?;
        Ok(Valor::Bool(true))
    });
    vm.def(r, "has", 2, |vm, _, a| {
        let o = alvo(vm, a)?;
        let k = vm.para_chave(arg(a, 1))?;
        Ok(Valor::Bool(vm.tem_prop(o, k)?))
    });
    vm.def(r, "deleteProperty", 2, |vm, _, a| {
        let o = alvo(vm, a)?;
        let k = vm.para_chave(arg(a, 1))?;
        Ok(Valor::Bool(vm.apagar(o, k)?))
    });
    vm.def(r, "ownKeys", 1, |vm, _, a| {
        let o = alvo(vm, a)?;
        let c = vm.chaves_proprias(o, false, true, true)?;
        let v = c.into_iter().map(|k| vm.valor_da_chave(k)).collect();
        Ok(vm.novo_array(v))
    });
    vm.def(r, "getPrototypeOf", 1, |vm, _, a| {
        let o = alvo(vm, a)?;
        let p = vm.objs[o as usize].proto;
        Ok(if p == NENHUM { Valor::Nulo } else { Valor::Obj(p) })
    });
    vm.def(r, "setPrototypeOf", 2, |vm, _, a| {
        alvo(vm, a)?;
        vm.mudar_proto(arg(a, 0), arg(a, 1))?;
        Ok(Valor::Bool(true))
    });
    vm.def(r, "defineProperty", 3, |vm, _, a| {
        let o = alvo(vm, a)?;
        let k = vm.para_chave(arg(a, 1))?;
        Ok(Valor::Bool(definir_com_descritor(vm, o, k, arg(a, 2)).is_ok()))
    });
    vm.def(r, "getOwnPropertyDescriptor", 2, |vm, _, a| {
        let o = alvo(vm, a)?;
        let k = vm.para_chave(arg(a, 1))?;
        de_descritor(vm, o, k)
    });
    vm.def(r, "apply", 3, |vm, _, a| {
        let l = vm.lista_de_array(arg(a, 2))?;
        vm.chamar(arg(a, 0), arg(a, 1), &l)
    });
    vm.def(r, "construct", 2, |vm, _, a| {
        let l = vm.lista_de_array(arg(a, 1))?;
        let nt = if a.len() > 2 { a[2] } else { arg(a, 0) };
        vm.construir(arg(a, 0), &l, nt)
    });
    vm.def(r, "isExtensible", 1, |vm, _, a| {
        let o = alvo(vm, a)?;
        Ok(Valor::Bool(vm.objs[o as usize].extensivel()))
    });
    vm.def(r, "preventExtensions", 1, |vm, _, a| {
        let o = alvo(vm, a)?;
        vm.objs[o as usize].flags &= !O_EXTENSIVEL;
        Ok(Valor::Bool(true))
    });
}

// -----------------------------------------------------------------------------
// Funções globais
// -----------------------------------------------------------------------------

/// Percent-encoding do `encodeURI`: `seguros` são os que ficam como estão.
fn codificar_uri(vm: &mut Vm, v: Valor, seguros: &[u8]) -> Res {
    let s = vm.para_texto(v)?;
    let mut r = String::with_capacity(s.len());
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || seguros.contains(&b) {
            r.push(b as char);
        } else {
            r.push_str(&format!("%{:02X}", b));
        }
    }
    Ok(vm.txt(&r))
}

fn decodificar_uri(vm: &mut Vm, v: Valor, reservados: &[u8]) -> Res {
    let s = vm.para_texto(v)?;
    let b = s.as_bytes();
    let mut saida = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' {
            let h = (|| Some((b.get(i + 1).and_then(|c| (*c as char).to_digit(16))? * 16 + b.get(i + 2).and_then(|c| (*c as char).to_digit(16))?) as u8))();
            let Some(x) = h else {
                let p = vm.p.erro_uri;
                return Err(vm.novo_erro(p, "URI malformed"));
            };
            if reservados.contains(&x) {
                saida.extend_from_slice(&b[i..i + 3]);
            } else {
                saida.push(x);
            }
            i += 3;
        } else {
            saida.push(b[i]);
            i += 1;
        }
    }
    match String::from_utf8(saida) {
        Ok(t) => Ok(vm.txt(&t)),
        Err(_) => {
            let p = vm.p.erro_uri;
            Err(vm.novo_erro(p, "URI malformed"))
        }
    }
}

pub fn parse_float(s: &str) -> f64 {
    let t = s.trim_start_matches(|c: char| c.is_whitespace() || c == '\u{FEFF}');
    let b = t.as_bytes();
    let mut i = 0;
    if matches!(b.first(), Some(b'+' | b'-')) {
        i = 1;
    }
    if t[i..].starts_with("Infinity") {
        return if b[0] == b'-' { f64::NEG_INFINITY } else { f64::INFINITY };
    }
    let ini_dig = i;
    while i < b.len() && b[i].is_ascii_digit() {
        i += 1;
    }
    if i < b.len() && b[i] == b'.' {
        i += 1;
        while i < b.len() && b[i].is_ascii_digit() {
            i += 1;
        }
    }
    if i == ini_dig || (i == ini_dig + 1 && b[ini_dig] == b'.') {
        return f64::NAN;
    }
    if i < b.len() && matches!(b[i], b'e' | b'E') {
        let mut j = i + 1;
        if matches!(b.get(j), Some(b'+' | b'-')) {
            j += 1;
        }
        if b.get(j).is_some_and(|c| c.is_ascii_digit()) {
            while j < b.len() && b[j].is_ascii_digit() {
                j += 1;
            }
            i = j;
        }
    }
    t[..i].parse().unwrap_or(f64::NAN)
}

pub fn parse_int(s: &str, base: i32) -> f64 {
    let t = s.trim_start_matches(|c: char| c.is_whitespace() || c == '\u{FEFF}');
    let (neg, mut t) = match t.strip_prefix('-') {
        Some(r) => (true, r),
        None => (false, t.strip_prefix('+').unwrap_or(t)),
    };
    let mut base = base;
    if base != 0 && !(2..=36).contains(&base) {
        return f64::NAN;
    }
    if base == 0 || base == 16 {
        if let Some(r) = t.strip_prefix("0x").or_else(|| t.strip_prefix("0X")) {
            t = r;
            base = 16;
        }
    }
    if base == 0 {
        base = 10;
    }
    let mut v = 0.0f64;
    let mut algum = false;
    for c in t.chars() {
        match c.to_digit(base as u32) {
            Some(d) => {
                v = v * base as f64 + d as f64;
                algum = true;
            }
            None => break,
        }
    }
    if !algum {
        return f64::NAN;
    }
    if neg { -v } else { v }
}

fn globais(vm: &mut Vm) {
    let g = vm.global;
    vm.def_valor(g, vm.a.undefined, Valor::Undef, 0);
    let (k_nan, k_inf) = (vm.n.atomo("NaN"), vm.n.atomo("Infinity"));
    vm.def_valor(g, k_nan, Valor::Num(f64::NAN), 0);
    vm.def_valor(g, k_inf, Valor::Num(f64::INFINITY), 0);
    let f = vm.def(g, "parseInt", 2, |vm, _, a| {
        let s = vm.para_texto(arg(a, 0))?;
        let b = vm.para_i32(arg(a, 1))?;
        Ok(num(parse_int(&s, b)))
    });
    let _ = f;
    vm.def(g, "parseFloat", 1, |vm, _, a| {
        let s = vm.para_texto(arg(a, 0))?;
        Ok(num(parse_float(&s)))
    });
    vm.def(g, "isNaN", 1, |vm, _, a| Ok(Valor::Bool(vm.para_numero(arg(a, 0))?.is_nan())));
    vm.def(g, "isFinite", 1, |vm, _, a| Ok(Valor::Bool(vm.para_numero(arg(a, 0))?.is_finite())));
    vm.def(g, "encodeURIComponent", 1, |vm, _, a| codificar_uri(vm, arg(a, 0), b"-_.!~*'()"));
    vm.def(g, "encodeURI", 1, |vm, _, a| codificar_uri(vm, arg(a, 0), b"-_.!~*'();/?:@&=+$,#"));
    vm.def(g, "decodeURIComponent", 1, |vm, _, a| decodificar_uri(vm, arg(a, 0), b""));
    vm.def(g, "decodeURI", 1, |vm, _, a| decodificar_uri(vm, arg(a, 0), b";/?:@&=+$,#"));
    vm.def(g, "escape", 1, |vm, _, a| {
        let s = vm.para_texto(arg(a, 0))?;
        let mut r = String::new();
        for u in s.encode_utf16() {
            if u < 128 && ((u as u8).is_ascii_alphanumeric() || b"@*_+-./".contains(&(u as u8))) {
                r.push(u as u8 as char);
            } else if u < 256 {
                r.push_str(&format!("%{:02X}", u));
            } else {
                r.push_str(&format!("%u{:04X}", u));
            }
        }
        Ok(vm.txt(&r))
    });
    vm.def(g, "unescape", 1, |vm, _, a| {
        let s = vm.para_texto(arg(a, 0))?;
        let c: Vec<char> = s.chars().collect();
        let mut r = String::new();
        let mut i = 0;
        while i < c.len() {
            if c[i] == '%' {
                if i + 5 < c.len() && c[i + 1] == 'u' {
                    let h: String = c[i + 2..i + 6].iter().collect();
                    if let Ok(v) = u32::from_str_radix(&h, 16) {
                        r.push(char::from_u32(v).unwrap_or('\u{FFFD}'));
                        i += 6;
                        continue;
                    }
                }
                if i + 2 < c.len() {
                    let h: String = c[i + 1..i + 3].iter().collect();
                    if let Ok(v) = u32::from_str_radix(&h, 16) {
                        r.push(char::from_u32(v).unwrap_or('\u{FFFD}'));
                        i += 3;
                        continue;
                    }
                }
            }
            r.push(c[i]);
            i += 1;
        }
        Ok(vm.txt(&r))
    });
    vm.def(g, "eval", 1, |vm, _, a| {
        let Valor::Texto(t) = arg(a, 0) else { return Ok(arg(a, 0)) };
        let s = vm.n.texto(t);
        vm.avaliar(&s)
    });
    vm.def(g, "queueMicrotask", 1, |vm, _, a| {
        funcao_arg(vm, arg(a, 0))?;
        vm.microtarefas.push_back(Tarefa::Chamar(a[0]));
        Ok(Valor::Undef)
    });
}

// -----------------------------------------------------------------------------
// console
// -----------------------------------------------------------------------------

/// Um valor para o console, como o node mostra.
pub fn inspecionar(vm: &mut Vm, v: Valor, prof: u32, vistos: &mut Vec<ObjId>) -> String {
    match v {
        Valor::Texto(t) if prof > 0 => {
            let s = vm.n.texto(t);
            if s.contains('\'') && !s.contains('"') {
                format!("\"{}\"", s)
            } else {
                format!("'{}'", s.replace('\\', "\\\\").replace('\'', "\\'").replace('\n', "\\n"))
            }
        }
        Valor::Num(n) if n == 0.0 && n.is_sign_negative() => String::from("-0"),
        Valor::Obj(id) => inspecionar_obj(vm, id, prof, vistos),
        outro => vm.texto_de_primitivo(outro),
    }
}

fn chave_simples(s: &str) -> bool {
    !s.is_empty() && s.bytes().enumerate().all(|(i, c)| c.is_ascii_alphabetic() || c == b'_' || c == b'$' || (i > 0 && c.is_ascii_digit()))
}

fn inspecionar_obj(vm: &mut Vm, id: ObjId, prof: u32, vistos: &mut Vec<ObjId>) -> String {
    if vistos.contains(&id) {
        return String::from("[Circular *1]");
    }
    let v = Valor::Obj(id);
    let chaves = vm.chaves_proprias(id, true, true, true).unwrap_or_default();
    let mut itens: Vec<String> = Vec::new();
    let e_arr = matches!(vm.objs[id as usize].tipo, Tipo::Array(_));
    let tipado = matches!(vm.objs[id as usize].tipo, Tipo::Tipado { .. });
    let mut prefixo = String::new();
    let nome_classe = {
        let k = K_CONSTRUCTOR;
        match vm.prop_simples(id, k) {
            Some(Valor::Obj(c)) => match vm.prop_simples_nome(c) {
                Some(n) => n,
                None => String::new(),
            },
            _ => String::new(),
        }
    };
    match &vm.objs[id as usize].tipo {
        Tipo::Funcao(_) | Tipo::Nativa(_) | Tipo::Ligada(_) => {
            let nome = vm.obter(v, K_NAME).unwrap_or(Valor::Undef);
            let nome = vm.texto_de_primitivo(nome);
            let classe = matches!(&vm.objs[id as usize].tipo, Tipo::Funcao(c) if c.func.construtor);
            prefixo = if classe {
                format!("[class {}]", if nome.is_empty() { "(anonymous)" } else { &nome })
            } else if nome.is_empty() {
                String::from("[Function (anonymous)]")
            } else {
                format!("[Function: {}]", nome)
            };
            if chaves.is_empty() {
                return prefixo;
            }
        }
        Tipo::Erro => {
            let st = vm.prop_simples(id, vm.a.stack);
            if let Some(Valor::Texto(t)) = st {
                return vm.n.texto(t).to_string();
            }
            return vm.descrever_erro(v);
        }
        Tipo::Regex(_) => {
            let t = vm.chamar_to_string(v);
            return t;
        }
        Tipo::Data(d) => {
            let d = *d;
            return super::nativos2::data_iso(d).unwrap_or_else(|| String::from("Invalid Date"));
        }
        Tipo::Booleano(b) => return format!("[Boolean: {}]", b),
        Tipo::Numero(n) => return format!("[Number: {}]", numero_para_texto(*n)),
        Tipo::Texto(t) => {
            let t = *t;
            return format!("[String: '{}']", vm.n.texto(t));
        }
        Tipo::Promessa(p) => {
            let (e, x) = (p.estado, p.valor);
            vistos.push(id);
            let d = match e {
                0 => String::from("<pending>"),
                1 => inspecionar(vm, x, prof + 1, vistos),
                _ => format!("<rejected> {}", inspecionar(vm, x, prof + 1, vistos)),
            };
            vistos.pop();
            return format!("Promise {{ {} }}", d);
        }
        Tipo::Mapa(c) => {
            let pares: Vec<(Valor, Valor)> = c.itens.iter().flatten().copied().collect();
            vistos.push(id);
            let t: Vec<String> = pares
                .into_iter()
                .map(|(k, x)| {
                    let a = inspecionar(vm, k, prof + 1, vistos);
                    let b = inspecionar(vm, x, prof + 1, vistos);
                    format!("{} => {}", a, b)
                })
                .collect();
            vistos.pop();
            let n = t.len();
            return if t.is_empty() { String::from("Map(0) {}") } else { format!("Map({}) {{ {} }}", n, t.join(", ")) };
        }
        Tipo::Conjunto(c) => {
            let vals: Vec<Valor> = c.itens.iter().flatten().map(|p| p.0).collect();
            vistos.push(id);
            let t: Vec<String> = vals.into_iter().map(|x| inspecionar(vm, x, prof + 1, vistos)).collect();
            vistos.pop();
            let n = t.len();
            return if t.is_empty() { String::from("Set(0) {}") } else { format!("Set({}) {{ {} }}", n, t.join(", ")) };
        }
        _ => {}
    }
    if prof > 2 {
        return String::from(if e_arr { "[Array]" } else { "[Object]" });
    }
    vistos.push(id);
    if e_arr {
        let n = comprimento(vm, v).unwrap_or(0);
        let mut buracos = 0;
        for i in 0..n.min(100) {
            let x = match &vm.objs[id as usize].tipo {
                Tipo::Array(a) => a[i],
                _ => Valor::Undef,
            };
            if x == Valor::Vazio {
                buracos += 1;
                continue;
            }
            if buracos > 0 {
                itens.push(format!("<{} empty item{}>", buracos, if buracos > 1 { "s" } else { "" }));
                buracos = 0;
            }
            itens.push(inspecionar(vm, x, prof + 1, vistos));
        }
        if buracos > 0 {
            itens.push(format!("<{} empty item{}>", buracos, if buracos > 1 { "s" } else { "" }));
        }
        if n > 100 {
            let resto = n - 100;
            itens.push(format!("... {} more item{}", resto, if resto > 1 { "s" } else { "" }));
        }
    }
    for k in chaves {
        let simb = vm.n.e_simbolo(k);
        let t = vm.n.texto(k);
        if (e_arr || tipado) && !simb && vm.n.indice(k) != NENHUM {
            continue;
        }
        let nome = if simb {
            format!("[Symbol({})]", t)
        } else if chave_simples(&t) {
            t.to_string()
        } else {
            format!("'{}'", t)
        };
        let props = {
            let o = &vm.objs[id as usize];
            o.achar(k).map(|i| o.props[i])
        };
        let x = match props {
            Some(p) if p.flags & P_ACESSOR != 0 => {
                let (mut g, mut s) = (false, false);
                if let Valor::Obj(pid) = p.valor {
                    if let Tipo::Acessor(a, b) = vm.objs[pid as usize].tipo {
                        g = a != Valor::Undef;
                        s = b != Valor::Undef;
                    }
                }
                String::from(match (g, s) {
                    (true, true) => "[Getter/Setter]",
                    (true, false) => "[Getter]",
                    _ => "[Setter]",
                })
            }
            _ => {
                let val = vm.obter_obj(id, k, v).unwrap_or(Valor::Undef);
                inspecionar(vm, val, prof + 1, vistos)
            }
        };
        itens.push(format!("{}: {}", nome, x));
    }
    vistos.pop();
    if e_arr {
        return if itens.is_empty() { String::from("[]") } else { format!("[ {} ]", itens.join(", ")) };
    }
    if prefixo.is_empty() && !nome_classe.is_empty() && nome_classe != "Object" {
        prefixo = nome_classe;
    }
    if vm.objs[id as usize].proto == NENHUM && prefixo.is_empty() {
        prefixo = String::from("[Object: null prototype]");
    }
    if itens.is_empty() {
        return if prefixo.is_empty() { String::from("{}") } else if prefixo.starts_with('[') { format!("{} {{}}", prefixo) } else { format!("{} {{}}", prefixo) };
    }
    let corpo = format!("{{ {} }}", itens.join(", "));
    if prefixo.is_empty() { corpo } else { format!("{} {}", prefixo, corpo) }
}

impl Vm {
    fn chamar_to_string(&mut self, v: Valor) -> String {
        match self.para_texto(v) {
            Ok(t) => t.to_string(),
            Err(_) => String::from("[object]"),
        }
    }

    fn prop_simples_nome(&mut self, c: ObjId) -> Option<String> {
        self.materializar(c);
        match self.prop_simples(c, K_NAME) {
            Some(Valor::Texto(t)) => Some(self.n.texto(t).to_string()),
            _ => None,
        }
    }
}

pub fn formatar_console(vm: &mut Vm, a: &[Valor]) -> String {
    let mut s = String::new();
    let mut inicio = 0;
    // console.log("%s é %d", x, y)
    if let Some(Valor::Texto(t)) = a.first() {
        let f = vm.n.texto(*t);
        if f.contains('%') && a.len() > 1 {
            let c: Vec<char> = f.chars().collect();
            let mut i = 0;
            let mut n = 1;
            while i < c.len() {
                if c[i] == '%' && i + 1 < c.len() {
                    match c[i + 1] {
                        's' | 'd' | 'i' | 'f' | 'o' | 'O' | 'j' if n < a.len() => {
                            let mut vistos = Vec::new();
                            let x = a[n];
                            let tx = match c[i + 1] {
                                'd' | 'i' | 'f' => {
                                    let v = vm.para_numero(x).unwrap_or(f64::NAN);
                                    numero_para_texto(if c[i + 1] == 'i' { mat::trunc(v) } else { v })
                                }
                                's' => match x {
                                    Valor::Texto(_) => vm.para_texto(x).map_or(String::new(), |r| r.to_string()),
                                    _ => inspecionar(vm, x, 1, &mut vistos),
                                },
                                _ => inspecionar(vm, x, 1, &mut vistos),
                            };
                            s.push_str(&tx);
                            n += 1;
                            i += 2;
                            continue;
                        }
                        'c' if n < a.len() => {
                            n += 1;
                            i += 2;
                            continue;
                        }
                        '%' => {
                            s.push('%');
                            i += 2;
                            continue;
                        }
                        _ => {}
                    }
                }
                s.push(c[i]);
                i += 1;
            }
            inicio = n;
            if inicio < a.len() {
                s.push(' ');
            }
        }
    }
    for (i, x) in a.iter().enumerate().skip(inicio) {
        if i > inicio {
            s.push(' ');
        }
        let mut vistos = Vec::new();
        let t = inspecionar(vm, *x, 0, &mut vistos);
        s.push_str(&t);
    }
    s
}

fn console(vm: &mut Vm) {
    let c = vm.novo_objeto();
    let g = vm.global;
    vm.def_oculta(g, "console", Valor::Obj(c));
    fn saida(vm: &mut Vm, nivel: u8, a: &[Valor]) -> Res {
        let t = formatar_console(vm, a);
        (vm.ganchos.console)(vm, nivel, &t);
        Ok(Valor::Undef)
    }
    vm.def(c, "log", 0, |vm, _, a| saida(vm, 0, a));
    vm.def(c, "info", 0, |vm, _, a| saida(vm, 1, a));
    vm.def(c, "debug", 0, |vm, _, a| saida(vm, 0, a));
    vm.def(c, "warn", 0, |vm, _, a| saida(vm, 2, a));
    vm.def(c, "error", 0, |vm, _, a| saida(vm, 3, a));
    vm.def(c, "trace", 0, |vm, _, a| saida(vm, 0, a));
    vm.def(c, "assert", 0, |vm, _, a| {
        if !vm.para_bool(arg(a, 0)) {
            let t = formatar_console(vm, a.get(1..).unwrap_or(&[]));
            let m = format!("Assertion failed: {}", t);
            (vm.ganchos.console)(vm, 3, &m);
        }
        Ok(Valor::Undef)
    });
    for n in ["group", "groupCollapsed", "groupEnd", "time", "timeEnd", "timeLog", "count", "dir", "table", "clear", "profile", "profileEnd"] {
        vm.def(c, n, 0, |_, _, _| Ok(Valor::Undef));
    }
}
