//! A ponte entre o JavaScript e a página: `document`, os nós do DOM, os
//! eventos, os timers e a rede. O navegador implementa `Hospedeiro`; os testes
//! no Linux implementam o mesmo com um hospedeiro falso.
//!
//! As APIs que são só lógica (URL, fetch, Event...) estão em JavaScript, em
//! `web_prelude.js`, por cima das funções nativas que estão aqui.

use crate::css::{self, EstadoCss, Estilo, Seletor, Tam};
use crate::dom::{Dom, NADA, NoId};
use crate::js::lexico::{K_LENGTH, K_NAME};
use crate::js::nativos::{arg, construtor};
use crate::js::vm::*;
use crate::js::{relatar_erro, rodar_microtarefas};
use alloc::boxed::Box;
use alloc::collections::BTreeMap;
use alloc::format;
use alloc::rc::Rc;
use alloc::string::{String, ToString};
use alloc::vec;
use alloc::vec::Vec;

/// O que a página pede ao navegador.
pub trait Hospedeiro {
    fn dom(&mut self) -> &mut Dom;
    fn estado_css(&mut self) -> &EstadoCss;
    fn valor_campo(&mut self, no: NoId) -> Option<String>;
    fn definir_valor_campo(&mut self, no: NoId, v: &str);
    fn marcado(&mut self, no: NoId) -> bool;
    fn definir_marcado(&mut self, no: NoId, v: bool);
    fn indice_selecionado(&mut self, no: NoId) -> i32;
    fn definir_indice(&mut self, no: NoId, i: i32);
    /// A caixa do elemento na página (refaz a diagramação, se preciso).
    fn retangulo(&mut self, no: NoId) -> Option<(i32, i32, i32, i32)>;
    fn estilo(&mut self, no: NoId) -> Option<Rc<Estilo>>;
    fn janela(&mut self) -> (i32, i32);
    fn altura_documento(&mut self) -> i32;
    fn rolagem(&mut self) -> i32;
    fn rolar_para(&mut self, y: i32);
    fn url(&mut self) -> String;
    /// Ir para outro endereço (a página atual acaba).
    fn navegar(&mut self, url: &str, substituir: bool);
    /// `history.pushState`: muda o endereço sem carregar.
    fn mudar_endereco(&mut self, url: &str, substituir: bool);
    fn voltar(&mut self, n: i32);
    fn focar(&mut self, no: Option<NoId>);
    fn foco(&mut self) -> Option<NoId>;
    /// O que acontece depois de um clique que ninguém cancelou.
    fn acao_clique(&mut self, no: NoId);
    fn enviar_formulario(&mut self, form: NoId);
    fn limpar_formulario(&mut self, form: NoId);
    /// Um pedido de rede; a resposta vem por `Web::resposta`.
    fn pedir(&mut self, id: u32, metodo: &str, url: &str, cabecalhos: &[(String, String)], corpo: Option<Vec<u8>>, credenciais: bool);
    fn cancelar_pedido(&mut self, id: u32);
    /// Baixar uma imagem (`new Image()`); o evento `load` vem depois.
    fn pedir_imagem(&mut self, url: &str);
    fn tamanho_imagem(&mut self, url: &str) -> Option<(i32, i32)>;
    fn avisar(&mut self, texto: &str);
    fn console(&mut self, nivel: u8, texto: &str);
    fn cookies(&mut self) -> String;
    fn definir_cookie(&mut self, c: &str);
    fn ler_armazenamento(&mut self) -> String;
    fn gravar_armazenamento(&mut self, t: &str);
    fn mudou_titulo(&mut self);
}

const CL_NO: u32 = 1;
const CL_ESTILO: u32 = 2;
const CL_DATASET: u32 = 3;

struct Ouvinte {
    alvo: ObjId,
    tipo: Rc<str>,
    f: Valor,
    captura: bool,
    uma_vez: bool,
}

struct Timer {
    id: u32,
    quando: u64,
    f: Valor,
    args: Vec<Valor>,
    intervalo: Option<u64>,
    /// requestAnimationFrame
    quadro: bool,
}

pub struct Contexto {
    host: *mut dyn Hospedeiro,
    nos: BTreeMap<NoId, ObjId>,
    protos: BTreeMap<&'static str, ObjId>,
    ouvintes: Vec<Ouvinte>,
    timers: Vec<Timer>,
    prox_timer: u32,
    pedidos: BTreeMap<u32, Valor>,
    prox_pedido: u32,
    /// O construtor Event do prelúdio.
    fabrica_evento: Valor,
    script_atual: NoId,
    scripts_novos: Vec<NoId>,
    scripts_rodados: Vec<NoId>,
    seletores: BTreeMap<String, Rc<Vec<Seletor>>>,
    pub erros: u32,
    imagens_esperando: Vec<(NoId, String)>,
    pronto: u8, // readyState: 0 loading, 1 interactive, 2 complete
    guardados: Vec<Valor>,
}

fn ctx(vm: &Vm) -> &'static mut Contexto {
    unsafe { &mut *(vm.hospede as *mut Contexto) }
}

fn host(vm: &Vm) -> &'static mut dyn Hospedeiro {
    unsafe { &mut *ctx(vm).host }
}

fn dom(vm: &Vm) -> &'static mut Dom {
    host(vm).dom()
}

/// O elemento (ou nó) de um valor.
fn no_de(vm: &mut Vm, v: Valor) -> Result<NoId, Valor> {
    if let Valor::Obj(o) = v {
        if let Tipo::Hospede { classe: CL_NO, id } = vm.objs[o as usize].tipo {
            return Ok(id);
        }
    }
    Err(vm.erro_tipo("Illegal invocation"))
}

fn talvez_no(vm: &Vm, v: Valor) -> Option<NoId> {
    if let Valor::Obj(o) = v {
        if let Tipo::Hospede { classe: CL_NO, id } = vm.objs[o as usize].tipo {
            return Some(id);
        }
    }
    None
}

fn proto_da_tag(tag: &str) -> &'static str {
    match tag {
        "a" => "HTMLAnchorElement",
        "img" => "HTMLImageElement",
        "input" => "HTMLInputElement",
        "button" => "HTMLButtonElement",
        "select" => "HTMLSelectElement",
        "textarea" => "HTMLTextAreaElement",
        "option" => "HTMLOptionElement",
        "form" => "HTMLFormElement",
        "script" => "HTMLScriptElement",
        "link" => "HTMLLinkElement",
        "style" => "HTMLStyleElement",
        "meta" => "HTMLMetaElement",
        "iframe" => "HTMLIFrameElement",
        "label" => "HTMLLabelElement",
        "canvas" => "HTMLCanvasElement",
        "td" | "th" => "HTMLTableCellElement",
        "table" => "HTMLTableElement",
        "tr" => "HTMLTableRowElement",
        "div" => "HTMLDivElement",
        "span" => "HTMLSpanElement",
        "p" => "HTMLParagraphElement",
        "body" => "HTMLBodyElement",
        "html" => "HTMLHtmlElement",
        "head" => "HTMLHeadElement",
        "ul" => "HTMLUListElement",
        "ol" => "HTMLOListElement",
        "li" => "HTMLLIElement",
        "video" | "audio" => "HTMLMediaElement",
        _ => "HTMLElement",
    }
}

/// O objeto JavaScript de um nó (sempre o mesmo para o mesmo nó).
pub fn embrulhar(vm: &mut Vm, no: NoId) -> Valor {
    if no == NADA {
        return Valor::Nulo;
    }
    let c = ctx(vm);
    if let Some(o) = c.nos.get(&no) {
        return Valor::Obj(*o);
    }
    let d = dom(vm);
    let nome = match &d.nos[no as usize].dados {
        crate::dom::Dados::Documento => "Document",
        crate::dom::Dados::Fragmento => "DocumentFragment",
        crate::dom::Dados::Elemento { nome, .. } => proto_da_tag(nome),
        crate::dom::Dados::Texto(_) => "Text",
        crate::dom::Dados::Comentario(_) => "Comment",
    };
    let proto = c.protos[nome];
    let o = vm.novo_obj_com(proto, Tipo::Hospede { classe: CL_NO, id: no });
    c.nos.insert(no, o);
    Valor::Obj(o)
}

fn lista_de_nos(vm: &mut Vm, nos: &[NoId]) -> Valor {
    let v: Vec<Valor> = nos.iter().map(|n| embrulhar(vm, *n)).collect();
    let a = vm.novo_array(v);
    if let Valor::Obj(id) = a {
        vm.objs[id as usize].proto = ctx(vm).protos["NodeList"];
    }
    a
}

fn txt_opt(vm: &mut Vm, t: Option<&str>) -> Valor {
    match t {
        Some(s) => vm.txt(s),
        None => Valor::Nulo,
    }
}

fn seletores(vm: &mut Vm, t: &str) -> Result<Rc<Vec<Seletor>>, Valor> {
    let c = ctx(vm);
    if let Some(s) = c.seletores.get(t) {
        return Ok(s.clone());
    }
    let Some(l) = css::ler_lista_seletores(t) else {
        return Err(vm.erro_sintaxe(&format!("'{}' is not a valid selector", t)));
    };
    let r = Rc::new(l);
    c.seletores.insert(t.to_string(), r.clone());
    Ok(r)
}

/// `style.backgroundColor` → `background-color`
fn para_kebab(nome: &str) -> String {
    let mut r = String::with_capacity(nome.len() + 4);
    for c in nome.chars() {
        if c.is_ascii_uppercase() {
            r.push('-');
            r.push(c.to_ascii_lowercase());
        } else {
            r.push(c);
        }
    }
    if r.starts_with("webkit-") || r.starts_with("moz-") || r.starts_with("ms-") {
        r.insert(0, '-');
    }
    if r == "css-float" {
        return String::from("float");
    }
    r
}

/// As declarações de um atributo `style`, como texto.
fn decls_inline(t: &str) -> Vec<(String, String)> {
    let mut v = Vec::new();
    let (mut ini, mut prof, mut aspa) = (0, 0, None);
    let mut fechar = |p: &str| {
        if let Some((a, b)) = p.split_once(':') {
            let a = a.trim();
            if !a.is_empty() {
                v.push((if a.starts_with("--") { a.to_string() } else { a.to_ascii_lowercase() }, b.trim().to_string()));
            }
        }
    };
    for (i, c) in t.char_indices() {
        match c {
            '"' | '\'' if aspa.is_none() => aspa = Some(c),
            x if Some(x) == aspa => aspa = None,
            '(' if aspa.is_none() => prof += 1,
            ')' if aspa.is_none() => prof -= 1,
            ';' if aspa.is_none() && prof <= 0 => {
                fechar(&t[ini..i]);
                ini = i + 1;
            }
            _ => {}
        }
    }
    fechar(&t[ini..]);
    v
}

fn escrever_inline(vm: &mut Vm, no: NoId, l: &[(String, String)]) {
    let t = l.iter().map(|(a, b)| format!("{}: {};", a, b)).collect::<Vec<_>>().join(" ");
    dom(vm).definir_atributo(no, "style", &t);
}

fn estilo_definir(vm: &mut Vm, no: NoId, prop: &str, valor: &str) {
    let mut l = decls_inline(dom(vm).atributo(no, "style").unwrap_or(""));
    l.retain(|(a, _)| a != prop);
    if !valor.is_empty() {
        l.push((prop.to_string(), valor.to_string()));
    }
    escrever_inline(vm, no, &l);
}

const NAO_ESTILO: [&str; 11] = ["setProperty", "removeProperty", "getPropertyValue", "getPropertyPriority", "cssText", "length", "item", "constructor", "toString", "valueOf", "hasOwnProperty"];

fn gancho_obter(vm: &mut Vm, _o: ObjId, classe: u32, id: u32, k: Txt) -> Option<Valor> {
    if vm.n.e_simbolo(k) {
        return None;
    }
    match classe {
        CL_ESTILO => {
            let nome = vm.n.texto(k);
            if NAO_ESTILO.contains(&&*nome) || nome.starts_with("__") {
                return None;
            }
            let p = if nome.starts_with("--") { nome.to_string() } else { para_kebab(&nome) };
            let l = decls_inline(dom(vm).atributo(id, "style").unwrap_or(""));
            let v = l.iter().rev().find(|(a, _)| *a == p).map_or("", |(_, b)| b.as_str()).to_string();
            Some(vm.txt(&v))
        }
        CL_DATASET => {
            let nome = vm.n.texto(k);
            let a = format!("data-{}", para_kebab(&nome));
            dom(vm).atributo(id, &a).map(str::to_string).map(|v| vm.txt(&v))
        }
        _ => None,
    }
}

fn gancho_definir(vm: &mut Vm, _o: ObjId, classe: u32, id: u32, k: Txt, v: Valor) -> bool {
    if vm.n.e_simbolo(k) {
        return false;
    }
    match classe {
        CL_ESTILO => {
            let nome = vm.n.texto(k);
            if NAO_ESTILO.contains(&&*nome) {
                return false;
            }
            let p = if nome.starts_with("--") { nome.to_string() } else { para_kebab(&nome) };
            let t = if v.nulo_ou_undef() { Rc::from("") } else { vm.para_texto(v).unwrap_or_else(|_| Rc::from("")) };
            estilo_definir(vm, id, &p, &t);
            true
        }
        CL_DATASET => {
            let nome = vm.n.texto(k);
            let a = format!("data-{}", para_kebab(&nome));
            let t = vm.para_texto(v).unwrap_or_else(|_| Rc::from(""));
            dom(vm).definir_atributo(id, &a, &t);
            true
        }
        _ => false,
    }
}

fn gancho_apagar(vm: &mut Vm, _o: ObjId, classe: u32, id: u32, k: Txt) -> bool {
    if classe == CL_DATASET && !vm.n.e_simbolo(k) {
        let nome = vm.n.texto(k);
        dom(vm).remover_atributo(id, &format!("data-{}", para_kebab(&nome)));
        return true;
    }
    false
}

fn gancho_chaves(vm: &mut Vm, _o: ObjId, classe: u32, id: u32) -> Vec<Txt> {
    let mut r = Vec::new();
    if classe == CL_DATASET {
        let nomes: Vec<String> = dom(vm).atributos(id).iter().filter_map(|(a, _)| a.strip_prefix("data-").map(str::to_string)).collect();
        for a in nomes {
            let mut s = String::new();
            let mut sobe = false;
            for c in a.chars() {
                if c == '-' {
                    sobe = true;
                } else if sobe {
                    s.push(c.to_ascii_uppercase());
                    sobe = false;
                } else {
                    s.push(c);
                }
            }
            r.push(vm.n.atomo(&s));
        }
    }
    r
}

fn raizes(vm: &mut Vm) -> Vec<Valor> {
    let c = ctx(vm);
    let mut r: Vec<Valor> = c.nos.values().map(|o| Valor::Obj(*o)).collect();
    r.extend(c.protos.values().map(|o| Valor::Obj(*o)));
    for o in &c.ouvintes {
        r.push(o.f);
        r.push(Valor::Obj(o.alvo));
    }
    for t in &c.timers {
        r.push(t.f);
        r.extend_from_slice(&t.args);
    }
    r.extend(c.pedidos.values().copied());
    r.push(c.fabrica_evento);
    r.extend_from_slice(&c.guardados);
    r
}

// -----------------------------------------------------------------------------
// Atributos refletidos
// -----------------------------------------------------------------------------

