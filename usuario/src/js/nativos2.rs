//! Os objetos embutidos, parte 2: String, RegExp, Number, Math, JSON, Date,
//! Map, Set, Promise, Proxy e os arrays tipados.

use super::lexico::*;
use super::nativos::*;
use super::regex::{Entrada, Regex};
use super::vm::*;
use crate::mat;
use alloc::boxed::Box;
use alloc::format;
use alloc::rc::Rc;
use alloc::string::{String, ToString};
use alloc::vec;
use alloc::vec::Vec;

pub fn instalar(vm: &mut Vm) {
    texto(vm);
    regexp(vm);
    numero(vm);
    math(vm);
    json(vm);
    data(vm);
    colecoes(vm);
    promessas(vm);
    proxy(vm);
    tipados(vm);
}

// -----------------------------------------------------------------------------
// String
// -----------------------------------------------------------------------------

fn this_texto(vm: &mut Vm, this: Valor) -> Result<Txt, Valor> {
    match this {
        Valor::Texto(t) => Ok(t),
        Valor::Undef | Valor::Nulo => Err(vm.erro_tipo("String.prototype method called on null or undefined")),
        Valor::Obj(id) => {
            if let Tipo::Texto(t) = vm.objs[id as usize].tipo {
                return Ok(t);
            }
            vm.para_texto_id(this)
        }
        _ => vm.para_texto_id(this),
    }
}

/// Um pedaço do texto, em unidades de UTF-16.
fn sub_utf16(s: &str, ascii: bool, a: usize, b: usize) -> String {
    if a >= b {
        return String::new();
    }
    if ascii {
        return String::from(&s[a..b.min(s.len())]);
    }
    let u: Vec<u16> = s.encode_utf16().skip(a).take(b - a).collect();
    String::from_utf16_lossy(&u)
}

/// Quantas unidades de UTF-16 há nos primeiros `bytes` bytes.
fn unidades(s: &str, bytes: usize) -> usize {
    s[..bytes].chars().map(char::len_utf16).sum()
}

/// O byte onde começa a unidade `i` (ou o fim do texto).
fn byte_de(s: &str, i: usize) -> usize {
    let mut u = 0;
    for (b, c) in s.char_indices() {
        if u >= i {
            return b;
        }
        u += c.len_utf16();
    }
    s.len()
}

fn e_branco(c: char) -> bool {
    c.is_whitespace() || c == '\u{FEFF}'
}

fn regex_de(vm: &Vm, v: Valor) -> Option<Rc<Regex>> {
    if let Valor::Obj(id) = v {
        if let Tipo::Regex(r) = &vm.objs[id as usize].tipo {
            return Some(r.clone());
        }
    }
    None
}

fn expandir_troca(modelo: &str, e: &Entrada, g: &[i32], r: Option<&Regex>, saida: &mut String) {
    if !modelo.contains('$') {
        saida.push_str(modelo);
        return;
    }
    let c: Vec<char> = modelo.chars().collect();
    let mut i = 0;
    while i < c.len() {
        if c[i] == '$' && i + 1 < c.len() {
            let d = c[i + 1];
            match d {
                '$' => {
                    saida.push('$');
                    i += 2;
                    continue;
                }
                '&' => {
                    saida.push_str(&e.pedaco(g[0] as usize, g[1] as usize));
                    i += 2;
                    continue;
                }
                '`' => {
                    saida.push_str(&e.pedaco(0, g[0] as usize));
                    i += 2;
                    continue;
                }
                '\'' => {
                    saida.push_str(&e.pedaco(g[1] as usize, e.len()));
                    i += 2;
                    continue;
                }
                '0'..='9' => {
                    let n_grupos = g.len() / 2 - 1;
                    let mut n = d.to_digit(10).unwrap() as usize;
                    let mut usados = 2;
                    if let Some(d2) = c.get(i + 2).and_then(|x| x.to_digit(10)) {
                        let dois = n * 10 + d2 as usize;
                        if dois >= 1 && dois <= n_grupos {
                            n = dois;
                            usados = 3;
                        }
                    }
                    if n >= 1 && n <= n_grupos {
                        let (a, b) = (g[n * 2], g[n * 2 + 1]);
                        if a >= 0 && b >= 0 {
                            saida.push_str(&e.pedaco(a as usize, b as usize));
                        }
                        i += usados;
                        continue;
                    }
                }
                '<' => {
                    if let Some(r) = r {
                        if !r.nomes.is_empty() {
                            if let Some(f) = c[i + 2..].iter().position(|x| *x == '>') {
                                let nome: String = c[i + 2..i + 2 + f].iter().collect();
                                if let Some((_, n)) = r.nomes.iter().find(|(x, _)| *x == nome) {
                                    let (a, b) = (g[*n * 2], g[*n * 2 + 1]);
                                    if a >= 0 && b >= 0 {
                                        saida.push_str(&e.pedaco(a as usize, b as usize));
                                    }
                                }
                                i += f + 3;
                                continue;
                            }
                        }
                    }
                }
                _ => {}
            }
        }
        saida.push(c[i]);
        i += 1;
    }
}

fn ler_last_index(vm: &mut Vm, id: ObjId) -> Result<usize, Valor> {
    let k = vm.a.last_index;
    let v = vm.obter_obj(id, k, Valor::Obj(id))?;
    Ok(vm.para_inteiro(v)?.max(0.0) as usize)
}

fn escrever_last_index(vm: &mut Vm, id: ObjId, i: usize) {
    let k = vm.a.last_index;
    vm.def_valor(id, k, num(i as f64), P_ESCRITA);
}

/// Monta o array de resultado de um `exec`.
fn resultado_exec(vm: &mut Vm, r: &Regex, e: &Entrada, g: &[i32], entrada: Valor) -> Valor {
    let mut itens = Vec::with_capacity(g.len() / 2);
    for par in g.chunks(2) {
        if par[0] >= 0 && par[1] >= 0 {
            let s = e.pedaco(par[0] as usize, par[1] as usize);
            itens.push(vm.txt(&s));
        } else {
            itens.push(Valor::Undef);
        }
    }
    let grupos = if r.nomes.is_empty() {
        Valor::Undef
    } else {
        let o = vm.novo_obj_com(NENHUM, Tipo::Comum);
        for (nome, n) in &r.nomes {
            let k = vm.n.atomo(nome);
            vm.def_valor(o, k, itens[*n], P_TUDO);
        }
        Valor::Obj(o)
    };
    let arr = vm.novo_array(itens);
    if let Valor::Obj(id) = arr {
        let (ki, kn, kg) = (vm.a.index, vm.a.input, vm.a.groups);
        vm.def_valor(id, ki, Valor::Int(g[0]), P_TUDO);
        vm.def_valor(id, kn, entrada, P_TUDO);
        vm.def_valor(id, kg, grupos, P_TUDO);
    }
    arr
}

pub fn regex_exec(vm: &mut Vm, id: ObjId, t: Txt) -> Res {
    let Tipo::Regex(r) = &vm.objs[id as usize].tipo else { return Err(vm.erro_tipo("not a RegExp")) };
    let r = r.clone();
    let s = vm.n.texto(t);
    let e = Entrada::de(&s);
    let usa_last = r.global || r.pegajosa;
    let inicio = if usa_last { ler_last_index(vm, id)? } else { 0 };
    if inicio > e.len() {
        escrever_last_index(vm, id, 0);
        return Ok(Valor::Nulo);
    }
    match r.casar(&e, inicio) {
        None => {
            if usa_last {
                escrever_last_index(vm, id, 0);
            }
            Ok(Valor::Nulo)
        }
        Some(g) => {
            if usa_last {
                escrever_last_index(vm, id, g[1] as usize);
            }
            Ok(resultado_exec(vm, &r, &e, &g, Valor::Texto(t)))
        }
    }
}

pub fn nova_regex(vm: &mut Vm, fonte: &str, flags: &str) -> Res {
    let r = match Regex::compilar(fonte, flags) {
        Ok(r) => r,
        Err(e) => return Err(vm.erro_sintaxe(&e)),
    };
    let p = vm.p.regex;
    let id = vm.novo_obj_com(p, Tipo::Regex(Rc::new(r)));
    escrever_last_index(vm, id, 0);
    Ok(Valor::Obj(id))
}

fn trocar(vm: &mut Vm, this: Valor, a: &[Valor], todas: bool) -> Res {
    let t = this_texto(vm, this)?;
    let s = vm.n.texto(t);
    let padrao = arg(a, 0);
    let troca = arg(a, 1);
    let func = vm.e_funcao(troca);
    let modelo: Rc<str> = if func { Rc::from("") } else { vm.para_texto(troca)? };
    let mut saida = String::with_capacity(s.len());
    if let Some(r) = regex_de(vm, padrao) {
        let id = padrao.obj().unwrap();
        if todas && !r.global {
            return Err(vm.erro_tipo("replaceAll must be called with a global RegExp"));
        }
        let e = Entrada::de(&s);
        let mut pos = if r.global || !r.pegajosa { 0 } else { ler_last_index(vm, id)? };
        let mut ultimo = 0;
        while pos <= e.len() {
            let Some(g) = r.casar(&e, pos) else { break };
            let (ini, fim) = (g[0] as usize, g[1] as usize);
            saida.push_str(&e.pedaco(ultimo, ini));
            if func {
                let mut args = Vec::with_capacity(g.len() / 2 + 2);
                for par in g.chunks(2) {
                    if par[0] >= 0 && par[1] >= 0 {
                        let p = e.pedaco(par[0] as usize, par[1] as usize);
                        args.push(vm.txt(&p));
                    } else {
                        args.push(Valor::Undef);
                    }
                }
                args.push(Valor::Int(g[0]));
                args.push(Valor::Texto(t));
                if !r.nomes.is_empty() {
                    let o = vm.novo_obj_com(NENHUM, Tipo::Comum);
                    for (nome, n) in &r.nomes {
                        let k = vm.n.atomo(nome);
                        vm.def_valor(o, k, args[*n], P_TUDO);
                    }
                    args.push(Valor::Obj(o));
                }
                let x = vm.chamar(troca, Valor::Undef, &args)?;
                let x = vm.para_texto(x)?;
                saida.push_str(&x);
            } else {
                expandir_troca(&modelo, &e, &g, Some(&r), &mut saida);
            }
            ultimo = fim;
            pos = if fim == ini { fim + 1 } else { fim };
            if !r.global {
                break;
            }
        }
        if r.global {
            escrever_last_index(vm, id, 0);
        }
        if ultimo <= e.len() {
            saida.push_str(&e.pedaco(ultimo, e.len()));
        }
        return Ok(vm.txt(&saida));
    }
    // Um padrão com Symbol.replace próprio.
    if let Valor::Obj(_) = padrao {
        let k = vm.s.replace;
        let m = vm.obter(padrao, k)?;
        if !m.nulo_ou_undef() {
            return vm.chamar(m, padrao, &[Valor::Texto(t), troca]);
        }
    }
    let p = vm.para_texto(padrao)?;
    let e = Entrada::de(&s);
    let mut pos_byte = 0;
    let mut algum = false;
    loop {
        let Some(rel) = s[pos_byte..].find(&*p) else { break };
        let ini = pos_byte + rel;
        saida.push_str(&s[pos_byte..ini]);
        let u_ini = unidades(&s, ini) as i32;
        let u_fim = u_ini + p.chars().map(char::len_utf16).sum::<usize>() as i32;
        if func {
            let m = vm.txt(&p);
            let x = vm.chamar(troca, Valor::Undef, &[m, Valor::Int(u_ini), Valor::Texto(t)])?;
            let x = vm.para_texto(x)?;
            saida.push_str(&x);
        } else {
            expandir_troca(&modelo, &e, &[u_ini, u_fim], None, &mut saida);
        }
        algum = true;
        pos_byte = ini + p.len();
        if !todas {
            break;
        }
        if p.is_empty() {
            let Some(c) = s[pos_byte..].chars().next() else { break };
            saida.push(c);
            pos_byte += c.len_utf8();
        }
    }
    let _ = algum;
    saida.push_str(&s[pos_byte.min(s.len())..]);
    Ok(vm.txt(&saida))
}

fn dividir(vm: &mut Vm, this: Valor, a: &[Valor]) -> Res {
    let t = this_texto(vm, this)?;
    let s = vm.n.texto(t);
    let lim = if arg(a, 1) == Valor::Undef { u32::MAX } else { vm.para_numero(a[1])? as u32 };
    if lim == 0 {
        return Ok(vm.novo_array(Vec::new()));
    }
    let sep = arg(a, 0);
    let mut saida: Vec<Valor> = Vec::new();
    if let Some(r) = regex_de(vm, sep) {
        let e = Entrada::de(&s);
        if e.is_empty() {
            if r.casar(&e, 0).is_some() {
                return Ok(vm.novo_array(Vec::new()));
            }
            return Ok(vm.novo_array(vec![Valor::Texto(t)]));
        }
        let (mut p, mut q) = (0usize, 0usize);
        while q < e.len() {
            let Some(g) = r.casar(&e, q) else { break };
            let (ini, fim) = (g[0] as usize, g[1] as usize);
            if ini >= e.len() {
                break;
            }
            if fim == ini && ini == p {
                // Casou vazio bem aqui: avança uma letra.
                if ini > q {
                    q = ini;
                } else {
                    q += 1;
                }
                if fim == ini && q > p && q <= e.len() {
                    let pedaco = e.pedaco(p, q);
                    if r.casar(&e, q).is_some_and(|x| x[0] as usize == q && x[1] as usize == q) && q < e.len() {
                        let x = vm.txt(&pedaco);
                        saida.push(x);
                        if saida.len() as u32 >= lim {
                            return Ok(vm.novo_array(saida));
                        }
                        p = q;
                    }
                }
                continue;
            }
            if fim == ini && ini == e.len() {
                break;
            }
            let x = e.pedaco(p, ini);
            let x = vm.txt(&x);
            saida.push(x);
            if saida.len() as u32 >= lim {
                return Ok(vm.novo_array(saida));
            }
            for par in g.chunks(2).skip(1) {
                if par[0] >= 0 && par[1] >= 0 {
                    let x = e.pedaco(par[0] as usize, par[1] as usize);
                    let x = vm.txt(&x);
                    saida.push(x);
                } else {
                    saida.push(Valor::Undef);
                }
                if saida.len() as u32 >= lim {
                    return Ok(vm.novo_array(saida));
                }
            }
            p = fim;
            q = if fim == ini { fim + 1 } else { fim };
        }
        let x = e.pedaco(p, e.len());
        let x = vm.txt(&x);
        saida.push(x);
        return Ok(vm.novo_array(saida));
    }
    if sep == Valor::Undef {
        return Ok(vm.novo_array(vec![Valor::Texto(t)]));
    }
    let d = vm.para_texto(sep)?;
    if d.is_empty() {
        if s.is_ascii() {
            for c in s.bytes().take(lim as usize) {
                let mut buf = [0u8; 4];
                let a = vm.n.atomo((c as char).encode_utf8(&mut buf));
                saida.push(Valor::Texto(a));
            }
        } else {
            let u: Vec<u16> = s.encode_utf16().collect();
            for x in u.iter().take(lim as usize) {
                let q = String::from_utf16_lossy(&[*x]);
                saida.push(vm.txt(&q));
            }
        }
        return Ok(vm.novo_array(saida));
    }
    for p in s.split(&*d) {
        if saida.len() as u32 >= lim {
            break;
        }
        saida.push(vm.txt(p));
    }
    Ok(vm.novo_array(saida))
}

fn preencher(vm: &mut Vm, this: Valor, a: &[Valor], inicio: bool) -> Res {
    let t = this_texto(vm, this)?;
    let n = vm.para_inteiro(arg(a, 0))?;
    let atual = vm.comprimento_texto(t);
    if n <= atual as f64 {
        return Ok(Valor::Texto(t));
    }
    if n > 1e8 {
        return Err(vm.erro_intervalo("Invalid string length"));
    }
    let f: Rc<str> = if arg(a, 1) == Valor::Undef { Rc::from(" ") } else { vm.para_texto(a[1])? };
    if f.is_empty() {
        return Ok(Valor::Texto(t));
    }
    let falta = n as usize - atual;
    let u: Vec<u16> = f.encode_utf16().cycle().take(falta).collect();
    let extra = String::from_utf16_lossy(&u);
    let s = vm.n.texto(t);
    let r = if inicio { format!("{}{}", extra, s) } else { format!("{}{}", s, extra) };
    Ok(vm.txt(&r))
}

