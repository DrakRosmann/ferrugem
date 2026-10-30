//! HTML: do texto para uma árvore de elementos.
//!
//! As páginas reais estão cheias de HTML "errado" que os navegadores
//! aceitam, então este leitor também é tolerante, como manda a especificação
//! do HTML5 (de um jeito simplificado):
//!
//! - elementos vazios (`<br>`, `<img>`, `<meta>`...) não têm fim;
//! - um bloco novo fecha o `<p>` aberto; um `<li>` fecha o `<li>` anterior
//!   (o mesmo para `<dt>`/`<dd>`, `<tr>`, `<td>`/`<th>` e `<option>`);
//! - um fim (`</x>`) sem começo é ignorado; um fim fecha também o que
//!   ficou aberto dentro dele;
//! - o conteúdo de `<script>` e `<style>` é texto cru (não é interpretado),
//!   e o de `<title>` e `<textarea>` não tem marcas, mas tem entidades;
//! - as entidades (`&amp;`, `&eacute;`, `&#233;`, `&#xE9;`) viram letras.

use alloc::string::{String, ToString};
use alloc::vec::Vec;

#[derive(Debug, Clone, PartialEq)]
pub enum No {
    Elemento(Elemento),
    Texto(String),
}

#[derive(Debug, Clone, PartialEq)]
pub struct Elemento {
    /// Em minúsculas.
    pub nome: String,
    pub atributos: Vec<(String, String)>,
    pub filhos: Vec<No>,
}

impl Elemento {
    pub fn atributo(&self, nome: &str) -> Option<&str> {
        self.atributos.iter().find(|(n, _)| n == nome).map(|(_, v)| v.as_str())
    }

    /// O primeiro elemento com esse nome (em qualquer nível).
    pub fn achar(&self, nome: &str) -> Option<&Elemento> {
        for f in &self.filhos {
            if let No::Elemento(e) = f {
                if e.nome == nome {
                    return Some(e);
                }
                if let Some(achado) = e.achar(nome) {
                    return Some(achado);
                }
            }
        }
        None
    }

    /// Todo o texto de dentro, junto.
    pub fn texto(&self) -> String {
        let mut t = String::new();
        for f in &self.filhos {
            match f {
                No::Texto(s) => t.push_str(s),
                No::Elemento(e) => t.push_str(&e.texto()),
            }
        }
        t
    }
}

const VAZIOS: [&str; 15] =
    ["area", "base", "br", "col", "embed", "hr", "img", "input", "link", "meta", "param", "source", "track", "wbr", "keygen"];

/// Os blocos que fecham um `<p>` aberto.
const FECHAM_P: [&str; 33] = [
    "address", "article", "aside", "blockquote", "center", "details", "dialog", "dir", "div", "dl", "fieldset",
    "figcaption", "figure", "footer", "form", "h1", "h2", "h3", "h4", "h5", "h6", "header", "hgroup", "hr", "main",
    "menu", "nav", "ol", "p", "pre", "section", "table", "ul",
];

fn e_vazio(nome: &str) -> bool {
    VAZIOS.contains(&nome)
}

// -----------------------------------------------------------------------------
// Entidades
// -----------------------------------------------------------------------------