/// `el.id`, `el.href`...: dado[0] = nome do atributo; dado[1]: 0 texto, 1 lógico, 2 URL, 3 inteiro.
fn refletir_get(vm: &mut Vm, this: Valor, _: &[Valor]) -> Res {
    let n = no_de(vm, this)?;
    let (a, tipo) = (vm.dado_nativa(0), vm.dado_nativa(1));
    let Valor::Texto(a) = a else { return Ok(Valor::Undef) };
    let nome = vm.n.texto(a);
    let v = dom(vm).atributo(n, &nome).map(str::to_string);
    Ok(match tipo {
        Valor::Int(1) => Valor::Bool(v.is_some()),
        Valor::Int(2) => {
            let t = v.unwrap_or_default();
            if t.is_empty() {
                return Ok(Valor::Texto(crate::js::lexico::K_VAZIO));
            }
            let u = resolver_url(vm, &t);
            vm.txt(&u)
        }
        Valor::Int(3) => Valor::Int(v.and_then(|x| x.trim().parse().ok()).unwrap_or(0)),
        Valor::Int(4) => Valor::Int(v.and_then(|x| x.trim().parse().ok()).unwrap_or(1)),
        _ => {
            let t = v.unwrap_or_default();
            vm.txt(&t)
        }
    })
}

fn refletir_set(vm: &mut Vm, this: Valor, a: &[Valor]) -> Res {
    let n = no_de(vm, this)?;
    let (at, tipo) = (vm.dado_nativa(0), vm.dado_nativa(1));
    let Valor::Texto(at) = at else { return Ok(Valor::Undef) };
    let nome = vm.n.texto(at);
    if tipo == Valor::Int(1) {
        if vm.para_bool(arg(a, 0)) {
            dom(vm).definir_atributo(n, &nome, "");
        } else {
            dom(vm).remover_atributo(n, &nome);
        }
    } else {
        let v = vm.para_texto(arg(a, 0))?;
        dom(vm).definir_atributo(n, &nome, &v);
        if &*nome == "src" && dom(vm).e(n, "img") {
            let u = resolver_url(vm, &v);
            host(vm).pedir_imagem(&u);
            ctx(vm).imagens_esperando.push((n, u));
        }
        if &*nome == "src" && dom(vm).e(n, "script") {
            agendar_scripts(vm, n);
        }
    }
    Ok(Valor::Undef)
}

fn refletir(vm: &mut Vm, proto: ObjId, prop: &str, atributo: &str, tipo: i32) {
    let k = vm.n.atomo(prop);
    let at = vm.n.atomo(atributo);
    let dado = [Valor::Texto(at), Valor::Int(tipo)];
    let g = vm.nativa_com(k, 0, refletir_get, dado, false);
    let s = vm.nativa_com(k, 1, refletir_set, dado, false);
    vm.def_par(proto, k, Valor::Obj(g), Valor::Obj(s), P_CONFIG | P_ENUM);
}

fn resolver_url(vm: &mut Vm, t: &str) -> String {
    let base = base_do_documento(vm);
    crate::http::Url::ler(&base).and_then(|u| u.juntar(t.trim())).map_or_else(|| t.to_string(), |u| u.texto())
}

fn base_do_documento(vm: &mut Vm) -> String {
    let url = host(vm).url();
    let d = dom(vm);
    if let Some(b) = d.achar(0, "base").and_then(|b| d.atributo(b, "href")) {
        if let Some(u) = crate::http::Url::ler(&url).and_then(|u| u.juntar(b)) {
            return u.texto();
        }
    }
    url
}

// -----------------------------------------------------------------------------
// Inserir nós
// -----------------------------------------------------------------------------

/// `<script>` que entraram no documento pelo JavaScript precisam rodar.
fn agendar_scripts(vm: &mut Vm, n: NoId) {
    let d = dom(vm);
    if !d.contem(0, n) {
        return;
    }
    let mut achados = Vec::new();
    if d.e(n, "script") {
        achados.push(n);
    }
    d.todos(n, "script", &mut achados);
    for s in achados {
        let c = ctx(vm);
        if c.scripts_rodados.contains(&s) || !script_executavel(dom(vm), s) {
            continue;
        }
        if dom(vm).atributo(s, "src").is_some_and(|x| !x.is_empty()) {
            c.scripts_rodados.push(s);
            c.scripts_novos.push(s);
            continue;
        }
        // Um script sem `src` roda na hora em que entra no documento.
        let fonte = dom(vm).texto(s);
        if fonte.trim().is_empty() {
            continue;
        }
        c.scripts_rodados.push(s);
        let antes = core::mem::replace(&mut c.script_atual, s);
        if let Err(e) = vm.avaliar(&fonte) {
            ctx(vm).erros += 1;
            relatar_erro(vm, e);
        }
        ctx(vm).script_atual = antes;
    }
}

/// É JavaScript que este navegador roda? (módulos ainda não)
pub fn script_executavel(d: &Dom, n: NoId) -> bool {
    let t = d.atributo(n, "type").unwrap_or("").trim().to_ascii_lowercase();
    matches!(t.as_str(), "" | "text/javascript" | "application/javascript" | "text/ecmascript" | "application/ecmascript" | "application/x-javascript" | "text/jscript")
}

fn de_valor_para_no(vm: &mut Vm, v: Valor) -> Result<NoId, Valor> {
    if let Some(n) = talvez_no(vm, v) {
        return Ok(n);
    }
    let t = vm.para_texto(v)?;
    Ok(dom(vm).criar_texto(&t))
}

fn inserir_varios(vm: &mut Vm, pai: NoId, antes: NoId, a: &[Valor]) -> Result<(), Valor> {
    for x in a {
        let n = de_valor_para_no(vm, *x)?;
        let filhos = if matches!(dom(vm).nos[n as usize].dados, crate::dom::Dados::Fragmento) { dom(vm).filhos(n).to_vec() } else { vec![n] };
        dom(vm).inserir(pai, n, antes);
        for f in filhos {
            agendar_scripts(vm, f);
        }
    }
    Ok(())
}

/// Lê um fragmento de HTML; os scripts dele não rodam (como manda o HTML).
fn marcar_scripts_inertes(vm: &mut Vm, nos: &[NoId]) {
    let d = dom(vm);
    let mut achados = Vec::new();
    for n in nos {
        if d.e(*n, "script") {
            achados.push(*n);
        }
        d.todos(*n, "script", &mut achados);
    }
    ctx(vm).scripts_rodados.extend(achados);
}

// -----------------------------------------------------------------------------
// Eventos
// -----------------------------------------------------------------------------

fn def_opcoes(vm: &mut Vm, op: Valor) -> Result<(bool, bool), Valor> {
    match op {
        Valor::Obj(_) => {
            let c = vm.obter_nome(op, "capture")?;
            let u = vm.obter_nome(op, "once")?;
            Ok((vm.para_bool(c), vm.para_bool(u)))
        }
        o => Ok((vm.para_bool(o), false)),
    }
}

fn alvo_de(vm: &Vm, this: Valor) -> ObjId {
    match this {
        Valor::Obj(o) => o,
        _ => vm.global,
    }
}

fn caminho_evento(vm: &mut Vm, alvo: ObjId) -> Vec<ObjId> {
    let mut v = vec![alvo];
    if let Tipo::Hospede { classe: CL_NO, id } = vm.objs[alvo as usize].tipo {
        let d = dom(vm);
        let mut p = d.pai(id);
        while p != NADA {
            if let Valor::Obj(o) = embrulhar(vm, p) {
                v.push(o);
            }
            p = d.pai(p);
        }
        // Só o que está no documento chega à janela.
        if d.contem(0, id) {
            v.push(vm.global);
        }
    }
    v
}

fn chamar_ouvintes(vm: &mut Vm, alvo: ObjId, tipo: &Rc<str>, ev: Valor, captura: bool, parar: Txt) {
    let c = ctx(vm);
    let lista: Vec<(Valor, bool)> = c.ouvintes.iter().filter(|o| o.alvo == alvo && o.captura == captura && o.tipo == *tipo).map(|o| (o.f, o.uma_vez)).collect();
    for (f, uma) in lista {
        let c = ctx(vm);
        // Removido por um ouvinte anterior?
        let Some(i) = c.ouvintes.iter().position(|o| o.alvo == alvo && o.captura == captura && o.tipo == *tipo && o.f == f) else { continue };
        if uma {
            c.ouvintes.remove(i);
        }
        let this = Valor::Obj(alvo);
        let r = if vm.e_funcao(f) {
            vm.chamar(f, this, &[ev])
        } else {
            // Um objeto com handleEvent.
            match vm.obter_nome(f, "handleEvent") {
                Ok(h) => vm.chamar(h, f, &[ev]),
                Err(e) => Err(e),
            }
        };
        if let Err(e) = r {
            ctx(vm).erros += 1;
            relatar_erro(vm, e);
        }
        let k = vm.obter(ev, parar).unwrap_or(Valor::Undef);
        if vm.para_bool(k) {
            return;
        }
    }
}

/// O manipulador `onclick` (propriedade, ou atributo compilado na hora).
fn chamar_manipulador(vm: &mut Vm, alvo: ObjId, tipo: &str, ev: Valor) {
    let nome = format!("on{}", tipo);
    let k = vm.n.atomo(&nome);
    let this = Valor::Obj(alvo);
    let mut f = {
        let o = &vm.objs[alvo as usize];
        o.achar(k).map_or(Valor::Undef, |i| o.props[i].valor)
    };
    if f == Valor::Undef {
        // `<body onload>` vale para a janela.
        let no_attr = match vm.objs[alvo as usize].tipo {
            Tipo::Hospede { classe: CL_NO, id } => Some(id),
            _ if alvo == vm.global && matches!(tipo, "load" | "unload" | "beforeunload" | "resize" | "scroll" | "hashchange" | "popstate") => Some(dom(vm).body()),
            _ => None,
        };
        if let Some(n) = no_attr {
            if let Some(codigo) = dom(vm).atributo(n, &nome).map(str::to_string) {
                let fonte = format!("(function(event){{\n{}\n}})", codigo);
                match vm.avaliar(&fonte) {
                    Ok(g) => {
                        vm.def_valor(alvo, k, g, P_TUDO);
                        f = g;
                    }
                    Err(e) => {
                        relatar_erro(vm, e);
                        return;
                    }
                }
            }
        }
    }
    if !vm.e_funcao(f) {
        return;
    }
    match vm.chamar(f, this, &[ev]) {
        // `return false` num manipulador cancela o evento.
        Ok(Valor::Bool(false)) => {
            let _ = chamar_prevenir(vm, ev);
        }
        Ok(_) => {}
        Err(e) => {
            ctx(vm).erros += 1;
            relatar_erro(vm, e);
        }
    }
}

fn chamar_prevenir(vm: &mut Vm, ev: Valor) -> Res {
    let f = vm.obter_nome(ev, "preventDefault")?;
    vm.chamar(f, ev, &[])
}

/// Despacha um evento já criado: captura, alvo, borbulha. Devolve se a ação
/// padrão continua valendo.
fn despachar(vm: &mut Vm, alvo: ObjId, ev: Valor) -> Result<bool, Valor> {
    let tipo = vm.obter_nome(ev, "type")?;
    let tipo: Rc<str> = vm.para_texto(tipo)?;
    let borbulha = vm.obter_nome(ev, "bubbles")?;
    let borbulha = vm.para_bool(borbulha);
    let parar = vm.atomo("__parar");
    let caminho = caminho_evento(vm, alvo);
    vm.raizes.push(ev);
    vm.definir_nome(ev, "target", Valor::Obj(alvo))?;
    let path: Vec<Valor> = caminho.iter().map(|o| Valor::Obj(*o)).collect();
    let arr = vm.novo_array(path);
    vm.definir_nome(ev, "__caminho", arr)?;
    let fase_atual = vm.atomo("currentTarget");
    let fase = vm.atomo("eventPhase");
    // Captura: da janela até o pai do alvo.
    for (i, o) in caminho.iter().enumerate().rev() {
        if i == 0 {
            break;
        }
        vm.definir(ev, fase_atual, Valor::Obj(*o))?;
        vm.definir(ev, fase, Valor::Int(1))?;
        chamar_ouvintes(vm, *o, &tipo, ev, true, parar);
        let k = vm.obter(ev, parar)?;
        if vm.para_bool(k) {
            break;
        }
    }
    let k = vm.obter(ev, parar)?;
    if !vm.para_bool(k) {
        for (i, o) in caminho.iter().enumerate() {
            if i > 0 && !borbulha {
                break;
            }
            vm.definir(ev, fase_atual, Valor::Obj(*o))?;
            vm.definir(ev, fase, Valor::Int(if i == 0 { 2 } else { 3 }))?;
            if i == 0 {
                chamar_ouvintes(vm, *o, &tipo, ev, true, parar);
            }
            chamar_ouvintes(vm, *o, &tipo, ev, false, parar);
            chamar_manipulador(vm, *o, &tipo, ev);
            let k = vm.obter(ev, parar)?;
            if vm.para_bool(k) {
                break;
            }
        }
    }
    vm.definir(ev, fase_atual, Valor::Nulo)?;
    vm.definir(ev, fase, Valor::Int(0))?;
    vm.raizes.pop();
    let c = vm.obter_nome(ev, "defaultPrevented")?;
    Ok(!vm.para_bool(c))
}

// -----------------------------------------------------------------------------
// A instalação
// -----------------------------------------------------------------------------

fn nova_interface(vm: &mut Vm, nome: &'static str, pai: Option<&'static str>) -> ObjId {
    let base = pai.map_or(vm.p.objeto, |p| ctx(vm).protos[p]);
    let proto = vm.novo_obj_com(base, Tipo::Comum);
    construtor(vm, nome, 0, |vm, _, _| Err(vm.erro_tipo("Illegal constructor")), proto);
    ctx(vm).protos.insert(nome, proto);
    let t = vm.txt(nome);
    let k = vm.s.to_string_tag;
    vm.def_valor(proto, k, t, P_CONFIG);
    proto
}

