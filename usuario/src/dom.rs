//! O DOM: a árvore do documento, numa arena. Cada nó é um número que vale
//! para sempre (os estilos, as caixas e o JavaScript guardam esses números).

use crate::html::{self, Elemento, No};
use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

pub type NoId = u32;
pub const NADA: NoId = u32::MAX;

pub enum Dados {
    Documento,
    /// `document.createDocumentFragment()`
    Fragmento,
    Elemento { nome: String, atributos: Vec<(String, String)> },
    Texto(String),
    Comentario(String),
}

pub struct NoDom {
    pub pai: NoId,
    pub filhos: Vec<NoId>,
    pub dados: Dados,
}

pub struct Dom {
    pub nos: Vec<NoDom>,
    pub raiz: NoId,
    /// Muda a cada alteração: é como se sabe que é preciso refazer os estilos.
    pub versao: u64,
}

const SEM_FIM: [&str; 15] = ["area", "base", "br", "col", "embed", "hr", "img", "input", "link", "meta", "param", "source", "track", "wbr", "keygen"];

impl Dom {
    pub fn novo() -> Dom {
        Dom { nos: alloc::vec![NoDom { pai: NADA, filhos: Vec::new(), dados: Dados::Documento }], raiz: 0, versao: 1 }
    }

    /// Lê um documento HTML inteiro, garantindo `<html>`, `<head>` e `<body>`.
    pub fn de_html(fonte: &str) -> Dom {
        let arvore = html::analisar(fonte);
        let mut d = Dom::novo();
        d.importar_filhos(&arvore, 0);
        d.normalizar();
        d
    }

    fn novo_no(&mut self, dados: Dados) -> NoId {
        self.nos.push(NoDom { pai: NADA, filhos: Vec::new(), dados });
        self.nos.len() as NoId - 1
    }

    pub fn criar_elemento(&mut self, nome: &str) -> NoId {
        self.novo_no(Dados::Elemento { nome: nome.to_ascii_lowercase(), atributos: Vec::new() })
    }

    pub fn criar_texto(&mut self, t: &str) -> NoId {
        self.novo_no(Dados::Texto(t.into()))
    }

    pub fn criar_comentario(&mut self, t: &str) -> NoId {
        self.novo_no(Dados::Comentario(t.into()))
    }

    pub fn criar_fragmento(&mut self) -> NoId {
        self.novo_no(Dados::Fragmento)
    }

    fn importar_filhos(&mut self, e: &Elemento, pai: NoId) {
        // Iterativo: uma página pode ter milhares de níveis de <div> sem fechar.
        let mut pilha: Vec<(&Elemento, usize, NoId)> = alloc::vec![(e, 0, pai)];
        while let Some(topo) = pilha.last_mut() {
            let (el, i, p) = (topo.0, topo.1, topo.2);
            if i >= el.filhos.len() {
                pilha.pop();
                continue;
            }
            topo.1 += 1;
            match &el.filhos[i] {
                No::Texto(t) => {
                    let n = self.criar_texto(t);
                    self.nos[n as usize].pai = p;
                    self.nos[p as usize].filhos.push(n);
                }
                No::Elemento(f) => {
                    let n = self.novo_no(Dados::Elemento { nome: f.nome.clone(), atributos: f.atributos.clone() });
                    self.nos[n as usize].pai = p;
                    self.nos[p as usize].filhos.push(n);
                    pilha.push((f, 0, n));
                }
            }
        }
    }

    /// Lê um pedaço de HTML (`innerHTML`) como filhos novos, sem pai ainda.
    pub fn ler_fragmento(&mut self, fonte: &str) -> Vec<NoId> {
        let arvore = html::analisar(fonte);
        let f = self.criar_fragmento();
        self.importar_filhos(&arvore, f);
        let filhos = core::mem::take(&mut self.nos[f as usize].filhos);
        for c in &filhos {
            self.nos[*c as usize].pai = NADA;
        }
        filhos
    }