const ENTIDADES: [(&str, char); 72] = [
    ("amp", '&'), ("lt", '<'), ("gt", '>'), ("quot", '"'), ("apos", '\''), ("nbsp", '\u{A0}'),
    ("copy", '©'), ("reg", '®'), ("trade", '™'), ("deg", '°'), ("middot", '·'), ("bull", '•'),
    ("hellip", '…'), ("ndash", '–'), ("mdash", '—'), ("lsquo", '‘'), ("rsquo", '’'), ("ldquo", '“'),
    ("rdquo", '”'), ("laquo", '«'), ("raquo", '»'), ("times", '×'), ("divide", '÷'), ("euro", '€'),
    ("pound", '£'), ("yen", '¥'), ("cent", '¢'), ("sect", '§'), ("para", '¶'), ("larr", '←'),
    ("rarr", '→'), ("uarr", '↑'), ("darr", '↓'), ("harr", '↔'), ("plusmn", '±'), ("frac12", '½'),
    ("aacute", 'á'), ("agrave", 'à'), ("acirc", 'â'), ("atilde", 'ã'), ("auml", 'ä'), ("ccedil", 'ç'),
    ("eacute", 'é'), ("egrave", 'è'), ("ecirc", 'ê'), ("euml", 'ë'), ("iacute", 'í'), ("igrave", 'ì'),
    ("icirc", 'î'), ("oacute", 'ó'), ("ograve", 'ò'), ("ocirc", 'ô'), ("otilde", 'õ'), ("ouml", 'ö'),
    ("uacute", 'ú'), ("ugrave", 'ù'), ("ucirc", 'û'), ("uuml", 'ü'), ("ntilde", 'ñ'), ("Aacute", 'Á'),
    ("Agrave", 'À'), ("Acirc", 'Â'), ("Atilde", 'Ã'), ("Ccedil", 'Ç'), ("Eacute", 'É'), ("Ecirc", 'Ê'),
    ("Iacute", 'Í'), ("Oacute", 'Ó'), ("Ocirc", 'Ô'), ("Otilde", 'Õ'), ("Uacute", 'Ú'), ("shy", '\u{AD}'),
];

/// Troca as entidades pelas letras.
pub fn decodificar_entidades(texto: &str) -> String {
    if !texto.contains('&') {
        return texto.to_string();
    }
    let mut r = String::with_capacity(texto.len());
    let mut resto = texto;
    while let Some(i) = resto.find('&') {
        r.push_str(&resto[..i]);
        resto = &resto[i..];
        let fim = resto[1..].find(|c: char| !c.is_ascii_alphanumeric() && c != '#').map_or(resto.len(), |j| j + 1);
        let nome = &resto[1..fim];
        let letra = if let Some(numero) = nome.strip_prefix('#') {
            let valor = match numero.strip_prefix(['x', 'X']) {
                Some(hexa) => u32::from_str_radix(hexa, 16).ok(),
                None => numero.parse().ok(),
            };
            valor.map(|v| match v {
                // Os números 128-159 são o Windows-1252 (aspas e travessões "curvos").
                0x91 => '‘',
                0x92 => '’',
                0x93 => '“',
                0x94 => '”',
                0x96 => '–',
                0x97 => '—',
                v => char::from_u32(v).unwrap_or('\u{FFFD}'),
            })
        } else {
            ENTIDADES.iter().find(|(n, _)| *n == nome).map(|(_, c)| *c)
        };
        match letra {
            Some(c) if !nome.is_empty() => {
                r.push(c);
                resto = &resto[fim..];
                if resto.starts_with(';') {
                    resto = &resto[1..];
                }
            }
            _ => {
                r.push('&');
                resto = &resto[1..];
            }
        }
    }
    r.push_str(resto);
    r
}

// -----------------------------------------------------------------------------
// A construção da árvore
// -----------------------------------------------------------------------------

struct Construtor {
    /// Os elementos abertos; o primeiro é a raiz ("documento").
    pilha: Vec<Elemento>,
}

impl Construtor {
    fn aberto(&self, nome: &str) -> bool {
        self.pilha.iter().skip(1).any(|e| e.nome == nome)
    }

    /// Fecha o elemento do topo (ele vira filho do de baixo).
    fn fechar_topo(&mut self) {
        if self.pilha.len() > 1 {
            let e = self.pilha.pop().unwrap();
            self.pilha.last_mut().unwrap().filhos.push(No::Elemento(e));
        }
    }