fn instalar_no(vm: &mut Vm, p: ObjId) {
    vm.def_acessor(p, "nodeType", Some(|vm, this, _| {
        let n = no_de(vm, this)?;
        Ok(Valor::Int(match dom(vm).nos[n as usize].dados {
            crate::dom::Dados::Documento => 9,
            crate::dom::Dados::Fragmento => 11,
            crate::dom::Dados::Elemento { .. } => 1,
            crate::dom::Dados::Texto(_) => 3,
            crate::dom::Dados::Comentario(_) => 8,
        }))
    }), None);
    vm.def_acessor(p, "nodeName", Some(|vm, this, _| {
        let n = no_de(vm, this)?;
        let t = match &dom(vm).nos[n as usize].dados {
            crate::dom::Dados::Documento => String::from("#document"),
            crate::dom::Dados::Fragmento => String::from("#document-fragment"),
            crate::dom::Dados::Elemento { nome, .. } => nome.to_ascii_uppercase(),
            crate::dom::Dados::Texto(_) => String::from("#text"),
            crate::dom::Dados::Comentario(_) => String::from("#comment"),
        };
        Ok(vm.txt(&t))
    }), None);
    vm.def_acessor(p, "parentNode", Some(|vm, this, _| {
        let n = no_de(vm, this)?;
        Ok(embrulhar(vm, dom(vm).pai(n)))
    }), None);
    vm.def_acessor(p, "parentElement", Some(|vm, this, _| {
        let n = no_de(vm, this)?;
        let q = dom(vm).pai(n);
        Ok(if q != NADA && dom(vm).e_elemento(q) { embrulhar(vm, q) } else { Valor::Nulo })
    }), None);
    vm.def_acessor(p, "childNodes", Some(|vm, this, _| {
        let n = no_de(vm, this)?;
        let f = dom(vm).filhos(n).to_vec();
        Ok(lista_de_nos(vm, &f))
    }), None);
    vm.def_acessor(p, "firstChild", Some(|vm, this, _| {
        let n = no_de(vm, this)?;
        Ok(embrulhar(vm, dom(vm).filhos(n).first().copied().unwrap_or(NADA)))
    }), None);
    vm.def_acessor(p, "lastChild", Some(|vm, this, _| {
        let n = no_de(vm, this)?;
        Ok(embrulhar(vm, dom(vm).filhos(n).last().copied().unwrap_or(NADA)))
    }), None);
    fn irmao(vm: &mut Vm, this: Valor, depois: bool, so_elem: bool) -> Res {
        let n = no_de(vm, this)?;
        let d = dom(vm);
        let q = d.pai(n);
        if q == NADA {
            return Ok(Valor::Nulo);
        }
        let f = d.filhos(q);
        let Some(i) = f.iter().position(|c| *c == n) else { return Ok(Valor::Nulo) };
        let r = if depois { f[i + 1..].iter().copied().find(|c| !so_elem || d.e_elemento(*c)) } else { f[..i].iter().rev().copied().find(|c| !so_elem || d.e_elemento(*c)) };
        Ok(embrulhar(vm, r.unwrap_or(NADA)))
    }
    vm.def_acessor(p, "nextSibling", Some(|vm, this, _| irmao(vm, this, true, false)), None);
    vm.def_acessor(p, "previousSibling", Some(|vm, this, _| irmao(vm, this, false, false)), None);
    vm.def_acessor(p, "nextElementSibling", Some(|vm, this, _| irmao(vm, this, true, true)), None);
    vm.def_acessor(p, "previousElementSibling", Some(|vm, this, _| irmao(vm, this, false, true)), None);
    vm.def_acessor(p, "ownerDocument", Some(|vm, this, _| {
        let n = no_de(vm, this)?;
        Ok(if n == 0 { Valor::Nulo } else { embrulhar(vm, 0) })
    }), None);
    vm.def_acessor(p, "isConnected", Some(|vm, this, _| {
        let n = no_de(vm, this)?;
        Ok(Valor::Bool(dom(vm).contem(0, n)))
    }), None);
    vm.def_acessor(p, "nodeValue", Some(|vm, this, _| {
        let n = no_de(vm, this)?;
        let t = match &dom(vm).nos[n as usize].dados {
            crate::dom::Dados::Texto(t) | crate::dom::Dados::Comentario(t) => Some(t.clone()),
            _ => None,
        };
        Ok(txt_opt(vm, t.as_deref()))
    }), Some(|vm, this, a| {
        let n = no_de(vm, this)?;
        let t = vm.para_texto(arg(a, 0))?;
        if !dom(vm).e_elemento(n) {
            dom(vm).definir_texto(n, &t);
        }
        Ok(Valor::Undef)
    }));
    vm.def_acessor(p, "textContent", Some(|vm, this, _| {
        let n = no_de(vm, this)?;
        if n == 0 {
            return Ok(Valor::Nulo);
        }
        let t = dom(vm).texto(n);
        Ok(vm.txt(&t))
    }), Some(|vm, this, a| {
        let n = no_de(vm, this)?;
        let t = if arg(a, 0).nulo_ou_undef() { Rc::from("") } else { vm.para_texto(a[0])? };
        dom(vm).definir_texto(n, &t);
        Ok(Valor::Undef)
    }));
    vm.def(p, "hasChildNodes", 0, |vm, this, _| {
        let n = no_de(vm, this)?;
        Ok(Valor::Bool(!dom(vm).filhos(n).is_empty()))
    });
    vm.def(p, "contains", 1, |vm, this, a| {
        let n = no_de(vm, this)?;
        let Some(o) = talvez_no(vm, arg(a, 0)) else { return Ok(Valor::Bool(false)) };
        Ok(Valor::Bool(dom(vm).contem(n, o)))
    });
    vm.def(p, "getRootNode", 0, |vm, this, _| {
        let mut n = no_de(vm, this)?;
        let d = dom(vm);
        while d.pai(n) != NADA {
            n = d.pai(n);
        }
        Ok(embrulhar(vm, n))
    });
    vm.def(p, "cloneNode", 1, |vm, this, a| {
        let n = no_de(vm, this)?;
        let fundo = vm.para_bool(arg(a, 0));
        let c = dom(vm).clonar(n, fundo);
        Ok(embrulhar(vm, c))
    });
    vm.def(p, "appendChild", 1, |vm, this, a| {
        let n = no_de(vm, this)?;
        let f = no_de(vm, arg(a, 0))?;
        inserir_varios(vm, n, NADA, &[arg(a, 0)])?;
        let _ = f;
        Ok(arg(a, 0))
    });
    vm.def(p, "insertBefore", 2, |vm, this, a| {
        let n = no_de(vm, this)?;
        no_de(vm, arg(a, 0))?;
        let antes = talvez_no(vm, arg(a, 1)).unwrap_or(NADA);
        if antes != NADA && dom(vm).pai(antes) != n {
            return Err(vm.erro_tipo("The node before which the new node is to be inserted is not a child of this node."));
        }
        inserir_varios(vm, n, antes, &[arg(a, 0)])?;
        Ok(arg(a, 0))
    });
    vm.def(p, "removeChild", 1, |vm, this, a| {
        let n = no_de(vm, this)?;
        let f = no_de(vm, arg(a, 0))?;
        if dom(vm).pai(f) != n {
            return Err(vm.erro_tipo("The node to be removed is not a child of this node."));
        }
        dom(vm).soltar(f);
        Ok(arg(a, 0))
    });
    vm.def(p, "replaceChild", 2, |vm, this, a| {
        let n = no_de(vm, this)?;
        no_de(vm, arg(a, 0))?;
        let velho = no_de(vm, arg(a, 1))?;
        if dom(vm).pai(velho) != n {
            return Err(vm.erro_tipo("The node to be replaced is not a child of this node."));
        }
        inserir_varios(vm, n, velho, &[arg(a, 0)])?;
        dom(vm).soltar(velho);
        Ok(arg(a, 1))
    });
    vm.def(p, "normalize", 0, |_, _, _| Ok(Valor::Undef));
    vm.def(p, "compareDocumentPosition", 1, |vm, this, a| {
        let n = no_de(vm, this)?;
        let o = no_de(vm, arg(a, 0))?;
        let d = dom(vm);
        if n == o {
            return Ok(Valor::Int(0));
        }
        if d.contem(n, o) {
            return Ok(Valor::Int(20));
        }
        if d.contem(o, n) {
            return Ok(Valor::Int(10));
        }
        // A ordem no documento.
        let mut todos = Vec::new();
        d.todos(0, "*", &mut todos);
        let (pn, po) = (todos.iter().position(|x| *x == n), todos.iter().position(|x| *x == o));
        Ok(Valor::Int(match (pn, po) {
            (Some(a), Some(b)) if a < b => 4,
            (Some(_), Some(_)) => 2,
            _ => 1,
        }))
    });
    vm.def(p, "isSameNode", 1, |vm, this, a| Ok(Valor::Bool(vm.igual_estrito(this, arg(a, 0)))));
    for (n, v) in [("ELEMENT_NODE", 1), ("TEXT_NODE", 3), ("COMMENT_NODE", 8), ("DOCUMENT_NODE", 9), ("DOCUMENT_FRAGMENT_NODE", 11), ("DOCUMENT_POSITION_PRECEDING", 2), ("DOCUMENT_POSITION_FOLLOWING", 4), ("DOCUMENT_POSITION_CONTAINS", 8), ("DOCUMENT_POSITION_CONTAINED_BY", 16)] {
        let k = vm.n.atomo(n);
        vm.def_valor(p, k, Valor::Int(v), 0);
    }
}

fn instalar_consulta(vm: &mut Vm, p: ObjId) {
    fn qs(vm: &mut Vm, this: Valor, a: &[Valor], so_um: bool) -> Res {
        let n = no_de(vm, this)?;
        let t = vm.para_texto(arg(a, 0))?;
        let l = seletores(vm, &t)?;
        let mut s = Vec::new();
        let d: &Dom = dom(vm);
        let e = host(vm).estado_css();
        css::selecionar(d, e, n, &l, so_um, &mut s);
        Ok(if so_um { embrulhar(vm, s.first().copied().unwrap_or(NADA)) } else { lista_de_nos(vm, &s) })
    }
    vm.def(p, "querySelector", 1, |vm, this, a| qs(vm, this, a, true));
    vm.def(p, "querySelectorAll", 1, |vm, this, a| qs(vm, this, a, false));
    vm.def(p, "getElementsByTagName", 1, |vm, this, a| {
        let n = no_de(vm, this)?;
        let t = vm.para_texto(arg(a, 0))?.to_ascii_lowercase();
        let mut s = Vec::new();
        dom(vm).todos(n, &t, &mut s);
        Ok(lista_de_nos(vm, &s))
    });
    vm.def(p, "getElementsByClassName", 1, |vm, this, a| {
        let n = no_de(vm, this)?;
        let t = vm.para_texto(arg(a, 0))?;
        let mut todos = Vec::new();
        let d = dom(vm);
        d.todos(n, "*", &mut todos);
        let quer: Vec<&str> = t.split_ascii_whitespace().collect();
        let s: Vec<NoId> = todos.into_iter().filter(|x| quer.iter().all(|c| d.tem_classe(*x, c))).collect();
        Ok(lista_de_nos(vm, &s))
    });
    fn elementos(vm: &mut Vm, n: NoId) -> Vec<NoId> {
        let d = dom(vm);
        d.filhos(n).iter().copied().filter(|c| d.e_elemento(*c)).collect()
    }
    vm.def_acessor(p, "children", Some(|vm, this, _| {
        let n = no_de(vm, this)?;
        let s = elementos(vm, n);
        Ok(lista_de_nos(vm, &s))
    }), None);
    vm.def_acessor(p, "childElementCount", Some(|vm, this, _| {
        let n = no_de(vm, this)?;
        Ok(num(elementos(vm, n).len() as f64))
    }), None);
    vm.def_acessor(p, "firstElementChild", Some(|vm, this, _| {
        let n = no_de(vm, this)?;
        let s = elementos(vm, n);
        Ok(embrulhar(vm, s.first().copied().unwrap_or(NADA)))
    }), None);
    vm.def_acessor(p, "lastElementChild", Some(|vm, this, _| {
        let n = no_de(vm, this)?;
        let s = elementos(vm, n);
        Ok(embrulhar(vm, s.last().copied().unwrap_or(NADA)))
    }), None);
    vm.def(p, "append", 0, |vm, this, a| {
        let n = no_de(vm, this)?;
        inserir_varios(vm, n, NADA, a)?;
        Ok(Valor::Undef)
    });
    vm.def(p, "prepend", 0, |vm, this, a| {
        let n = no_de(vm, this)?;
        let antes = dom(vm).filhos(n).first().copied().unwrap_or(NADA);
        inserir_varios(vm, n, antes, a)?;
        Ok(Valor::Undef)
    });
    vm.def(p, "replaceChildren", 0, |vm, this, a| {
        let n = no_de(vm, this)?;
        dom(vm).definir_texto(n, "");
        inserir_varios(vm, n, NADA, a)?;
        Ok(Valor::Undef)
    });
}

fn instalar_filho(vm: &mut Vm, p: ObjId) {
    vm.def(p, "remove", 0, |vm, this, _| {
        let n = no_de(vm, this)?;
        dom(vm).soltar(n);
        Ok(Valor::Undef)
    });
    vm.def(p, "before", 0, |vm, this, a| {
        let n = no_de(vm, this)?;
        let q = dom(vm).pai(n);
        if q != NADA {
            inserir_varios(vm, q, n, a)?;
        }
        Ok(Valor::Undef)
    });
    vm.def(p, "after", 0, |vm, this, a| {
        let n = no_de(vm, this)?;
        let q = dom(vm).pai(n);
        if q != NADA {
            let f = dom(vm).filhos(q);
            let i = f.iter().position(|c| *c == n).unwrap_or(0);
            let prox = f.get(i + 1).copied().unwrap_or(NADA);
            inserir_varios(vm, q, prox, a)?;
        }
        Ok(Valor::Undef)
    });
    vm.def(p, "replaceWith", 0, |vm, this, a| {
        let n = no_de(vm, this)?;
        let q = dom(vm).pai(n);
        if q != NADA {
            inserir_varios(vm, q, n, a)?;
            dom(vm).soltar(n);
        }
        Ok(Valor::Undef)
    });
}

fn valor_de_elemento(vm: &mut Vm, n: NoId) -> String {
    host(vm).valor_campo(n).unwrap_or_else(|| dom(vm).atributo(n, "value").unwrap_or("").to_string())
}