fn texto(vm: &mut Vm) {
    let p = vm.p.texto;
    let c = construtor(vm, "String", 1, |vm, _, a| {
        let t = if a.is_empty() {
            K_VAZIO
        } else if let (Valor::Simbolo(_), Valor::Undef) = (a[0], vm.novo_alvo) {
            let s = vm.texto_de_primitivo(a[0]);
            vm.n.novo_texto(&s)
        } else {
            vm.para_texto_id(a[0])?
        };
        if vm.novo_alvo != Valor::Undef {
            let nt = vm.novo_alvo;
            let proto = vm.proto_de_novo_alvo(nt, vm.p.texto)?;
            return Ok(Valor::Obj(vm.novo_obj_com(proto, Tipo::Texto(t))));
        }
        Ok(Valor::Texto(t))
    }, p);
    vm.def(c, "fromCharCode", 1, |vm, _, a| {
        let mut u = Vec::with_capacity(a.len());
        for x in a {
            u.push(vm.para_i32(*x)? as u16);
        }
        let s = String::from_utf16_lossy(&u);
        Ok(vm.txt(&s))
    });
    vm.def(c, "fromCodePoint", 1, |vm, _, a| {
        let mut s = String::new();
        for x in a {
            let n = vm.para_numero(*x)?;
            if n < 0.0 || n > 1114111.0 || mat::trunc(n) != n {
                return Err(vm.erro_intervalo("Invalid code point"));
            }
            s.push(char::from_u32(n as u32).unwrap_or('\u{FFFD}'));
        }
        Ok(vm.txt(&s))
    });
    vm.def(c, "raw", 1, |vm, _, a| {
        let k = vm.a.raw;
        let crus = vm.obter(arg(a, 0), k)?;
        let itens = vm.lista_de_array(crus)?;
        let mut s = String::new();
        for (i, x) in itens.iter().enumerate() {
            let t = vm.para_texto(*x)?;
            s.push_str(&t);
            if i + 1 < itens.len() {
                if let Some(v) = a.get(i + 1) {
                    let t = vm.para_texto(*v)?;
                    s.push_str(&t);
                }
            }
        }
        Ok(vm.txt(&s))
    });

    vm.def(p, "toString", 0, |vm, this, _| Ok(Valor::Texto(this_texto(vm, this)?)));
    vm.def(p, "valueOf", 0, |vm, this, _| Ok(Valor::Texto(this_texto(vm, this)?)));
    vm.def(p, "charAt", 1, |vm, this, a| {
        let t = this_texto(vm, this)?;
        let i = vm.para_inteiro(arg(a, 0))?;
        if i < 0.0 {
            return Ok(Valor::Texto(K_VAZIO));
        }
        Ok(vm.char_em(t, i as usize).unwrap_or(Valor::Texto(K_VAZIO)))
    });
    vm.def(p, "charCodeAt", 1, |vm, this, a| {
        let t = this_texto(vm, this)?;
        let i = vm.para_inteiro(arg(a, 0))?;
        if i < 0.0 {
            return Ok(Valor::Num(f64::NAN));
        }
        let i = i as usize;
        let s = vm.n.texto(t);
        if vm.n.e_ascii(t) {
            return Ok(s.as_bytes().get(i).map_or(Valor::Num(f64::NAN), |b| Valor::Int(*b as i32)));
        }
        Ok(s.encode_utf16().nth(i).map_or(Valor::Num(f64::NAN), |u| Valor::Int(u as i32)))
    });
    vm.def(p, "codePointAt", 1, |vm, this, a| {
        let t = this_texto(vm, this)?;
        let i = vm.para_inteiro(arg(a, 0))?;
        if i < 0.0 {
            return Ok(Valor::Undef);
        }
        let s = vm.n.texto(t);
        let u: Vec<u16> = s.encode_utf16().skip(i as usize).take(2).collect();
        Ok(match char::decode_utf16(u.iter().copied()).next() {
            Some(Ok(c)) => Valor::Int(c as i32),
            Some(Err(e)) => Valor::Int(e.unpaired_surrogate() as i32),
            None => Valor::Undef,
        })
    });
    vm.def(p, "at", 1, |vm, this, a| {
        let t = this_texto(vm, this)?;
        let n = vm.comprimento_texto(t) as f64;
        let i = vm.para_inteiro(arg(a, 0))?;
        let i = if i < 0.0 { n + i } else { i };
        if i < 0.0 || i >= n {
            return Ok(Valor::Undef);
        }
        Ok(vm.char_em(t, i as usize).unwrap_or(Valor::Undef))
    });
    vm.def(p, "indexOf", 1, |vm, this, a| {
        let t = this_texto(vm, this)?;
        let s = vm.n.texto(t);
        let d = vm.para_texto(arg(a, 0))?;
        let de = vm.para_inteiro(arg(a, 1))?.max(0.0) as usize;
        let b = byte_de(&s, de);
        Ok(s[b..].find(&*d).map_or(Valor::Int(-1), |r| num(unidades(&s, b + r) as f64)))
    });
    vm.def(p, "lastIndexOf", 1, |vm, this, a| {
        let t = this_texto(vm, this)?;
        let s = vm.n.texto(t);
        let d = vm.para_texto(arg(a, 0))?;
        let de = vm.para_numero(arg(a, 1))?;
        let limite = if de.is_nan() { s.len() } else { byte_de(&s, de.max(0.0) as usize) };
        let fim = (limite + d.len()).min(s.len());
        let fim = if s.is_char_boundary(fim) { fim } else { s.len() };
        Ok(s[..fim].rfind(&*d).map_or(Valor::Int(-1), |r| num(unidades(&s, r) as f64)))
    });
    vm.def(p, "includes", 1, |vm, this, a| {
        if regex_de(vm, arg(a, 0)).is_some() {
            return Err(vm.erro_tipo("First argument to String.prototype.includes must not be a regular expression"));
        }
        let t = this_texto(vm, this)?;
        let s = vm.n.texto(t);
        let d = vm.para_texto(arg(a, 0))?;
        let de = vm.para_inteiro(arg(a, 1))?.max(0.0) as usize;
        Ok(Valor::Bool(s[byte_de(&s, de)..].contains(&*d)))
    });
    vm.def(p, "startsWith", 1, |vm, this, a| {
        if regex_de(vm, arg(a, 0)).is_some() {
            return Err(vm.erro_tipo("First argument to String.prototype.startsWith must not be a regular expression"));
        }
        let t = this_texto(vm, this)?;
        let s = vm.n.texto(t);
        let d = vm.para_texto(arg(a, 0))?;
        let de = vm.para_inteiro(arg(a, 1))?.max(0.0) as usize;
        Ok(Valor::Bool(s[byte_de(&s, de)..].starts_with(&*d)))
    });
    vm.def(p, "endsWith", 1, |vm, this, a| {
        let t = this_texto(vm, this)?;
        let s = vm.n.texto(t);
        let d = vm.para_texto(arg(a, 0))?;
        let fim = if arg(a, 1) == Valor::Undef { s.len() } else { byte_de(&s, vm.para_inteiro(a[1])?.max(0.0) as usize) };
        Ok(Valor::Bool(s[..fim].ends_with(&*d)))
    });
    vm.def(p, "slice", 2, |vm, this, a| {
        let t = this_texto(vm, this)?;
        let n = vm.comprimento_texto(t);
        let i = indice_relativo(vm, arg(a, 0), n, 0)?;
        let f = indice_relativo(vm, arg(a, 1), n, n)?;
        let ascii = vm.n.e_ascii(t);
        let s = vm.n.texto(t);
        let r = sub_utf16(&s, ascii, i, f);
        Ok(vm.txt(&r))
    });
    vm.def(p, "substring", 2, |vm, this, a| {
        let t = this_texto(vm, this)?;
        let n = vm.comprimento_texto(t) as f64;
        let i = vm.para_inteiro(arg(a, 0))?.clamp(0.0, n);
        let f = if arg(a, 1) == Valor::Undef { n } else { vm.para_inteiro(a[1])?.clamp(0.0, n) };
        let (i, f) = if i > f { (f, i) } else { (i, f) };
        let ascii = vm.n.e_ascii(t);
        let s = vm.n.texto(t);
        let r = sub_utf16(&s, ascii, i as usize, f as usize);
        Ok(vm.txt(&r))
    });
    vm.def(p, "substr", 2, |vm, this, a| {
        let t = this_texto(vm, this)?;
        let n = vm.comprimento_texto(t);
        let i = indice_relativo(vm, arg(a, 0), n, 0)?;
        let q = if arg(a, 1) == Valor::Undef { n as f64 } else { vm.para_inteiro(a[1])? };
        let f = (i as f64 + q.max(0.0)).min(n as f64) as usize;
        let ascii = vm.n.e_ascii(t);
        let s = vm.n.texto(t);
        let r = sub_utf16(&s, ascii, i, f);
        Ok(vm.txt(&r))
    });
    fn maiusc(vm: &mut Vm, this: Valor, _: &[Valor]) -> Res {
        let t = this_texto(vm, this)?;
        let s = vm.n.texto(t);
        let r = s.to_uppercase();
        Ok(vm.txt(&r))
    }
    fn minusc(vm: &mut Vm, this: Valor, _: &[Valor]) -> Res {
        let t = this_texto(vm, this)?;
        let s = vm.n.texto(t);
        let r = s.to_lowercase();
        Ok(vm.txt(&r))
    }
    vm.def(p, "toUpperCase", 0, maiusc);
    vm.def(p, "toLocaleUpperCase", 0, maiusc);
    vm.def(p, "toLowerCase", 0, minusc);
    vm.def(p, "toLocaleLowerCase", 0, minusc);
    fn aparar(vm: &mut Vm, this: Valor, modo: u8) -> Res {
        let t = this_texto(vm, this)?;
        let s = vm.n.texto(t);
        let r = match modo {
            0 => s.trim_matches(e_branco),
            1 => s.trim_start_matches(e_branco),
            _ => s.trim_end_matches(e_branco),
        };
        Ok(vm.txt(r))
    }
    vm.def(p, "trim", 0, |vm, this, _| aparar(vm, this, 0));
    vm.def(p, "trimStart", 0, |vm, this, _| aparar(vm, this, 1));
    vm.def(p, "trimLeft", 0, |vm, this, _| aparar(vm, this, 1));
    vm.def(p, "trimEnd", 0, |vm, this, _| aparar(vm, this, 2));
    vm.def(p, "trimRight", 0, |vm, this, _| aparar(vm, this, 2));
    vm.def(p, "padStart", 2, |vm, this, a| preencher(vm, this, a, true));
    vm.def(p, "padEnd", 2, |vm, this, a| preencher(vm, this, a, false));
    vm.def(p, "repeat", 1, |vm, this, a| {
        let t = this_texto(vm, this)?;
        let n = vm.para_inteiro(arg(a, 0))?;
        if n < 0.0 || n.is_infinite() {
            return Err(vm.erro_intervalo(&format!("Invalid count value: {}", numero_para_texto(n))));
        }
        let s = vm.n.texto(t);
        if s.len() as f64 * n > 2e8 {
            return Err(vm.erro_intervalo("Invalid string length"));
        }
        let r = s.repeat(n as usize);
        Ok(vm.txt(&r))
    });
    vm.def(p, "concat", 1, |vm, this, a| {
        let mut t = this_texto(vm, this)?;
        for x in a {
            let y = vm.para_texto_id(*x)?;
            t = vm.n.concatenar(t, y);
        }
        Ok(Valor::Texto(t))
    });
    vm.def(p, "split", 2, dividir);
    vm.def(p, "replace", 2, |vm, this, a| trocar(vm, this, a, false));
    vm.def(p, "replaceAll", 2, |vm, this, a| trocar(vm, this, a, true));
    vm.def(p, "match", 1, |vm, this, a| {
        let t = this_texto(vm, this)?;
        let rv = match regex_de(vm, arg(a, 0)) {
            Some(_) => arg(a, 0),
            None => {
                let fonte = if arg(a, 0) == Valor::Undef { Rc::from("(?:)") } else { vm.para_texto(a[0])? };
                nova_regex(vm, &fonte, "")?
            }
        };
        let id = rv.obj().unwrap();
        let r = regex_de(vm, rv).unwrap();
        if !r.global {
            return regex_exec(vm, id, t);
        }
        let s = vm.n.texto(t);
        let e = Entrada::de(&s);
        let mut saida = Vec::new();
        let mut pos = 0;
        while pos <= e.len() {
            let Some(g) = r.casar(&e, pos) else { break };
            let x = e.pedaco(g[0] as usize, g[1] as usize);
            saida.push(vm.txt(&x));
            pos = if g[1] == g[0] { g[1] as usize + 1 } else { g[1] as usize };
        }
        escrever_last_index(vm, id, 0);
        if saida.is_empty() {
            return Ok(Valor::Nulo);
        }
        Ok(vm.novo_array(saida))
    });
    vm.def(p, "matchAll", 1, |vm, this, a| {
        let t = this_texto(vm, this)?;
        let (fonte, flags): (Rc<str>, Rc<str>) = match regex_de(vm, arg(a, 0)) {
            Some(r) => {
                if !r.global {
                    return Err(vm.erro_tipo("String.prototype.matchAll called with a non-global RegExp argument"));
                }
                (r.fonte.clone(), r.flags.clone())
            }
            None => (vm.para_texto(arg(a, 0))?, Rc::from("g")),
        };
        let copia = nova_regex(vm, &fonte, &flags)?;
        if let (Valor::Obj(novo), Some(Valor::Obj(velho))) = (copia, Some(arg(a, 0))) {
            if regex_de(vm, arg(a, 0)).is_some() {
                let i = ler_last_index(vm, velho)?;
                escrever_last_index(vm, novo, i);
            }
        }
        let pi = vm.p.iter_regex;
        Ok(Valor::Obj(vm.novo_obj_com(pi, Tipo::IterRegex { regex: copia, texto: t, global: true, fim: false })))
    });
    vm.def(p, "search", 1, |vm, this, a| {
        let t = this_texto(vm, this)?;
        let r = match regex_de(vm, arg(a, 0)) {
            Some(r) => r,
            None => {
                let fonte = vm.para_texto(arg(a, 0))?;
                match Regex::compilar(&fonte, "") {
                    Ok(r) => Rc::new(r),
                    Err(e) => return Err(vm.erro_sintaxe(&e)),
                }
            }
        };
        let s = vm.n.texto(t);
        let e = Entrada::de(&s);
        Ok(r.casar(&e, 0).map_or(Valor::Int(-1), |g| Valor::Int(g[0])))
    });
    vm.def(p, "normalize", 0, |vm, this, _| Ok(Valor::Texto(this_texto(vm, this)?)));
    vm.def(p, "isWellFormed", 0, |_, _, _| Ok(Valor::Bool(true)));
    vm.def(p, "toWellFormed", 0, |vm, this, _| Ok(Valor::Texto(this_texto(vm, this)?)));
    vm.def(p, "localeCompare", 1, |vm, this, a| {
        let t = this_texto(vm, this)?;
        let (x, y) = (vm.n.texto(t), vm.para_texto(arg(a, 0))?);
        // Uma aproximação da ordem local: sem diferenciar maiúsculas primeiro.
        let (lx, ly) = (x.to_lowercase(), y.to_lowercase());
        let o = lx.cmp(&ly).then_with(|| y.cmp(&x));
        Ok(Valor::Int(o as i32))
    });
    let k = vm.s.iterator;
    let f = vm.nova_nativa("[Symbol.iterator]", 0, |vm, this, _| {
        let t = this_texto(vm, this)?;
        let pi = vm.p.iter_texto;
        Ok(Valor::Obj(vm.novo_obj_com(pi, Tipo::IterTexto { alvo: t, i: 0 })))
    });
    vm.def_valor(p, k, Valor::Obj(f), P_OCULTA);
    let pi = vm.p.iter_texto;
    def_marca(vm, pi, "String Iterator");
    vm.def(pi, "next", 0, |vm, this, _| {
        let Valor::Obj(id) = this else { return Err(vm.erro_tipo("incompatible receiver")) };
        let Tipo::IterTexto { alvo, i } = vm.objs[id as usize].tipo else { return Err(vm.erro_tipo("incompatible receiver")) };
        let s = vm.n.texto(alvo);
        // `i` é em bytes aqui: o iterador anda por letras inteiras.
        let Some(c) = s.get(i as usize..).and_then(|r| r.chars().next()) else {
            return Ok(resultado_iter(vm, Valor::Undef, true));
        };
        if let Tipo::IterTexto { i: ii, .. } = &mut vm.objs[id as usize].tipo {
            *ii += c.len_utf8() as u32;
        }
        let mut buf = [0u8; 4];
        let a = vm.n.atomo(c.encode_utf8(&mut buf));
        Ok(resultado_iter(vm, Valor::Texto(a), false))
    });
}