    fn normalizar(&mut self) {
        let raiz_filhos = self.nos[0].filhos.clone();
        let html = raiz_filhos.iter().copied().find(|n| self.e(*n, "html"));
        let html = match html {
            Some(h) => h,
            None => {
                let h = self.criar_elemento("html");
                for c in &raiz_filhos {
                    self.nos[*c as usize].pai = h;
                }
                self.nos[h as usize].filhos = raiz_filhos;
                self.nos[h as usize].pai = 0;
                self.nos[0].filhos = alloc::vec![h];
                h
            }
        };
        let filhos = self.nos[html as usize].filhos.clone();
        let mut head = filhos.iter().copied().find(|n| self.e(*n, "head"));
        let mut body = filhos.iter().copied().find(|n| self.e(*n, "body") || self.e(*n, "frameset"));
        if head.is_none() {
            let h = self.criar_elemento("head");
            self.nos[h as usize].pai = html;
            head = Some(h);
        }
        if body.is_none() {
            let b = self.criar_elemento("body");
            self.nos[b as usize].pai = html;
            body = Some(b);
        }
        let (head, body) = (head.unwrap(), body.unwrap());
        // O que está solto em <html>: metadados vão para o head, o resto para o body.
        let mut novos_html = alloc::vec![head, body];
        let mut no_comeco = true;
        for c in filhos {
            if c == head || c == body {
                if c == body {
                    no_comeco = false;
                }
                continue;
            }
            let meta = matches!(self.nome(c), "title" | "meta" | "link" | "style" | "base" | "script") && no_comeco;
            if meta {
                self.nos[c as usize].pai = head;
                self.nos[head as usize].filhos.push(c);
            } else if self.texto_em_branco(c) {
                continue;
            } else {
                no_comeco = false;
                self.nos[c as usize].pai = body;
                self.nos[body as usize].filhos.push(c);
            }
        }
        core::mem::swap(&mut self.nos[html as usize].filhos, &mut novos_html);
    }

    fn texto_em_branco(&self, n: NoId) -> bool {
        matches!(&self.nos[n as usize].dados, Dados::Texto(t) if t.trim().is_empty())
    }

    pub fn e_elemento(&self, n: NoId) -> bool {
        matches!(self.nos[n as usize].dados, Dados::Elemento { .. })
    }

    /// O nome do elemento em minúsculas ("" se não for elemento).
    pub fn nome(&self, n: NoId) -> &str {
        match &self.nos[n as usize].dados {
            Dados::Elemento { nome, .. } => nome,
            _ => "",
        }
    }

    pub fn e(&self, n: NoId, nome: &str) -> bool {
        self.nome(n) == nome
    }

    pub fn atributo(&self, n: NoId, nome: &str) -> Option<&str> {
        match &self.nos[n as usize].dados {
            Dados::Elemento { atributos, .. } => atributos.iter().find(|(k, _)| k == nome).map(|(_, v)| v.as_str()),
            _ => None,
        }
    }

    pub fn atributos(&self, n: NoId) -> &[(String, String)] {
        match &self.nos[n as usize].dados {
            Dados::Elemento { atributos, .. } => atributos,
            _ => &[],
        }
    }

    pub fn definir_atributo(&mut self, n: NoId, nome: &str, valor: &str) {
        self.versao += 1;
        if let Dados::Elemento { atributos, .. } = &mut self.nos[n as usize].dados {
            match atributos.iter_mut().find(|(k, _)| k == nome) {
                Some(a) => a.1 = valor.into(),
                None => atributos.push((nome.into(), valor.into())),
            }
        }
    }

    pub fn remover_atributo(&mut self, n: NoId, nome: &str) {
        self.versao += 1;
        if let Dados::Elemento { atributos, .. } = &mut self.nos[n as usize].dados {
            atributos.retain(|(k, _)| k != nome);
        }
    }

    pub fn pai(&self, n: NoId) -> NoId {
        self.nos[n as usize].pai
    }

    pub fn filhos(&self, n: NoId) -> &[NoId] {
        &self.nos[n as usize].filhos
    }

    pub fn tem_classe(&self, n: NoId, c: &str) -> bool {
        self.atributo(n, "class").is_some_and(|v| v.split_ascii_whitespace().any(|x| x == c))
    }