fn instalar_elemento(vm: &mut Vm, p: ObjId) {
    instalar_consulta(vm, p);
    instalar_filho(vm, p);
    vm.def_acessor(p, "tagName", Some(|vm, this, _| {
        let n = no_de(vm, this)?;
        let t = dom(vm).nome(n).to_ascii_uppercase();
        Ok(vm.txt(&t))
    }), None);
    vm.def_acessor(p, "localName", Some(|vm, this, _| {
        let n = no_de(vm, this)?;
        let t = dom(vm).nome(n).to_string();
        Ok(vm.txt(&t))
    }), None);
    vm.def_acessor(p, "namespaceURI", Some(|vm, _, _| Ok(vm.txt("http://www.w3.org/1999/xhtml"))), None);
    refletir(vm, p, "id", "id", 0);
    refletir(vm, p, "className", "class", 0);
    refletir(vm, p, "slot", "slot", 0);
    vm.def(p, "getAttribute", 1, |vm, this, a| {
        let n = no_de(vm, this)?;
        let k = vm.para_texto(arg(a, 0))?.to_ascii_lowercase();
        let v = dom(vm).atributo(n, &k).map(str::to_string);
        Ok(txt_opt(vm, v.as_deref()))
    });
    vm.def(p, "setAttribute", 2, |vm, this, a| {
        let n = no_de(vm, this)?;
        let k = vm.para_texto(arg(a, 0))?.to_ascii_lowercase();
        let v = vm.para_texto(arg(a, 1))?;
        dom(vm).definir_atributo(n, &k, &v);
        if k == "src" && dom(vm).e(n, "img") {
            let u = resolver_url(vm, &v);
            host(vm).pedir_imagem(&u);
            ctx(vm).imagens_esperando.push((n, u));
        }
        if k == "src" && dom(vm).e(n, "script") {
            agendar_scripts(vm, n);
        }
        Ok(Valor::Undef)
    });
    vm.def(p, "removeAttribute", 1, |vm, this, a| {
        let n = no_de(vm, this)?;
        let k = vm.para_texto(arg(a, 0))?.to_ascii_lowercase();
        dom(vm).remover_atributo(n, &k);
        Ok(Valor::Undef)
    });
    vm.def(p, "hasAttribute", 1, |vm, this, a| {
        let n = no_de(vm, this)?;
        let k = vm.para_texto(arg(a, 0))?.to_ascii_lowercase();
        Ok(Valor::Bool(dom(vm).atributo(n, &k).is_some()))
    });
    vm.def(p, "hasAttributes", 0, |vm, this, _| {
        let n = no_de(vm, this)?;
        Ok(Valor::Bool(!dom(vm).atributos(n).is_empty()))
    });
    vm.def(p, "toggleAttribute", 1, |vm, this, a| {
        let n = no_de(vm, this)?;
        let k = vm.para_texto(arg(a, 0))?.to_ascii_lowercase();
        let tem = dom(vm).atributo(n, &k).is_some();
        let quer = if arg(a, 1) == Valor::Undef { !tem } else { vm.para_bool(a[1]) };
        if quer && !tem {
            dom(vm).definir_atributo(n, &k, "");
        } else if !quer && tem {
            dom(vm).remover_atributo(n, &k);
        }
        Ok(Valor::Bool(quer))
    });
    vm.def(p, "getAttributeNames", 0, |vm, this, _| {
        let n = no_de(vm, this)?;
        let l: Vec<String> = dom(vm).atributos(n).iter().map(|x| x.0.clone()).collect();
        let v = l.iter().map(|s| vm.txt(s)).collect();
        Ok(vm.novo_array(v))
    });
    vm.def_acessor(p, "attributes", Some(|vm, this, _| {
        let n = no_de(vm, this)?;
        let l: Vec<(String, String)> = dom(vm).atributos(n).to_vec();
        let mut v = Vec::with_capacity(l.len());
        for (a, b) in &l {
            let o = vm.novo_objeto();
            let (x, y) = (vm.txt(a), vm.txt(b));
            vm.def_texto(o, "name", x);
            vm.def_texto(o, "value", y);
            v.push(Valor::Obj(o));
        }
        let arr = vm.novo_array(v);
        for (i, (a, _)) in l.iter().enumerate() {
            let k = vm.n.atomo(a);
            let item = vm.obter_elem(arr, Valor::Int(i as i32))?;
            if let Valor::Obj(id) = arr {
                vm.def_valor(id, k, item, P_OCULTA);
            }
        }
        Ok(arr)
    }), None);
    vm.def_acessor(p, "innerHTML", Some(|vm, this, _| {
        let n = no_de(vm, this)?;
        let mut s = String::new();
        dom(vm).serializar_filhos(n, &mut s);
        Ok(vm.txt(&s))
    }), Some(|vm, this, a| {
        let n = no_de(vm, this)?;
        let t = if arg(a, 0) == Valor::Nulo { Rc::from("") } else { vm.para_texto(arg(a, 0))? };
        dom(vm).definir_html(n, &t);
        let f = dom(vm).filhos(n).to_vec();
        marcar_scripts_inertes(vm, &f);
        Ok(Valor::Undef)
    }));
    vm.def_acessor(p, "outerHTML", Some(|vm, this, _| {
        let n = no_de(vm, this)?;
        let mut s = String::new();
        dom(vm).serializar(n, &mut s);
        Ok(vm.txt(&s))
    }), Some(|vm, this, a| {
        let n = no_de(vm, this)?;
        let t = vm.para_texto(arg(a, 0))?;
        let q = dom(vm).pai(n);
        if q != NADA {
            let novos = dom(vm).ler_fragmento(&t);
            marcar_scripts_inertes(vm, &novos);
            for f in novos {
                dom(vm).inserir(q, f, n);
            }
            dom(vm).soltar(n);
        }
        Ok(Valor::Undef)
    }));
    vm.def(p, "insertAdjacentHTML", 2, |vm, this, a| {
        let n = no_de(vm, this)?;
        let onde = vm.para_texto(arg(a, 0))?.to_ascii_lowercase();
        let t = vm.para_texto(arg(a, 1))?;
        let novos = dom(vm).ler_fragmento(&t);
        marcar_scripts_inertes(vm, &novos);
        inserir_adjacente(vm, n, &onde, &novos);
        Ok(Valor::Undef)
    });
    vm.def(p, "insertAdjacentElement", 2, |vm, this, a| {
        let n = no_de(vm, this)?;
        let onde = vm.para_texto(arg(a, 0))?.to_ascii_lowercase();
        let f = no_de(vm, arg(a, 1))?;
        inserir_adjacente(vm, n, &onde, &[f]);
        Ok(arg(a, 1))
    });
    vm.def(p, "insertAdjacentText", 2, |vm, this, a| {
        let n = no_de(vm, this)?;
        let onde = vm.para_texto(arg(a, 0))?.to_ascii_lowercase();
        let t = vm.para_texto(arg(a, 1))?;
        let f = dom(vm).criar_texto(&t);
        inserir_adjacente(vm, n, &onde, &[f]);
        Ok(Valor::Undef)
    });
    vm.def(p, "matches", 1, |vm, this, a| {
        let n = no_de(vm, this)?;
        let t = vm.para_texto(arg(a, 0))?;
        let l = seletores(vm, &t)?;
        let d: &Dom = dom(vm);
        Ok(Valor::Bool(css::casa_seletores(d, host(vm).estado_css(), &l, n)))
    });
    vm.def(p, "closest", 1, |vm, this, a| {
        let mut n = no_de(vm, this)?;
        let t = vm.para_texto(arg(a, 0))?;
        let l = seletores(vm, &t)?;
        let d: &Dom = dom(vm);
        while n != NADA && d.e_elemento(n) {
            if css::casa_seletores(d, host(vm).estado_css(), &l, n) {
                return Ok(embrulhar(vm, n));
            }
            n = d.pai(n);
        }
        Ok(Valor::Nulo)
    });
    // A geometria.
    fn retangulo_js(vm: &mut Vm, this: Valor) -> Result<(i32, i32, i32, i32), Valor> {
        let n = no_de(vm, this)?;
        let r = host(vm).retangulo(n).unwrap_or((0, 0, 0, 0));
        Ok(r)
    }
    vm.def(p, "getBoundingClientRect", 0, |vm, this, _| {
        let (x, y, w, h) = retangulo_js(vm, this)?;
        let sy = host(vm).rolagem();
        let o = vm.novo_objeto();
        let y = y - sy;
        for (k, v) in [("x", x), ("y", y), ("left", x), ("top", y), ("width", w), ("height", h), ("right", x + w), ("bottom", y + h)] {
            vm.def_texto(o, k, Valor::Int(v));
        }
        Ok(Valor::Obj(o))
    });
    vm.def(p, "getClientRects", 0, |vm, this, _| {
        let (x, y, w, h) = retangulo_js(vm, this)?;
        let sy = host(vm).rolagem();
        let o = vm.novo_objeto();
        let y = y - sy;
        for (k, v) in [("x", x), ("y", y), ("left", x), ("top", y), ("width", w), ("height", h), ("right", x + w), ("bottom", y + h)] {
            vm.def_texto(o, k, Valor::Int(v));
        }
        Ok(vm.novo_array(vec![Valor::Obj(o)]))
    });
    macro_rules! medida {
        ($nome:literal, $f:expr) => {
            vm.def_acessor(p, $nome, Some(|vm, this, _| {
                let r = retangulo_js(vm, this)?;
                let f: fn((i32, i32, i32, i32)) -> i32 = $f;
                Ok(Valor::Int(f(r)))
            }), None);
        };
    }
    medida!("offsetWidth", |r| r.2);
    medida!("offsetHeight", |r| r.3);
    medida!("offsetLeft", |r| r.0);
    medida!("offsetTop", |r| r.1);
    medida!("clientWidth", |r| r.2);
    medida!("clientHeight", |r| r.3);
    medida!("scrollWidth", |r| r.2);
    vm.def_acessor(p, "scrollHeight", Some(|vm, this, _| {
        let n = no_de(vm, this)?;
        if dom(vm).e(n, "html") || dom(vm).e(n, "body") {
            return Ok(Valor::Int(host(vm).altura_documento()));
        }
        Ok(Valor::Int(retangulo_js(vm, this)?.3))
    }), None);
    vm.def_acessor(p, "clientLeft", Some(|_, _, _| Ok(Valor::Int(0))), None);
    vm.def_acessor(p, "clientTop", Some(|_, _, _| Ok(Valor::Int(0))), None);
    vm.def_acessor(p, "scrollLeft", Some(|_, _, _| Ok(Valor::Int(0))), Some(|_, _, _| Ok(Valor::Undef)));
    vm.def_acessor(p, "scrollTop", Some(|vm, this, _| {
        let n = no_de(vm, this)?;
        Ok(Valor::Int(if dom(vm).e(n, "html") || dom(vm).e(n, "body") { host(vm).rolagem() } else { 0 }))
    }), Some(|vm, this, a| {
        let n = no_de(vm, this)?;
        if dom(vm).e(n, "html") || dom(vm).e(n, "body") {
            let y = vm.para_numero(arg(a, 0))?;
            host(vm).rolar_para(y as i32);
        }
        Ok(Valor::Undef)
    }));
    vm.def_acessor(p, "offsetParent", Some(|vm, this, _| {
        let n = no_de(vm, this)?;
        let d = dom(vm);
        Ok(if d.contem(0, n) && n != d.body() { embrulhar(vm, d.body()) } else { Valor::Nulo })
    }), None);
    vm.def(p, "scrollIntoView", 0, |vm, this, _| {
        let (_, y, _, _) = retangulo_js(vm, this)?;
        host(vm).rolar_para(y);
        Ok(Valor::Undef)
    });
    for n in ["scrollTo", "scrollBy", "scroll", "animate", "setPointerCapture", "releasePointerCapture", "attachShadow"] {
        vm.def(p, n, 0, |_, _, _| Ok(Valor::Undef));
    }
    vm.def_acessor(p, "style", Some(|vm, this, _| {
        let n = no_de(vm, this)?;
        let Valor::Obj(o) = this else { return Ok(Valor::Undef) };
        let k = vm.atomo("__estilo");
        if let Some(v) = vm.prop_simples(o, k) {
            return Ok(v);
        }
        let proto = ctx(vm).protos["CSSStyleDeclaration"];
        let e = vm.novo_obj_com(proto, Tipo::Hospede { classe: CL_ESTILO, id: n });
        vm.def_valor(o, k, Valor::Obj(e), 0);
        Ok(Valor::Obj(e))
    }), None);
    vm.def_acessor(p, "dataset", Some(|vm, this, _| {
        let n = no_de(vm, this)?;
        let Valor::Obj(o) = this else { return Ok(Valor::Undef) };
        let k = vm.atomo("__dataset");
        if let Some(v) = vm.prop_simples(o, k) {
            return Ok(v);
        }
        let po = vm.p.objeto;
        let e = vm.novo_obj_com(po, Tipo::Hospede { classe: CL_DATASET, id: n });
        vm.def_valor(o, k, Valor::Obj(e), 0);
        Ok(Valor::Obj(e))
    }), None);
    // classList vem do prelúdio (usa `className`).
}

fn inserir_adjacente(vm: &mut Vm, n: NoId, onde: &str, novos: &[NoId]) {
    let d = dom(vm);
    let q = d.pai(n);
    let (pai, antes) = match onde {
        "beforebegin" => (q, n),
        "afterbegin" => (n, d.filhos(n).first().copied().unwrap_or(NADA)),
        "beforeend" => (n, NADA),
        _ => {
            let i = if q != NADA { d.filhos(q).iter().position(|c| *c == n) } else { None };
            (q, i.and_then(|i| d.filhos(q).get(i + 1).copied()).unwrap_or(NADA))
        }
    };
    if pai == NADA {
        return;
    }
    for f in novos {
        d.inserir(pai, *f, antes);
        agendar_scripts(vm, *f);
    }
}

fn instalar_html(vm: &mut Vm, p: ObjId) {
    refletir(vm, p, "title", "title", 0);
    refletir(vm, p, "lang", "lang", 0);
    refletir(vm, p, "dir", "dir", 0);
    refletir(vm, p, "hidden", "hidden", 1);
    refletir(vm, p, "accessKey", "accesskey", 0);
    vm.def_acessor(p, "tabIndex", Some(|vm, this, _| {
        let n = no_de(vm, this)?;
        Ok(Valor::Int(dom(vm).atributo(n, "tabindex").and_then(|v| v.parse().ok()).unwrap_or(-1)))
    }), Some(|vm, this, a| {
        let n = no_de(vm, this)?;
        let v = vm.para_texto(arg(a, 0))?;
        dom(vm).definir_atributo(n, "tabindex", &v);
        Ok(Valor::Undef)
    }));
    fn texto_visivel(vm: &mut Vm, this: Valor, _: &[Valor]) -> Res {
        let n = no_de(vm, this)?;
        let t = dom(vm).texto(n);
        Ok(vm.txt(&t))
    }
    fn definir_texto_visivel(vm: &mut Vm, this: Valor, a: &[Valor]) -> Res {
        let n = no_de(vm, this)?;
        let t = vm.para_texto(arg(a, 0))?;
        dom(vm).definir_texto(n, &t);
        Ok(Valor::Undef)
    }
    vm.def_acessor(p, "innerText", Some(texto_visivel), Some(definir_texto_visivel));
    vm.def_acessor(p, "outerText", Some(texto_visivel), Some(definir_texto_visivel));
    vm.def(p, "focus", 0, |vm, this, _| {
        let n = no_de(vm, this)?;
        host(vm).focar(Some(n));
        Ok(Valor::Undef)
    });
    vm.def(p, "blur", 0, |vm, this, _| {
        no_de(vm, this)?;
        host(vm).focar(None);
        Ok(Valor::Undef)
    });
    vm.def(p, "click", 0, |vm, this, _| {
        let n = no_de(vm, this)?;
        let f = ctx(vm).fabrica_evento;
        let t = vm.txt("click");
        let ini = vm.novo_objeto();
        vm.def_texto(ini, "bubbles", Valor::Bool(true));
        vm.def_texto(ini, "cancelable", Valor::Bool(true));
        let ev = vm.construir(f, &[t, Valor::Obj(ini)], f)?;
        let Valor::Obj(o) = this else { return Ok(Valor::Undef) };
        if despachar(vm, o, ev)? {
            host(vm).acao_clique(n);
        }
        Ok(Valor::Undef)
    });
}