// -----------------------------------------------------------------------------
// RegExp
// -----------------------------------------------------------------------------

/// A fonte como aparece entre barras: `/` escapada e nunca vazia.
fn fonte_escapada(f: &str) -> String {
    if f.is_empty() {
        return String::from("(?:)");
    }
    let (mut r, mut classe, mut escape) = (String::with_capacity(f.len()), false, false);
    for c in f.chars() {
        if escape {
            escape = false;
        } else if c == '\\' {
            escape = true;
        } else if c == '[' {
            classe = true;
        } else if c == ']' {
            classe = false;
        } else if c == '/' && !classe {
            r.push('\\');
        } else if c == '\n' {
            r.push_str("\\n");
            continue;
        }
        r.push(c);
    }
    r
}

fn regexp(vm: &mut Vm) {
    let p = vm.p.regex;
    construtor(vm, "RegExp", 2, |vm, _, a| {
        let (fonte, flags): (Rc<str>, Rc<str>) = match regex_de(vm, arg(a, 0)) {
            Some(r) => {
                let f = if arg(a, 1) == Valor::Undef { r.flags.clone() } else { vm.para_texto(a[1])? };
                (r.fonte.clone(), f)
            }
            None => {
                let fo = if arg(a, 0) == Valor::Undef { Rc::from("(?:)") } else { vm.para_texto(a[0])? };
                let fl = if arg(a, 1) == Valor::Undef { Rc::from("") } else { vm.para_texto(a[1])? };
                (fo, fl)
            }
        };
        nova_regex(vm, &fonte, &flags)
    }, p);
    vm.def(p, "exec", 1, |vm, this, a| {
        let Valor::Obj(id) = this else { return Err(vm.erro_tipo("not a RegExp")) };
        let t = vm.para_texto_id(arg(a, 0))?;
        regex_exec(vm, id, t)
    });
    vm.def(p, "test", 1, |vm, this, a| {
        let Valor::Obj(id) = this else { return Err(vm.erro_tipo("not a RegExp")) };
        let t = vm.para_texto_id(arg(a, 0))?;
        let r = regex_exec(vm, id, t)?;
        Ok(Valor::Bool(r != Valor::Nulo))
    });
    vm.def(p, "toString", 0, |vm, this, _| {
        let Some(r) = regex_de(vm, this) else { return Ok(vm.txt("/(?:)/")) };
        let s = format!("/{}/{}", fonte_escapada(&r.fonte), r.flags);
        Ok(vm.txt(&s))
    });
    vm.def(p, "compile", 2, |_, this, _| Ok(this));
    vm.def_acessor(p, "source", Some(|vm, this, _| {
        let Some(r) = regex_de(vm, this) else { return Ok(vm.txt("(?:)")) };
        let s = fonte_escapada(&r.fonte);
        Ok(vm.txt(&s))
    }), None);
    vm.def_acessor(p, "flags", Some(|vm, this, _| {
        let Some(r) = regex_de(vm, this) else { return Ok(Valor::Texto(K_VAZIO)) };
        let f = r.flags.clone();
        Ok(vm.txt(&f))
    }), None);
    macro_rules! flag {
        ($nome:literal, $campo:ident) => {
            vm.def_acessor(p, $nome, Some(|vm, this, _| Ok(regex_de(vm, this).map_or(Valor::Undef, |r| Valor::Bool(r.$campo)))), None);
        };
    }
    flag!("global", global);
    flag!("ignoreCase", ignorar);
    flag!("multiline", multilinha);
    flag!("dotAll", ponto_tudo);
    flag!("unicode", unicode);
    flag!("sticky", pegajosa);
    flag!("hasIndices", indices);

    let pi = vm.p.iter_regex;
    def_marca(vm, pi, "RegExp String Iterator");
    vm.def(pi, "next", 0, |vm, this, _| {
        let Valor::Obj(id) = this else { return Err(vm.erro_tipo("incompatible receiver")) };
        let Tipo::IterRegex { regex, texto, fim, .. } = vm.objs[id as usize].tipo else { return Err(vm.erro_tipo("incompatible receiver")) };
        if fim {
            return Ok(resultado_iter(vm, Valor::Undef, true));
        }
        let rid = regex.obj().unwrap();
        let m = regex_exec(vm, rid, texto)?;
        if m == Valor::Nulo {
            if let Tipo::IterRegex { fim, .. } = &mut vm.objs[id as usize].tipo {
                *fim = true;
            }
            return Ok(resultado_iter(vm, Valor::Undef, true));
        }
        let casado = vm.obter_elem(m, Valor::Int(0))?;
        if let Valor::Texto(c) = casado {
            if vm.n.tamanho_bytes(c) == 0 {
                let i = ler_last_index(vm, rid)?;
                escrever_last_index(vm, rid, i + 1);
            }
        }
        Ok(resultado_iter(vm, m, false))
    });
}

// -----------------------------------------------------------------------------
// Number e Math
// -----------------------------------------------------------------------------

fn this_num(vm: &mut Vm, this: Valor) -> Result<f64, Valor> {
    match this {
        Valor::Int(i) => Ok(i as f64),
        Valor::Num(n) => Ok(n),
        Valor::Obj(id) => match vm.objs[id as usize].tipo {
            Tipo::Numero(n) => Ok(n),
            _ => Err(vm.erro_tipo("Number.prototype.valueOf requires that 'this' be a Number")),
        },
        _ => Err(vm.erro_tipo("Number.prototype.valueOf requires that 'this' be a Number")),
    }
}

fn digito(d: u32) -> char {
    char::from_digit(d, 36).unwrap_or('0')
}

fn numero_em_base(n: f64, base: u32) -> String {
    if n.is_nan() {
        return String::from("NaN");
    }
    if n.is_infinite() {
        return String::from(if n > 0.0 { "Infinity" } else { "-Infinity" });
    }
    let neg = n < 0.0;
    let x = mat::abs(n);
    let mut inteiro = mat::floor(x);
    let mut frac = x - inteiro;
    let mut s = Vec::new();
    if inteiro == 0.0 {
        s.push('0');
    }
    let b = base as f64;
    while inteiro > 0.0 {
        let q = mat::floor(inteiro / b);
        let d = (inteiro - q * b) as u32;
        s.push(digito(d));
        inteiro = q;
    }
    if neg {
        s.push('-');
    }
    s.reverse();
    let mut r: String = s.into_iter().collect();
    if frac > 0.0 {
        r.push('.');
        // Até a precisão do número: o último bit significativo.
        let mut delta = (x * f64::EPSILON).max(f64::MIN_POSITIVE);
        loop {
            frac *= b;
            delta *= b;
            let d = mat::floor(frac);
            r.push(digito(d as u32));
            frac -= d;
            if frac > 0.5 || (frac == 0.5 && (d as u32 & 1) == 1) {
                if frac + delta > 1.0 {
                    // Arredonda para cima, propagando.
                    let mut c: Vec<char> = r.chars().collect();
                    let mut i = c.len();
                    loop {
                        i -= 1;
                        if c[i] == '.' {
                            continue;
                        }
                        let v = c[i].to_digit(36).unwrap_or(0) + 1;
                        if v == base {
                            c[i] = '0';
                            if i == 0 {
                                break;
                            }
                        } else {
                            c[i] = digito(v);
                            break;
                        }
                    }
                    r = c.into_iter().collect();
                    break;
                }
            }
            if frac < delta {
                break;
            }
        }
    }
    r
}

/// `toFixed`: o Rust arredonda empates para o par; o JavaScript, para cima.
fn com_casas(x: f64, casas: usize) -> String {
    let t = format!("{:.*}", casas, x);
    // O Rust escreve o valor binário exato: 1.005 é 1.00499999...
    let longo = format!("{:.*}", casas + 60, x);
    let corte = longo.len() - 60;
    let (mais, resto) = longo.split_at(corte);
    if resto.starts_with('5') && resto[1..].bytes().all(|d| d == b'0') {
        // Um empate exato: soma uma unidade na última casa.
        let mut c: Vec<u8> = mais.as_bytes().to_vec();
        if c.last() == Some(&b'.') {
            c.pop();
        }
        let mut i = c.len();
        loop {
            if i == 0 {
                c.insert(0, b'1');
                break;
            }
            i -= 1;
            if c[i] == b'.' {
                continue;
            }
            if c[i] == b'9' {
                c[i] = b'0';
            } else {
                c[i] += 1;
                break;
            }
        }
        return String::from_utf8(c).unwrap_or(t);
    }
    t
}

fn numero(vm: &mut Vm) {
    let p = vm.p.numero;
    let c = construtor(vm, "Number", 1, |vm, _, a| {
        let n = if a.is_empty() { 0.0 } else { vm.para_numero(a[0])? };
        if vm.novo_alvo != Valor::Undef {
            let nt = vm.novo_alvo;
            let proto = vm.proto_de_novo_alvo(nt, vm.p.numero)?;
            return Ok(Valor::Obj(vm.novo_obj_com(proto, Tipo::Numero(n))));
        }
        Ok(num(n))
    }, p);
    for (nome, v) in [
        ("MAX_VALUE", f64::MAX),
        ("MIN_VALUE", 5e-324),
        ("EPSILON", f64::EPSILON),
        ("MAX_SAFE_INTEGER", 9007199254740991.0),
        ("MIN_SAFE_INTEGER", -9007199254740991.0),
        ("POSITIVE_INFINITY", f64::INFINITY),
        ("NEGATIVE_INFINITY", f64::NEG_INFINITY),
        ("NaN", f64::NAN),
    ] {
        let k = vm.n.atomo(nome);
        vm.def_valor(c, k, num(v), 0);
    }
    fn e_num(v: Valor) -> Option<f64> {
        match v {
            Valor::Int(i) => Some(i as f64),
            Valor::Num(n) => Some(n),
            _ => None,
        }
    }
    vm.def(c, "isInteger", 1, |_, _, a| Ok(Valor::Bool(e_num(arg(a, 0)).is_some_and(|n| n.is_finite() && mat::trunc(n) == n))));
    vm.def(c, "isSafeInteger", 1, |_, _, a| Ok(Valor::Bool(e_num(arg(a, 0)).is_some_and(|n| n.is_finite() && mat::trunc(n) == n && mat::abs(n) <= 9007199254740991.0))));
    vm.def(c, "isFinite", 1, |_, _, a| Ok(Valor::Bool(e_num(arg(a, 0)).is_some_and(|n| n.is_finite()))));
    vm.def(c, "isNaN", 1, |_, _, a| Ok(Valor::Bool(e_num(arg(a, 0)).is_some_and(|n| n.is_nan()))));
    let g = vm.global;
    let (kpi, kpf) = (vm.n.atomo("parseInt"), vm.n.atomo("parseFloat"));
    let (fi, ff) = (vm.obter_obj(g, kpi, Valor::Obj(g)), vm.obter_obj(g, kpf, Valor::Obj(g)));
    vm.def_valor(c, kpi, fi.unwrap_or(Valor::Undef), P_OCULTA);
    vm.def_valor(c, kpf, ff.unwrap_or(Valor::Undef), P_OCULTA);

    vm.def(p, "valueOf", 0, |vm, this, _| Ok(num(this_num(vm, this)?)));
    vm.def(p, "toString", 1, |vm, this, a| {
        let n = this_num(vm, this)?;
        let b = if arg(a, 0) == Valor::Undef { 10.0 } else { vm.para_inteiro(a[0])? };
        if !(2.0..=36.0).contains(&b) {
            return Err(vm.erro_intervalo("toString() radix must be between 2 and 36"));
        }
        let t = if b == 10.0 { numero_para_texto(n) } else { numero_em_base(n, b as u32) };
        Ok(vm.txt(&t))
    });
    vm.def(p, "toLocaleString", 0, |vm, this, _| {
        let n = this_num(vm, this)?;
        // Separadores do português do Brasil: 1.234,5
        let t = numero_para_texto(mat::abs(n));
        let (i, f) = t.split_once('.').unwrap_or((&t, ""));
        let mut r = String::new();
        for (j, c) in i.chars().enumerate() {
            if j > 0 && (i.len() - j) % 3 == 0 {
                r.push('.');
            }
            r.push(c);
        }
        if !f.is_empty() {
            r.push(',');
            r.push_str(&f[..f.len().min(3)]);
        }
        let r = if n < 0.0 { format!("-{}", r) } else { r };
        Ok(vm.txt(&r))
    });
    vm.def(p, "toFixed", 1, |vm, this, a| {
        let n = this_num(vm, this)?;
        let d = vm.para_inteiro(arg(a, 0))?;
        if !(0.0..=100.0).contains(&d) {
            return Err(vm.erro_intervalo("toFixed() digits argument must be between 0 and 100"));
        }
        if !n.is_finite() || mat::abs(n) >= 1e21 {
            let t = numero_para_texto(n);
            return Ok(vm.txt(&t));
        }
        let mut t = com_casas(mat::abs(n), d as usize);
        if n < 0.0 && t.bytes().any(|c| c.is_ascii_digit() && c != b'0') || (n < 0.0 && n != 0.0) {
            t = format!("-{}", t);
        }
        Ok(vm.txt(&t))
    });
    vm.def(p, "toPrecision", 1, |vm, this, a| {
        let n = this_num(vm, this)?;
        if arg(a, 0) == Valor::Undef || !n.is_finite() {
            let t = numero_para_texto(n);
            return Ok(vm.txt(&t));
        }
        let pr = vm.para_inteiro(a[0])?;
        if !(1.0..=100.0).contains(&pr) {
            return Err(vm.erro_intervalo("toPrecision() argument must be between 1 and 100"));
        }
        let pr = pr as i32;
        let x = mat::abs(n);
        let t = format!("{:.*e}", (pr - 1) as usize, x);
        let (m, e) = t.split_once('e').unwrap();
        let e: i32 = e.parse().unwrap_or(0);
        let s = if e < -6 || e >= pr {
            format!("{}e{}{}", m, if e < 0 { '-' } else { '+' }, e.abs())
        } else {
            com_casas(x, (pr - 1 - e).max(0) as usize)
        };
        let s = if n < 0.0 { format!("-{}", s) } else { s };
        Ok(vm.txt(&s))
    });
    vm.def(p, "toExponential", 1, |vm, this, a| {
        let n = this_num(vm, this)?;
        if !n.is_finite() {
            let t = numero_para_texto(n);
            return Ok(vm.txt(&t));
        }
        let t = if arg(a, 0) == Valor::Undef {
            format!("{:e}", n)
        } else {
            let d = vm.para_inteiro(a[0])?;
            if !(0.0..=100.0).contains(&d) {
                return Err(vm.erro_intervalo("toExponential() argument must be between 0 and 100"));
            }
            format!("{:.*e}", d as usize, n)
        };
        let (m, e) = t.split_once('e').unwrap();
        let e: i32 = e.parse().unwrap_or(0);
        let s = format!("{}e{}{}", m, if e < 0 { '-' } else { '+' }, e.abs());
        Ok(vm.txt(&s))
    });
}