    /// Fecha até (e inclusive) o último `nome` aberto, sem passar das `barreiras`.
    fn fechar_ate(&mut self, nome: &str, barreiras: &[&str]) -> bool {
        let Some(i) = self.pilha.iter().rposition(|e| e.nome == nome || barreiras.contains(&e.nome.as_str())) else {
            return false;
        };
        if i == 0 || self.pilha[i].nome != nome {
            return false;
        }
        while self.pilha.len() > i {
            self.fechar_topo();
        }
        true
    }

    fn texto(&mut self, t: String) {
        if t.is_empty() {
            return;
        }
        let filhos = &mut self.pilha.last_mut().unwrap().filhos;
        if let Some(No::Texto(anterior)) = filhos.last_mut() {
            anterior.push_str(&t);
        } else {
            filhos.push(No::Texto(t));
        }
    }

    fn comeco(&mut self, nome: String, atributos: Vec<(String, String)>) {
        let n = nome.as_str();
        if FECHAM_P.contains(&n) && self.aberto("p") {
            self.fechar_ate("p", &["button", "table", "td", "th"]);
        }
        match n {
            "li" => {
                self.fechar_ate("li", &["ul", "ol", "menu", "table"]);
            }
            "dt" | "dd" => {
                if !self.fechar_ate("dt", &["dl"]) {
                    self.fechar_ate("dd", &["dl"]);
                }
            }
            "tr" => {
                self.fechar_ate("tr", &["table"]);
            }
            "td" | "th" => {
                if !self.fechar_ate("td", &["tr", "table"]) {
                    self.fechar_ate("th", &["tr", "table"]);
                }
            }
            "thead" | "tbody" | "tfoot" => {
                for s in ["thead", "tbody", "tfoot"] {
                    self.fechar_ate(s, &["table"]);
                }
            }
            "option" => {
                self.fechar_ate("option", &["select"]);
            }
            // A, B e I não se aninham em si mesmos (um <a> dentro de <a> fecha o primeiro).
            "a" => {
                self.fechar_ate("a", &["td", "th", "table"]);
            }
            _ => {}
        }
        let e = Elemento { nome, atributos, filhos: Vec::new() };
        if e_vazio(&e.nome) {
            self.pilha.last_mut().unwrap().filhos.push(No::Elemento(e));
        } else {
            self.pilha.push(e);
        }
    }

    fn fim(&mut self, nome: &str) {
        if nome == "p" && !self.aberto("p") {
            // </p> sozinho vale como <p></p> (uma quebra)
            self.pilha.last_mut().unwrap().filhos.push(No::Elemento(Elemento {
                nome: "p".into(),
                atributos: Vec::new(),
                filhos: Vec::new(),
            }));
            return;
        }
        if nome == "body" || nome == "html" {
            return; // o resto da página ainda vem depois, às vezes
        }
        self.fechar_ate(nome, &[]);
    }
}

fn ler_atributos(mut s: &str) -> Vec<(String, String)> {
    let mut atributos = Vec::new();
    loop {
        s = s.trim_start_matches(|c: char| c.is_whitespace() || c == '/');
        if s.is_empty() {
            return atributos;
        }
        let fim = s.find(|c: char| c.is_whitespace() || c == '=' || c == '/').unwrap_or(s.len());
        let nome = s[..fim].to_ascii_lowercase();
        s = s[fim..].trim_start();
        let mut valor = String::new();
        if let Some(depois) = s.strip_prefix('=') {
            let depois = depois.trim_start();
            if let Some(aspa) = depois.chars().next().filter(|c| *c == '"' || *c == '\'') {
                let conteudo = &depois[1..];
                let fim = conteudo.find(aspa).unwrap_or(conteudo.len());
                valor = decodificar_entidades(&conteudo[..fim]);
                s = conteudo.get(fim + 1..).unwrap_or("");
            } else {
                let fim = depois.find(char::is_whitespace).unwrap_or(depois.len());
                valor = decodificar_entidades(&depois[..fim]);
                s = &depois[fim..];
            }
        }
        if !nome.is_empty() && !atributos.iter().any(|(n, _)| *n == nome) {
            atributos.push((nome, valor));
        }
    }
}