    /// Tira o nó do pai dele.
    pub fn soltar(&mut self, n: NoId) {
        let p = self.nos[n as usize].pai;
        if p != NADA {
            self.nos[p as usize].filhos.retain(|c| *c != n);
            self.nos[n as usize].pai = NADA;
            self.versao += 1;
        }
    }

    /// Põe `n` em `pai`, antes de `antes` (NADA = no fim). Um fragmento entrega os filhos.
    pub fn inserir(&mut self, pai: NoId, n: NoId, antes: NoId) {
        if n == pai || self.contem(n, pai) {
            return;
        }
        self.versao += 1;
        if matches!(self.nos[n as usize].dados, Dados::Fragmento) {
            for c in core::mem::take(&mut self.nos[n as usize].filhos) {
                self.nos[c as usize].pai = NADA;
                self.inserir(pai, c, antes);
            }
            return;
        }
        self.soltar(n);
        let lista = &mut self.nos[pai as usize].filhos;
        match lista.iter().position(|c| *c == antes) {
            Some(i) if antes != NADA => lista.insert(i, n),
            _ => lista.push(n),
        }
        self.nos[n as usize].pai = pai;
    }

    /// `a` contém `b`? (ou é o próprio)
    pub fn contem(&self, a: NoId, b: NoId) -> bool {
        let mut x = b;
        while x != NADA {
            if x == a {
                return true;
            }
            x = self.nos[x as usize].pai;
        }
        false
    }

    pub fn clonar(&mut self, n: NoId, fundo: bool) -> NoId {
        let dados = match &self.nos[n as usize].dados {
            Dados::Documento | Dados::Fragmento => Dados::Fragmento,
            Dados::Elemento { nome, atributos } => Dados::Elemento { nome: nome.clone(), atributos: atributos.clone() },
            Dados::Texto(t) => Dados::Texto(t.clone()),
            Dados::Comentario(t) => Dados::Comentario(t.clone()),
        };
        let novo = self.novo_no(dados);
        if fundo {
            for c in self.nos[n as usize].filhos.clone() {
                let f = self.clonar(c, true);
                self.nos[f as usize].pai = novo;
                self.nos[novo as usize].filhos.push(f);
            }
        }
        novo
    }

    /// `textContent`
    pub fn texto(&self, n: NoId) -> String {
        let mut s = String::new();
        self.juntar_texto(n, &mut s);
        s
    }

    fn juntar_texto(&self, n: NoId, s: &mut String) {
        match &self.nos[n as usize].dados {
            Dados::Texto(t) => s.push_str(t),
            Dados::Comentario(_) => {}
            _ => {
                for c in &self.nos[n as usize].filhos {
                    self.juntar_texto(*c, s);
                }
            }
        }
    }

    /// `textContent = t`
    pub fn definir_texto(&mut self, n: NoId, t: &str) {
        self.versao += 1;
        if let Dados::Texto(x) | Dados::Comentario(x) = &mut self.nos[n as usize].dados {
            *x = t.into();
            return;
        }
        for c in core::mem::take(&mut self.nos[n as usize].filhos) {
            self.nos[c as usize].pai = NADA;
        }
        if !t.is_empty() {
            let f = self.criar_texto(t);
            self.nos[f as usize].pai = n;
            self.nos[n as usize].filhos.push(f);
        }
    }

    /// `innerHTML = t`
    pub fn definir_html(&mut self, n: NoId, t: &str) {
        self.versao += 1;
        for c in core::mem::take(&mut self.nos[n as usize].filhos) {
            self.nos[c as usize].pai = NADA;
        }
        for f in self.ler_fragmento(t) {
            self.nos[f as usize].pai = n;
            self.nos[n as usize].filhos.push(f);
        }
    }