fn math(vm: &mut Vm) {
    let m = vm.novo_objeto();
    let g = vm.global;
    vm.def_oculta(g, "Math", Valor::Obj(m));
    def_marca(vm, m, "Math");
    for (nome, v) in [
        ("PI", mat::PI),
        ("E", mat::E),
        ("LN2", 0.6931471805599453),
        ("LN10", 2.302585092994046),
        ("LOG2E", 1.4426950408889634),
        ("LOG10E", 0.4342944819032518),
        ("SQRT2", 1.4142135623730951),
        ("SQRT1_2", 0.7071067811865476),
    ] {
        let k = vm.n.atomo(nome);
        vm.def_valor(m, k, Valor::Num(v), 0);
    }
    macro_rules! f1 {
        ($nome:literal, $f:expr) => {
            vm.def(m, $nome, 1, |vm, _, a| {
                let x = vm.para_numero(arg(a, 0))?;
                let f: fn(f64) -> f64 = $f;
                Ok(num(f(x)))
            });
        };
    }
    f1!("abs", mat::abs);
    f1!("floor", mat::floor);
    f1!("ceil", mat::ceil);
    f1!("trunc", mat::trunc);
    f1!("round", mat::round);
    f1!("sqrt", mat::sqrt);
    f1!("cbrt", mat::cbrt);
    f1!("exp", mat::exp);
    f1!("log", mat::ln);
    f1!("log10", mat::log10);
    f1!("log2", mat::log2);
    f1!("sin", mat::sin);
    f1!("cos", mat::cos);
    f1!("tan", mat::tan);
    f1!("asin", mat::asin);
    f1!("acos", mat::acos);
    f1!("atan", mat::atan);
    f1!("sinh", mat::sinh);
    f1!("cosh", mat::cosh);
    f1!("tanh", mat::tanh);
    f1!("expm1", |x| if mat::abs(x) < 1e-5 { x + x * x / 2.0 } else { mat::exp(x) - 1.0 });
    f1!("log1p", |x| if mat::abs(x) < 1e-5 { x - x * x / 2.0 } else { mat::ln(1.0 + x) });
    f1!("asinh", |x| if x == 0.0 || x.is_infinite() { x } else { mat::ln(x + mat::sqrt(x * x + 1.0)) });
    f1!("acosh", |x| mat::ln(x + mat::sqrt(x * x - 1.0)));
    f1!("atanh", |x| if x == 0.0 { x } else { 0.5 * mat::ln((1.0 + x) / (1.0 - x)) });
    f1!("sign", |x| if x.is_nan() || x == 0.0 { x } else if x > 0.0 { 1.0 } else { -1.0 });
    f1!("fround", |x| (x as f32) as f64);
    f1!("clz32", |x| (para_i32(x) as u32).leading_zeros() as f64);
    vm.def(m, "atan2", 2, |vm, _, a| {
        let (y, x) = (vm.para_numero(arg(a, 0))?, vm.para_numero(arg(a, 1))?);
        Ok(num(mat::atan2(y, x)))
    });
    vm.def(m, "pow", 2, |vm, _, a| {
        let (x, y) = (vm.para_numero(arg(a, 0))?, vm.para_numero(arg(a, 1))?);
        Ok(num(mat::pow(x, y)))
    });
    vm.def(m, "imul", 2, |vm, _, a| {
        let (x, y) = (vm.para_i32(arg(a, 0))?, vm.para_i32(arg(a, 1))?);
        Ok(Valor::Int(x.wrapping_mul(y)))
    });
    vm.def(m, "hypot", 2, |vm, _, a| {
        let mut s = 0.0;
        for x in a {
            let v = vm.para_numero(*x)?;
            if v.is_infinite() {
                return Ok(Valor::Num(f64::INFINITY));
            }
            s += v * v;
        }
        Ok(num(mat::sqrt(s)))
    });
    fn limite(vm: &mut Vm, a: &[Valor], maximo: bool) -> Res {
        let mut r = if maximo { f64::NEG_INFINITY } else { f64::INFINITY };
        let mut nan = false;
        for x in a {
            let v = vm.para_numero(*x)?;
            if v.is_nan() {
                nan = true;
            }
            let troca = if maximo { v > r || (v == 0.0 && r == 0.0 && !v.is_sign_negative()) } else { v < r || (v == 0.0 && r == 0.0 && v.is_sign_negative()) };
            if troca {
                r = v;
            }
        }
        Ok(if nan { Valor::Num(f64::NAN) } else { num(r) })
    }
    vm.def(m, "max", 2, |vm, _, a| limite(vm, a, true));
    vm.def(m, "min", 2, |vm, _, a| limite(vm, a, false));
    vm.def(m, "random", 0, |vm, _, _| {
        // xorshift64*: o relógio entra na semente.
        let mut x = vm.semente ^ (vm.ganchos.agora_ms)();
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        vm.semente = x;
        let r = x.wrapping_mul(0x2545F4914F6CDD1D) >> 11;
        Ok(Valor::Num(r as f64 / 9007199254740992.0))
    });
}

// -----------------------------------------------------------------------------
// JSON
// -----------------------------------------------------------------------------

struct LeitorJson<'a> {
    b: &'a [u8],
    s: &'a str,
    i: usize,
    prof: u32,
}

impl LeitorJson<'_> {
    fn brancos(&mut self) {
        while self.i < self.b.len() && matches!(self.b[self.i], b' ' | b'\n' | b'\r' | b'\t') {
            self.i += 1;
        }
    }

    fn erro(&self, vm: &mut Vm) -> Valor {
        if self.i >= self.b.len() {
            return vm.erro_sintaxe("Unexpected end of JSON input");
        }
        let c = self.s[self.i..].chars().next().unwrap_or(' ');
        vm.erro_sintaxe(&format!("Unexpected token '{}', \"{}\" is not valid JSON", c, &self.s[..self.s.len().min(30)]))
    }

    fn valor(&mut self, vm: &mut Vm) -> Res {
        self.brancos();
        self.prof += 1;
        if self.prof > 512 {
            return Err(vm.erro_intervalo("JSON nested too deeply"));
        }
        let r = self.valor_dentro(vm);
        self.prof -= 1;
        r
    }

    fn valor_dentro(&mut self, vm: &mut Vm) -> Res {
        match self.b.get(self.i) {
            Some(b'{') => {
                self.i += 1;
                let o = vm.novo_objeto();
                self.brancos();
                if self.b.get(self.i) == Some(&b'}') {
                    self.i += 1;
                    return Ok(Valor::Obj(o));
                }
                loop {
                    self.brancos();
                    if self.b.get(self.i) != Some(&b'"') {
                        return Err(self.erro(vm));
                    }
                    let k = self.texto(vm)?;
                    let k = vm.n.atomo(&k);
                    self.brancos();
                    if self.b.get(self.i) != Some(&b':') {
                        return Err(self.erro(vm));
                    }
                    self.i += 1;
                    let v = self.valor(vm)?;
                    vm.definir_propria(o, k, v, P_TUDO);
                    self.brancos();
                    match self.b.get(self.i) {
                        Some(b',') => self.i += 1,
                        Some(b'}') => {
                            self.i += 1;
                            return Ok(Valor::Obj(o));
                        }
                        _ => return Err(self.erro(vm)),
                    }
                }
            }
            Some(b'[') => {
                self.i += 1;
                let mut v = Vec::new();
                self.brancos();
                if self.b.get(self.i) == Some(&b']') {
                    self.i += 1;
                    return Ok(vm.novo_array(v));
                }
                loop {
                    v.push(self.valor(vm)?);
                    self.brancos();
                    match self.b.get(self.i) {
                        Some(b',') => self.i += 1,
                        Some(b']') => {
                            self.i += 1;
                            return Ok(vm.novo_array(v));
                        }
                        _ => return Err(self.erro(vm)),
                    }
                }
            }
            Some(b'"') => {
                let t = self.texto(vm)?;
                Ok(vm.txt(&t))
            }
            Some(b't') if self.s[self.i..].starts_with("true") => {
                self.i += 4;
                Ok(Valor::Bool(true))
            }
            Some(b'f') if self.s[self.i..].starts_with("false") => {
                self.i += 5;
                Ok(Valor::Bool(false))
            }
            Some(b'n') if self.s[self.i..].starts_with("null") => {
                self.i += 4;
                Ok(Valor::Nulo)
            }
            Some(c) if c.is_ascii_digit() || *c == b'-' => {
                let ini = self.i;
                self.i += 1;
                while self.i < self.b.len() && (self.b[self.i].is_ascii_digit() || matches!(self.b[self.i], b'.' | b'e' | b'E' | b'+' | b'-')) {
                    self.i += 1;
                }
                let t = &self.s[ini..self.i];
                // Zeros à esquerda não valem em JSON.
                let d = t.strip_prefix('-').unwrap_or(t).as_bytes();
                if d.is_empty() || (d[0] == b'0' && d.get(1).is_some_and(|x| x.is_ascii_digit())) {
                    self.i = ini;
                    return Err(self.erro(vm));
                }
                match t.parse::<f64>() {
                    Ok(n) => Ok(num(n)),
                    Err(_) => {
                        self.i = ini;
                        Err(self.erro(vm))
                    }
                }
            }
            _ => Err(self.erro(vm)),
        }
    }

    fn texto(&mut self, vm: &mut Vm) -> Result<String, Valor> {
        self.i += 1;
        let mut r = String::new();
        loop {
            let ini = self.i;
            while self.i < self.b.len() && !matches!(self.b[self.i], b'"' | b'\\') && self.b[self.i] >= 0x20 {
                self.i += 1;
            }
            r.push_str(&self.s[ini..self.i]);
            match self.b.get(self.i) {
                Some(b'"') => {
                    self.i += 1;
                    return Ok(r);
                }
                Some(b'\\') => {
                    self.i += 1;
                    let e = self.b.get(self.i).copied();
                    self.i += 1;
                    match e {
                        Some(b'n') => r.push('\n'),
                        Some(b't') => r.push('\t'),
                        Some(b'r') => r.push('\r'),
                        Some(b'b') => r.push('\u{8}'),
                        Some(b'f') => r.push('\u{C}'),
                        Some(b'/') => r.push('/'),
                        Some(b'\\') => r.push('\\'),
                        Some(b'"') => r.push('"'),
                        Some(b'u') => {
                            let le = |s: &Self, p: usize| -> Option<u32> {
                                let h = s.s.get(p..p + 4)?;
                                u32::from_str_radix(h, 16).ok()
                            };
                            let Some(v) = le(self, self.i) else { return Err(self.erro(vm)) };
                            self.i += 4;
                            if (0xD800..0xDC00).contains(&v) && self.s[self.i..].starts_with("\\u") {
                                if let Some(b) = le(self, self.i + 2) {
                                    if (0xDC00..0xE000).contains(&b) {
                                        self.i += 6;
                                        r.push(char::from_u32(0x10000 + ((v - 0xD800) << 10) + (b - 0xDC00)).unwrap_or('\u{FFFD}'));
                                        continue;
                                    }
                                }
                            }
                            r.push(char::from_u32(v).unwrap_or('\u{FFFD}'));
                        }
                        _ => {
                            self.i -= 1;
                            return Err(self.erro(vm));
                        }
                    }
                }
                _ => return Err(self.erro(vm)),
            }
        }
    }
}

fn aspas_json(s: &str, r: &mut String) {
    r.push('"');
    for c in s.chars() {
        match c {
            '"' => r.push_str("\\\""),
            '\\' => r.push_str("\\\\"),
            '\n' => r.push_str("\\n"),
            '\r' => r.push_str("\\r"),
            '\t' => r.push_str("\\t"),
            '\u{8}' => r.push_str("\\b"),
            '\u{C}' => r.push_str("\\f"),
            c if (c as u32) < 0x20 => r.push_str(&format!("\\u{:04x}", c as u32)),
            c => r.push(c),
        }
    }
    r.push('"');
}

struct Escritor {
    r: String,
    substituto: Valor,
    lista: Option<Vec<Txt>>,
    espaco: String,
    pilha: Vec<ObjId>,
}

impl Escritor {
    /// Devolve false se o valor não aparece (undefined, funções).
    fn valor(&mut self, vm: &mut Vm, dono: Valor, chave: Valor, mut v: Valor, recuo: &str) -> Result<bool, Valor> {
        if let Valor::Obj(_) = v {
            let k = vm.a.to_json;
            let f = vm.obter(v, k)?;
            if vm.e_funcao(f) {
                v = vm.chamar(f, v, &[chave])?;
            }
        }
        if self.substituto != Valor::Undef {
            v = vm.chamar(self.substituto, dono, &[chave, v])?;
        }
        if let Valor::Obj(id) = v {
            match vm.objs[id as usize].tipo {
                Tipo::Numero(n) => v = num(n),
                Tipo::Booleano(b) => v = Valor::Bool(b),
                Tipo::Texto(t) => v = Valor::Texto(t),
                _ => {}
            }
        }
        match v {
            Valor::Nulo => self.r.push_str("null"),
            Valor::Bool(b) => self.r.push_str(if b { "true" } else { "false" }),
            Valor::Int(i) => {
                let mut buf = [0u8; 12];
                self.r.push_str(escrever_int(i, &mut buf));
            }
            Valor::Num(n) => {
                if n.is_finite() {
                    self.r.push_str(&numero_para_texto(n));
                } else {
                    self.r.push_str("null");
                }
            }
            Valor::Texto(t) => {
                let s = vm.n.texto(t);
                aspas_json(&s, &mut self.r);
            }
            Valor::Obj(id) => {
                if vm.e_funcao(v) {
                    return Ok(false);
                }
                if self.pilha.contains(&id) {
                    return Err(vm.erro_tipo("Converting circular structure to JSON"));
                }
                if self.pilha.len() > 400 {
                    return Err(vm.erro_intervalo("JSON nested too deeply"));
                }
                self.pilha.push(id);
                let dentro = format!("{}{}", recuo, self.espaco);
                if vm.e_array(v) {
                    let n = vm.obter(v, K_LENGTH)?;
                    let n = vm.para_inteiro(n)? as usize;
                    self.r.push('[');
                    for i in 0..n {
                        if i > 0 {
                            self.r.push(',');
                        }
                        if !self.espaco.is_empty() {
                            self.r.push('\n');
                            self.r.push_str(&dentro);
                        }
                        let x = vm.obter_elem(v, num(i as f64))?;
                        let ci = vm.para_texto_id(num(i as f64))?;
                        if !self.valor(vm, v, Valor::Texto(ci), x, &dentro)? {
                            self.r.push_str("null");
                        }
                    }
                    if n > 0 && !self.espaco.is_empty() {
                        self.r.push('\n');
                        self.r.push_str(recuo);
                    }
                    self.r.push(']');
                } else {
                    let chaves = match &self.lista {
                        Some(l) => l.clone(),
                        None => vm.chaves_proprias(id, true, true, false)?,
                    };
                    self.r.push('{');
                    let mut algum = false;
                    for k in chaves {
                        let x = vm.obter_obj(id, k, v)?;
                        let marca = self.r.len();
                        if algum {
                            self.r.push(',');
                        }
                        if !self.espaco.is_empty() {
                            self.r.push('\n');
                            self.r.push_str(&dentro);
                        }
                        let t = vm.n.texto(k);
                        aspas_json(&t, &mut self.r);
                        self.r.push(':');
                        if !self.espaco.is_empty() {
                            self.r.push(' ');
                        }
                        if self.valor(vm, v, Valor::Texto(k), x, &dentro)? {
                            algum = true;
                        } else {
                            self.r.truncate(marca);
                        }
                    }
                    if algum && !self.espaco.is_empty() {
                        self.r.push('\n');
                        self.r.push_str(recuo);
                    }
                    self.r.push('}');
                }
                self.pilha.pop();
            }
            _ => return Ok(false),
        }
        Ok(true)
    }
}