/// Acha `</nome` sem diferenciar maiúsculas (o fim de um script, por exemplo).
fn achar_fim(s: &str, nome: &str) -> Option<usize> {
    let alvo = alloc::format!("</{}", nome);
    let minusculo = s.to_ascii_lowercase();
    minusculo.find(&alvo)
}

/// Lê um documento HTML. A raiz é um elemento "documento".
pub fn analisar(fonte: &str) -> Elemento {
    let mut c = Construtor {
        pilha: alloc::vec![Elemento { nome: "documento".into(), atributos: Vec::new(), filhos: Vec::new() }],
    };
    let mut s = fonte;
    while !s.is_empty() {
        let Some(i) = s.find('<') else {
            c.texto(decodificar_entidades(s));
            break;
        };
        c.texto(decodificar_entidades(&s[..i]));
        s = &s[i..];
        if let Some(resto) = s.strip_prefix("<!--") {
            s = resto.find("-->").map_or("", |f| &resto[f + 3..]);
            continue;
        }
        if s.starts_with("<!") || s.starts_with("<?") {
            s = s.find('>').map_or("", |f| &s[f + 1..]);
            continue;
        }
        let fechamento = s.starts_with("</");
        let inicio = if fechamento { 2 } else { 1 };
        let nome_fim = s[inicio..]
            .find(|c: char| c.is_whitespace() || c == '>' || c == '/')
            .map_or(s.len(), |f| f + inicio);
        let nome = s[inicio..nome_fim].to_ascii_lowercase();
        if nome.is_empty() || !nome.starts_with(|c: char| c.is_ascii_alphabetic()) {
            c.texto("<".into()); // um "<" solto no texto
            s = &s[1..];
            continue;
        }
        // O fim da marca: o primeiro ">" fora de aspas.
        let mut aspa = None;
        let mut fim_marca = s.len();
        for (j, ch) in s[nome_fim..].char_indices() {
            match (aspa, ch) {
                (None, '"' | '\'') => aspa = Some(ch),
                (Some(a), ch) if ch == a => aspa = None,
                (None, '>') => {
                    fim_marca = nome_fim + j;
                    break;
                }
                _ => {}
            }
        }
        let dentro = &s[nome_fim..fim_marca];
        s = s.get(fim_marca + 1..).unwrap_or("");
        if fechamento {
            c.fim(&nome);
            continue;
        }
        let atributos = ler_atributos(dentro);
        match nome.as_str() {
            "script" | "style" | "textarea" | "title" | "xmp" | "noembed" => {
                let fim = achar_fim(s, &nome).unwrap_or(s.len());
                let conteudo = &s[..fim];
                let texto = if nome == "title" || nome == "textarea" {
                    decodificar_entidades(conteudo)
                } else {
                    conteudo.to_string()
                };
                let mut e = Elemento { nome: nome.clone(), atributos, filhos: Vec::new() };
                if !texto.is_empty() {
                    e.filhos.push(No::Texto(texto));
                }
                c.pilha.last_mut().unwrap().filhos.push(No::Elemento(e));
                s = &s[fim..];
                s = s.find('>').map_or("", |f| &s[f + 1..]);
            }
            _ => c.comeco(nome, atributos),
        }
    }
    while c.pilha.len() > 1 {
        c.fechar_topo();
    }
    c.pilha.pop().unwrap()
}

/// Bytes de uma página em texto: UTF-8, ou Latin-1 (ISO-8859-1/Windows-1252)
/// se não for UTF-8 válido.
pub fn decodificar_bytes(bytes: &[u8]) -> String {
    match core::str::from_utf8(bytes) {
        Ok(s) => s.to_string(),
        Err(_) => bytes.iter().map(|&b| b as char).collect(),
    }
}