fn instalar_formularios(vm: &mut Vm) {
    let p = |vm: &Vm, n: &str| ctx(vm).protos[n];
    let pi = p(vm, "HTMLInputElement");
    let pt = p(vm, "HTMLTextAreaElement");
    let ps = p(vm, "HTMLSelectElement");
    let pb = p(vm, "HTMLButtonElement");
    let po = p(vm, "HTMLOptionElement");
    let pf = p(vm, "HTMLFormElement");
    fn ler_valor(vm: &mut Vm, this: Valor, _: &[Valor]) -> Res {
        let n = no_de(vm, this)?;
        let t = valor_de_elemento(vm, n);
        Ok(vm.txt(&t))
    }
    fn mudar_valor(vm: &mut Vm, this: Valor, a: &[Valor]) -> Res {
        let n = no_de(vm, this)?;
        let t = if arg(a, 0).nulo_ou_undef() { Rc::from("") } else { vm.para_texto(arg(a, 0))? };
        host(vm).definir_valor_campo(n, &t);
        Ok(Valor::Undef)
    }
    for q in [pi, pt, ps, pb] {
        vm.def_acessor(q, "value", Some(ler_valor), Some(mudar_valor));
        refletir(vm, q, "name", "name", 0);
        refletir(vm, q, "disabled", "disabled", 1);
        refletir(vm, q, "autofocus", "autofocus", 1);
        vm.def_acessor(q, "form", Some(|vm, this, _| {
            let mut n = no_de(vm, this)?;
            let d = dom(vm);
            if let Some(i) = d.atributo(n, "form").map(str::to_string) {
                return Ok(embrulhar(vm, d.por_id(0, &i).unwrap_or(NADA)));
            }
            while n != NADA {
                if d.e(n, "form") {
                    return Ok(embrulhar(vm, n));
                }
                n = d.pai(n);
            }
            Ok(Valor::Nulo)
        }), None);
        vm.def(q, "checkValidity", 0, |_, _, _| Ok(Valor::Bool(true)));
        vm.def(q, "reportValidity", 0, |_, _, _| Ok(Valor::Bool(true)));
        vm.def(q, "setCustomValidity", 1, |_, _, _| Ok(Valor::Undef));
    }
    for q in [pi, pt] {
        for (a, b, t) in [("placeholder", "placeholder", 0), ("required", "required", 1), ("readOnly", "readonly", 1), ("maxLength", "maxlength", 3), ("minLength", "minlength", 3), ("autocomplete", "autocomplete", 0)] {
            refletir(vm, q, a, b, t);
        }
        vm.def(q, "select", 0, |_, _, _| Ok(Valor::Undef));
        vm.def(q, "setSelectionRange", 2, |_, _, _| Ok(Valor::Undef));
        vm.def_acessor(q, "selectionStart", Some(|vm, this, _| {
            let n = no_de(vm, this)?;
            let t = valor_de_elemento(vm, n);
            Ok(num(t.encode_utf16().count() as f64))
        }), Some(|_, _, _| Ok(Valor::Undef)));
        vm.def_acessor(q, "selectionEnd", Some(|vm, this, _| {
            let n = no_de(vm, this)?;
            let t = valor_de_elemento(vm, n);
            Ok(num(t.encode_utf16().count() as f64))
        }), Some(|_, _, _| Ok(Valor::Undef)));
    }
    for (a, b, t) in [("min", "min", 0), ("max", "max", 0), ("step", "step", 0), ("pattern", "pattern", 0), ("size", "size", 3), ("accept", "accept", 0), ("multiple", "multiple", 1), ("src", "src", 2), ("alt", "alt", 0)] {
        refletir(vm, pi, a, b, t);
    }
    vm.def_acessor(pi, "type", Some(|vm, this, _| {
        let n = no_de(vm, this)?;
        let t = dom(vm).atributo(n, "type").unwrap_or("text").to_ascii_lowercase();
        Ok(vm.txt(&t))
    }), Some(|vm, this, a| {
        let n = no_de(vm, this)?;
        let t = vm.para_texto(arg(a, 0))?;
        dom(vm).definir_atributo(n, "type", &t);
        Ok(Valor::Undef)
    }));
    vm.def_acessor(pi, "defaultValue", Some(|vm, this, _| {
        let n = no_de(vm, this)?;
        let t = dom(vm).atributo(n, "value").unwrap_or("").to_string();
        Ok(vm.txt(&t))
    }), Some(|vm, this, a| {
        let n = no_de(vm, this)?;
        let t = vm.para_texto(arg(a, 0))?;
        dom(vm).definir_atributo(n, "value", &t);
        Ok(Valor::Undef)
    }));
    vm.def_acessor(pi, "checked", Some(|vm, this, _| {
        let n = no_de(vm, this)?;
        Ok(Valor::Bool(host(vm).marcado(n)))
    }), Some(|vm, this, a| {
        let n = no_de(vm, this)?;
        let b = vm.para_bool(arg(a, 0));
        host(vm).definir_marcado(n, b);
        Ok(Valor::Undef)
    }));
    refletir(vm, pi, "defaultChecked", "checked", 1);
    vm.def_acessor(pb, "type", Some(|vm, this, _| {
        let n = no_de(vm, this)?;
        let t = dom(vm).atributo(n, "type").unwrap_or("").to_ascii_lowercase();
        Ok(vm.txt(if matches!(t.as_str(), "button" | "reset") { &t } else { "submit" }))
    }), Some(|vm, this, a| {
        let n = no_de(vm, this)?;
        let t = vm.para_texto(arg(a, 0))?;
        dom(vm).definir_atributo(n, "type", &t);
        Ok(Valor::Undef)
    }));
    refletir(vm, pt, "rows", "rows", 3);
    refletir(vm, pt, "cols", "cols", 3);
    vm.def_acessor(pt, "defaultValue", Some(|vm, this, _| {
        let n = no_de(vm, this)?;
        let t = dom(vm).texto(n);
        Ok(vm.txt(&t))
    }), Some(|vm, this, a| {
        let n = no_de(vm, this)?;
        let t = vm.para_texto(arg(a, 0))?;
        dom(vm).definir_texto(n, &t);
        Ok(Valor::Undef)
    }));
    vm.def_acessor(pt, "type", Some(|vm, _, _| Ok(vm.txt("textarea"))), None);
    refletir(vm, ps, "multiple", "multiple", 1);
    refletir(vm, ps, "required", "required", 1);
    vm.def_acessor(ps, "type", Some(|vm, _, _| Ok(vm.txt("select-one"))), None);
    vm.def_acessor(ps, "selectedIndex", Some(|vm, this, _| {
        let n = no_de(vm, this)?;
        Ok(Valor::Int(host(vm).indice_selecionado(n)))
    }), Some(|vm, this, a| {
        let n = no_de(vm, this)?;
        let i = vm.para_i32(arg(a, 0))?;
        host(vm).definir_indice(n, i);
        Ok(Valor::Undef)
    }));
    vm.def_acessor(ps, "options", Some(|vm, this, _| {
        let n = no_de(vm, this)?;
        let mut s = Vec::new();
        dom(vm).todos(n, "option", &mut s);
        Ok(lista_de_nos(vm, &s))
    }), None);
    vm.def_acessor(ps, "length", Some(|vm, this, _| {
        let n = no_de(vm, this)?;
        let mut s = Vec::new();
        dom(vm).todos(n, "option", &mut s);
        Ok(num(s.len() as f64))
    }), None);
    vm.def_acessor(po, "value", Some(|vm, this, _| {
        let n = no_de(vm, this)?;
        let d = dom(vm);
        let t = d.atributo(n, "value").map(str::to_string).unwrap_or_else(|| d.texto(n).split_whitespace().collect::<Vec<_>>().join(" "));
        Ok(vm.txt(&t))
    }), Some(|vm, this, a| {
        let n = no_de(vm, this)?;
        let t = vm.para_texto(arg(a, 0))?;
        dom(vm).definir_atributo(n, "value", &t);
        Ok(Valor::Undef)
    }));
    vm.def_acessor(po, "text", Some(|vm, this, _| {
        let n = no_de(vm, this)?;
        let t = dom(vm).texto(n).split_whitespace().collect::<Vec<_>>().join(" ");
        Ok(vm.txt(&t))
    }), Some(|vm, this, a| {
        let n = no_de(vm, this)?;
        let t = vm.para_texto(arg(a, 0))?;
        dom(vm).definir_texto(n, &t);
        Ok(Valor::Undef)
    }));
    refletir(vm, po, "disabled", "disabled", 1);
    refletir(vm, po, "defaultSelected", "selected", 1);
    vm.def_acessor(po, "selected", Some(|vm, this, _| {
        let n = no_de(vm, this)?;
        let d = dom(vm);
        let mut s = n;
        while s != NADA && !d.e(s, "select") {
            s = d.pai(s);
        }
        if s == NADA {
            return Ok(Valor::Bool(d.atributo(n, "selected").is_some()));
        }
        let mut l = Vec::new();
        d.todos(s, "option", &mut l);
        let i = host(vm).indice_selecionado(s);
        Ok(Valor::Bool(l.get(i.max(0) as usize) == Some(&n)))
    }), Some(|vm, this, a| {
        let n = no_de(vm, this)?;
        let d = dom(vm);
        let mut s = n;
        while s != NADA && !d.e(s, "select") {
            s = d.pai(s);
        }
        if s != NADA && vm.para_bool(arg(a, 0)) {
            let mut l = Vec::new();
            d.todos(s, "option", &mut l);
            if let Some(i) = l.iter().position(|x| *x == n) {
                host(vm).definir_indice(s, i as i32);
            }
        }
        Ok(Valor::Undef)
    }));
    for (a, b, t) in [("name", "name", 0), ("method", "method", 0), ("action", "action", 2), ("target", "target", 0), ("enctype", "enctype", 0), ("noValidate", "novalidate", 1)] {
        refletir(vm, pf, a, b, t);
    }
    fn campos_do_form(vm: &mut Vm, n: NoId) -> Vec<NoId> {
        let d = dom(vm);
        let mut t = Vec::new();
        d.todos(n, "*", &mut t);
        t.into_iter().filter(|x| matches!(d.nome(*x), "input" | "select" | "textarea" | "button" | "fieldset" | "object" | "output")).collect()
    }
    vm.def_acessor(pf, "elements", Some(|vm, this, _| {
        let n = no_de(vm, this)?;
        let s = campos_do_form(vm, n);
        let a = lista_de_nos(vm, &s);
        for x in s {
            if let Some(nome) = dom(vm).atributo(x, "name").map(str::to_string) {
                let k = vm.n.atomo(&nome);
                let e = embrulhar(vm, x);
                if let Valor::Obj(id) = a {
                    vm.def_valor(id, k, e, P_OCULTA);
                }
            }
        }
        Ok(a)
    }), None);
    vm.def_acessor(pf, "length", Some(|vm, this, _| {
        let n = no_de(vm, this)?;
        Ok(num(campos_do_form(vm, n).len() as f64))
    }), None);
    vm.def(pf, "submit", 0, |vm, this, _| {
        let n = no_de(vm, this)?;
        host(vm).enviar_formulario(n);
        Ok(Valor::Undef)
    });
    vm.def(pf, "requestSubmit", 0, |vm, this, _| {
        let n = no_de(vm, this)?;
        host(vm).enviar_formulario(n);
        Ok(Valor::Undef)
    });
    vm.def(pf, "reset", 0, |vm, this, _| {
        let n = no_de(vm, this)?;
        host(vm).limpar_formulario(n);
        Ok(Valor::Undef)
    });
    vm.def(pf, "checkValidity", 0, |_, _, _| Ok(Valor::Bool(true)));
    vm.def(pf, "reportValidity", 0, |_, _, _| Ok(Valor::Bool(true)));
    // Outros elementos com atributos próprios.
    let pa = p(vm, "HTMLAnchorElement");
    for (a, b, t) in [("href", "href", 2), ("target", "target", 0), ("rel", "rel", 0), ("download", "download", 0), ("hreflang", "hreflang", 0), ("type", "type", 0), ("name", "name", 0)] {
        refletir(vm, pa, a, b, t);
    }
    let pg = p(vm, "HTMLImageElement");
    for (a, b, t) in [("src", "src", 2), ("alt", "alt", 0), ("srcset", "srcset", 0), ("sizes", "sizes", 0), ("loading", "loading", 0), ("decoding", "decoding", 0), ("crossOrigin", "crossorigin", 0), ("useMap", "usemap", 0)] {
        refletir(vm, pg, a, b, t);
    }
    fn tamanho_img(vm: &mut Vm, this: Valor, natural: bool) -> Result<(i32, i32), Valor> {
        let n = no_de(vm, this)?;
        let src = dom(vm).atributo(n, "src").unwrap_or("").to_string();
        let u = resolver_url(vm, &src);
        let nat = host(vm).tamanho_imagem(&u).unwrap_or((0, 0));
        if natural {
            return Ok(nat);
        }
        let d = dom(vm);
        let w = d.atributo(n, "width").and_then(|v| v.parse().ok());
        let h = d.atributo(n, "height").and_then(|v| v.parse().ok());
        if w.is_none() && h.is_none() {
            if let Some(r) = host(vm).retangulo(n) {
                if r.2 > 0 {
                    return Ok((r.2, r.3));
                }
            }
        }
        Ok((w.unwrap_or(nat.0), h.unwrap_or(nat.1)))
    }
    vm.def_acessor(pg, "naturalWidth", Some(|vm, this, _| Ok(Valor::Int(tamanho_img(vm, this, true)?.0))), None);
    vm.def_acessor(pg, "naturalHeight", Some(|vm, this, _| Ok(Valor::Int(tamanho_img(vm, this, true)?.1))), None);
    vm.def_acessor(pg, "width", Some(|vm, this, _| Ok(Valor::Int(tamanho_img(vm, this, false)?.0))), Some(|vm, this, a| {
        let n = no_de(vm, this)?;
        let t = vm.para_texto(arg(a, 0))?;
        dom(vm).definir_atributo(n, "width", &t);
        Ok(Valor::Undef)
    }));
    vm.def_acessor(pg, "height", Some(|vm, this, _| Ok(Valor::Int(tamanho_img(vm, this, false)?.1))), Some(|vm, this, a| {
        let n = no_de(vm, this)?;
        let t = vm.para_texto(arg(a, 0))?;
        dom(vm).definir_atributo(n, "height", &t);
        Ok(Valor::Undef)
    }));
    vm.def_acessor(pg, "complete", Some(|vm, this, _| {
        let n = no_de(vm, this)?;
        let src = dom(vm).atributo(n, "src").unwrap_or("").to_string();
        if src.is_empty() {
            return Ok(Valor::Bool(true));
        }
        let u = resolver_url(vm, &src);
        Ok(Valor::Bool(host(vm).tamanho_imagem(&u).is_some()))
    }), None);
    vm.def_acessor(pg, "currentSrc", Some(|vm, this, _| {
        let n = no_de(vm, this)?;
        let src = dom(vm).atributo(n, "src").unwrap_or("").to_string();
        let u = resolver_url(vm, &src);
        Ok(vm.txt(&u))
    }), None);
    let ps2 = p(vm, "HTMLScriptElement");
    for (a, b, t) in [("src", "src", 2), ("type", "type", 0), ("async", "async", 1), ("defer", "defer", 1), ("crossOrigin", "crossorigin", 0), ("integrity", "integrity", 0), ("noModule", "nomodule", 1)] {
        refletir(vm, ps2, a, b, t);
    }
    vm.def_acessor(ps2, "text", Some(|vm, this, _| {
        let n = no_de(vm, this)?;
        let t = dom(vm).texto(n);
        Ok(vm.txt(&t))
    }), Some(|vm, this, a| {
        let n = no_de(vm, this)?;
        let t = vm.para_texto(arg(a, 0))?;
        dom(vm).definir_texto(n, &t);
        Ok(Valor::Undef)
    }));
    let pl = p(vm, "HTMLLinkElement");
    for (a, b, t) in [("rel", "rel", 0), ("href", "href", 2), ("media", "media", 0), ("type", "type", 0), ("as", "as", 0), ("crossOrigin", "crossorigin", 0)] {
        refletir(vm, pl, a, b, t);
    }
    let pm = p(vm, "HTMLMetaElement");
    for (a, b, t) in [("name", "name", 0), ("content", "content", 0), ("httpEquiv", "http-equiv", 0), ("charset", "charset", 0)] {
        refletir(vm, pm, a, b, t);
    }
    let py = p(vm, "HTMLStyleElement");
    refletir(vm, py, "media", "media", 0);
    refletir(vm, py, "type", "type", 0);
    let pif = p(vm, "HTMLIFrameElement");
    for (a, b, t) in [("src", "src", 2), ("name", "name", 0), ("width", "width", 0), ("height", "height", 0), ("sandbox", "sandbox", 0), ("allow", "allow", 0)] {
        refletir(vm, pif, a, b, t);
    }
    vm.def_acessor(pif, "contentWindow", Some(|_, _, _| Ok(Valor::Nulo)), None);
    vm.def_acessor(pif, "contentDocument", Some(|_, _, _| Ok(Valor::Nulo)), None);
    let pla = p(vm, "HTMLLabelElement");
    refletir(vm, pla, "htmlFor", "for", 0);
    let pc = p(vm, "HTMLTableCellElement");
    refletir(vm, pc, "colSpan", "colspan", 4);
    refletir(vm, pc, "rowSpan", "rowspan", 4);
    let pcv = p(vm, "HTMLCanvasElement");
    refletir(vm, pcv, "width", "width", 3);
    refletir(vm, pcv, "height", "height", 3);
    vm.def(pcv, "getContext", 1, |_, _, _| Ok(Valor::Nulo));
    vm.def(pcv, "toDataURL", 0, |vm, _, _| Ok(vm.txt("data:,")));
    let pol = p(vm, "HTMLOListElement");
    refletir(vm, pol, "start", "start", 3);
    let pli = p(vm, "HTMLLIElement");
    refletir(vm, pli, "value", "value", 3);
    let pmd = p(vm, "HTMLMediaElement");
    refletir(vm, pmd, "src", "src", 2);
    for n in ["play", "pause", "load"] {
        vm.def(pmd, n, 0, |vm, _, _| {
            let f = crate::js::nativos2::nova_promessa(vm);
            Ok(f)
        });
    }
    vm.def(pmd, "canPlayType", 1, |vm, _, _| Ok(Valor::Texto(vm.n.atomo(""))));
}