    /// `outerHTML`
    pub fn serializar(&self, n: NoId, saida: &mut String) {
        match &self.nos[n as usize].dados {
            Dados::Texto(t) => {
                let cru = self.pai(n) != NADA && matches!(self.nome(self.pai(n)), "script" | "style" | "xmp" | "noembed");
                if cru {
                    saida.push_str(t);
                } else {
                    saida.push_str(&t.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('\u{A0}', "&nbsp;"));
                }
            }
            Dados::Comentario(t) => saida.push_str(&format!("<!--{}-->", t)),
            Dados::Documento | Dados::Fragmento => self.serializar_filhos(n, saida),
            Dados::Elemento { nome, atributos } => {
                saida.push('<');
                saida.push_str(nome);
                for (k, v) in atributos {
                    saida.push(' ');
                    saida.push_str(k);
                    saida.push_str("=\"");
                    saida.push_str(&v.replace('&', "&amp;").replace('"', "&quot;"));
                    saida.push('"');
                }
                saida.push('>');
                if !SEM_FIM.contains(&nome.as_str()) {
                    self.serializar_filhos(n, saida);
                    saida.push_str("</");
                    saida.push_str(nome);
                    saida.push('>');
                }
            }
        }
    }

    pub fn serializar_filhos(&self, n: NoId, saida: &mut String) {
        for c in &self.nos[n as usize].filhos {
            self.serializar(*c, saida);
        }
    }

    /// O primeiro elemento com esse nome (em ordem de documento).
    pub fn achar(&self, de: NoId, nome: &str) -> Option<NoId> {
        for c in &self.nos[de as usize].filhos {
            if self.e(*c, nome) {
                return Some(*c);
            }
            if let Some(a) = self.achar(*c, nome) {
                return Some(a);
            }
        }
        None
    }

    pub fn todos(&self, de: NoId, nome: &str, saida: &mut Vec<NoId>) {
        for c in &self.nos[de as usize].filhos {
            if self.e(*c, nome) || (nome == "*" && self.e_elemento(*c)) {
                saida.push(*c);
            }
            self.todos(*c, nome, saida);
        }
    }

    pub fn por_id(&self, de: NoId, id: &str) -> Option<NoId> {
        for c in &self.nos[de as usize].filhos {
            if self.atributo(*c, "id") == Some(id) {
                return Some(*c);
            }
            if let Some(a) = self.por_id(*c, id) {
                return Some(a);
            }
        }
        None
    }

    /// Algum link com `javascript:`?
    pub fn todos_href_js(&self) -> bool {
        let mut l = Vec::new();
        self.todos(0, "a", &mut l);
        l.iter().any(|n| self.atributo(*n, "href").is_some_and(|h| h.trim_start().to_ascii_lowercase().starts_with("javascript:")))
    }

    pub fn html(&self) -> NoId {
        self.achar(0, "html").unwrap_or(0)
    }

    pub fn head(&self) -> NoId {
        self.achar(self.html(), "head").unwrap_or(0)
    }

    pub fn body(&self) -> NoId {
        self.achar(self.html(), "body").unwrap_or(self.html())
    }

    pub fn titulo(&self) -> String {
        self.achar(0, "title").map(|t| self.texto(t).split_whitespace().collect::<Vec<_>>().join(" ")).unwrap_or_default()
    }

    /// O elemento anterior/seguinte entre os irmãos.
    pub fn irmao_elemento(&self, n: NoId, depois: bool) -> Option<NoId> {
        let p = self.pai(n);
        if p == NADA {
            return None;
        }
        let f = &self.nos[p as usize].filhos;
        let i = f.iter().position(|c| *c == n)?;
        if depois {
            f[i + 1..].iter().copied().find(|c| self.e_elemento(*c))
        } else {
            f[..i].iter().rev().copied().find(|c| self.e_elemento(*c))
        }
    }

    pub fn atributo_ou<'a>(&'a self, n: NoId, nome: &str, padrao: &'a str) -> &'a str {
        self.atributo(n, nome).unwrap_or(padrao)
    }

    pub fn texto_de_no(&self, n: NoId) -> Option<&str> {
        match &self.nos[n as usize].dados {
            Dados::Texto(t) => Some(t),
            _ => None,
        }
    }
}

#[allow(dead_code)]
fn _usa(s: &str) -> String {
    s.to_string()
}