fn reviver(vm: &mut Vm, dono: Valor, chave: Txt, f: Valor) -> Res {
    let v = vm.obter(dono, chave)?;
    if let Valor::Obj(id) = v {
        let chaves = vm.chaves_proprias(id, true, true, false)?;
        for k in chaves {
            let novo = reviver(vm, v, k, f)?;
            if novo == Valor::Undef {
                vm.apagar(id, k)?;
            } else {
                vm.definir_propria(id, k, novo, P_TUDO);
            }
        }
    }
    let c = vm.valor_da_chave(chave);
    vm.chamar(f, dono, &[c, v])
}

fn json(vm: &mut Vm) {
    let j = vm.novo_objeto();
    let g = vm.global;
    vm.def_oculta(g, "JSON", Valor::Obj(j));
    def_marca(vm, j, "JSON");
    vm.def(j, "parse", 2, |vm, _, a| {
        let t = vm.para_texto(arg(a, 0))?;
        let mut l = LeitorJson { b: t.as_bytes(), s: &t, i: 0, prof: 0 };
        let v = l.valor(vm)?;
        l.brancos();
        if l.i < l.b.len() {
            return Err(l.erro(vm));
        }
        if vm.e_funcao(arg(a, 1)) {
            let raiz = vm.novo_objeto();
            let k = K_VAZIO;
            vm.def_valor(raiz, k, v, P_TUDO);
            return reviver(vm, Valor::Obj(raiz), k, a[1]);
        }
        Ok(v)
    });
    vm.def(j, "stringify", 3, |vm, _, a| {
        let mut e = Escritor { r: String::new(), substituto: Valor::Undef, lista: None, espaco: String::new(), pilha: Vec::new() };
        let sub = arg(a, 1);
        if vm.e_funcao(sub) {
            e.substituto = sub;
        } else if vm.e_array(sub) {
            let itens = vm.lista_de_array(sub)?;
            let mut l = Vec::new();
            for x in itens {
                if matches!(x, Valor::Texto(_) | Valor::Int(_) | Valor::Num(_)) {
                    let k = vm.para_chave(x)?;
                    if !l.contains(&k) {
                        l.push(k);
                    }
                }
            }
            e.lista = Some(l);
        }
        match arg(a, 2) {
            Valor::Int(n) => e.espaco = " ".repeat(n.clamp(0, 10) as usize),
            Valor::Num(n) => e.espaco = " ".repeat(n.clamp(0.0, 10.0) as usize),
            Valor::Texto(t) => e.espaco = vm.n.texto(t).chars().take(10).collect(),
            _ => {}
        }
        let dono = vm.novo_objeto();
        vm.def_valor(dono, K_VAZIO, arg(a, 0), P_TUDO);
        let ok = e.valor(vm, Valor::Obj(dono), Valor::Texto(K_VAZIO), arg(a, 0), "")?;
        if !ok {
            return Ok(Valor::Undef);
        }
        Ok(vm.txt(&e.r))
    });
}

// -----------------------------------------------------------------------------
// Date (o sistema não tem fuso horário: tudo é UTC)
// -----------------------------------------------------------------------------

const MS_DIA: f64 = 86400000.0;

fn dias_de_civil(a: i64, m: i64, d: i64) -> i64 {
    let a = if m <= 2 { a - 1 } else { a };
    let era = a.div_euclid(400);
    let ano = a - era * 400;
    let dia_ano = (153 * (m + if m > 2 { -3 } else { 9 }) + 2) / 5 + d - 1;
    let dia_era = ano * 365 + ano / 4 - ano / 100 + dia_ano;
    era * 146097 + dia_era - 719468
}

fn civil_de_dias(z: i64) -> (i64, i64, i64) {
    let z = z + 719468;
    let era = z.div_euclid(146097);
    let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = mp + if mp < 10 { 3 } else { -9 };
    (if m <= 2 { y + 1 } else { y }, m, d)
}

fn montar_data(a: f64, m: f64, d: f64, h: f64, mi: f64, s: f64, ms: f64) -> f64 {
    if !(a.is_finite() && m.is_finite() && d.is_finite() && h.is_finite() && mi.is_finite() && s.is_finite() && ms.is_finite()) {
        return f64::NAN;
    }
    if mat::abs(a) > 400000.0 || mat::abs(m) > 4800000.0 {
        return f64::NAN;
    }
    let (a, m) = (mat::trunc(a), mat::trunc(m));
    let ano = a + mat::floor(m / 12.0);
    let mes = m - mat::floor(m / 12.0) * 12.0;
    let dias = dias_de_civil(ano as i64, mes as i64 + 1, 1) as f64 + mat::trunc(d) - 1.0;
    let t = dias * MS_DIA + mat::trunc(h) * 3600000.0 + mat::trunc(mi) * 60000.0 + mat::trunc(s) * 1000.0 + mat::trunc(ms);
    if mat::abs(t) > 8.64e15 { f64::NAN } else { t }
}

/// (ano, mês 0-11, dia, hora, min, seg, ms, dia da semana)
fn partes_data(t: f64) -> (i64, i64, i64, i64, i64, i64, i64, i64) {
    let dias = mat::floor(t / MS_DIA);
    let resto = (t - dias * MS_DIA) as i64;
    let (a, m, d) = civil_de_dias(dias as i64);
    let semana = (dias as i64 + 4).rem_euclid(7);
    (a, m - 1, d, resto / 3600000, resto / 60000 % 60, resto / 1000 % 60, resto % 1000, semana)
}

const DIAS: [&str; 7] = ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"];
const MESES: [&str; 12] = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];

pub fn data_iso(t: f64) -> Option<String> {
    if t.is_nan() {
        return None;
    }
    let (a, m, d, h, mi, s, ms, _) = partes_data(t);
    let ano = if (0..=9999).contains(&a) {
        format!("{:04}", a)
    } else {
        format!("{}{:06}", if a < 0 { '-' } else { '+' }, a.abs())
    };
    Some(format!("{}-{:02}-{:02}T{:02}:{:02}:{:02}.{:03}Z", ano, m + 1, d, h, mi, s, ms))
}

/// Datas ISO ("2026-11-06T10:00:00Z") e as do tipo "Nov 6 2026 10:00 GMT".
fn ler_data(s: &str) -> f64 {
    let t = s.trim();
    if let Some(v) = ler_data_iso(t) {
        return v;
    }
    // Pedaços soltos: nome do mês, números, hora, fuso.
    let (mut ano, mut mes, mut dia, mut hora, mut min, mut seg) = (None, None, None, 0.0, 0.0, 0.0);
    let mut fuso = 0.0;
    for p in t.split(|c: char| c.is_whitespace() || c == ',') {
        if p.is_empty() {
            continue;
        }
        if p.contains(':') {
            let v: Vec<f64> = p.split(':').map(|x| x.parse().unwrap_or(f64::NAN)).collect();
            hora = v[0];
            min = v.get(1).copied().unwrap_or(0.0);
            seg = v.get(2).copied().unwrap_or(0.0);
        } else if let Some(i) = MESES.iter().position(|m| p.len() >= 3 && p[..3.min(p.len())].eq_ignore_ascii_case(m) && p.is_ascii()) {
            mes = Some(i as f64);
        } else if p.contains('/') {
            let v: Vec<f64> = p.split('/').map(|x| x.parse().unwrap_or(f64::NAN)).collect();
            if v.len() == 3 {
                if v[0] > 31.0 {
                    (ano, mes, dia) = (Some(v[0]), Some(v[1] - 1.0), Some(v[2]));
                } else {
                    (mes, dia, ano) = (Some(v[0] - 1.0), Some(v[1]), Some(v[2]));
                }
            }
        } else if p.starts_with('(') || p.ends_with(')') {
        } else if let Some(r) = p.strip_prefix("GMT").or_else(|| p.strip_prefix("UTC")).or(if p == "Z" { Some("") } else { None }) {
            if let Ok(f) = r.parse::<i32>() {
                fuso = (f / 100) as f64 * 60.0 + (f % 100) as f64;
            }
        } else if let Ok(n) = p.parse::<f64>() {
            if p.starts_with(['+', '-']) && p.len() == 5 {
                let f = n as i32;
                fuso = (f / 100) as f64 * 60.0 + (f % 100) as f64;
            } else if dia.is_none() && n <= 31.0 {
                dia = Some(n);
            } else {
                ano = Some(n);
            }
        } else if p.len() >= 3 && DIAS.iter().any(|d| p.is_ascii() && p[..3].eq_ignore_ascii_case(d)) {
        } else {
            return f64::NAN;
        }
    }
    match (ano, mes) {
        (Some(a), Some(m)) => montar_data(a, m, dia.unwrap_or(1.0), hora, min, seg, 0.0) - fuso * 60000.0,
        _ => f64::NAN,
    }
}

fn ler_data_iso(t: &str) -> Option<f64> {
    let b = t.as_bytes();
    let num_de = |a: usize, c: usize| -> Option<f64> {
        let p = t.get(a..c)?;
        if p.bytes().all(|x| x.is_ascii_digit()) && !p.is_empty() { p.parse().ok() } else { None }
    };
    let (ano, mut i) = if matches!(b.first(), Some(b'+' | b'-')) {
        let a = num_de(1, 7)?;
        (if b[0] == b'-' { -a } else { a }, 7)
    } else {
        (num_de(0, 4)?, 4)
    };
    let (mut mes, mut dia) = (1.0, 1.0);
    if b.get(i) == Some(&b'-') {
        mes = num_de(i + 1, i + 3)?;
        i += 3;
        if b.get(i) == Some(&b'-') {
            dia = num_de(i + 1, i + 3)?;
            i += 3;
        }
    }
    let (mut h, mut mi, mut s, mut ms, mut fuso) = (0.0, 0.0, 0.0, 0.0, 0.0);
    if matches!(b.get(i), Some(b'T' | b' ')) {
        h = num_de(i + 1, i + 3)?;
        if b.get(i + 3) != Some(&b':') {
            return None;
        }
        mi = num_de(i + 4, i + 6)?;
        i += 6;
        if b.get(i) == Some(&b':') {
            s = num_de(i + 1, i + 3)?;
            i += 3;
            if b.get(i) == Some(&b'.') {
                let ini = i + 1;
                i = ini;
                while b.get(i).is_some_and(|c| c.is_ascii_digit()) {
                    i += 1;
                }
                let d = t.get(ini..i)?;
                ms = mat::trunc(format!("0.{}", d).parse::<f64>().ok()? * 1000.0);
            }
        }
        match b.get(i) {
            Some(b'Z') => i += 1,
            Some(c @ (b'+' | b'-')) => {
                let sinal = if *c == b'-' { -1.0 } else { 1.0 };
                let fh = num_de(i + 1, i + 3)?;
                let fm = if b.get(i + 3) == Some(&b':') { num_de(i + 4, i + 6)? } else { num_de(i + 3, i + 5)? };
                i = t.len();
                fuso = sinal * (fh * 60.0 + fm);
            }
            _ => {}
        }
    }
    if i != t.len() || !(1.0..=12.0).contains(&mes) || !(1.0..=31.0).contains(&dia) || h > 24.0 || mi > 59.0 || s > 59.0 {
        return None;
    }
    Some(montar_data(ano, mes - 1.0, dia, h, mi, s, ms) - fuso * 60000.0)
}

fn this_data(vm: &mut Vm, this: Valor) -> Result<f64, Valor> {
    if let Valor::Obj(id) = this {
        if let Tipo::Data(d) = vm.objs[id as usize].tipo {
            return Ok(d);
        }
    }
    Err(vm.erro_tipo("this is not a Date object."))
}

fn texto_data(t: f64, modo: u8) -> String {
    if t.is_nan() {
        return String::from("Invalid Date");
    }
    let (a, m, d, h, mi, s, _, w) = partes_data(t);
    let (dw, mn) = (DIAS[w as usize], MESES[m as usize]);
    match modo {
        0 => format!("{} {} {:02} {:04} {:02}:{:02}:{:02} GMT+0000 (Coordinated Universal Time)", dw, mn, d, a, h, mi, s),
        1 => format!("{} {} {:02} {:04}", dw, mn, d, a),
        2 => format!("{:02}:{:02}:{:02} GMT+0000 (Coordinated Universal Time)", h, mi, s),
        3 => format!("{}, {:02} {} {:04} {:02}:{:02}:{:02} GMT", dw, d, mn, a, h, mi, s),
        4 => format!("{:02}/{:02}/{:04}, {:02}:{:02}:{:02}", d, m + 1, a, h, mi, s),
        5 => format!("{:02}/{:02}/{:04}", d, m + 1, a),
        _ => format!("{:02}:{:02}:{:02}", h, mi, s),
    }
}