fn instalar_documento(vm: &mut Vm, p: ObjId) {
    instalar_consulta(vm, p);
    macro_rules! no_fixo {
        ($nome:literal, $f:expr) => {
            vm.def_acessor(p, $nome, Some(|vm, _, _| {
                let f: fn(&Dom) -> NoId = $f;
                Ok(embrulhar(vm, f(dom(vm))))
            }), None);
        };
    }
    no_fixo!("documentElement", |d| d.html());
    no_fixo!("head", |d| d.head());
    no_fixo!("scrollingElement", |d| d.html());
    vm.def_acessor(p, "body", Some(|vm, _, _| Ok(embrulhar(vm, dom(vm).body()))), Some(|_, _, _| Ok(Valor::Undef)));
    vm.def_acessor(p, "title", Some(|vm, _, _| {
        let t = dom(vm).titulo();
        Ok(vm.txt(&t))
    }), Some(|vm, _, a| {
        let t = vm.para_texto(arg(a, 0))?;
        let d = dom(vm);
        let n = match d.achar(0, "title") {
            Some(n) => n,
            None => {
                let n = d.criar_elemento("title");
                d.inserir(d.head(), n, NADA);
                n
            }
        };
        d.definir_texto(n, &t);
        host(vm).mudou_titulo();
        Ok(Valor::Undef)
    }));
    vm.def(p, "getElementById", 1, |vm, _, a| {
        let t = vm.para_texto(arg(a, 0))?;
        Ok(embrulhar(vm, dom(vm).por_id(0, &t).unwrap_or(NADA)))
    });
    vm.def(p, "getElementsByName", 1, |vm, _, a| {
        let t = vm.para_texto(arg(a, 0))?;
        let d = dom(vm);
        let mut todos = Vec::new();
        d.todos(0, "*", &mut todos);
        let s: Vec<NoId> = todos.into_iter().filter(|x| d.atributo(*x, "name") == Some(&*t)).collect();
        Ok(lista_de_nos(vm, &s))
    });
    vm.def(p, "createElement", 1, |vm, _, a| {
        let t = vm.para_texto(arg(a, 0))?;
        let n = dom(vm).criar_elemento(&t);
        Ok(embrulhar(vm, n))
    });
    vm.def(p, "createElementNS", 2, |vm, _, a| {
        let t = vm.para_texto(arg(a, 1))?;
        let n = dom(vm).criar_elemento(&t);
        Ok(embrulhar(vm, n))
    });
    vm.def(p, "createTextNode", 1, |vm, _, a| {
        let t = vm.para_texto(arg(a, 0))?;
        let n = dom(vm).criar_texto(&t);
        Ok(embrulhar(vm, n))
    });
    vm.def(p, "createComment", 1, |vm, _, a| {
        let t = vm.para_texto(arg(a, 0))?;
        let n = dom(vm).criar_comentario(&t);
        Ok(embrulhar(vm, n))
    });
    vm.def(p, "createDocumentFragment", 0, |vm, _, _| {
        let n = dom(vm).criar_fragmento();
        Ok(embrulhar(vm, n))
    });
    vm.def(p, "importNode", 2, |vm, _, a| {
        let n = no_de(vm, arg(a, 0))?;
        let f = vm.para_bool(arg(a, 1));
        let c = dom(vm).clonar(n, f);
        Ok(embrulhar(vm, c))
    });
    vm.def(p, "adoptNode", 1, |vm, _, a| {
        let n = no_de(vm, arg(a, 0))?;
        dom(vm).soltar(n);
        Ok(arg(a, 0))
    });
    vm.def_acessor(p, "readyState", Some(|vm, _, _| {
        let t = ["loading", "interactive", "complete"][ctx(vm).pronto as usize];
        Ok(vm.txt(t))
    }), None);
    fn url_doc(vm: &mut Vm, _: Valor, _: &[Valor]) -> Res {
        let t = host(vm).url();
        Ok(vm.txt(&t))
    }
    vm.def_acessor(p, "URL", Some(url_doc), None);
    vm.def_acessor(p, "documentURI", Some(url_doc), None);
    vm.def_acessor(p, "baseURI", Some(|vm, _, _| {
        let t = base_do_documento(vm);
        Ok(vm.txt(&t))
    }), None);
    vm.def_acessor(p, "cookie", Some(|vm, _, _| {
        let t = host(vm).cookies();
        Ok(vm.txt(&t))
    }), Some(|vm, _, a| {
        let t = vm.para_texto(arg(a, 0))?;
        host(vm).definir_cookie(&t);
        Ok(Valor::Undef)
    }));
    vm.def_acessor(p, "activeElement", Some(|vm, _, _| {
        let f = host(vm).foco().unwrap_or_else(|| dom(vm).body());
        Ok(embrulhar(vm, f))
    }), None);
    vm.def_acessor(p, "currentScript", Some(|vm, _, _| {
        let n = ctx(vm).script_atual;
        Ok(embrulhar(vm, n))
    }), None);
    vm.def_acessor(p, "characterSet", Some(|vm, _, _| Ok(vm.txt("UTF-8"))), None);
    vm.def_acessor(p, "charset", Some(|vm, _, _| Ok(vm.txt("UTF-8"))), None);
    vm.def_acessor(p, "compatMode", Some(|vm, _, _| Ok(vm.txt("CSS1Compat"))), None);
    vm.def_acessor(p, "contentType", Some(|vm, _, _| Ok(vm.txt("text/html"))), None);
    vm.def_acessor(p, "visibilityState", Some(|vm, _, _| Ok(vm.txt("visible"))), None);
    vm.def_acessor(p, "hidden", Some(|_, _, _| Ok(Valor::Bool(false))), None);
    vm.def_acessor(p, "referrer", Some(|_, _, _| Ok(Valor::Texto(crate::js::lexico::K_VAZIO))), None);
    vm.def_acessor(p, "doctype", Some(|_, _, _| Ok(Valor::Nulo)), None);
    vm.def(p, "hasFocus", 0, |_, _, _| Ok(Valor::Bool(true)));
    vm.def(p, "open", 0, |_, this, _| Ok(this));
    vm.def(p, "close", 0, |_, _, _| Ok(Valor::Undef));
    fn escrever(vm: &mut Vm, a: &[Valor], fim: &str) -> Res {
        let mut s = String::new();
        for x in a {
            s.push_str(&vm.para_texto(*x)?);
        }
        s.push_str(fim);
        let d = dom(vm);
        let novos = d.ler_fragmento(&s);
        let sc = ctx(vm).script_atual;
        let (pai, antes) = if sc != NADA && d.pai(sc) != NADA {
            let q = d.pai(sc);
            let f = d.filhos(q);
            let i = f.iter().position(|c| *c == sc).unwrap_or(0);
            (q, f.get(i + 1).copied().unwrap_or(NADA))
        } else {
            (d.body(), NADA)
        };
        for f in novos {
            d.inserir(pai, f, antes);
            agendar_scripts(vm, f);
        }
        Ok(Valor::Undef)
    }
    vm.def(p, "write", 1, |vm, _, a| escrever(vm, a, ""));
    vm.def(p, "writeln", 1, |vm, _, a| escrever(vm, a, "\n"));
    fn lista_tag(vm: &mut Vm, tag: &str) -> Res {
        let mut s = Vec::new();
        dom(vm).todos(0, tag, &mut s);
        Ok(lista_de_nos(vm, &s))
    }
    vm.def_acessor(p, "forms", Some(|vm, _, _| lista_tag(vm, "form")), None);
    vm.def_acessor(p, "images", Some(|vm, _, _| lista_tag(vm, "img")), None);
    vm.def_acessor(p, "scripts", Some(|vm, _, _| lista_tag(vm, "script")), None);
    vm.def_acessor(p, "links", Some(|vm, _, _| {
        let d = dom(vm);
        let mut t = Vec::new();
        d.todos(0, "a", &mut t);
        t.retain(|x| d.atributo(*x, "href").is_some());
        Ok(lista_de_nos(vm, &t))
    }), None);
}

/// As funções nativas que o prelúdio usa.
fn instalar_nativas_prelude(vm: &mut Vm) -> ObjId {
    let n = vm.novo_objeto();
    vm.def(n, "url", 0, |vm, _, _| {
        let t = host(vm).url();
        Ok(vm.txt(&t))
    });
    vm.def(n, "navegar", 2, |vm, _, a| {
        let t = vm.para_texto(arg(a, 0))?;
        let s = vm.para_bool(arg(a, 1));
        let u = resolver_url(vm, &t);
        host(vm).navegar(&u, s);
        Ok(Valor::Undef)
    });
    vm.def(n, "mudarEndereco", 2, |vm, _, a| {
        let t = vm.para_texto(arg(a, 0))?;
        let s = vm.para_bool(arg(a, 1));
        let u = resolver_url(vm, &t);
        host(vm).mudar_endereco(&u, s);
        Ok(Valor::Undef)
    });
    vm.def(n, "resolver", 2, |vm, _, a| {
        let t = vm.para_texto(arg(a, 0))?;
        let base = if arg(a, 1).nulo_ou_undef() { base_do_documento(vm) } else { vm.para_texto(a[1])?.to_string() };
        Ok(match crate::http::Url::ler(&base).and_then(|u| u.juntar(t.trim())) {
            Some(u) => vm.txt(&u.texto()),
            None => Valor::Nulo,
        })
    });
    vm.def(n, "voltar", 1, |vm, _, a| {
        let i = vm.para_i32(arg(a, 0))?;
        host(vm).voltar(i);
        Ok(Valor::Undef)
    });
    vm.def(n, "avisar", 1, |vm, _, a| {
        let t = vm.para_texto(arg(a, 0))?;
        host(vm).avisar(&t);
        Ok(Valor::Undef)
    });
    vm.def(n, "janela", 0, |vm, _, _| {
        let (w, h) = host(vm).janela();
        Ok(vm.novo_array(vec![Valor::Int(w), Valor::Int(h)]))
    });
    vm.def(n, "rolagem", 0, |vm, _, _| Ok(Valor::Int(host(vm).rolagem())));
    vm.def(n, "rolar", 1, |vm, _, a| {
        let y = vm.para_numero(arg(a, 0))?;
        host(vm).rolar_para(y as i32);
        Ok(Valor::Undef)
    });
    vm.def(n, "midia", 1, |vm, _, a| {
        let t = vm.para_texto(arg(a, 0))?.to_ascii_lowercase();
        let (w, h) = host(vm).janela();
        Ok(Valor::Bool(css::midia_vale_tela(&t, w, h)))
    });
    vm.def(n, "lerArmazenamento", 0, |vm, _, _| {
        let t = host(vm).ler_armazenamento();
        Ok(vm.txt(&t))
    });
    vm.def(n, "gravarArmazenamento", 1, |vm, _, a| {
        let t = vm.para_texto(arg(a, 0))?;
        host(vm).gravar_armazenamento(&t);
        Ok(Valor::Undef)
    });
    vm.def(n, "estilo", 1, |vm, _, a| {
        let no = no_de(vm, arg(a, 0))?;
        let Some(e) = host(vm).estilo(no) else { return Ok(Valor::Nulo) };
        let r = host(vm).retangulo(no).unwrap_or((0, 0, 0, 0));
        estilo_para_objeto(vm, &e, r)
    });
    vm.def(n, "pedir", 5, |vm, _, a| {
        let metodo = vm.para_texto(arg(a, 0))?.to_ascii_uppercase();
        let url_t = vm.para_texto(arg(a, 1))?;
        let url = resolver_url(vm, &url_t);
        let cab = vm.lista_de_array(arg(a, 2))?;
        let mut cabecalhos = Vec::new();
        for par in cab {
            let k = vm.obter_elem(par, Valor::Int(0))?;
            let v = vm.obter_elem(par, Valor::Int(1))?;
            cabecalhos.push((vm.para_texto(k)?.to_string(), vm.para_texto(v)?.to_string()));
        }
        let corpo = match arg(a, 3) {
            Valor::Undef | Valor::Nulo => None,
            Valor::Obj(o) => match &vm.objs[o as usize].tipo {
                Tipo::Buffer(b) => Some(b.clone()),
                _ => Some(vm.para_texto(arg(a, 3))?.as_bytes().to_vec()),
            },
            o => Some(vm.para_texto(o)?.as_bytes().to_vec()),
        };
        let cb = arg(a, 4);
        let c = ctx(vm);
        c.prox_pedido += 1;
        let id = c.prox_pedido;
        c.pedidos.insert(id, cb);
        // A política de mesma origem: outro site só com pedidos simples, sem
        // credenciais, e a resposta confere o Access-Control-Allow-Origin.
        let pagina = host(vm).url();
        let mesma = mesma_origem(&pagina, &url);
        if !mesma {
            let destino = crate::http::Url::ler(&url);
            let esquema_ok = destino.as_ref().is_some_and(|u| u.esquema == "http" || u.esquema == "https");
            let simples = matches!(&*metodo, "GET" | "HEAD" | "POST")
                && cabecalhos.iter().all(|(k, v)| {
                    let k = k.to_ascii_lowercase();
                    matches!(k.as_str(), "accept" | "accept-language" | "content-language")
                        || (k == "content-type" && matches!(v.split(';').next().unwrap_or("").trim().to_ascii_lowercase().as_str(), "text/plain" | "application/x-www-form-urlencoded" | "multipart/form-data"))
                });
            if !esquema_ok || !simples {
                // (sem "preflight" por enquanto: o pedido nem sai)
                let m = if esquema_ok { "pedido entre origens que exigiria preflight (não implementado)" } else { "esquema não permitido" };
                ctx(vm).guardados.push(cb);
                let k = vm.txt(m);
                ctx(vm).guardados.pop();
                let f = ctx(vm).pedidos.remove(&id).unwrap_or(Valor::Undef);
                let nat = vm.nativa_com(crate::js::lexico::K_VAZIO, 0, |vm, _, _| {
                    let (f, m) = (vm.dado_nativa(0), vm.dado_nativa(1));
                    vm.chamar(f, Valor::Undef, &[Valor::Int(0), Valor::Texto(crate::js::lexico::K_VAZIO), Valor::Texto(crate::js::lexico::K_VAZIO), Valor::Nulo, m])
                }, [f, k], false);
                vm.microtarefas.push_back(Tarefa::Chamar(Valor::Obj(nat)));
                return Ok(Valor::Int(id as i32));
            }
        }
        host(vm).pedir(id, &metodo, &url, &cabecalhos, corpo, mesma);
        Ok(Valor::Int(id as i32))
    });
    vm.def(n, "cancelarPedido", 1, |vm, _, a| {
        let i = vm.para_i32(arg(a, 0))? as u32;
        ctx(vm).pedidos.remove(&i);
        host(vm).cancelar_pedido(i);
        Ok(Valor::Undef)
    });
    vm.def(n, "decodificar", 1, |vm, _, a| {
        let bytes = bytes_de(vm, arg(a, 0));
        let t = String::from_utf8_lossy(&bytes).into_owned();
        Ok(vm.txt(&t))
    });
    vm.def(n, "codificar", 1, |vm, _, a| {
        let t = vm.para_texto(arg(a, 0))?;
        let p = vm.p.buffer;
        Ok(Valor::Obj(vm.novo_obj_com(p, Tipo::Buffer(t.as_bytes().to_vec()))))
    });
    vm.def(n, "base64", 1, |vm, _, a| {
        let t = vm.para_texto(arg(a, 0))?;
        const AL: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
        let mut b = Vec::with_capacity(t.len());
        for c in t.chars() {
            if c as u32 > 255 {
                return Err(vm.erro_tipo("The string to be encoded contains characters outside of the Latin1 range."));
            }
            b.push(c as u8);
        }
        let mut r = String::new();
        for g in b.chunks(3) {
            let v = (g[0] as u32) << 16 | (*g.get(1).unwrap_or(&0) as u32) << 8 | *g.get(2).unwrap_or(&0) as u32;
            r.push(AL[(v >> 18 & 63) as usize] as char);
            r.push(AL[(v >> 12 & 63) as usize] as char);
            r.push(if g.len() > 1 { AL[(v >> 6 & 63) as usize] as char } else { '=' });
            r.push(if g.len() > 2 { AL[(v & 63) as usize] as char } else { '=' });
        }
        Ok(vm.txt(&r))
    });
    vm.def(n, "de64", 1, |vm, _, a| {
        let t = vm.para_texto(arg(a, 0))?;
        let mut r = String::new();
        let (mut ac, mut bits) = (0u32, 0);
        for c in t.bytes() {
            let v = match c {
                b'A'..=b'Z' => c - b'A',
                b'a'..=b'z' => c - b'a' + 26,
                b'0'..=b'9' => c - b'0' + 52,
                b'+' | b'-' => 62,
                b'/' | b'_' => 63,
                b'=' | b' ' | b'\n' | b'\r' | b'\t' => continue,
                _ => return Err(vm.erro_tipo("The string to be decoded is not correctly encoded.")),
            };
            ac = ac << 6 | v as u32;
            bits += 6;
            if bits >= 8 {
                bits -= 8;
                r.push(char::from_u32(ac >> bits & 0xFF).unwrap_or(' '));
            }
        }
        Ok(vm.txt(&r))
    });
    n
}