fn data(vm: &mut Vm) {
    let p = vm.p.data;
    let c = construtor(vm, "Date", 7, |vm, _, a| {
        if vm.novo_alvo == Valor::Undef {
            let t = (vm.ganchos.agora_ms)() as f64;
            let s = texto_data(t, 0);
            return Ok(vm.txt(&s));
        }
        let t = match a.len() {
            0 => (vm.ganchos.agora_ms)() as f64,
            1 => {
                let v = if let Valor::Obj(id) = a[0] {
                    if let Tipo::Data(d) = vm.objs[id as usize].tipo { num(d) } else { vm.para_primitivo(a[0], None)? }
                } else {
                    a[0]
                };
                if let Valor::Texto(s) = v {
                    let s = vm.n.texto(s);
                    ler_data(&s)
                } else {
                    let n = vm.para_numero(v)?;
                    if mat::abs(n) > 8.64e15 { f64::NAN } else { mat::trunc(n) }
                }
            }
            _ => {
                let mut v = [f64::NAN, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0];
                for (i, x) in a.iter().take(7).enumerate() {
                    v[i] = vm.para_numero(*x)?;
                }
                if (0.0..=99.0).contains(&mat::trunc(v[0])) {
                    v[0] = 1900.0 + mat::trunc(v[0]);
                }
                montar_data(v[0], v[1], v[2], v[3], v[4], v[5], v[6])
            }
        };
        let nt = vm.novo_alvo;
        let proto = vm.proto_de_novo_alvo(nt, vm.p.data)?;
        Ok(Valor::Obj(vm.novo_obj_com(proto, Tipo::Data(t))))
    }, p);
    vm.def(c, "now", 0, |vm, _, _| Ok(num((vm.ganchos.agora_ms)() as f64)));
    vm.def(c, "parse", 1, |vm, _, a| {
        let s = vm.para_texto(arg(a, 0))?;
        Ok(num(ler_data(&s)))
    });
    vm.def(c, "UTC", 7, |vm, _, a| {
        let mut v = [f64::NAN, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0];
        for (i, x) in a.iter().take(7).enumerate() {
            v[i] = vm.para_numero(*x)?;
        }
        if (0.0..=99.0).contains(&mat::trunc(v[0])) {
            v[0] = 1900.0 + mat::trunc(v[0]);
        }
        Ok(num(montar_data(v[0], v[1], v[2], v[3], v[4], v[5], v[6])))
    });
    macro_rules! ler {
        ($($nome:literal),+ ; $f:expr) => {
            $(vm.def(p, $nome, 0, |vm, this, _| {
                let t = this_data(vm, this)?;
                if t.is_nan() {
                    return Ok(Valor::Num(f64::NAN));
                }
                let f: fn((i64, i64, i64, i64, i64, i64, i64, i64)) -> i64 = $f;
                Ok(num(f(partes_data(t)) as f64))
            });)+
        };
    }
    ler!("getFullYear", "getUTCFullYear"; |x| x.0);
    ler!("getMonth", "getUTCMonth"; |x| x.1);
    ler!("getDate", "getUTCDate"; |x| x.2);
    ler!("getHours", "getUTCHours"; |x| x.3);
    ler!("getMinutes", "getUTCMinutes"; |x| x.4);
    ler!("getSeconds", "getUTCSeconds"; |x| x.5);
    ler!("getMilliseconds", "getUTCMilliseconds"; |x| x.6);
    ler!("getDay", "getUTCDay"; |x| x.7);
    ler!("getYear"; |x| x.0 - 1900);
    vm.def(p, "getTimezoneOffset", 0, |_, _, _| Ok(Valor::Int(0)));
    fn ler_tempo(vm: &mut Vm, this: Valor, _: &[Valor]) -> Res {
        Ok(num(this_data(vm, this)?))
    }
    vm.def(p, "getTime", 0, ler_tempo);
    vm.def(p, "valueOf", 0, ler_tempo);
    vm.def(p, "setTime", 1, |vm, this, a| {
        let t = vm.para_numero(arg(a, 0))?;
        this_data(vm, this)?;
        if let Valor::Obj(id) = this {
            vm.objs[id as usize].tipo = Tipo::Data(t);
        }
        Ok(num(t))
    });
    /// Muda os campos a partir de `campo` (0 ano, 1 mês, 2 dia, 3 hora, 4 min, 5 seg, 6 ms).
    fn mudar(vm: &mut Vm, this: Valor, a: &[Valor], campo: usize, maximo: usize) -> Res {
        let t = this_data(vm, this)?;
        let base = if t.is_nan() { if campo == 0 { 0.0 } else { return Ok(Valor::Num(f64::NAN)) } } else { t };
        let x = partes_data(base);
        let mut v = [x.0 as f64, x.1 as f64, x.2 as f64, x.3 as f64, x.4 as f64, x.5 as f64, x.6 as f64];
        for (i, y) in a.iter().take(maximo).enumerate() {
            v[campo + i] = vm.para_numero(*y)?;
        }
        if a.is_empty() {
            v[campo] = f64::NAN;
        }
        let n = montar_data(v[0], v[1], v[2], v[3], v[4], v[5], v[6]);
        if let Valor::Obj(id) = this {
            vm.objs[id as usize].tipo = Tipo::Data(n);
        }
        Ok(num(n))
    }
    macro_rules! muda {
        ($($nome:literal),+ ; $campo:expr, $maximo:expr) => {
            $(vm.def(p, $nome, 1, |vm, this, a| mudar(vm, this, a, $campo, $maximo));)+
        };
    }
    muda!("setFullYear", "setUTCFullYear"; 0, 3);
    muda!("setMonth", "setUTCMonth"; 1, 2);
    muda!("setDate", "setUTCDate"; 2, 1);
    muda!("setHours", "setUTCHours"; 3, 4);
    muda!("setMinutes", "setUTCMinutes"; 4, 3);
    muda!("setSeconds", "setUTCSeconds"; 5, 2);
    muda!("setMilliseconds", "setUTCMilliseconds"; 6, 1);
    macro_rules! escreve {
        ($($nome:literal),+ ; $modo:expr) => {
            $(vm.def(p, $nome, 0, |vm, this, _| {
                let t = this_data(vm, this)?;
                let s = texto_data(t, $modo);
                Ok(vm.txt(&s))
            });)+
        };
    }
    escreve!("toString"; 0);
    escreve!("toDateString"; 1);
    escreve!("toTimeString"; 2);
    escreve!("toUTCString", "toGMTString"; 3);
    escreve!("toLocaleString"; 4);
    escreve!("toLocaleDateString"; 5);
    escreve!("toLocaleTimeString"; 6);
    vm.def(p, "toISOString", 0, |vm, this, _| {
        let t = this_data(vm, this)?;
        match data_iso(t) {
            Some(s) => Ok(vm.txt(&s)),
            None => Err(vm.erro_intervalo("Invalid time value")),
        }
    });
    vm.def(p, "toJSON", 1, |vm, this, _| {
        let t = this_data(vm, this)?;
        Ok(match data_iso(t) {
            Some(s) => vm.txt(&s),
            None => Valor::Nulo,
        })
    });
    let k = vm.s.to_primitive;
    let f = vm.nova_nativa("[Symbol.toPrimitive]", 1, |vm, this, a| {
        let d = vm.para_texto(arg(a, 0))?;
        let ordem = if &*d == "number" { [vm.a.value_of, vm.a.to_string] } else { [vm.a.to_string, vm.a.value_of] };
        for m in ordem {
            let g = vm.obter(this, m)?;
            if vm.e_funcao(g) {
                let r = vm.chamar(g, this, &[])?;
                if r.obj().is_none() {
                    return Ok(r);
                }
            }
        }
        Err(vm.erro_tipo("Cannot convert object to primitive value"))
    });
    vm.def_valor(p, k, Valor::Obj(f), P_CONFIG);
}

// -----------------------------------------------------------------------------
// Map, Set, WeakMap, WeakSet, WeakRef
// -----------------------------------------------------------------------------

fn chave_colecao(vm: &mut Vm, v: Valor) -> (u8, u64) {
    match v {
        Valor::Texto(t) => (1, vm.n.atomo_de(t) as u64),
        Valor::Int(i) => (2, (if i == 0 { 0.0f64 } else { i as f64 }).to_bits()),
        Valor::Num(n) => (2, if n == 0.0 { 0.0f64.to_bits() } else if n.is_nan() { f64::NAN.to_bits() } else { n.to_bits() }),
        Valor::Bool(b) => (3, b as u64),
        Valor::Undef | Valor::Vazio => (4, 0),
        Valor::Nulo => (5, 0),
        Valor::Obj(o) => (6, o as u64),
        Valor::Simbolo(s) => (7, s as u64),
        Valor::Caixa(_) => (8, 0),
    }
}

fn colecao(vm: &mut Vm, this: Valor) -> Result<&mut Box<Colecao>, Valor> {
    if let Valor::Obj(id) = this {
        // (o borrow checker não deixa devolver no `match` e seguir para o erro)
        if matches!(vm.objs[id as usize].tipo, Tipo::Mapa(_) | Tipo::Conjunto(_)) {
            match &mut vm.objs[id as usize].tipo {
                Tipo::Mapa(c) | Tipo::Conjunto(c) => return Ok(c),
                _ => unreachable!(),
            }
        }
    }
    Err(vm.erro_tipo("Method called on incompatible receiver"))
}

fn colocar(vm: &mut Vm, this: Valor, k: Valor, v: Valor) -> Result<(), Valor> {
    let chave = chave_colecao(vm, k);
    let k = if let Valor::Num(n) = k { if n == 0.0 { Valor::Int(0) } else { k } } else { k };
    let c = colecao(vm, this)?;
    if let Some(&i) = c.indice.get(&chave) {
        c.itens[i as usize] = Some((k, v));
    } else {
        c.indice.insert(chave, c.itens.len() as u32);
        c.itens.push(Some((k, v)));
        c.vivos += 1;
    }
    Ok(())
}

fn iter_colecao(vm: &mut Vm, this: Valor, tipo: u8) -> Res {
    colecao(vm, this)?;
    let p = vm.p.iter_colecao;
    Ok(Valor::Obj(vm.novo_obj_com(p, Tipo::IterColecao { alvo: this, i: 0, tipo })))
}

fn colecoes(vm: &mut Vm) {
    let pm = vm.p.mapa;
    let ps = vm.p.conjunto;
    fn novo(vm: &mut Vm, a: &[Valor], padrao: ObjId, mapa: bool, nome: &str) -> Res {
        if vm.novo_alvo == Valor::Undef {
            return Err(vm.erro_tipo(&format!("Constructor {} requires 'new'", nome)));
        }
        let nt = vm.novo_alvo;
        let proto = vm.proto_de_novo_alvo(nt, padrao)?;
        let c = Box::new(Colecao::default());
        let id = vm.novo_obj_com(proto, if mapa { Tipo::Mapa(c) } else { Tipo::Conjunto(c) });
        let o = Valor::Obj(id);
        if !arg(a, 0).nulo_ou_undef() {
            let itens = vm.iteravel_para_lista(a[0])?;
            for x in itens {
                if mapa {
                    if x.obj().is_none() {
                        return Err(vm.erro_tipo("Iterator value is not an entry object"));
                    }
                    let k = vm.obter_elem(x, Valor::Int(0))?;
                    let v = vm.obter_elem(x, Valor::Int(1))?;
                    colocar(vm, o, k, v)?;
                } else {
                    colocar(vm, o, x, x)?;
                }
            }
        }
        Ok(o)
    }
    construtor(vm, "Map", 0, |vm, _, a| { let p = vm.p.mapa; novo(vm, a, p, true, "Map") }, pm);
    construtor(vm, "Set", 0, |vm, _, a| { let p = vm.p.conjunto; novo(vm, a, p, false, "Set") }, ps);
    def_marca(vm, pm, "Map");
    def_marca(vm, ps, "Set");
    fn pegar(vm: &mut Vm, this: Valor, a: &[Valor]) -> Res {
        let chave = chave_colecao(vm, arg(a, 0));
        let c = colecao(vm, this)?;
        Ok(c.indice.get(&chave).and_then(|i| c.itens[*i as usize]).map_or(Valor::Undef, |p| p.1))
    }
    fn tem(vm: &mut Vm, this: Valor, a: &[Valor]) -> Res {
        let chave = chave_colecao(vm, arg(a, 0));
        let c = colecao(vm, this)?;
        Ok(Valor::Bool(c.indice.contains_key(&chave)))
    }
    fn apagar(vm: &mut Vm, this: Valor, a: &[Valor]) -> Res {
        let chave = chave_colecao(vm, arg(a, 0));
        let c = colecao(vm, this)?;
        Ok(Valor::Bool(match c.indice.remove(&chave) {
            Some(i) => {
                c.itens[i as usize] = None;
                c.vivos -= 1;
                true
            }
            None => false,
        }))
    }
    fn limpar(vm: &mut Vm, this: Valor, _: &[Valor]) -> Res {
        let c = colecao(vm, this)?;
        c.indice.clear();
        for x in c.itens.iter_mut() {
            *x = None;
        }
        c.vivos = 0;
        Ok(Valor::Undef)
    }
    fn para_cada(vm: &mut Vm, this: Valor, a: &[Valor]) -> Res {
        let f = arg(a, 0);
        let mut i = 0;
        loop {
            let c = colecao(vm, this)?;
            let Some(x) = c.itens.get(i) else { break };
            let x = *x;
            i += 1;
            if let Some((k, v)) = x {
                vm.chamar(f, arg(a, 1), &[v, k, this])?;
            }
        }
        Ok(Valor::Undef)
    }
    fn tamanho(vm: &mut Vm, this: Valor, _: &[Valor]) -> Res {
        Ok(num(colecao(vm, this)?.vivos as f64))
    }
    vm.def(pm, "get", 1, pegar);
    vm.def(pm, "set", 2, |vm, this, a| {
        colocar(vm, this, arg(a, 0), arg(a, 1))?;
        Ok(this)
    });
    vm.def(ps, "add", 1, |vm, this, a| {
        colocar(vm, this, arg(a, 0), arg(a, 0))?;
        Ok(this)
    });
    for p in [pm, ps] {
        vm.def(p, "has", 1, tem);
        vm.def(p, "delete", 1, apagar);
        vm.def(p, "clear", 0, limpar);
        vm.def(p, "forEach", 1, para_cada);
        vm.def_acessor(p, "size", Some(tamanho), None);
    }
    vm.def(pm, "keys", 0, |vm, this, _| iter_colecao(vm, this, 1));
    let f = vm.def(pm, "entries", 0, |vm, this, _| iter_colecao(vm, this, 2));
    vm.def(pm, "values", 0, |vm, this, _| iter_colecao(vm, this, 0));
    let k = vm.s.iterator;
    vm.def_valor(pm, k, Valor::Obj(f), P_OCULTA);
    vm.def(ps, "entries", 0, |vm, this, _| iter_colecao(vm, this, 2));
    let f = vm.def(ps, "values", 0, |vm, this, _| iter_colecao(vm, this, 0));
    let kv = vm.n.atomo("keys");
    vm.def_valor(ps, kv, Valor::Obj(f), P_OCULTA);
    vm.def_valor(ps, k, Valor::Obj(f), P_OCULTA);
    let pi = vm.p.iter_colecao;
    def_marca(vm, pi, "Map Iterator");
    vm.def(pi, "next", 0, |vm, this, _| {
        let Valor::Obj(id) = this else { return Err(vm.erro_tipo("incompatible receiver")) };
        let Tipo::IterColecao { alvo, i, tipo } = vm.objs[id as usize].tipo else { return Err(vm.erro_tipo("incompatible receiver")) };
        let mut j = i as usize;
        loop {
            let c = colecao(vm, alvo)?;
            let Some(x) = c.itens.get(j) else {
                if let Tipo::IterColecao { i: ii, .. } = &mut vm.objs[id as usize].tipo {
                    *ii = j as u32;
                }
                return Ok(resultado_iter(vm, Valor::Undef, true));
            };
            let x = *x;
            j += 1;
            if let Some((k, v)) = x {
                if let Tipo::IterColecao { i: ii, .. } = &mut vm.objs[id as usize].tipo {
                    *ii = j as u32;
                }
                let r = match tipo {
                    0 => v,
                    1 => k,
                    _ => vm.novo_array(vec![k, v]),
                };
                return Ok(resultado_iter(vm, r, false));
            }
        }
    });

    // As versões "fracas" seguram os objetos até a página acabar.
    let pwm = vm.novo_objeto();
    let pws = vm.novo_objeto();
    let g = vm.global;
    fn nova_fraca(vm: &mut Vm, a: &[Valor], nome: &str, mapa: bool) -> Res {
        let nt = vm.novo_alvo;
        let k = vm.n.atomo(nome);
        let g = vm.global;
        let c = vm.obter_obj(g, k, Valor::Obj(g))?;
        let proto = vm.obter(c, K_PROTOTYPE)?;
        let proto = vm.proto_de_novo_alvo(nt, proto.obj().unwrap_or(vm.p.objeto))?;
        novo(vm, a, proto, mapa, nome)
    }
    construtor(vm, "WeakMap", 0, |vm, _, a| nova_fraca(vm, a, "WeakMap", true), pwm);
    construtor(vm, "WeakSet", 0, |vm, _, a| nova_fraca(vm, a, "WeakSet", false), pws);
    def_marca(vm, pwm, "WeakMap");
    def_marca(vm, pws, "WeakSet");
    fn exige_obj(vm: &mut Vm, v: Valor) -> Result<(), Valor> {
        if matches!(v, Valor::Obj(_) | Valor::Simbolo(_)) {
            Ok(())
        } else {
            Err(vm.erro_tipo("Invalid value used in weak collection"))
        }
    }
    vm.def(pwm, "get", 1, pegar);
    vm.def(pwm, "set", 2, |vm, this, a| {
        exige_obj(vm, arg(a, 0))?;
        colocar(vm, this, arg(a, 0), arg(a, 1))?;
        Ok(this)
    });
    vm.def(pws, "add", 1, |vm, this, a| {
        exige_obj(vm, arg(a, 0))?;
        colocar(vm, this, arg(a, 0), arg(a, 0))?;
        Ok(this)
    });
    for p in [pwm, pws] {
        vm.def(p, "has", 1, tem);
        vm.def(p, "delete", 1, apagar);
    }
    let pr = vm.novo_objeto();
    construtor(vm, "WeakRef", 1, |vm, _, a| {
        let nt = vm.novo_alvo;
        let proto = vm.proto_de_novo_alvo(nt, vm.p.objeto)?;
        let o = vm.novo_obj_com(proto, Tipo::Comum);
        let k = vm.n.atomo("%alvo");
        vm.def_valor(o, k, arg(a, 0), 0);
        Ok(Valor::Obj(o))
    }, pr);
    vm.def(pr, "deref", 0, |vm, this, _| {
        let k = vm.n.atomo("%alvo");
        vm.obter(this, k)
    });
    let _ = g;
    let pf = vm.novo_objeto();
    construtor(vm, "FinalizationRegistry", 1, |vm, _, _| {
        let nt = vm.novo_alvo;
        let proto = vm.proto_de_novo_alvo(nt, vm.p.objeto)?;
        Ok(Valor::Obj(vm.novo_obj_com(proto, Tipo::Comum)))
    }, pf);
    vm.def(pf, "register", 2, |_, _, _| Ok(Valor::Undef));
    vm.def(pf, "unregister", 1, |_, _, _| Ok(Valor::Bool(false)));
}