fn bytes_de(vm: &Vm, v: Valor) -> Vec<u8> {
    let Valor::Obj(o) = v else { return Vec::new() };
    match vm.objs[o as usize].tipo {
        Tipo::Buffer(ref b) => b.clone(),
        Tipo::Tipado { buffer, tipo, inicio, n } => {
            let t = tamanho_tipado(tipo);
            if let Tipo::Buffer(b) = &vm.objs[buffer as usize].tipo {
                let i = inicio as usize;
                b.get(i..i + n as usize * t).unwrap_or(&[]).to_vec()
            } else {
                Vec::new()
            }
        }
        _ => Vec::new(),
    }
}

fn origem(u: &str) -> Option<(String, String, u16)> {
    let v = crate::http::Url::ler(u)?;
    Some((v.esquema, v.host, v.porta))
}

pub fn mesma_origem(a: &str, b: &str) -> bool {
    match (origem(a), origem(b)) {
        (Some(x), Some(y)) => x == y,
        _ => false,
    }
}

fn cor_css(c: u32) -> String {
    let (a, r, g, b) = (c >> 24, c >> 16 & 255, c >> 8 & 255, c & 255);
    if a == 255 {
        format!("rgb({}, {}, {})", r, g, b)
    } else {
        format!("rgba({}, {}, {}, {})", r, g, b, numero_para_texto(((a * 1000 / 255) as f64) / 1000.0))
    }
}

fn tam_css(t: Tam, base: i32) -> String {
    match t {
        Tam::Auto | Tam::MinConteudo | Tam::MaxConteudo | Tam::AjustaConteudo => String::from("auto"),
        Tam::Px(v) => format!("{}px", numero_para_texto(v as f64)),
        o => format!("{}px", numero_para_texto(o.resolver(Some(base as f32)).unwrap_or(0.0) as f64)),
    }
}

/// `getComputedStyle`: os valores mais perguntados.
fn estilo_para_objeto(vm: &mut Vm, e: &Estilo, r: (i32, i32, i32, i32)) -> Res {
    use crate::css::{Display as D, Posicao as P};
    let o = vm.novo_objeto();
    let disp = match e.display {
        D::Nenhum => "none",
        D::Bloco => "block",
        D::Inline => "inline",
        D::InlineBloco => "inline-block",
        D::Flex => "flex",
        D::InlineFlex => "inline-flex",
        D::Grid => "grid",
        D::InlineGrid => "inline-grid",
        D::Tabela => "table",
        D::InlineTabela => "inline-table",
        D::GrupoLinhas => "table-row-group",
        D::Linha => "table-row",
        D::Celula => "table-cell",
        D::Legenda => "table-caption",
        D::GrupoColunas => "table-column-group",
        D::Coluna => "table-column",
        D::ItemLista => "list-item",
        D::Conteudo => "contents",
    };
    let pos = match e.posicao {
        P::Estatica => "static",
        P::Relativa => "relative",
        P::Absoluta => "absolute",
        P::Fixa => "fixed",
        P::Grudada => "sticky",
    };
    let px = |v: i32| format!("{}px", v);
    let mut lista: Vec<(&str, String)> = vec![
        ("display", disp.into()),
        ("position", pos.into()),
        ("visibility", if e.visivel { "visible" } else { "hidden" }.into()),
        ("opacity", numero_para_texto(e.opacidade as f64)),
        ("color", cor_css(e.cor)),
        ("backgroundColor", cor_css(e.fundo)),
        ("fontSize", format!("{}px", numero_para_texto(e.fonte as f64))),
        ("fontWeight", if e.negrito { "700" } else { "400" }.into()),
        ("fontStyle", if e.italico { "italic" } else { "normal" }.into()),
        ("fontFamily", if e.mono { "monospace" } else { "sans-serif" }.into()),
        ("lineHeight", if e.altura_linha == 0.0 { "normal".into() } else if e.altura_linha < 0.0 { format!("{}px", numero_para_texto(-e.altura_linha as f64)) } else { format!("{}px", numero_para_texto((e.altura_linha * e.fonte) as f64)) }),
        ("width", px(r.2)),
        ("height", px(r.3)),
        ("overflow", if e.recorta { "hidden" } else if e.overflow_rola { "auto" } else { "visible" }.into()),
        ("float", ["none", "left", "right"][e.flutua.min(2) as usize].into()),
        ("textAlign", ["start", "center", "right", "justify"][e.alinha.min(3) as usize].into()),
        ("whiteSpace", ["normal", "nowrap", "pre", "pre-wrap", "pre-line"][e.branco.min(4) as usize].into()),
        ("boxSizing", if e.caixa_borda { "border-box" } else { "content-box" }.into()),
        ("zIndex", e.z.map_or(String::from("auto"), |z| z.to_string())),
        ("top", tam_css(e.topo, r.3)),
        ("right", tam_css(e.direita, r.2)),
        ("bottom", tam_css(e.baixo, r.3)),
        ("left", tam_css(e.esquerda, r.2)),
        ("flexDirection", ["row", "row-reverse", "column", "column-reverse"][e.flex_direcao.min(3) as usize].into()),
        ("pointerEvents", if e.sem_eventos { "none" } else { "auto" }.into()),
        ("cursor", if e.cursor_mao { "pointer" } else { "auto" }.into()),
    ];
    for (i, n) in ["Top", "Right", "Bottom", "Left"].iter().enumerate() {
        lista.push((alloc::boxed::Box::leak(format!("margin{}", n).into_boxed_str()), tam_css(e.margem[i], r.2)));
        lista.push((alloc::boxed::Box::leak(format!("padding{}", n).into_boxed_str()), tam_css(e.preench[i], r.2)));
        lista.push((alloc::boxed::Box::leak(format!("border{}Width", n).into_boxed_str()), format!("{}px", numero_para_texto(e.borda[i] as f64))));
    }
    for (k, v) in lista {
        let t = vm.txt(&v);
        vm.def_texto(o, k, t);
    }
    Ok(Valor::Obj(o))
}

// -----------------------------------------------------------------------------
// A página vista de fora
// -----------------------------------------------------------------------------

/// O JavaScript de uma página.
pub struct Web {
    pub vm: Box<Vm>,
    contexto: Box<Contexto>,
}

/// Quanto um script pode rodar de uma vez (depois disso, é interrompido).
const PRAZO_SCRIPT_MS: u64 = 8000;