// -----------------------------------------------------------------------------
// Promise
// -----------------------------------------------------------------------------

pub fn nova_promessa(vm: &mut Vm) -> Valor {
    let p = vm.p.promessa;
    Valor::Obj(vm.novo_obj_com(p, Tipo::Promessa(Box::new(Promessa { estado: 0, valor: Valor::Undef, reacoes: Vec::new(), tratada: false }))))
}

fn enfileirar_reacao(vm: &mut Vm, r: Reacao, estado: u8, valor: Valor) {
    let manipulador = if estado == 1 { r.ok } else { r.erro };
    vm.microtarefas.push_back(Tarefa::Reacao { manipulador, valor, derivada: r.derivada, rejeitada: estado == 2 });
}

fn assentar(vm: &mut Vm, p: Valor, estado: u8, valor: Valor) {
    let Valor::Obj(id) = p else { return };
    let Tipo::Promessa(pr) = &mut vm.objs[id as usize].tipo else { return };
    if pr.estado == 1 || pr.estado == 2 {
        return;
    }
    pr.estado = estado;
    pr.valor = valor;
    let reacoes = core::mem::take(&mut pr.reacoes);
    let sem_tratar = estado == 2 && !pr.tratada;
    if sem_tratar {
        vm.rejeicoes.push(p);
    }
    for r in reacoes {
        enfileirar_reacao(vm, r, estado, valor);
    }
}

pub fn rejeitar(vm: &mut Vm, p: Valor, e: Valor) {
    assentar(vm, p, 2, e);
}

/// O passo "resolver": um thenable é adotado; o resto cumpre a promessa.
pub fn resolver(vm: &mut Vm, p: Valor, v: Valor) {
    if v == p {
        let e = vm.erro_tipo("Chaining cycle detected for promise");
        return rejeitar(vm, p, e);
    }
    if let Valor::Obj(_) = v {
        let k = vm.a.then;
        match vm.obter(v, k) {
            Err(e) => return rejeitar(vm, p, e),
            Ok(t) if vm.e_funcao(t) => {
                if let Valor::Obj(id) = p {
                    if let Tipo::Promessa(pr) = &mut vm.objs[id as usize].tipo {
                        pr.estado = 3; // travada: esperando o thenable
                    }
                }
                vm.microtarefas.push_back(Tarefa::AdotarThenable { promessa: p, thenable: v, then: t });
                return;
            }
            _ => {}
        }
    }
    assentar(vm, p, 1, v);
}

fn funcoes_resolver(vm: &mut Vm, p: Valor) -> (Valor, Valor) {
    // A primeira que for chamada vale: as duas dividem a marca em `dado[1]`.
    let marca = Valor::Obj(vm.novo_objeto());
    let f = vm.nativa_com(K_VAZIO, 1, |vm, _, a| {
        let (p, m) = (vm.dado_nativa(0), vm.dado_nativa(1));
        if ja_chamada(vm, m) {
            return Ok(Valor::Undef);
        }
        resolver(vm, p, arg(a, 0));
        Ok(Valor::Undef)
    }, [p, marca], false);
    let g = vm.nativa_com(K_VAZIO, 1, |vm, _, a| {
        let (p, m) = (vm.dado_nativa(0), vm.dado_nativa(1));
        if ja_chamada(vm, m) {
            return Ok(Valor::Undef);
        }
        rejeitar(vm, p, arg(a, 0));
        Ok(Valor::Undef)
    }, [p, marca], false);
    (Valor::Obj(f), Valor::Obj(g))
}

fn ja_chamada(vm: &mut Vm, marca: Valor) -> bool {
    let Valor::Obj(id) = marca else { return false };
    let o = &mut vm.objs[id as usize];
    if o.flags & O_ARGUMENTOS != 0 {
        return true;
    }
    o.flags |= O_ARGUMENTOS;
    false
}

pub fn adotar(vm: &mut Vm, promessa: Valor, thenable: Valor, then: Valor) {
    vm.pilha.push(promessa);
    let (f, g) = funcoes_resolver(vm, promessa);
    vm.pilha.push(f);
    vm.pilha.push(g);
    let r = vm.chamar(then, thenable, &[f, g]);
    if let Err(e) = r {
        let _ = vm.chamar(g, Valor::Undef, &[e]);
    }
    vm.pilha.pop();
    vm.pilha.pop();
    vm.pilha.pop();
}

/// Um valor de marca em `dado[1]`: esta nativa continua uma função assíncrona.
const MARCA_ASSINC: Valor = Valor::Caixa(0xA51C);

pub fn rodar_reacao(vm: &mut Vm, manipulador: Valor, valor: Valor, derivada: Valor, rejeitada: bool) {
    // A continuação de um `await` roda direto, no nível de fora (assim o
    // coletor de lixo pode trabalhar dentro de funções assíncronas longas).
    if let Valor::Obj(id) = manipulador {
        if let Tipo::Nativa(d) = &vm.objs[id as usize].tipo {
            if d.dado[1] == MARCA_ASSINC {
                let Valor::Obj(g) = d.dado[0] else { return };
                let modo = if rejeitada { 1 } else { 0 };
                vm.pilha.push(Valor::Obj(g));
                avancar_generico(vm, g, modo, valor);
                vm.pilha.pop();
                return;
            }
        }
    }
    if !vm.e_funcao(manipulador) {
        if rejeitada {
            rejeitar(vm, derivada, valor);
        } else {
            resolver(vm, derivada, valor);
        }
        return;
    }
    vm.pilha.push(derivada);
    vm.pilha.push(valor);
    let r = vm.chamar(manipulador, Valor::Undef, &[valor]);
    vm.pilha.pop();
    vm.pilha.pop();
    match r {
        Ok(v) => resolver(vm, derivada, v),
        Err(e) => rejeitar(vm, derivada, e),
    }
}

fn avancar_generico(vm: &mut Vm, g: ObjId, modo: u8, valor: Valor) {
    let eh_gerador = matches!(&vm.objs[g as usize].tipo, Tipo::Gerador(x) if x.func.gerador);
    if eh_gerador {
        continuar_gerador_assinc(vm, g, modo, valor);
    } else {
        avancar_assincrona(vm, g, modo, valor);
    }
}

/// Espera `v`: quando a promessa assentar, o gerador `g` continua.
fn esperar(vm: &mut Vm, g: ObjId, v: Valor) {
    let k = K_VAZIO;
    let p = promessa_de(vm, v);
    let ok = vm.nativa_com(k, 1, |_, _, _| Ok(Valor::Undef), [Valor::Obj(g), MARCA_ASSINC], false);
    let er = vm.nativa_com(k, 1, |_, _, _| Ok(Valor::Undef), [Valor::Obj(g), MARCA_ASSINC], false);
    reagir(vm, p, Valor::Obj(ok), Valor::Obj(er), Valor::Undef);
}

/// `Promise.resolve(v)`, sem passar pelo código da página.
pub fn promessa_de(vm: &mut Vm, v: Valor) -> Valor {
    if let Valor::Obj(id) = v {
        if matches!(vm.objs[id as usize].tipo, Tipo::Promessa(_)) {
            return v;
        }
    }
    let p = nova_promessa(vm);
    resolver(vm, p, v);
    p
}

fn reagir(vm: &mut Vm, p: Valor, ok: Valor, erro: Valor, derivada: Valor) {
    let Valor::Obj(id) = p else { return };
    let Tipo::Promessa(pr) = &mut vm.objs[id as usize].tipo else { return };
    pr.tratada = true;
    let r = Reacao { ok, erro, derivada };
    if pr.estado == 1 || pr.estado == 2 {
        let (e, v) = (pr.estado, pr.valor);
        enfileirar_reacao(vm, r, e, v);
    } else {
        pr.reacoes.push(r);
    }
}

/// Uma função assíncrona anda até o próximo `await` (ou até acabar).
pub fn avancar_assincrona(vm: &mut Vm, g: ObjId, modo: u8, valor: Valor) {
    let r = vm.retomar(g, modo, valor);
    let aguardando = matches!(vm.sinal, Sinal::Await);
    vm.sinal = Sinal::Nada;
    let promessa = match &vm.objs[g as usize].tipo {
        Tipo::Gerador(x) => x.promessa,
        _ => Valor::Undef,
    };
    match r {
        Ok(v) if aguardando => esperar(vm, g, v),
        Ok(v) => resolver(vm, promessa, v),
        Err(e) => {
            vm.abortando = false;
            rejeitar(vm, promessa, e);
        }
    }
}

fn continuar_gerador_assinc(vm: &mut Vm, g: ObjId, modo: u8, valor: Valor) {
    let r = vm.retomar(g, modo, valor);
    let sinal = core::mem::replace(&mut vm.sinal, Sinal::Nada);
    let pedido = match &mut vm.objs[g as usize].tipo {
        Tipo::Gerador(x) => match sinal {
            Sinal::Await => None,
            _ => {
                if x.fila.is_empty() {
                    None
                } else {
                    Some(x.fila.remove(0))
                }
            }
        },
        _ => None,
    };
    match (r, sinal) {
        (Ok(v), Sinal::Await) => {
            esperar(vm, g, v);
            return;
        }
        (Ok(v), Sinal::Yield) => {
            if let Some((_, _, p)) = pedido {
                let it = resultado_iter(vm, v, false);
                resolver(vm, p, it);
            }
        }
        (Ok(v), Sinal::Nada) => {
            if let Some((_, _, p)) = pedido {
                let it = resultado_iter(vm, v, true);
                resolver(vm, p, it);
            }
        }
        (Err(e), _) => {
            if let Some((_, _, p)) = pedido {
                rejeitar(vm, p, e);
            }
        }
    }
    drenar_gerador_assinc(vm, g);
}

fn drenar_gerador_assinc(vm: &mut Vm, g: ObjId) {
    let prox = match &vm.objs[g as usize].tipo {
        Tipo::Gerador(x) if !matches!(x.estado, EstadoGerador::Rodando | EstadoGerador::SuspensoAwait) => x.fila.first().map(|p| (p.0, p.1)),
        _ => None,
    };
    if let Some((modo, v)) = prox {
        continuar_gerador_assinc(vm, g, modo, v);
    }
}

fn pedir_gerador_assinc(vm: &mut Vm, this: Valor, modo: u8, v: Valor) -> Res {
    let p = nova_promessa(vm);
    let Valor::Obj(g) = this else {
        let e = vm.erro_tipo("not an async generator");
        rejeitar(vm, p, e);
        return Ok(p);
    };
    let Tipo::Gerador(x) = &mut vm.objs[g as usize].tipo else {
        let e = vm.erro_tipo("not an async generator");
        rejeitar(vm, p, e);
        return Ok(p);
    };
    x.fila.push((modo, v, p));
    if x.fila.len() == 1 {
        drenar_gerador_assinc(vm, g);
    }
    Ok(p)
}

fn promessas(vm: &mut Vm) {
    let p = vm.p.promessa;
    let c = construtor(vm, "Promise", 1, |vm, _, a| {
        if vm.novo_alvo == Valor::Undef {
            return Err(vm.erro_tipo("Promise constructor cannot be invoked without 'new'"));
        }
        let ex = arg(a, 0);
        if !vm.e_funcao(ex) {
            return Err(vm.erro_tipo("Promise resolver is not a function"));
        }
        let nt = vm.novo_alvo;
        let proto = vm.proto_de_novo_alvo(nt, vm.p.promessa)?;
        let pr = nova_promessa(vm);
        if let Valor::Obj(id) = pr {
            vm.objs[id as usize].proto = proto;
        }
        let (f, g) = funcoes_resolver(vm, pr);
        if let Err(e) = vm.chamar(ex, Valor::Undef, &[f, g]) {
            vm.chamar(g, Valor::Undef, &[e])?;
        }
        Ok(pr)
    }, p);
    def_marca(vm, p, "Promise");
    vm.def(p, "then", 2, |vm, this, a| {
        if !matches!(this, Valor::Obj(i) if matches!(vm.objs[i as usize].tipo, Tipo::Promessa(_))) {
            return Err(vm.erro_tipo("Method Promise.prototype.then called on incompatible receiver"));
        }
        let d = nova_promessa(vm);
        reagir(vm, this, arg(a, 0), arg(a, 1), d);
        Ok(d)
    });
    vm.def(p, "catch", 1, |vm, this, a| {
        let k = vm.a.then;
        let f = vm.obter(this, k)?;
        vm.chamar(f, this, &[Valor::Undef, arg(a, 0)])
    });
    vm.def(p, "finally", 1, |vm, this, a| {
        let f = arg(a, 0);
        if !vm.e_funcao(f) {
            let k = vm.a.then;
            let t = vm.obter(this, k)?;
            return vm.chamar(t, this, &[f, f]);
        }
        let ok = vm.nativa_com(K_VAZIO, 1, |vm, _, a| {
            let f = vm.dado_nativa(0);
            let r = vm.chamar(f, Valor::Undef, &[])?;
            let p = promessa_de(vm, r);
            let devolve = vm.nativa_com(K_VAZIO, 0, |vm, _, _| Ok(vm.dado_nativa(0)), [arg(a, 0), Valor::Undef], false);
            let k = vm.a.then;
            let t = vm.obter(p, k)?;
            vm.chamar(t, p, &[Valor::Obj(devolve)])
        }, [f, Valor::Undef], false);
        let er = vm.nativa_com(K_VAZIO, 1, |vm, _, a| {
            let f = vm.dado_nativa(0);
            let r = vm.chamar(f, Valor::Undef, &[])?;
            let p = promessa_de(vm, r);
            let relanca = vm.nativa_com(K_VAZIO, 0, |vm, _, _| Err(vm.dado_nativa(0)), [arg(a, 0), Valor::Undef], false);
            let k = vm.a.then;
            let t = vm.obter(p, k)?;
            vm.chamar(t, p, &[Valor::Obj(relanca)])
        }, [f, Valor::Undef], false);
        let k = vm.a.then;
        let t = vm.obter(this, k)?;
        vm.chamar(t, this, &[Valor::Obj(ok), Valor::Obj(er)])
    });
    vm.def(c, "resolve", 1, |vm, _, a| Ok(promessa_de(vm, arg(a, 0))));
    vm.def(c, "reject", 1, |vm, _, a| {
        let p = nova_promessa(vm);
        rejeitar(vm, p, arg(a, 0));
        Ok(p)
    });
    vm.def(c, "withResolvers", 0, |vm, _, _| {
        let p = nova_promessa(vm);
        let (f, g) = funcoes_resolver(vm, p);
        let o = vm.novo_objeto();
        vm.def_texto(o, "promise", p);
        vm.def_texto(o, "resolve", f);
        vm.def_texto(o, "reject", g);
        Ok(Valor::Obj(o))
    });
    /// all (0), allSettled (1), any (2): o estado vai num objeto compartilhado.
    fn combinar(vm: &mut Vm, a: &[Valor], modo: i32) -> Res {
        let d = nova_promessa(vm);
        let itens = match vm.iteravel_para_lista(arg(a, 0)) {
            Ok(i) => i,
            Err(e) => {
                rejeitar(vm, d, e);
                return Ok(d);
            }
        };
        let n = itens.len();
        let resultados = vm.novo_array(vec![Valor::Undef; n]);
        let estado = vm.novo_objeto();
        vm.def_texto(estado, "falta", num(n as f64));
        vm.def_texto(estado, "r", resultados);
        vm.def_texto(estado, "d", d);
        vm.def_texto(estado, "m", Valor::Int(modo));
        if n == 0 {
            if modo == 2 {
                let pe = vm.p.erro_agregado;
                let e = vm.novo_erro(pe, "All promises were rejected");
                rejeitar(vm, d, e);
            } else {
                resolver(vm, d, resultados);
            }
            return Ok(d);
        }
        fn um(vm: &mut Vm, x: Valor, rejeitada: bool) -> Res {
            let (i, est) = (vm.dado_nativa(0), vm.dado_nativa(1));
            let r = vm.obter_nome(est, "r")?;
            let d = vm.obter_nome(est, "d")?;
            let m = vm.obter_nome(est, "m")?;
            let modo = if let Valor::Int(q) = m { q } else { 0 };
            if modo == 0 && rejeitada {
                rejeitar(vm, d, x);
                return Ok(Valor::Undef);
            }
            if modo == 2 && !rejeitada {
                resolver(vm, d, x);
                return Ok(Valor::Undef);
            }
            let v = if modo == 1 {
                let o = vm.novo_objeto();
                let s = vm.txt(if rejeitada { "rejected" } else { "fulfilled" });
                vm.def_texto(o, "status", s);
                vm.def_texto(o, if rejeitada { "reason" } else { "value" }, x);
                Valor::Obj(o)
            } else {
                x
            };
            vm.definir_elem(r, i, v)?;
            let f = vm.obter_nome(est, "falta")?;
            let f = vm.para_numero(f)? - 1.0;
            vm.definir_nome(est, "falta", num(f))?;
            if f == 0.0 {
                if modo == 2 {
                    let pe = vm.p.erro_agregado;
                    let e = vm.novo_erro(pe, "All promises were rejected");
                    if let Valor::Obj(id) = e {
                        let k = vm.a.errors;
                        vm.def_valor(id, k, r, P_OCULTA);
                    }
                    rejeitar(vm, d, e);
                } else {
                    resolver(vm, d, r);
                }
            }
            Ok(Valor::Undef)
        }
        for (i, x) in itens.into_iter().enumerate() {
            let p = promessa_de(vm, x);
            let dado = [num(i as f64), Valor::Obj(estado)];
            let ok = vm.nativa_com(K_VAZIO, 1, |vm, _, a| um(vm, arg(a, 0), false), dado, false);
            let er = vm.nativa_com(K_VAZIO, 1, |vm, _, a| um(vm, arg(a, 0), true), dado, false);
            reagir(vm, p, Valor::Obj(ok), Valor::Obj(er), Valor::Undef);
        }
        Ok(d)
    }
    vm.def(c, "all", 1, |vm, _, a| combinar(vm, a, 0));
    vm.def(c, "allSettled", 1, |vm, _, a| combinar(vm, a, 1));
    vm.def(c, "any", 1, |vm, _, a| combinar(vm, a, 2));
    vm.def(c, "race", 1, |vm, _, a| {
        let d = nova_promessa(vm);
        let itens = vm.iteravel_para_lista(arg(a, 0))?;
        let (f, g) = funcoes_resolver(vm, d);
        for x in itens {
            let p = promessa_de(vm, x);
            reagir(vm, p, f, g, Valor::Undef);
        }
        Ok(d)
    });
    // Geradores assíncronos.
    let pa = vm.p.gerador_assinc;
    def_marca(vm, pa, "AsyncGenerator");
    vm.def(pa, "next", 1, |vm, this, a| pedir_gerador_assinc(vm, this, 0, arg(a, 0)));
    vm.def(pa, "throw", 1, |vm, this, a| pedir_gerador_assinc(vm, this, 1, arg(a, 0)));
    vm.def(pa, "return", 1, |vm, this, a| pedir_gerador_assinc(vm, this, 2, arg(a, 0)));
    let f = vm.nova_nativa("[Symbol.asyncIterator]", 0, |_, this, _| Ok(this));
    let k = vm.s.async_iterator;
    vm.def_valor(pa, k, Valor::Obj(f), P_OCULTA);
}