impl Web {
    pub fn novo(hospedeiro: &mut dyn Hospedeiro, agora: fn() -> u64) -> Web {
        let mut vm = Vm::novo();
        let host_ptr: *mut (dyn Hospedeiro + 'static) = unsafe { core::mem::transmute(hospedeiro as *mut dyn Hospedeiro) };
        let mut contexto = Box::new(Contexto {
            host: host_ptr,
            nos: BTreeMap::new(),
            protos: BTreeMap::new(),
            ouvintes: Vec::new(),
            timers: Vec::new(),
            prox_timer: 1,
            pedidos: BTreeMap::new(),
            prox_pedido: 0,
            fabrica_evento: Valor::Undef,
            script_atual: NADA,
            scripts_novos: Vec::new(),
            scripts_rodados: Vec::new(),
            seletores: BTreeMap::new(),
            erros: 0,
            imagens_esperando: Vec::new(),
            pronto: 0,
            guardados: Vec::new(),
        });
        vm.hospede = &mut *contexto as *mut Contexto as *mut u8;
        vm.ganchos.agora_ms = agora;
        vm.ganchos.obter = gancho_obter;
        vm.ganchos.definir = gancho_definir;
        vm.ganchos.apagar = gancho_apagar;
        vm.ganchos.chaves = gancho_chaves;
        vm.ganchos.raizes = raizes;
        vm.ganchos.console = |vm, nivel, t| host(vm).console(nivel, t);
        let mut w = Web { vm, contexto };
        w.instalar();
        w
    }

    fn instalar(&mut self) {
        let vm = &mut *self.vm;
        let et = nova_interface(vm, "EventTarget", None);
        // `new EventTarget()` vale; as outras interfaces, não.
        let cet = vm.obter_nome(Valor::Obj(et), "constructor").unwrap_or(Valor::Undef);
        if let Valor::Obj(c) = cet {
            if let Tipo::Nativa(d) = &mut vm.objs[c as usize].tipo {
                d.f = |vm, _, _| {
                    let nt = vm.novo_alvo;
                    let proto = vm.proto_de_novo_alvo(nt, vm.p.objeto)?;
                    Ok(Valor::Obj(vm.novo_obj_com(proto, Tipo::Comum)))
                };
            }
        }
        vm.def(et, "addEventListener", 3, |vm, this, a| {
            let f = arg(a, 1);
            if f.nulo_ou_undef() {
                return Ok(Valor::Undef);
            }
            let tipo = vm.para_texto(arg(a, 0))?;
            let (captura, uma_vez) = def_opcoes(vm, arg(a, 2))?;
            let alvo = alvo_de(vm, this);
            let c = ctx(vm);
            if !c.ouvintes.iter().any(|o| o.alvo == alvo && o.f == f && o.captura == captura && o.tipo == tipo) {
                c.ouvintes.push(Ouvinte { alvo, tipo, f, captura, uma_vez });
            }
            Ok(Valor::Undef)
        });
        vm.def(et, "removeEventListener", 3, |vm, this, a| {
            let tipo = vm.para_texto(arg(a, 0))?;
            let (captura, _) = def_opcoes(vm, arg(a, 2))?;
            let f = arg(a, 1);
            let alvo = alvo_de(vm, this);
            ctx(vm).ouvintes.retain(|o| !(o.alvo == alvo && o.f == f && o.captura == captura && o.tipo == tipo));
            Ok(Valor::Undef)
        });
        vm.def(et, "dispatchEvent", 1, |vm, this, a| {
            let alvo = alvo_de(vm, this);
            Ok(Valor::Bool(despachar(vm, alvo, arg(a, 0))?))
        });
        let g = vm.global;
        let pw = nova_interface(vm, "Window", Some("EventTarget"));
        vm.objs[g as usize].proto = pw;

        let pn = nova_interface(vm, "Node", Some("EventTarget"));
        instalar_no(vm, pn);
        let pe = nova_interface(vm, "Element", Some("Node"));
        instalar_elemento(vm, pe);
        let ph = nova_interface(vm, "HTMLElement", Some("Element"));
        instalar_html(vm, ph);
        for nome in [
            "HTMLAnchorElement", "HTMLImageElement", "HTMLInputElement", "HTMLButtonElement", "HTMLSelectElement", "HTMLTextAreaElement", "HTMLOptionElement", "HTMLFormElement",
            "HTMLScriptElement", "HTMLLinkElement", "HTMLStyleElement", "HTMLMetaElement", "HTMLIFrameElement", "HTMLLabelElement", "HTMLCanvasElement", "HTMLTableCellElement",
            "HTMLTableElement", "HTMLTableRowElement", "HTMLDivElement", "HTMLSpanElement", "HTMLParagraphElement", "HTMLBodyElement", "HTMLHtmlElement", "HTMLHeadElement",
            "HTMLUListElement", "HTMLOListElement", "HTMLLIElement", "HTMLMediaElement",
        ] {
            nova_interface(vm, nome, Some("HTMLElement"));
        }
        instalar_formularios(vm);
        let pd = nova_interface(vm, "Document", Some("Node"));
        instalar_documento(vm, pd);
        let t = vm.txt("HTMLDocument");
        let k = vm.s.to_string_tag;
        vm.def_valor(pd, k, t, P_CONFIG);
        let pf = nova_interface(vm, "DocumentFragment", Some("Node"));
        instalar_consulta(vm, pf);
        let pc = nova_interface(vm, "CharacterData", Some("Node"));
        instalar_filho(vm, pc);
        vm.def_acessor(pc, "data", Some(|vm, this, _| {
            let n = no_de(vm, this)?;
            let t = dom(vm).texto_de_no(n).map(str::to_string).unwrap_or_default();
            Ok(vm.txt(&t))
        }), Some(|vm, this, a| {
            let n = no_de(vm, this)?;
            let t = vm.para_texto(arg(a, 0))?;
            dom(vm).definir_texto(n, &t);
            Ok(Valor::Undef)
        }));
        vm.def_acessor(pc, "length", Some(|vm, this, _| {
            let n = no_de(vm, this)?;
            Ok(num(dom(vm).texto_de_no(n).unwrap_or("").encode_utf16().count() as f64))
        }), None);
        nova_interface(vm, "Text", Some("CharacterData"));
        nova_interface(vm, "Comment", Some("CharacterData"));
        // NodeList: um array com `item`.
        let pa = vm.p.array;
        let pl = vm.novo_obj_com(pa, Tipo::Comum);
        ctx(vm).protos.insert("NodeList", pl);
        vm.def(pl, "item", 1, |vm, this, a| {
            let i = vm.para_i32(arg(a, 0))?;
            let v = vm.obter_elem(this, Valor::Int(i))?;
            Ok(if v == Valor::Undef { Valor::Nulo } else { v })
        });
        vm.def(pl, "namedItem", 1, |vm, this, a| {
            let t = vm.para_texto(arg(a, 0))?;
            let l = vm.lista_de_array(this)?;
            for x in l {
                if let Some(n) = talvez_no(vm, x) {
                    if dom(vm).atributo(n, "id") == Some(&*t) || dom(vm).atributo(n, "name") == Some(&*t) {
                        return Ok(x);
                    }
                }
            }
            Ok(Valor::Nulo)
        });
        let t = vm.txt("NodeList");
        let k = vm.s.to_string_tag;
        vm.def_valor(pl, k, t, P_CONFIG);
        // `String(lista)` é "[object NodeList]", não os itens.
        let kts = vm.a.to_string;
        let po = vm.p.objeto;
        let f = vm.obter_obj(po, kts, Valor::Obj(po)).unwrap_or(Valor::Undef);
        vm.def_valor(pl, kts, f, P_OCULTA);
        for nome in ["NodeList", "HTMLCollection"] {
            let kn = vm.n.atomo(nome);
            let c = vm.nativa_com(kn, 0, |vm, _, _| Err(vm.erro_tipo("Illegal constructor")), [Valor::Undef; 2], true);
            vm.materializar(c);
            vm.def_valor(c, crate::js::lexico::K_PROTOTYPE, Valor::Obj(pl), 0);
            vm.def_valor(g, kn, Valor::Obj(c), P_OCULTA);
        }
        // CSSStyleDeclaration
        let pes = vm.novo_objeto();
        ctx(vm).protos.insert("CSSStyleDeclaration", pes);
        fn no_estilo(vm: &Vm, this: Valor) -> NoId {
            if let Valor::Obj(o) = this {
                if let Tipo::Hospede { classe: CL_ESTILO, id } = vm.objs[o as usize].tipo {
                    return id;
                }
            }
            NADA
        }
        vm.def(pes, "setProperty", 2, |vm, this, a| {
            let n = no_estilo(vm, this);
            if n != NADA {
                let (p, v) = (vm.para_texto(arg(a, 0))?, vm.para_texto(arg(a, 1))?);
                estilo_definir(vm, n, &p, &v);
            }
            Ok(Valor::Undef)
        });
        vm.def(pes, "removeProperty", 1, |vm, this, a| {
            let n = no_estilo(vm, this);
            if n != NADA {
                let p = vm.para_texto(arg(a, 0))?;
                estilo_definir(vm, n, &p, "");
            }
            Ok(Valor::Texto(crate::js::lexico::K_VAZIO))
        });
        vm.def(pes, "getPropertyValue", 1, |vm, this, a| {
            let n = no_estilo(vm, this);
            let p = vm.para_texto(arg(a, 0))?;
            let l = decls_inline(if n != NADA { dom(vm).atributo(n, "style").unwrap_or("") } else { "" });
            let v = l.iter().rev().find(|(x, _)| **x == *p).map_or("", |(_, b)| b.as_str()).to_string();
            Ok(vm.txt(&v))
        });
        vm.def(pes, "getPropertyPriority", 1, |_, _, _| Ok(Valor::Texto(crate::js::lexico::K_VAZIO)));
        vm.def_acessor(pes, "cssText", Some(|vm, this, _| {
            let n = no_estilo(vm, this);
            let l = decls_inline(if n != NADA { dom(vm).atributo(n, "style").unwrap_or("") } else { "" });
            let t = l.iter().map(|(a, b)| format!("{}: {};", a, b)).collect::<Vec<_>>().join(" ");
            Ok(vm.txt(&t))
        }), Some(|vm, this, a| {
            let n = no_estilo(vm, this);
            if n != NADA {
                let t = vm.para_texto(arg(a, 0))?;
                dom(vm).definir_atributo(n, "style", &t);
            }
            Ok(Valor::Undef)
        }));
        // Janela e temporizadores.
        vm.def(g, "setTimeout", 2, |vm, _, a| novo_timer(vm, a, false));
        vm.def(g, "setInterval", 2, |vm, _, a| novo_timer(vm, a, true));
        fn limpar_timer(vm: &mut Vm, _: Valor, a: &[Valor]) -> Res {
            let i = vm.para_numero(arg(a, 0)).unwrap_or(0.0) as u32;
            ctx(vm).timers.retain(|t| t.id != i);
            Ok(Valor::Undef)
        }
        vm.def(g, "clearTimeout", 1, limpar_timer);
        vm.def(g, "clearInterval", 1, limpar_timer);
        vm.def(g, "cancelAnimationFrame", 1, limpar_timer);
        vm.def(g, "requestAnimationFrame", 1, |vm, _, a| {
            let agora = (vm.ganchos.agora_ms)();
            let c = ctx(vm);
            c.prox_timer += 1;
            let id = c.prox_timer;
            c.timers.push(Timer { id, quando: agora + 16, f: arg(a, 0), args: Vec::new(), intervalo: None, quadro: true });
            Ok(num(id as f64))
        });
        let doc = embrulhar(vm, 0);
        vm.def_texto(g, "document", doc);
        let nativas = instalar_nativas_prelude(vm);
        // O prelúdio em JavaScript.
        let fonte = include_str!("web_prelude.js");
        let f = vm.avaliar(fonte);
        match f {
            Ok(f) => {
                let r = vm.chamar(f, Valor::Undef, &[Valor::Obj(nativas)]);
                if let Err(e) = r {
                    relatar_erro(vm, e);
                }
            }
            Err(e) => relatar_erro(vm, e),
        }
        let k = vm.n.atomo("Event");
        let ev = vm.obter_obj(g, k, Valor::Obj(g)).unwrap_or(Valor::Undef);
        ctx(vm).fabrica_evento = ev;
        let _ = K_NAME;
        let _ = K_LENGTH;
    }

    fn entrar(&mut self, host: &mut dyn Hospedeiro) {
        let p: *mut (dyn Hospedeiro + 'static) = unsafe { core::mem::transmute(host as *mut dyn Hospedeiro) };
        self.contexto.host = p;
        self.vm.hospede = &mut *self.contexto as *mut Contexto as *mut u8;
        self.vm.prazo_ms = (self.vm.ganchos.agora_ms)() + PRAZO_SCRIPT_MS;
        self.vm.abortando = false;
    }

    fn sair(&mut self) {
        rodar_microtarefas(&mut self.vm);
        self.vm.abortando = false;
        self.vm.coletar_se_preciso();
    }

    pub fn erros(&self) -> u32 {
        self.contexto.erros
    }

    /// Roda um `<script>`.
    pub fn rodar_script(&mut self, host: &mut dyn Hospedeiro, no: NoId, fonte: &str) {
        self.entrar(host);
        self.contexto.script_atual = no;
        if no != NADA && !self.contexto.scripts_rodados.contains(&no) {
            self.contexto.scripts_rodados.push(no);
        }
        let vm = &mut *self.vm;
        if let Err(e) = vm.avaliar(fonte) {
            self.contexto.erros += 1;
            relatar_erro(vm, e);
        }
        self.contexto.script_atual = NADA;
        self.sair();
    }

    /// Avalia um `javascript:`.
    pub fn avaliar(&mut self, host: &mut dyn Hospedeiro, fonte: &str) {
        self.rodar_script(host, NADA, fonte);
    }

    pub fn marcar_estado(&mut self, pronto: u8) {
        self.contexto.pronto = pronto;
    }

    /// Scripts que o JavaScript pôs no documento e ainda precisam rodar.
    pub fn tomar_scripts_novos(&mut self) -> Vec<NoId> {
        core::mem::take(&mut self.contexto.scripts_novos)
    }

    pub fn marcar_script_rodado(&mut self, no: NoId) {
        if !self.contexto.scripts_rodados.contains(&no) {
            self.contexto.scripts_rodados.push(no);
        }
    }

    fn criar_evento(&mut self, tipo: &str, borbulha: bool, cancela: bool, extra: &[(&str, Valor)]) -> Result<Valor, Valor> {
        let vm = &mut *self.vm;
        let f = self.contexto.fabrica_evento;
        let t = vm.txt(tipo);
        let ini = vm.novo_objeto();
        vm.def_texto(ini, "bubbles", Valor::Bool(borbulha));
        vm.def_texto(ini, "cancelable", Valor::Bool(cancela));
        vm.raizes.push(Valor::Obj(ini));
        let ev = vm.construir(f, &[t, Valor::Obj(ini)], f);
        vm.raizes.pop();
        let ev = ev?;
        for (k, v) in extra {
            vm.definir_nome(ev, k, *v)?;
        }
        Ok(ev)
    }

    /// Um evento num nó (ou na janela, com `alvo` = NADA). Devolve se a ação
    /// padrão continua valendo.
    pub fn disparar(&mut self, host: &mut dyn Hospedeiro, alvo: NoId, tipo: &str, borbulha: bool, cancela: bool, extra: &[(&str, f64)]) -> bool {
        self.entrar(host);
        let r = self.disparar_dentro(alvo, tipo, borbulha, cancela, extra);
        self.sair();
        r
    }

    fn disparar_dentro(&mut self, alvo: NoId, tipo: &str, borbulha: bool, cancela: bool, extra: &[(&str, f64)]) -> bool {
        let extras: Vec<(&str, Valor)> = extra.iter().map(|(k, v)| (*k, num(*v))).collect();
        let ev = match self.criar_evento(tipo, borbulha, cancela, &extras) {
            Ok(e) => e,
            Err(e) => {
                relatar_erro(&mut self.vm, e);
                return true;
            }
        };
        let vm = &mut *self.vm;
        let o = if alvo == NADA { vm.global } else { embrulhar(vm, alvo).obj().unwrap_or(vm.global) };
        match despachar(vm, o, ev) {
            Ok(v) => v,
            Err(e) => {
                relatar_erro(vm, e);
                true
            }
        }
    }

    /// Um evento de teclado.
    pub fn disparar_tecla(&mut self, host: &mut dyn Hospedeiro, alvo: NoId, tipo: &str, tecla: &str, codigo: i32) -> bool {
        self.entrar(host);
        let t = self.vm.txt(tecla);
        let extras = [("key", t), ("code", t), ("keyCode", Valor::Int(codigo)), ("which", Valor::Int(codigo))];
        let r = match self.criar_evento(tipo, true, true, &extras) {
            Ok(ev) => {
                let vm = &mut *self.vm;
                let o = if alvo == NADA { embrulhar(vm, dom(vm).body()).obj().unwrap_or(vm.global) } else { embrulhar(vm, alvo).obj().unwrap_or(vm.global) };
                despachar(vm, o, ev).unwrap_or(true)
            }
            Err(_) => true,
        };
        self.sair();
        r
    }

    /// Roda os timers vencidos. Devolve em quantos ms há o próximo.
    pub fn passo(&mut self, host: &mut dyn Hospedeiro) -> Option<u64> {
        self.entrar(host);
        let agora = (self.vm.ganchos.agora_ms)();
        let mut devidos: Vec<u32> = self.contexto.timers.iter().filter(|t| t.quando <= agora).map(|t| t.id).collect();
        devidos.sort_by_key(|i| self.contexto.timers.iter().find(|t| t.id == *i).map_or(0, |t| t.quando));
        for id in devidos {
            let Some(i) = self.contexto.timers.iter().position(|t| t.id == id) else { continue };
            let (f, args, quadro) = {
                let t = &mut self.contexto.timers[i];
                (t.f, t.args.clone(), t.quadro)
            };
            match self.contexto.timers[i].intervalo {
                Some(iv) => self.contexto.timers[i].quando = agora + iv.max(4),
                None => {
                    self.contexto.timers.remove(i);
                }
            }
            let vm = &mut *self.vm;
            vm.pilha.push(f);
            let r = if quadro {
                vm.chamar(f, Valor::Obj(vm.global), &[num(agora as f64)])
            } else if let Valor::Texto(t) = f {
                let s = vm.n.texto(t);
                vm.avaliar(&s)
            } else {
                vm.chamar(f, Valor::Obj(vm.global), &args)
            };
            vm.pilha.pop();
            if let Err(e) = r {
                self.contexto.erros += 1;
                relatar_erro(vm, e);
            }
            rodar_microtarefas(vm);
            vm.prazo_ms = (vm.ganchos.agora_ms)() + PRAZO_SCRIPT_MS;
            vm.abortando = false;
        }
        self.sair();
        let agora = (self.vm.ganchos.agora_ms)();
        self.contexto.timers.iter().map(|t| t.quando.saturating_sub(agora)).min()
    }

    /// Chegou a resposta de um pedido de rede. Sem erro, `corpo` é o conteúdo.
    #[allow(clippy::too_many_arguments)]
    pub fn resposta(&mut self, host: &mut dyn Hospedeiro, id: u32, status: u16, url: &str, cabecalhos: &[(String, String)], corpo: Vec<u8>, erro: Option<&str>, de_outra_origem: bool) {
        self.entrar(host);
        let Some(cb) = self.contexto.pedidos.remove(&id) else {
            self.sair();
            return;
        };
        let vm = &mut *self.vm;
        let mut erro = erro.map(str::to_string);
        // Outro site: só se ele permitir.
        if de_outra_origem && erro.is_none() {
            let pagina = origem(&host_url(vm));
            let dele = pagina.as_ref().map(|(e, h, p)| {
                let padrao = (e == "http" && *p == 80) || (e == "https" && *p == 443);
                if padrao { format!("{}://{}", e, h) } else { format!("{}://{}:{}", e, h, p) }
            });
            let permite = cabecalhos.iter().find(|(k, _)| k.eq_ignore_ascii_case("access-control-allow-origin")).map(|(_, v)| v.trim());
            if !(permite == Some("*") || (permite.is_some() && permite == dele.as_deref())) {
                erro = Some(String::from("bloqueado pela política de mesma origem (CORS)"));
            }
        }
        let args: [Valor; 5] = if let Some(e) = erro {
            let t = vm.txt(&e);
            [Valor::Int(0), Valor::Texto(crate::js::lexico::K_VAZIO), Valor::Texto(crate::js::lexico::K_VAZIO), Valor::Nulo, t]
        } else {
            let u = vm.txt(url);
            let c = cabecalhos.iter().map(|(k, v)| format!("{}: {}", k.to_ascii_lowercase(), v)).collect::<Vec<_>>().join("\r\n");
            let c = vm.txt(&c);
            let p = vm.p.buffer;
            let b = vm.novo_obj_com(p, Tipo::Buffer(corpo));
            [Valor::Int(status as i32), u, c, Valor::Obj(b), Valor::Undef]
        };
        for a in args {
            vm.raizes.push(a);
        }
        vm.raizes.push(cb);
        if let Err(e) = vm.chamar(cb, Valor::Undef, &args) {
            self.contexto.erros += 1;
            relatar_erro(vm, e);
        }
        let n = vm.raizes.len();
        vm.raizes.truncate(n - 6);
        self.sair();
    }

    /// Uma imagem que a página pediu chegou (ou falhou).
    pub fn imagem_pronta(&mut self, host: &mut dyn Hospedeiro, url: &str, ok: bool) {
        let nos: Vec<NoId> = self.contexto.imagens_esperando.iter().filter(|(_, u)| u == url).map(|(n, _)| *n).collect();
        self.contexto.imagens_esperando.retain(|(_, u)| u != url);
        for n in nos {
            self.disparar(host, n, if ok { "load" } else { "error" }, false, false, &[]);
        }
    }
}

fn host_url(vm: &Vm) -> String {
    host(vm).url()
}

fn novo_timer(vm: &mut Vm, a: &[Valor], repete: bool) -> Res {
    let ms = vm.para_numero(arg(a, 1)).unwrap_or(0.0);
    let ms = if ms.is_nan() || ms < 0.0 { 0.0 } else { ms } as u64;
    let agora = (vm.ganchos.agora_ms)();
    let c = ctx(vm);
    c.prox_timer += 1;
    let id = c.prox_timer;
    c.timers.push(Timer { id, quando: agora + ms, f: arg(a, 0), args: a.get(2..).unwrap_or(&[]).to_vec(), intervalo: repete.then_some(ms), quadro: false });
    Ok(num(id as f64))
}