// -----------------------------------------------------------------------------
// Proxy
// -----------------------------------------------------------------------------

fn proxy(vm: &mut Vm) {
    let k = vm.n.atomo("Proxy");
    let c = vm.nativa_com(k, 2, |vm, _, a| {
        if vm.novo_alvo == Valor::Undef {
            return Err(vm.erro_tipo("Constructor Proxy requires 'new'"));
        }
        let (alvo, man) = (arg(a, 0), arg(a, 1));
        if alvo.obj().is_none() || man.obj().is_none() {
            return Err(vm.erro_tipo("Cannot create proxy with a non-object as target or handler"));
        }
        // O protótipo do alvo: `instanceof` e os métodos continuam funcionando.
        let proto = vm.objs[alvo.obj().unwrap() as usize].proto;
        Ok(Valor::Obj(vm.novo_obj_com(proto, Tipo::Proxy(alvo, man))))
    }, [Valor::Undef; 2], true);
    let g = vm.global;
    vm.def_valor(g, k, Valor::Obj(c), P_OCULTA);
}

// -----------------------------------------------------------------------------
// ArrayBuffer e arrays tipados
// -----------------------------------------------------------------------------

fn novo_tipado(vm: &mut Vm, a: &[Valor], tipo: TipoTipado, ip: usize) -> Res {
    if vm.novo_alvo == Valor::Undef {
        return Err(vm.erro_tipo("Constructor requires 'new'"));
    }
    let nt = vm.novo_alvo;
    let padrao = vm.p.tipado[ip];
    let proto = vm.proto_de_novo_alvo(nt, padrao)?;
    let tam = tamanho_tipado(tipo);
    let pb = vm.p.buffer;
    // new X(buffer, deslocamento, tamanho)
    if let Valor::Obj(b) = arg(a, 0) {
        if let Tipo::Buffer(d) = &vm.objs[b as usize].tipo {
            let total = d.len();
            let ini = vm.para_inteiro(arg(a, 1))?.max(0.0) as usize;
            if ini % tam != 0 || ini > total {
                return Err(vm.erro_intervalo("start offset is out of bounds"));
            }
            let n = if arg(a, 2) == Valor::Undef { (total - ini) / tam } else { vm.para_inteiro(a[2])?.max(0.0) as usize };
            if ini + n * tam > total {
                return Err(vm.erro_intervalo("Invalid typed array length"));
            }
            return Ok(Valor::Obj(vm.novo_obj_com(proto, Tipo::Tipado { buffer: b, tipo, inicio: ini as u32, n: n as u32 })));
        }
    }
    let lista = match arg(a, 0) {
        Valor::Obj(_) => {
            let k = vm.s.iterator;
            let m = vm.obter(a[0], k)?;
            Some(if m.nulo_ou_undef() { vm.lista_de_array(a[0])? } else { vm.iteravel_para_lista(a[0])? })
        }
        _ => None,
    };
    let n = match &lista {
        Some(l) => l.len(),
        None => {
            let n = vm.para_inteiro(arg(a, 0))?;
            if !(0.0..=268435456.0).contains(&n) {
                return Err(vm.erro_intervalo("Invalid typed array length"));
            }
            n as usize
        }
    };
    let b = vm.novo_obj_com(pb, Tipo::Buffer(vec![0u8; n * tam]));
    let id = vm.novo_obj_com(proto, Tipo::Tipado { buffer: b, tipo, inicio: 0, n: n as u32 });
    if let Some(l) = lista {
        for (i, x) in l.into_iter().enumerate() {
            let v = vm.para_numero(x)?;
            vm.tipado_escrever(id, i, v);
        }
    }
    Ok(Valor::Obj(id))
}

fn tipados(vm: &mut Vm) {
    let pb = vm.p.buffer;
    let c = construtor(vm, "ArrayBuffer", 1, |vm, _, a| {
        let n = vm.para_inteiro(arg(a, 0))?;
        if !(0.0..=268435456.0).contains(&n) {
            return Err(vm.erro_intervalo("Array buffer allocation failed"));
        }
        let nt = vm.novo_alvo;
        let proto = vm.proto_de_novo_alvo(nt, vm.p.buffer)?;
        Ok(Valor::Obj(vm.novo_obj_com(proto, Tipo::Buffer(vec![0u8; n as usize]))))
    }, pb);
    def_marca(vm, pb, "ArrayBuffer");
    vm.def(c, "isView", 1, |vm, _, a| Ok(Valor::Bool(matches!(arg(a, 0), Valor::Obj(i) if matches!(vm.objs[i as usize].tipo, Tipo::Tipado { .. })))));
    vm.def_acessor(pb, "byteLength", Some(|vm, this, _| {
        let Valor::Obj(id) = this else { return Ok(Valor::Int(0)) };
        Ok(if let Tipo::Buffer(d) = &vm.objs[id as usize].tipo { num(d.len() as f64) } else { Valor::Int(0) })
    }), None);
    vm.def(pb, "slice", 2, |vm, this, a| {
        let Valor::Obj(id) = this else { return Err(vm.erro_tipo("not an ArrayBuffer")) };
        let n = if let Tipo::Buffer(d) = &vm.objs[id as usize].tipo { d.len() } else { 0 };
        let i = indice_relativo(vm, arg(a, 0), n, 0)?;
        let f = indice_relativo(vm, arg(a, 1), n, n)?;
        let copia = if let Tipo::Buffer(d) = &vm.objs[id as usize].tipo { d[i..f.max(i)].to_vec() } else { Vec::new() };
        let p = vm.p.buffer;
        Ok(Valor::Obj(vm.novo_obj_com(p, Tipo::Buffer(copia))))
    });

    // Um protótipo comum com os métodos, que reaproveita os de Array.
    let base = vm.novo_objeto();
    let pa = vm.p.array;
    for nome in ["join", "forEach", "indexOf", "lastIndexOf", "includes", "reduce", "reduceRight", "find", "findIndex", "findLast", "findLastIndex", "some", "every", "at", "keys", "values", "entries", "toString"] {
        let k = vm.n.atomo(nome);
        if let Ok(f) = vm.obter_obj(pa, k, Valor::Obj(pa)) {
            vm.def_valor(base, k, f, P_OCULTA);
        }
    }
    let ki = vm.s.iterator;
    let fi = vm.array_iter_original;
    vm.def_valor(base, ki, fi, P_OCULTA);
    fn dados(vm: &Vm, this: Valor) -> Option<(ObjId, TipoTipado, u32, u32, ObjId)> {
        let Valor::Obj(id) = this else { return None };
        if let Tipo::Tipado { buffer, tipo, inicio, n } = vm.objs[id as usize].tipo { Some((buffer, tipo, inicio, n, id)) } else { None }
    }
    vm.def_acessor(base, "buffer", Some(|vm, this, _| Ok(dados(vm, this).map_or(Valor::Undef, |d| Valor::Obj(d.0)))), None);
    vm.def_acessor(base, "byteOffset", Some(|vm, this, _| Ok(dados(vm, this).map_or(Valor::Undef, |d| num(d.2 as f64)))), None);
    vm.def_acessor(base, "byteLength", Some(|vm, this, _| Ok(dados(vm, this).map_or(Valor::Undef, |d| num(d.3 as f64 * tamanho_tipado(d.1) as f64)))), None);
    vm.def_acessor(base, "length", Some(|vm, this, _| Ok(dados(vm, this).map_or(Valor::Undef, |d| num(d.3 as f64)))), None);
    vm.def(base, "set", 1, |vm, this, a| {
        let Some(d) = dados(vm, this) else { return Err(vm.erro_tipo("not a typed array")) };
        let de = vm.para_inteiro(arg(a, 1))?.max(0.0) as usize;
        let l = vm.lista_de_array(arg(a, 0))?;
        if de + l.len() > d.3 as usize {
            return Err(vm.erro_intervalo("offset is out of bounds"));
        }
        for (i, x) in l.into_iter().enumerate() {
            let v = vm.para_numero(x)?;
            vm.tipado_escrever(d.4, de + i, v);
        }
        Ok(Valor::Undef)
    });
    vm.def(base, "fill", 1, |vm, this, a| {
        let Some(d) = dados(vm, this) else { return Err(vm.erro_tipo("not a typed array")) };
        let v = vm.para_numero(arg(a, 0))?;
        let n = d.3 as usize;
        let i = indice_relativo(vm, arg(a, 1), n, 0)?;
        let f = indice_relativo(vm, arg(a, 2), n, n)?;
        for k in i..f.max(i) {
            vm.tipado_escrever(d.4, k, v);
        }
        Ok(this)
    });
    vm.def(base, "subarray", 2, |vm, this, a| {
        let Some(d) = dados(vm, this) else { return Err(vm.erro_tipo("not a typed array")) };
        let n = d.3 as usize;
        let i = indice_relativo(vm, arg(a, 0), n, 0)?;
        let f = indice_relativo(vm, arg(a, 1), n, n)?;
        let proto = vm.objs[d.4 as usize].proto;
        let tam = tamanho_tipado(d.1) as u32;
        Ok(Valor::Obj(vm.novo_obj_com(proto, Tipo::Tipado { buffer: d.0, tipo: d.1, inicio: d.2 + i as u32 * tam, n: f.saturating_sub(i) as u32 })))
    });
    vm.def(base, "slice", 2, |vm, this, a| {
        let Some(d) = dados(vm, this) else { return Err(vm.erro_tipo("not a typed array")) };
        let n = d.3 as usize;
        let i = indice_relativo(vm, arg(a, 0), n, 0)?;
        let f = indice_relativo(vm, arg(a, 1), n, n)?.max(i);
        let tam = tamanho_tipado(d.1);
        let copia = if let Tipo::Buffer(b) = &vm.objs[d.0 as usize].tipo {
            let ini = d.2 as usize + i * tam;
            b[ini..ini + (f - i) * tam].to_vec()
        } else {
            Vec::new()
        };
        let pb = vm.p.buffer;
        let b = vm.novo_obj_com(pb, Tipo::Buffer(copia));
        let proto = vm.objs[d.4 as usize].proto;
        Ok(Valor::Obj(vm.novo_obj_com(proto, Tipo::Tipado { buffer: b, tipo: d.1, inicio: 0, n: (f - i) as u32 })))
    });
    vm.def(base, "map", 1, |vm, this, a| {
        let Some(d) = dados(vm, this) else { return Err(vm.erro_tipo("not a typed array")) };
        let proto = vm.objs[d.4 as usize].proto;
        let tam = tamanho_tipado(d.1);
        let pb = vm.p.buffer;
        let b = vm.novo_obj_com(pb, Tipo::Buffer(vec![0u8; d.3 as usize * tam]));
        let novo = vm.novo_obj_com(proto, Tipo::Tipado { buffer: b, tipo: d.1, inicio: 0, n: d.3 });
        for i in 0..d.3 as usize {
            let x = vm.tipado_ler(d.4, i);
            let r = vm.chamar(arg(a, 0), arg(a, 1), &[x, num(i as f64), this])?;
            let v = vm.para_numero(r)?;
            vm.tipado_escrever(novo, i, v);
        }
        Ok(Valor::Obj(novo))
    });

    macro_rules! tipo {
        ($nome:literal, $t:expr, $i:expr) => {{
            let p = vm.novo_obj_com(base, Tipo::Comum);
            vm.p.tipado[$i] = p;
            let c = construtor(vm, $nome, 3, |vm, _, a| novo_tipado(vm, a, $t, $i), p);
            let kb = vm.n.atomo("BYTES_PER_ELEMENT");
            let n = Valor::Int(tamanho_tipado($t) as i32);
            vm.def_valor(c, kb, n, 0);
            vm.def_valor(p, kb, n, 0);
            def_marca(vm, p, $nome);
            vm.def(c, "from", 1, |vm, this, a| {
                let l = vm.iteravel_para_lista(arg(a, 0))?;
                let arr = vm.novo_array(l);
                vm.construir(this, &[arr], this)
            });
            vm.def(c, "of", 0, |vm, this, a| {
                let arr = vm.novo_array(a.to_vec());
                vm.construir(this, &[arr], this)
            });
        }};
    }
    tipo!("Int8Array", TipoTipado::I8, 0);
    tipo!("Uint8Array", TipoTipado::U8, 1);
    tipo!("Uint8ClampedArray", TipoTipado::U8C, 2);
    tipo!("Int16Array", TipoTipado::I16, 3);
    tipo!("Uint16Array", TipoTipado::U16, 4);
    tipo!("Int32Array", TipoTipado::I32, 5);
    tipo!("Uint32Array", TipoTipado::U32, 6);
    tipo!("Float32Array", TipoTipado::F32, 7);
    tipo!("Float64Array", TipoTipado::F64, 8);
}

#[allow(dead_code)]
fn _so_para_usar(_: &str) -> usize {
    "".to_string().len()
}
