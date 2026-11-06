//! navegador — um navegador web: HTML, CSS, JavaScript, formulários e imagens
//! PNG e JPEG. Rode dentro do `janelas`.
//!
//! O caminho de uma página:
//!
//! ```text
//!   endereço ─▶ processo filho: DNS, TCP, HTTP ─▶ pipe ─▶ bytes da resposta
//!                (a janela continua respondendo enquanto isso)
//!   bytes ─▶ dom::Dom ─▶ folhas de estilo e scripts, em ordem de documento
//!   DOM + folhas ─▶ css::calcular ─▶ o estilo de cada elemento
//!   DOM + estilos ─▶ caixas::Motor ─▶ lista de desenho
//!   JavaScript (web::Web) mexe no DOM ─▶ refaz só o que for preciso, uma vez por desenho
//! ```
//!
//! Atalhos: Ctrl+L (endereço), Ctrl+R (recarregar), Backspace (voltar),
//! espaço/PgDn/PgUp/setas/Home/End (rolar).

#![no_std]
#![no_main]

use ulib::caixas::{self, Desenho, Hospede, Lista, Medidor};
use ulib::css::{self, EstadoCss, Estilo};
use ulib::dom::{Dom, NADA, NoId};
use ulib::html;
use ulib::http::{self, Url};
use ulib::janela::{Janela, Seta};
use ulib::png;
use ulib::sys;
use ulib::ui::{self, CampoTexto, Tecla, Tela, Ui};
use ulib::web::{self, Hospedeiro, Web};
use ulib::*;

extern crate alloc;
use alloc::rc::Rc;

programa!(principal);

const COR_TEXTO: u32 = 0x1E1E1E;
const COR_FRACA: u32 = 0x6A6A6A;
const COR_LINK: u32 = 0x1A5FB4;
const COR_PAGINA: u32 = 0xFFFFFF;
const BARRA_ESTADO: i32 = 22;
const INICIO: &str = "sobre:inicio";
/// Pedidos de rede do JavaScript ao mesmo tempo.
const MAX_PEDIDOS_JS: usize = 4;

/// O estilo que só o navegador acrescenta quando o JavaScript está desligado.
const FOLHA_SEM_JS: &str = "noscript { display: block }";

// -----------------------------------------------------------------------------
// As imagens, as folhas e os scripts da página
// -----------------------------------------------------------------------------

enum EstadoImagem {
    Esperando,
    Pronta(png::Imagem),
    /// Não deu para baixar, ou não é PNG/JPEG.
    Falhou,
}

struct ImagemPagina {
    url: Url,
    texto: String,
    estado: EstadoImagem,
}

enum EstadoFolha {
    Pronta(css::Folha),
    Esperando(Url),
    Falhou,
}

/// Uma folha de estilo (na ordem do documento: a ordem importa).
struct FolhaPagina {
    /// De onde veio, para saber se o documento ainda a tem.
    no: NoId,
    chave: String,
    estado: EstadoFolha,
}

enum EstadoScript {
    Pronto(String),
    Esperando(Url),
}

struct ScriptPagina {
    no: NoId,
    estado: EstadoScript,
}

/// O que se baixa depois da página.
#[derive(Clone, Copy)]
enum Recurso {
    Folha(usize),
    Imagem(usize),
    Script(NoId),
}

#[derive(Clone, Copy, PartialEq)]
enum TipoCampo {
    Texto,
    Senha,
    AreaTexto,
    Caixa,
    Radio,
    Lista,
    Oculto,
}

impl TipoCampo {
    fn de_texto(self) -> bool {
        matches!(self, TipoCampo::Texto | TipoCampo::Senha | TipoCampo::AreaTexto)
    }
}

/// O estado de um campo de formulário (o que o usuário digitou, marcou ou escolheu).
struct Campo {
    no: NoId,
    tipo: TipoCampo,
    /// O "value" (nos campos de texto, o texto fica em `texto`).
    valor: String,
    /// A dica de um campo vazio (placeholder).
    rotulo: String,
    texto: CampoTexto,
    marcado: bool,
    /// (valor, texto) de cada opção de uma lista.
    opcoes: Vec<(String, String)>,
    escolhida: usize,
    /// O valor quando ganhou o foco (para o evento `change`).
    ao_focar: String,
}

/// Algo que o JavaScript pediu e que só o navegador pode fazer.
enum Acao {
    Navegar(String, bool),
    MudarEndereco(String, bool),
    Voltar(i32),
    Clique(NoId),
    Enviar(NoId),
    Limpar(NoId),
}

struct PedidoJs {
    id: u32,
    metodo: String,
    url: String,
    cabecalhos: Vec<(String, String)>,
    corpo: Option<Vec<u8>>,
    credenciais: bool,
}

// -----------------------------------------------------------------------------
// A página
// -----------------------------------------------------------------------------

struct Pagina {
    url: Url,
    titulo: String,
    dom: Dom,
    /// Feita aqui mesmo: uma página de erro, a inicial (pode ir para `sobre:`).
    interna: bool,
    imagens: Vec<ImagemPagina>,
    /// O tamanho da última diagramação (0 = refazer).
    largura: i32,
    altura_vista: i32,
    lista: Lista,
    calculado: css::Calculado,
    /// A versão do DOM da última vez que se calcularam os estilos.
    versao_estilos: u64,
    versao_campos: u64,
    versao_folhas: u64,
    estilos_validos: bool,
    /// Uma linha para a barra de estado ("HTTP 404", "123 KiB"...).
    info: String,
    /// `<meta http-equiv="refresh" content="0; url=...">`: ir para lá.
    refresh: Option<Url>,
    campos: Vec<Campo>,
    folhas: Vec<FolhaPagina>,
    estado: EstadoCss,
    /// O elemento com o foco.
    foco: Option<NoId>,
    pedir_foco: Option<NoId>,
    web: Option<Web>,
    scripts: Vec<ScriptPagina>,
    /// 0 antes de rodar os scripts; 1 rodando; 2 DOMContentLoaded feito; 3 load feito.
    fase: u8,
    acoes: Vec<Acao>,
    pedidos: Vec<PedidoJs>,
    cancelar: Vec<u32>,
    avisos: Vec<String>,
    console: Vec<String>,
    rolar_para: Option<i32>,
    rolagem: i32,
    /// A última vez que o mouse fez refazer os estilos (`:hover` é caro).
    ultimo_sobre: u64,
    sobre_pendente: Option<Vec<NoId>>,
    armazenamento: String,
}

fn e_folha(d: &Dom, n: NoId) -> Option<Option<String>> {
    match d.nome(n) {
        "style" => Some(None),
        "link" => {
            let rel = d.atributo(n, "rel").unwrap_or("").to_ascii_lowercase();
            let folha = rel.split_whitespace().any(|r| r == "stylesheet") && !rel.contains("alternate") || rel.contains("preload") && d.atributo(n, "as") == Some("style");
            if folha { d.atributo(n, "href").map(|h| Some(String::from(h))) } else { None }
        }
        _ => None,
    }
}

impl Pagina {
    fn de_dom(url: Url, dom: Dom, info: String, interna: bool) -> Pagina {
        let mut metas = Vec::new();
        dom.todos(0, "meta", &mut metas);
        let refresh = metas
            .iter()
            .filter(|m| dom.atributo(**m, "http-equiv").is_some_and(|h| h.eq_ignore_ascii_case("refresh")))
            .filter_map(|m| {
                let (espera, resto) = dom.atributo(*m, "content")?.split_once(';')?;
                let espera: f32 = espera.trim().parse().ok()?;
                let resto = resto.trim();
                let destino = if resto.len() > 4 && resto[..4].eq_ignore_ascii_case("url=") { &resto[4..] } else { resto };
                (espera <= 5.0).then(|| url.juntar(destino.trim_matches(['\'', '"', ' '])))?
            })
            .next();
        let titulo = dom.titulo();
        Pagina {
            url,
            titulo,
            dom,
            interna,
            imagens: Vec::new(),
            largura: 0,
            altura_vista: 0,
            lista: Lista::default(),
            calculado: css::Calculado::default(),
            versao_estilos: 0,
            versao_campos: 0,
            versao_folhas: 0,
            estilos_validos: false,
            info,
            refresh,
            campos: Vec::new(),
            folhas: Vec::new(),
            estado: EstadoCss::default(),
            foco: None,
            pedir_foco: None,
            web: None,
            scripts: Vec::new(),
            fase: 0,
            acoes: Vec::new(),
            pedidos: Vec::new(),
            cancelar: Vec::new(),
            avisos: Vec::new(),
            console: Vec::new(),
            rolar_para: None,
            rolagem: 0,
            ultimo_sobre: 0,
            sobre_pendente: None,
            armazenamento: String::new(),
        }
    }

    fn de_html(url: Url, texto: &str, info: String) -> Pagina {
        Pagina::de_dom(url, Dom::de_html(texto), info, false)
    }

    fn de_texto(url: Url, texto: &str, info: String) -> Pagina {
        let mut d = Dom::de_html("");
        let pre = d.criar_elemento("pre");
        let t = d.criar_texto(texto);
        d.inserir(pre, t, NADA);
        let b = d.body();
        d.inserir(b, pre, NADA);
        Pagina::de_dom(url, d, info, false)
    }

    fn de_imagem(url: Url, imagem: png::Imagem, info: String) -> Pagina {
        let nome = url.caminho.rsplit('/').next().unwrap_or("").to_string();
        let titulo = format!("{} ({} × {})", nome, imagem.largura, imagem.altura);
        let html = format!("<body style='margin:0;background:#222'><center><img src=\"{}\"></center>", escapar(&url.texto()).replace('"', "%22"));
        let mut p = Pagina::de_html(url.clone(), &html, info);
        // O src é a própria página; a imagem já está aqui.
        let texto = url.texto();
        p.imagens.push(ImagemPagina { url, texto, estado: EstadoImagem::Pronta(imagem) });
        p.titulo = titulo;
        p
    }

    fn base(&self) -> Url {
        self.dom.achar(0, "base").and_then(|b| self.dom.atributo(b, "href")).and_then(|h| self.url.juntar(h)).unwrap_or_else(|| self.url.clone())
    }

    fn pode_ir_para(&self, destino: &Url) -> bool {
        // Uma página da web não abre arquivos locais nem as páginas internas
        // (o "sobre:aceitar" passaria por cima da conferência de certificados).
        self.interna || !matches!(self.url.esquema.as_str(), "http" | "https") || matches!(destino.esquema.as_str(), "http" | "https")
    }

    /// As folhas de estilo no documento agora, sem perder as já baixadas.
    fn sincronizar_folhas(&mut self) {
        if self.versao_folhas == self.dom.versao {
            return;
        }
        self.versao_folhas = self.dom.versao;
        let base = self.base();
        let mut achadas = Vec::new();
        coletar_folhas(&self.dom, 0, &mut achadas);
        let mut antigas = core::mem::take(&mut self.folhas);
        let mut novas: Vec<FolhaPagina> = Vec::with_capacity(achadas.len());
        for no in achadas {
            let Some(href) = e_folha(&self.dom, no) else { continue };
            if self.dom.atributo(no, "media").is_some_and(|m| !css::midia_vale(&m.to_ascii_lowercase(), self.largura.max(900))) {
                continue;
            }
            let chave = match &href {
                Some(h) => h.clone(),
                None => self.dom.texto(no),
            };
            if let Some(i) = antigas.iter().position(|f| f.no == no && f.chave == chave) {
                novas.push(antigas.remove(i));
                continue;
            }
            self.estilos_validos = false;
            match href {
                None => {
                    let f = css::Folha::ler(&chave);
                    importacoes(&f, &base, no, &mut novas);
                    novas.push(FolhaPagina { no, chave, estado: EstadoFolha::Pronta(f) });
                }
                Some(h) => {
                    if let Some(u) = base.juntar(&h) {
                        if novas.len() < 48 {
                            novas.push(FolhaPagina { no, chave, estado: EstadoFolha::Esperando(u) });
                        }
                    }
                }
            }
        }
        // As importações (no == mesmo nó, chave começando por "@") continuam.
        for f in antigas {
            if f.chave.starts_with('@') {
                if let Some(i) = novas.iter().position(|x| x.no == f.no && !x.chave.starts_with('@')) {
                    novas.insert(i, f);
                }
            } else {
                self.estilos_validos = false;
            }
        }
        self.folhas = novas;
    }

    /// Os campos de formulário no documento agora.
    fn sincronizar_campos(&mut self) {
        if self.versao_campos == self.dom.versao {
            return;
        }
        self.versao_campos = self.dom.versao;
        let mut nos = Vec::new();
        for t in ["input", "select", "textarea"] {
            self.dom.todos(0, t, &mut nos);
        }
        for n in nos {
            self.campo_de(n);
            if self.dom.e(n, "select") {
                let opcoes = opcoes_da_lista(&self.dom, n);
                if let Some(c) = self.campos.iter_mut().find(|c| c.no == n) {
                    if c.opcoes != opcoes {
                        c.escolhida = c.escolhida.min(opcoes.len().saturating_sub(1));
                        c.opcoes = opcoes;
                    }
                }
            }
        }
    }

    /// O campo de um elemento (criado na primeira vez que alguém pergunta).
    fn campo_de(&mut self, n: NoId) -> Option<usize> {
        if let Some(i) = self.campos.iter().position(|c| c.no == n) {
            return Some(i);
        }
        let d = &self.dom;
        let (tipo, valor, rotulo, opcoes, escolhida, marcado) = match d.nome(n) {
            "input" => {
                let t = d.atributo(n, "type").unwrap_or("text").to_ascii_lowercase();
                let valor = String::from(d.atributo(n, "value").unwrap_or(""));
                let dica = String::from(d.atributo(n, "placeholder").unwrap_or(""));
                let tipo = match t.as_str() {
                    "hidden" | "submit" | "button" | "reset" | "file" | "image" => TipoCampo::Oculto,
                    "checkbox" => TipoCampo::Caixa,
                    "radio" => TipoCampo::Radio,
                    "password" => TipoCampo::Senha,
                    _ => TipoCampo::Texto,
                };
                (tipo, valor, dica, Vec::new(), 0, d.atributo(n, "checked").is_some())
            }
            "textarea" => (TipoCampo::AreaTexto, d.texto(n).trim().to_string(), String::from(d.atributo(n, "placeholder").unwrap_or("")), Vec::new(), 0, false),
            "select" => {
                let mut o = Vec::new();
                d.todos(n, "option", &mut o);
                let esc = o.iter().position(|x| d.atributo(*x, "selected").is_some()).unwrap_or(0);
                (TipoCampo::Lista, String::new(), String::new(), opcoes_da_lista(d, n), esc, false)
            }
            _ => return None,
        };
        let mut texto = CampoTexto::novo(if tipo.de_texto() { &valor } else { "" });
        texto.senha = tipo == TipoCampo::Senha;
        self.campos.push(Campo { no: n, tipo, valor, rotulo, texto, marcado, opcoes, escolhida, ao_focar: String::new() });
        Some(self.campos.len() - 1)
    }

    /// Refaz o que o DOM mudou: estilos e diagramação.
    fn atualizar(&mut self, t: &mut Tela, folha_padrao: &css::Folha, folha_sem_js: Option<&css::Folha>, largura: i32, altura: i32) {
        self.sincronizar_folhas();
        self.sincronizar_campos();
        let mudou_dom = self.versao_estilos != self.dom.versao;
        let mudou_janela = self.largura != largura || self.altura_vista != altura;
        if !mudou_dom && !mudou_janela && self.estilos_validos && self.largura != 0 {
            return;
        }
        let refazer_estilos = mudou_dom || !self.estilos_validos || self.estado.largura != largura || self.estado.altura != altura;
        self.largura = largura;
        self.altura_vista = altura;
        if refazer_estilos {
            self.estado.largura = largura;
            self.estado.altura = altura;
            self.estado.foco = self.foco;
            self.estado.marcados = self.campos.iter().filter(|c| c.marcado).map(|c| c.no).collect();
            let mut folhas: Vec<&css::Folha> = vec![folha_padrao];
            if let Some(f) = folha_sem_js {
                folhas.push(f);
            }
            folhas.extend(self.folhas.iter().filter_map(|f| match &f.estado {
                EstadoFolha::Pronta(x) => Some(x),
                _ => None,
            }));
            self.calculado = css::calcular(&self.dom, &folhas, &self.estado);
            self.versao_estilos = self.dom.versao;
            self.estilos_validos = true;
        }
        self.diagramar(t);
        let novo = self.dom.titulo();
        if !novo.is_empty() {
            self.titulo = novo;
        }
    }

    fn diagramar(&mut self, t: &mut Tela) {
        let base = self.base();
        let juntar = |s: &str| base.juntar(s).map(|u| u.texto());
        let mut m = MedidorTela(t);
        let mut h = HospedeLayout { imagens: &mut self.imagens, campos: &self.campos, dom: &self.dom };
        let motor = caixas::Motor::novo(&self.dom, &self.calculado, &mut m, &mut h, &juntar, self.largura, self.altura_vista);
        self.lista = motor.diagramar();
    }

    /// O elemento embaixo do ponto (coordenadas da página).
    fn elemento_em(&self, x: i32, y_pag: i32, rolagem: i32) -> Option<NoId> {
        let l = &self.lista;
        for a in l.alvos.iter().rev() {
            let y = if a.fixo { y_pag - rolagem } else { y_pag };
            if x >= a.x && y >= a.y && x < a.x + a.l && y < a.y + a.a {
                if a.recorte != 0 {
                    let (rx, ry, rl, ra) = l.recortes[a.recorte as usize];
                    if x < rx || y < ry || x >= rx + rl || y >= ry + ra {
                        continue;
                    }
                }
                return Some(a.no);
            }
        }
        None
    }

    fn ancestral_com(&self, mut n: NoId, nome: &str) -> Option<NoId> {
        while n != NADA {
            if self.dom.e(n, nome) {
                return Some(n);
            }
            n = self.dom.pai(n);
        }
        None
    }

    fn formulario_de(&self, n: NoId) -> Option<NoId> {
        if let Some(i) = self.dom.atributo(n, "form") {
            return self.dom.por_id(0, i);
        }
        self.ancestral_com(n, "form")
    }

    fn link_em(&self, n: NoId) -> Option<Url> {
        let mut x = n;
        while x != NADA {
            if self.dom.e(x, "a") || self.dom.e(x, "area") {
                if let Some(h) = self.dom.atributo(x, "href") {
                    if !h.trim_start().to_ascii_lowercase().starts_with("javascript:") {
                        return self.base().juntar(h.trim());
                    }
                    return None;
                }
            }
            x = self.dom.pai(x);
        }
        None
    }
}

fn importacoes(f: &css::Folha, base: &Url, no: NoId, saida: &mut Vec<FolhaPagina>) {
    for i in &f.importacoes {
        if let Some(u) = base.juntar(i) {
            saida.push(FolhaPagina { no, chave: format!("@{}", i), estado: EstadoFolha::Esperando(u) });
        }
    }
}

fn coletar_folhas(d: &Dom, n: NoId, s: &mut Vec<NoId>) {
    for f in d.filhos(n) {
        if e_folha(d, *f).is_some() {
            s.push(*f);
        } else if d.e_elemento(*f) {
            coletar_folhas(d, *f, s);
        }
    }
}

fn opcoes_da_lista(d: &Dom, n: NoId) -> Vec<(String, String)> {
    let mut o = Vec::new();
    d.todos(n, "option", &mut o);
    o.iter()
        .map(|x| {
            let texto = d.texto(*x).split_whitespace().collect::<Vec<_>>().join(" ");
            (d.atributo(*x, "value").map(String::from).unwrap_or_else(|| texto.clone()), texto)
        })
        .collect()
}

struct MedidorTela<'a, 'b>(&'a mut Tela<'b>);

impl Medidor for MedidorTela<'_, '_> {
    fn largura(&mut self, texto: &str, tam: u16, negrito: bool, mono: bool) -> i32 {
        self.0.medir(texto, tam, negrito, mono)
    }
    fn metricas(&mut self, tam: u16, mono: bool) -> (i32, i32) {
        self.0.metricas_texto(tam, mono)
    }
}

struct HospedeLayout<'a> {
    imagens: &'a mut Vec<ImagemPagina>,
    campos: &'a [Campo],
    dom: &'a Dom,
}

impl Hospede for HospedeLayout<'_> {
    fn imagem(&mut self, url: &str) -> (usize, Option<(i32, i32)>) {
        let i = match self.imagens.iter().position(|x| x.texto == url) {
            Some(i) => i,
            None => {
                let Some(u) = Url::ler(url) else { return (usize::MAX, None) };
                self.imagens.push(ImagemPagina { url: u, texto: url.to_string(), estado: EstadoImagem::Esperando });
                self.imagens.len() - 1
            }
        };
        match &self.imagens[i].estado {
            EstadoImagem::Pronta(img) => (i, Some((img.largura as i32, img.altura as i32))),
            EstadoImagem::Esperando => (i, None),
            EstadoImagem::Falhou => (usize::MAX, None),
        }
    }

    fn controle(&mut self, no: NoId) -> (i32, i32) {
        let Some(c) = self.campos.iter().find(|c| c.no == no) else { return (140, ui::ALTURA) };
        let d = self.dom;
        match c.tipo {
            TipoCampo::Caixa | TipoCampo::Radio => (16, 16),
            TipoCampo::Oculto => (0, 0),
            TipoCampo::Lista => {
                let maior = c.opcoes.iter().map(|(_, t)| t.chars().count() as i32).max().unwrap_or(4);
                (maior * 8 + 34, ui::ALTURA)
            }
            TipoCampo::AreaTexto => {
                let col = d.atributo(no, "cols").and_then(|v| v.parse::<i32>().ok()).unwrap_or(40).clamp(10, 80);
                (col * 7 + 12, ui::ALTURA)
            }
            _ => {
                let col = d.atributo(no, "size").and_then(|v| v.parse::<i32>().ok()).unwrap_or(20).clamp(4, 60);
                (col * 8 + 12, ui::ALTURA)
            }
        }
    }
}

// -----------------------------------------------------------------------------
// A ponte com o JavaScript
// -----------------------------------------------------------------------------

/// O hospedeiro do JavaScript: existe só enquanto um script roda.
struct Ponte<'a, 'b, 'c> {
    p: &'a mut Pagina,
    t: &'a mut Tela<'b>,
    folha_padrao: &'c css::Folha,
    folha_sem_js: Option<&'c css::Folha>,
    cookies: &'c mut Vec<Cookie>,
    largura: i32,
    altura: i32,
}

impl Hospedeiro for Ponte<'_, '_, '_> {
    fn dom(&mut self) -> &mut Dom {
        &mut self.p.dom
    }
    fn estado_css(&mut self) -> &EstadoCss {
        &self.p.estado
    }
    fn valor_campo(&mut self, no: NoId) -> Option<String> {
        let i = self.p.campo_de(no)?;
        let c = &self.p.campos[i];
        Some(match c.tipo {
            t if t.de_texto() => c.texto.texto.clone(),
            TipoCampo::Lista => c.opcoes.get(c.escolhida).map_or(String::new(), |o| o.0.clone()),
            TipoCampo::Caixa | TipoCampo::Radio if c.valor.is_empty() && self.p.dom.atributo(no, "value").is_none() => String::from("on"),
            _ => c.valor.clone(),
        })
    }
    fn definir_valor_campo(&mut self, no: NoId, v: &str) {
        let Some(i) = self.p.campo_de(no) else { return };
        let c = &mut self.p.campos[i];
        match c.tipo {
            t if t.de_texto() => c.texto.mudar(v),
            TipoCampo::Lista => {
                if let Some(j) = c.opcoes.iter().position(|o| o.0 == v) {
                    c.escolhida = j;
                }
            }
            _ => c.valor = v.to_string(),
        }
    }
    fn marcado(&mut self, no: NoId) -> bool {
        self.p.campo_de(no).is_some_and(|i| self.p.campos[i].marcado)
    }
    fn definir_marcado(&mut self, no: NoId, v: bool) {
        let Some(i) = self.p.campo_de(no) else { return };
        if v && self.p.campos[i].tipo == TipoCampo::Radio {
            desmarcar_grupo(self.p, no);
        }
        self.p.campos[i].marcado = v;
        self.p.estilos_validos = false;
    }
    fn indice_selecionado(&mut self, no: NoId) -> i32 {
        self.p.campo_de(no).map_or(-1, |i| if self.p.campos[i].opcoes.is_empty() { -1 } else { self.p.campos[i].escolhida as i32 })
    }
    fn definir_indice(&mut self, no: NoId, i: i32) {
        if let Some(c) = self.p.campo_de(no) {
            self.p.campos[c].escolhida = i.max(0) as usize;
        }
    }
    fn retangulo(&mut self, no: NoId) -> Option<(i32, i32, i32, i32)> {
        self.p.atualizar(self.t, self.folha_padrao, self.folha_sem_js, self.largura, self.altura);
        self.p.lista.retangulos.get(&no).copied()
    }
    fn estilo(&mut self, no: NoId) -> Option<Rc<Estilo>> {
        self.p.atualizar(self.t, self.folha_padrao, self.folha_sem_js, self.largura, self.altura);
        self.p.calculado.estilos.get(&no).cloned()
    }
    fn janela(&mut self) -> (i32, i32) {
        (self.largura, self.altura)
    }
    fn altura_documento(&mut self) -> i32 {
        self.p.atualizar(self.t, self.folha_padrao, self.folha_sem_js, self.largura, self.altura);
        self.p.lista.altura
    }
    fn rolagem(&mut self) -> i32 {
        self.p.rolar_para.unwrap_or(self.p.rolagem)
    }
    fn rolar_para(&mut self, y: i32) {
        self.p.rolar_para = Some(y.max(0));
    }
    fn url(&mut self) -> String {
        self.p.url.texto()
    }
    fn navegar(&mut self, url: &str, substituir: bool) {
        self.p.acoes.push(Acao::Navegar(url.to_string(), substituir));
    }
    fn mudar_endereco(&mut self, url: &str, substituir: bool) {
        if let Some(u) = Url::ler(url) {
            self.p.url = u;
        }
        self.p.acoes.push(Acao::MudarEndereco(url.to_string(), substituir));
    }
    fn voltar(&mut self, n: i32) {
        self.p.acoes.push(Acao::Voltar(n));
    }
    fn focar(&mut self, no: Option<NoId>) {
        self.p.pedir_foco = no;
        if no.is_none() {
            self.p.foco = None;
            self.t.soltar_foco();
        }
    }
    fn foco(&mut self) -> Option<NoId> {
        self.p.foco
    }
    fn acao_clique(&mut self, no: NoId) {
        self.p.acoes.push(Acao::Clique(no));
    }
    fn enviar_formulario(&mut self, form: NoId) {
        self.p.acoes.push(Acao::Enviar(form));
    }
    fn limpar_formulario(&mut self, form: NoId) {
        self.p.acoes.push(Acao::Limpar(form));
    }
    fn pedir(&mut self, id: u32, metodo: &str, url: &str, cabecalhos: &[(String, String)], corpo: Option<Vec<u8>>, credenciais: bool) {
        self.p.pedidos.push(PedidoJs { id, metodo: metodo.to_string(), url: url.to_string(), cabecalhos: cabecalhos.to_vec(), corpo, credenciais });
    }
    fn cancelar_pedido(&mut self, id: u32) {
        self.p.pedidos.retain(|x| x.id != id);
        self.p.cancelar.push(id);
    }
    fn pedir_imagem(&mut self, url: &str) {
        if !self.p.imagens.iter().any(|x| x.texto == url) {
            if let Some(u) = Url::ler(url) {
                self.p.imagens.push(ImagemPagina { url: u, texto: url.to_string(), estado: EstadoImagem::Esperando });
            }
        }
    }
    fn tamanho_imagem(&mut self, url: &str) -> Option<(i32, i32)> {
        match &self.p.imagens.iter().find(|x| x.texto == url)?.estado {
            EstadoImagem::Pronta(i) => Some((i.largura as i32, i.altura as i32)),
            _ => None,
        }
    }
    fn avisar(&mut self, texto: &str) {
        self.p.avisos.push(texto.to_string());
    }
    fn console(&mut self, nivel: u8, texto: &str) {
        if nivel >= 2 {
            eprintln!("js: {}", texto);
        }
        if self.p.console.len() < 500 {
            self.p.console.push(texto.to_string());
        }
    }
    fn cookies(&mut self) -> String {
        cabecalho_cookie(self.cookies, &self.p.url, true)
    }
    fn definir_cookie(&mut self, c: &str) {
        guardar_cookie(self.cookies, &self.p.url, c, true);
    }
    fn ler_armazenamento(&mut self) -> String {
        let a = arquivo_armazenamento(&self.p.url);
        a.and_then(|c| sys::ler_arquivo(&c).ok()).map_or(String::new(), |d| String::from_utf8_lossy(&d).into_owned())
    }
    fn gravar_armazenamento(&mut self, t: &str) {
        // Limite de 2 MiB por site, como os navegadores (5 MB) fazem.
        if t.len() <= 2 << 20 {
            if let Some(a) = arquivo_armazenamento(&self.p.url) {
                let _ = sys::escrever_arquivo(&a, t.as_bytes());
            }
        }
    }
    fn mudou_titulo(&mut self) {
        self.p.titulo = self.p.dom.titulo();
    }
}

/// O arquivo do `localStorage` de um site.
fn arquivo_armazenamento(u: &Url) -> Option<String> {
    if !matches!(u.esquema.as_str(), "http" | "https") {
        return None;
    }
    let limpo: String = format!("{}_{}_{}", u.esquema, u.host, u.porta).chars().map(|c| if c.is_ascii_alphanumeric() || c == '.' { c } else { '_' }).collect();
    Some(format!("{}/.navegador_{}.json", variavel("HOME").unwrap_or("/tmp"), limpo))
}

fn desmarcar_grupo(p: &mut Pagina, no: NoId) {
    let nome = p.dom.atributo(no, "name").unwrap_or("").to_string();
    let form = p.formulario_de(no);
    let mesmos: Vec<NoId> = p.campos.iter().filter(|c| c.tipo == TipoCampo::Radio).map(|c| c.no).filter(|n| p.dom.atributo(*n, "name").unwrap_or("") == nome && p.formulario_de(*n) == form).collect();
    for c in p.campos.iter_mut() {
        if mesmos.contains(&c.no) {
            c.marcado = false;
        }
    }
}

// -----------------------------------------------------------------------------
// Cookies
// -----------------------------------------------------------------------------

struct Cookie {
    nome: String,
    valor: String,
    dominio: String,
    /// Só o host exato (sem o atributo Domain).
    so_host: bool,
    caminho: String,
    seguro: bool,
    http_only: bool,
}

fn dominio_casa(c: &Cookie, host: &str) -> bool {
    host == c.dominio || (!c.so_host && host.ends_with(&c.dominio) && host.as_bytes().get(host.len() - c.dominio.len() - 1) == Some(&b'.'))
}

fn caminho_de(u: &Url) -> &str {
    u.caminho.split('?').next().unwrap_or("/")
}

/// O cabeçalho `Cookie` para este endereço.
fn cabecalho_cookie(l: &[Cookie], u: &Url, para_js: bool) -> String {
    let caminho = caminho_de(u);
    let mut v = Vec::new();
    for c in l {
        if dominio_casa(c, &u.host) && caminho.starts_with(&c.caminho) && (!c.seguro || u.esquema == "https") && !(para_js && c.http_only) {
            v.push(format!("{}={}", c.nome, c.valor));
        }
    }
    v.join("; ")
}

fn guardar_cookie(l: &mut Vec<Cookie>, u: &Url, cabecalho: &str, de_js: bool) {
    if !matches!(u.esquema.as_str(), "http" | "https") {
        return;
    }
    let mut partes = cabecalho.split(';');
    let Some((nome, valor)) = partes.next().and_then(|p| p.split_once('=')) else { return };
    let (nome, valor) = (nome.trim(), valor.trim());
    if nome.is_empty() {
        return;
    }
    let mut c = Cookie { nome: nome.into(), valor: valor.into(), dominio: u.host.clone(), so_host: true, caminho: String::from("/"), seguro: false, http_only: false };
    let mut expirado = false;
    if let Some(i) = caminho_de(u).rfind('/') {
        c.caminho = caminho_de(u)[..i.max(1)].to_string();
    }
    for a in partes {
        let (k, v) = a.split_once('=').unwrap_or((a, ""));
        let (k, v) = (k.trim().to_ascii_lowercase(), v.trim());
        match k.as_str() {
            "domain" => {
                let d = v.trim_start_matches('.').to_ascii_lowercase();
                // Um site só define cookies para si mesmo ou para o domínio de
                // que faz parte, e nunca para um domínio de um nível ("com").
                let ok = d.contains('.') && (u.host == d || u.host.ends_with(&format!(".{}", d)));
                if !ok {
                    return;
                }
                c.dominio = d;
                c.so_host = false;
            }
            "path" if v.starts_with('/') => c.caminho = v.to_string(),
            "secure" => c.seguro = true,
            "httponly" => c.http_only = true,
            "max-age" => expirado = v.parse::<i64>().is_ok_and(|n| n <= 0),
            "expires" => expirado = v.contains("1970"),
            _ => {}
        }
    }
    if de_js && c.http_only {
        return;
    }
    if c.seguro && u.esquema != "https" {
        return;
    }
    if let Some(i) = l.iter().position(|x| x.nome == c.nome && x.dominio == c.dominio && x.caminho == c.caminho) {
        if de_js && l[i].http_only {
            return;
        }
        l.remove(i);
    }
    if !expirado && l.len() < 2000 {
        l.push(c);
    }
}

// -----------------------------------------------------------------------------
// Páginas internas
// -----------------------------------------------------------------------------

/// Uma página feita aqui mesmo (erros, a página inicial).
fn pagina_interna(url: Url, titulo: &str, corpo: &str) -> Pagina {
    let texto = format!(
        "<!doctype html><html><head><title>{}</title><style>body{{max-width:760px;margin:18px auto;padding:0 18px;font-size:16px;line-height:1.45;color:#1e1e1e}}code{{background:#f0f0f0;padding:1px 4px;border-radius:3px}}a{{color:#1a5fb4}}</style></head><body>{}</body></html>",
        escapar(titulo), corpo
    );
    let mut p = Pagina::de_dom(url, Dom::de_html(&texto), String::new(), true);
    p.fase = 3;
    p
}

fn escapar(t: &str) -> String {
    t.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")
}

fn pagina_de_erro(url: Url, mensagem: &str) -> Pagina {
    let endereco = escapar(&url.texto());
    let mut corpo = format!("<h1>Não deu para abrir a página</h1><p><b>{}</b></p><p>{}</p>", endereco, escapar(mensagem));
    if let Some(problema) = mensagem.strip_prefix("certificado: ") {
        corpo = format!(
            "<h1>A identidade do site não foi confirmada</h1><p><b>{}</b></p><p>{}.</p>\
             <p>Pode ser um certificado mal configurado, ou alguém se passando pelo site. Se você continuar, a \
             conexão será cifrada, mas não há garantia de com quem: não digite senhas nem dados pessoais.</p>\
             <p><a href=\"sobre:aceitar?{}\">Continuar mesmo assim</a> (só nesta sessão)</p>",
            endereco,
            escapar(problema),
            escapar(&url.texto())
        );
    } else if url.esquema == "https" && mensagem.contains("TLS") || mensagem.contains("curva") || mensagem.contains("1.3") {
        let http = format!("http{}", url.texto().get(5..).unwrap_or(""));
        corpo += &format!(
            "<p>Este navegador fala só o TLS 1.3 (com a troca de chaves X25519). Tente a \
             <a href=\"{}\">versão sem segurança</a>, se o site tiver uma.</p>",
            escapar(&http)
        );
    }
    corpo += "<p><a href=\"sobre:inicio\">Página inicial</a></p>";
    pagina_interna(url, "Erro", &corpo)
}

const PAGINA_INICIAL: &str = r#"
<h1>Navegador do FerrugemOS</h1>
<p>Um navegador escrito do zero, como o resto do sistema: o TCP, o DNS, o HTTP e o TLS 1.3 são nossos, assim como o leitor de HTML, o CSS (com flexbox, grid, floats, tabelas e posicionamento), o motor de JavaScript (ES2020, com DOM, eventos, <code>fetch</code> e armazenamento) e os decodificadores de PNG e JPEG.</p>
<p>Digite um endereço lá em cima, ou palavras para buscar no <b>DuckDuckGo</b>. O botão <code>JS</code> liga e desliga o JavaScript.</p>
<p style="font-size:13px">Nas conexões HTTPS, o certificado do site é conferido com as autoridades de <code>/etc/ssl/certs.pem</code>. O console de uma página fica em <code>sobre:console</code>.</p>
<h2>Para começar</h2>
<ul>
<li><a href="https://pt.wikipedia.org/wiki/Rust_(linguagem_de_programa%C3%A7%C3%A3o)">Wikipédia: Rust (linguagem de programação)</a></li>
<li><a href="https://lite.duckduckgo.com/lite/">DuckDuckGo Lite</a>: busca na web</li>
<li><a href="https://text.npr.org/">text.npr.org</a>: notícias em texto puro</li>
<li><a href="http://68k.news/">68k.news</a>: o Google Notícias em HTML simples</li>
<li><a href="http://info.cern.ch/hypertext/WWW/TheProject.html">O primeiro site da web</a> (CERN, 1991)</li>
<li><a href="https://example.com/">example.com</a></li>
</ul>
<h2>Atalhos</h2>
<p><code>Ctrl+L</code> vai para o endereço, <code>Ctrl+R</code> recarrega, <code>Backspace</code> volta; <code>espaço</code>, <code>PgDn</code>, <code>PgUp</code>, as setas, <code>Home</code> e <code>End</code> rolam a página.</p>
"#;

// -----------------------------------------------------------------------------
// Os downloads (num processo filho, que manda o resultado por um pipe)
// -----------------------------------------------------------------------------

/// O que vem pelo pipe: `p` + 8 bytes (quantos bytes já chegaram), várias
/// vezes; e no fim `r` + u32 (tamanho) + "estado\nurl\ntipo\nseguranca\ncabeçalhos"
/// + o corpo, ou `e` + a mensagem de erro.
struct Busca {
    pid: i32,
    fd: i32,
    bruto: Vec<u8>,
    recebidos: usize,
    url: Url,
}

enum Resultado {
    Resposta { status: u16, url: Url, tipo: String, corpo: Vec<u8>, seguranca: Option<String>, cabecalhos: Vec<(String, String)> },
    Erro(String),
}

struct PedidoRede<'a> {
    metodo: &'a str,
    cabecalhos: &'a [(String, String)],
    corpo: Option<&'a [u8]>,
}

impl Busca {
    /// `excecoes`: os sites cujo certificado o usuário aceitou sem conferir.
    fn comecar(url: &Url, excecoes: &[String], pedido: PedidoRede) -> Option<Busca> {
        let (leitura, escrita) = sys::pipe().ok()?;
        match sys::fork() {
            Ok(0) => {
                let _ = sys::fechar(leitura);
                let mut ultimo = 0;
                let raizes = ulib::x509::Raizes::do_sistema();
                let abrir = |u: &Url| http::abrir(u, if excecoes.contains(&u.host) { None } else { Some(&raizes) });
                let p = http::Pedido { metodo: pedido.metodo, cabecalhos: pedido.cabecalhos, corpo: pedido.corpo };
                let r = http::obter_pedido(url, &abrir, &p, &mut |n| {
                    if n - ultimo >= 16 * 1024 {
                        ultimo = n;
                        let mut m = [b'p'; 9];
                        m[1..].copy_from_slice(&(n as u64).to_le_bytes());
                        let _ = sys::escrever_tudo(escrita, &m);
                    }
                });
                let mut saida = Vec::new();
                match r {
                    Ok(r) => {
                        let cab: Vec<String> = r.cabecalhos.iter().map(|(k, v)| format!("{}: {}", k, v.replace(['\r', '\n'], " "))).collect();
                        let meta = format!("{}\n{}\n{}\n{}\n{}", r.status, r.url.texto(), r.tipo(), r.seguranca.as_deref().unwrap_or(""), cab.join("\n"));
                        saida.push(b'r');
                        saida.extend_from_slice(&(meta.len() as u32).to_le_bytes());
                        saida.extend_from_slice(meta.as_bytes());
                        saida.extend_from_slice(&r.corpo);
                    }
                    Err(e) => {
                        saida.push(b'e');
                        saida.extend_from_slice(e.as_bytes());
                    }
                }
                let _ = sys::escrever_tudo(escrita, &saida);
                sys::sair(0);
            }
            Ok(pid) => {
                let _ = sys::fechar(escrita);
                Some(Busca { pid, fd: leitura, bruto: Vec::new(), recebidos: 0, url: url.clone() })
            }
            Err(_) => {
                let _ = sys::fechar(leitura);
                let _ = sys::fechar(escrita);
                None
            }
        }
    }

    /// Lê o que já chegou, sem esperar. Devolve o resultado quando termina.
    fn avancar(&mut self) -> Option<Resultado> {
        let mut buf = vec![0u8; 64 * 1024];
        loop {
            match sys::poll(&[(self.fd, sys::POLLIN)], 0) {
                Ok(r) if r[0] != 0 => {}
                _ => return None,
            }
            match sys::ler(self.fd, &mut buf) {
                Ok(0) | Err(_) => break,
                Ok(n) => self.bruto.extend_from_slice(&buf[..n]),
            }
            // As mensagens de progresso do começo.
            while self.bruto.len() >= 9 && self.bruto[0] == b'p' {
                self.recebidos = u64::from_le_bytes(self.bruto[1..9].try_into().unwrap()) as usize;
                self.bruto.drain(..9);
            }
        }
        // Fim: o filho fechou o pipe.
        while self.bruto.len() >= 9 && self.bruto[0] == b'p' {
            self.bruto.drain(..9);
        }
        Some(match self.bruto.first() {
            Some(b'r') if self.bruto.len() >= 5 => {
                let n = u32::from_le_bytes(self.bruto[1..5].try_into().unwrap()) as usize;
                let meta = String::from_utf8_lossy(&self.bruto[5..5 + n]).into_owned();
                let mut partes = meta.splitn(5, '\n');
                let status = partes.next().and_then(|s| s.parse().ok()).unwrap_or(0);
                let url = partes.next().and_then(Url::ler).unwrap_or_else(|| self.url.clone());
                let tipo = String::from(partes.next().unwrap_or(""));
                let seguranca = partes.next().filter(|s| !s.is_empty()).map(String::from);
                let cabecalhos = partes.next().unwrap_or("").split('\n').filter_map(|l| l.split_once(':')).map(|(a, b)| (a.trim().to_string(), b.trim().to_string())).collect();
                Resultado::Resposta { status, url, tipo, corpo: self.bruto.split_off(5 + n), seguranca, cabecalhos }
            }
            Some(b'e') => Resultado::Erro(String::from_utf8_lossy(&self.bruto[1..]).into_owned()),
            _ => Resultado::Erro(String::from("o download terminou sem resposta")),
        })
    }

    fn cancelar(self) {
        let _ = sys::matar(self.pid, sys::SIGKILL);
    }
}

impl Drop for Busca {
    fn drop(&mut self) {
        let _ = sys::fechar(self.fd);
        let _ = sys::esperar(self.pid);
    }
}

fn tamanho_legivel(n: usize) -> String {
    if n < 1024 {
        format!("{} bytes", n)
    } else if n < 1024 * 1024 {
        format!("{} KiB", n / 1024)
    } else {
        format!("{}.{} MiB", n / (1024 * 1024), n % (1024 * 1024) * 10 / (1024 * 1024))
    }
}

// -----------------------------------------------------------------------------
// O navegador
// -----------------------------------------------------------------------------

struct Navegador {
    pagina: Option<Box<Pagina>>,
    /// (endereço, rolagem) de cada página visitada; `atual` é a de agora.
    historico: Vec<(Url, i32)>,
    atual: usize,
    busca: Option<Busca>,
    /// A página que está sendo baixada entra no histórico ao chegar?
    busca_nova: bool,
    /// Uma folha, imagem ou script sendo baixado.
    busca_recurso: Option<(Busca, Recurso)>,
    buscas_js: Vec<(u32, Busca, bool)>,
    endereco: CampoTexto,
    visitados: Vec<String>,
    /// Ir para esta âncora depois da diagramação.
    ancora: Option<String>,
    rolagem_pendente: Option<i32>,
    /// Quantos refreshes seguidos (contra laços).
    refreshes: u32,
    /// Sites cujo certificado o usuário aceitou sem conferir (nesta sessão).
    excecoes: Vec<String>,
    /// O POST do próximo `ir` (um formulário enviado).
    envio: Option<Vec<u8>>,
    /// A lista (`<select>`) aberta: (campo, x, y, largura), na página.
    lista_aberta: Option<(usize, i32, i32, i32)>,
    js_ligado: bool,
    folha_padrao: css::Folha,
    folha_sem_js: css::Folha,
    cookies: Vec<Cookie>,
    /// O elemento embaixo do mouse.
    sobre: Option<NoId>,
    apertado_antes: bool,
    /// O console da última página (para `sobre:console`).
    console_anterior: Vec<String>,
    aviso: Option<(String, u64)>,
}

impl Navegador {
    /// O que foi digitado na barra: um endereço, ou palavras para buscar.
    fn interpretar_endereco(texto: &str) -> Option<Url> {
        let t = texto.trim();
        if t.is_empty() {
            return None;
        }
        if let Some(u) = Url::ler(t) {
            return Some(u);
        }
        if t.starts_with('/') {
            return Url::ler(&format!("file://{}", t));
        }
        if !t.contains(' ') && (t.contains('.') || t.contains(':') || t == "localhost") {
            return Url::ler(&format!("http://{}", t));
        }
        let termos: String = t
            .bytes()
            .map(|b| match b {
                b' ' => String::from("+"),
                b'a'..=b'z' | b'A'..=b'Z' | b'0'..=b'9' | b'-' | b'_' | b'.' => String::from(b as char),
                b => format!("%{:02X}", b),
            })
            .collect();
        Url::ler(&format!("https://lite.duckduckgo.com/lite/?q={}", termos))
    }

    fn cancelar_tudo(&mut self) {
        if let Some(b) = self.busca.take() {
            b.cancelar();
        }
        if let Some((b, _)) = self.busca_recurso.take() {
            b.cancelar();
        }
        for (_, b, _) in self.buscas_js.drain(..) {
            b.cancelar();
        }
    }

    fn ir(&mut self, url: Url, novo_no_historico: bool) {
        // Os links dos resultados do DuckDuckGo passam por um redirecionamento
        // (que precisa de JavaScript); o destino vem no parâmetro "uddg".
        if url.host.ends_with("duckduckgo.com") && url.caminho.starts_with("/l/") {
            let destino = url.caminho.split(['?', '&']).find_map(|p| p.strip_prefix("uddg="));
            if let Some(u) = destino.and_then(|d| Url::ler(&http::decodificar_porcento(d))) {
                return self.ir(u, novo_no_historico);
            }
        }
        let envio = self.envio.take();
        self.lista_aberta = None;
        // Só o fragmento mudou: a mesma página, outro lugar.
        if let Some(p) = &self.pagina {
            let mut sem = url.clone();
            sem.fragmento = p.url.fragmento.clone();
            if sem == p.url && !url.fragmento.is_empty() && novo_no_historico && envio.is_none() {
                self.lembrar_rolagem();
                self.historico.truncate(self.atual + 1);
                self.historico.push((url.clone(), 0));
                self.atual = self.historico.len() - 1;
                self.pagina.as_mut().unwrap().url = url.clone();
                self.ancora = Some(url.fragmento.clone());
                self.endereco.mudar(&url.texto());
                return;
            }
        }
        self.cancelar_tudo();
        self.endereco.mudar(&url.texto());
        match url.esquema.as_str() {
            "sobre" => {
                let p = match url.caminho.as_str() {
                    "inicio" => pagina_interna(url.clone(), "Página inicial", PAGINA_INICIAL),
                    "console" => {
                        let c = if self.pagina.as_ref().is_some_and(|p| !p.console.is_empty()) { &self.pagina.as_ref().unwrap().console } else { &self.console_anterior };
                        let linhas: Vec<String> = c.iter().map(|l| escapar(l)).collect();
                        let corpo = format!("<h1>Console</h1><pre style='white-space:pre-wrap'>{}</pre>", if linhas.is_empty() { String::from("(vazio)") } else { linhas.join("\n") });
                        pagina_interna(url.clone(), "Console", &corpo)
                    }
                    c if c.starts_with("aceitar?") => {
                        // O "Continuar mesmo assim" da página de erro de certificado.
                        let Some(destino) = Url::ler(&c["aceitar?".len()..]) else { return };
                        if !self.excecoes.contains(&destino.host) {
                            self.excecoes.push(destino.host.clone());
                        }
                        return self.ir(destino, false);
                    }
                    _ => pagina_de_erro(url.clone(), "Essa página interna não existe."),
                };
                self.mostrar(p, novo_no_historico);
            }
            "file" => {
                let caminho = url.caminho.split('?').next().unwrap_or("/").to_string();
                let p = match sys::ler_arquivo(&caminho) {
                    Ok(dados) => {
                        let tipo = if caminho.ends_with(".png") {
                            "image/png"
                        } else if caminho.ends_with(".jpg") || caminho.ends_with(".jpeg") {
                            "image/jpeg"
                        } else if caminho.ends_with(".html") || caminho.ends_with(".htm") {
                            "text/html"
                        } else {
                            "text/plain"
                        };
                        let info = tamanho_legivel(dados.len());
                        self.pagina_da_resposta(url.clone(), 200, tipo, dados, info)
                    }
                    Err(e) => pagina_de_erro(url.clone(), &format!("{}: {}", caminho, e)),
                };
                self.mostrar(p, novo_no_historico);
            }
            _ => {
                let mut cabecalhos = Vec::new();
                let c = cabecalho_cookie(&self.cookies, &url, false);
                if !c.is_empty() {
                    cabecalhos.push((String::from("Cookie"), c));
                }
                let metodo = if envio.is_some() { "POST" } else { "GET" };
                if envio.is_some() {
                    cabecalhos.push((String::from("Content-Type"), String::from("application/x-www-form-urlencoded")));
                }
                self.busca = Busca::comecar(&url, &self.excecoes, PedidoRede { metodo, cabecalhos: &cabecalhos, corpo: envio.as_deref() });
                self.busca_nova = novo_no_historico;
                if self.busca.is_none() {
                    self.mostrar(pagina_de_erro(url, "não consegui criar o processo do download"), novo_no_historico);
                }
            }
        }
    }

    fn lembrar_rolagem(&mut self) {
        let r = self.pagina.as_ref().map_or(0, |p| p.rolagem);
        if let Some(h) = self.historico.get_mut(self.atual) {
            h.1 = r;
        }
    }

    fn mostrar(&mut self, p: Pagina, novo_no_historico: bool) {
        if novo_no_historico {
            self.lembrar_rolagem();
            self.historico.truncate(if self.historico.is_empty() { 0 } else { self.atual + 1 });
            self.historico.push((p.url.clone(), 0));
            self.atual = self.historico.len() - 1;
            self.rolagem_pendente = Some(0);
        } else {
            // voltar/avançar/recarregar: a rolagem de quando saímos
            if let Some(h) = self.historico.get_mut(self.atual) {
                h.0 = p.url.clone();
            }
            self.rolagem_pendente = Some(self.historico.get(self.atual).map_or(0, |h| h.1));
        }
        let texto = p.url.texto();
        if !self.visitados.contains(&texto) {
            self.visitados.push(texto.clone());
        }
        self.ancora = (!p.url.fragmento.is_empty()).then(|| p.url.fragmento.clone());
        self.endereco.mudar(&texto);
        let refresh = p.refresh.clone().filter(|d| p.pode_ir_para(d));
        if let Some(velha) = self.pagina.take() {
            self.console_anterior = velha.console;
        }
        self.sobre = None;
        let mut caixa = Box::new(p);
        marcar_visitados(&mut caixa, &self.visitados);
        self.pagina = Some(caixa);
        // O refresh substitui esta página no histórico (até 5 seguidos).
        if let Some(destino) = refresh {
            if self.refreshes < 5 {
                self.refreshes += 1;
                self.ir(destino, false);
            }
        } else {
            self.refreshes = 0;
        }
    }

    fn pagina_da_resposta(&self, url: Url, status: u16, tipo: &str, corpo: Vec<u8>, info: String) -> Pagina {
        let info = if status == 200 { info } else { format!("HTTP {} · {}", status, info) };
        let parece_html = corpo.iter().take(512).skip_while(|b| b.is_ascii_whitespace()).next() == Some(&b'<');
        if tipo == "image/png" || tipo == "image/jpeg" || corpo.starts_with(b"\x89PNG") || corpo.starts_with(&[0xFF, 0xD8, 0xFF]) {
            return match png::decodificar_imagem(&corpo) {
                Some(img) => Pagina::de_imagem(url, img, info),
                None => pagina_de_erro(url, "não consegui ler a imagem (estragada, ou um tipo de PNG/JPEG que não conheço)"),
            };
        }
        if tipo == "text/html" || tipo == "application/xhtml+xml" || tipo.is_empty() && parece_html {
            return Pagina::de_html(url, &html::decodificar_bytes(&corpo), info);
        }
        if tipo.starts_with("text/") || tipo.ends_with("json") || tipo.ends_with("xml") || tipo.ends_with("javascript") || tipo.is_empty() {
            return Pagina::de_texto(url, &html::decodificar_bytes(&corpo), info);
        }
        let nome = url.caminho.split('?').next().unwrap_or("").rsplit('/').next().unwrap_or("").to_string();
        let nome = if nome.is_empty() { String::from("download") } else { nome.replace(['/', '\\'], "_") };
        let destino = format!("{}/{}", variavel("HOME").unwrap_or("/tmp"), nome);
        let gravou = sys::escrever_arquivo(&destino, &corpo);
        let mensagem = match gravou {
            Ok(()) => format!(
                "<h1>Arquivo baixado</h1><p>O tipo <code>{}</code> não é mostrado aqui; os {} foram gravados em <code>{}</code>.</p>",
                escapar(tipo),
                tamanho_legivel(corpo.len()),
                escapar(&destino)
            ),
            Err(e) => format!("<h1>Não deu para gravar</h1><p>{}: {}</p>", escapar(&destino), e),
        };
        let mut p = pagina_interna(url, &nome, &mensagem);
        p.info = info;
        p
    }

    /// Roda `f` com o JavaScript da página, se houver.
    fn com_web(&mut self, t: &mut Tela, largura: i32, altura: i32, f: impl FnOnce(&mut Web, &mut Ponte)) {
        let Some(mut p) = self.pagina.take() else { return };
        if let Some(mut w) = p.web.take() {
            let mut ponte = Ponte { p: &mut p, t, folha_padrao: &self.folha_padrao, folha_sem_js: (!self.js_ligado).then_some(&self.folha_sem_js), cookies: &mut self.cookies, largura, altura };
            f(&mut w, &mut ponte);
            p.web = Some(w);
        }
        self.pagina = Some(p);
    }

    fn disparar(&mut self, t: &mut Tela, largura: i32, altura: i32, alvo: NoId, tipo: &str, borbulha: bool, cancela: bool, extra: &[(&str, f64)]) -> bool {
        let mut r = true;
        self.com_web(t, largura, altura, |w, p| r = w.disparar(p, alvo, tipo, borbulha, cancela, extra));
        r
    }

    /// Scripts e recursos: um passo de cada vez, sem travar a janela.
    fn acompanhar(&mut self, t: &mut Tela, largura: i32, altura: i32) {
        if let Some(b) = self.busca.as_mut() {
            t.acordar_em(30);
            if let Some(r) = b.avancar() {
                let b = self.busca.take().unwrap();
                let pagina = match r {
                    Resultado::Resposta { status, url, tipo, corpo, seguranca, cabecalhos } => {
                        for (k, v) in &cabecalhos {
                            if k.eq_ignore_ascii_case("set-cookie") {
                                guardar_cookie(&mut self.cookies, &url, v, false);
                            }
                        }
                        let mut info = tamanho_legivel(corpo.len());
                        if let Some(s) = seguranca {
                            info += " · ";
                            info += &s;
                        }
                        self.pagina_da_resposta(url, status, &tipo, corpo, info)
                    }
                    Resultado::Erro(e) => pagina_de_erro(b.url.clone(), &e),
                };
                drop(b);
                let novo = self.busca_nova;
                self.mostrar(pagina, novo);
            }
        }
        // Os pedidos do JavaScript (estes podem ser vários ao mesmo tempo).
        let mut i = 0;
        while i < self.buscas_js.len() {
            t.acordar_em(30);
            match self.buscas_js[i].1.avancar() {
                None => i += 1,
                Some(r) => {
                    let (id, b, cred) = self.buscas_js.remove(i);
                    let url_pedida = b.url.clone();
                    drop(b);
                    self.resposta_js(t, largura, altura, id, r, &url_pedida, cred);
                }
            }
        }
        if let Some(p) = self.pagina.as_mut() {
            for id in core::mem::take(&mut p.cancelar) {
                if let Some(i) = self.buscas_js.iter().position(|x| x.0 == id) {
                    let (_, b, _) = self.buscas_js.remove(i);
                    b.cancelar();
                }
            }
            let pedidos: Vec<PedidoJs> = if self.buscas_js.len() < MAX_PEDIDOS_JS && !p.pedidos.is_empty() {
                let n = (MAX_PEDIDOS_JS - self.buscas_js.len()).min(p.pedidos.len());
                p.pedidos.drain(..n).collect()
            } else {
                Vec::new()
            };
            for q in pedidos {
                self.iniciar_pedido_js(q, t, largura, altura);
            }
        }
        // As folhas, os scripts e as imagens, um de cada vez.
        if let Some((b, recurso)) = self.busca_recurso.as_mut() {
            t.acordar_em(30);
            if let Some(r) = b.avancar() {
                let recurso = *recurso;
                self.busca_recurso = None;
                self.recurso_chegou(t, largura, altura, recurso, r);
            }
        }
        if self.busca_recurso.is_none() && self.busca.is_none() {
            self.proximo_recurso(t, largura, altura);
        }
    }

    fn recurso_chegou(&mut self, t: &mut Tela, largura: i32, altura: i32, recurso: Recurso, r: Resultado) {
        let Some(p) = self.pagina.as_mut() else { return };
        match recurso {
            Recurso::Folha(i) => {
                if i >= p.folhas.len() {
                    return;
                }
                let no = p.folhas[i].no;
                let mut novas = Vec::new();
                p.folhas[i].estado = match r {
                    Resultado::Resposta { status: 200, corpo, url, .. } => {
                        let f = css::Folha::ler(&html::decodificar_bytes(&corpo));
                        importacoes(&f, &url, no, &mut novas);
                        EstadoFolha::Pronta(f)
                    }
                    _ => EstadoFolha::Falhou,
                };
                for (k, n) in novas.into_iter().enumerate() {
                    p.folhas.insert(i + k, n);
                }
                p.estilos_validos = false;
                p.largura = 0;
            }
            Recurso::Imagem(i) => {
                if i >= p.imagens.len() {
                    return;
                }
                let mut ok = false;
                p.imagens[i].estado = match r {
                    Resultado::Resposta { status: 200, corpo, .. } => match png::decodificar_imagem(&corpo) {
                        Some(img) if img.largura as u64 * img.altura as u64 <= 16 << 20 => {
                            ok = true;
                            EstadoImagem::Pronta(img)
                        }
                        _ => EstadoImagem::Falhou,
                    },
                    _ => EstadoImagem::Falhou,
                };
                p.largura = 0;
                let u = p.imagens[i].texto.clone();
                self.com_web(t, largura, altura, |w, q| w.imagem_pronta(q, &u, ok));
            }
            Recurso::Script(no) => {
                let fonte = match r {
                    Resultado::Resposta { status: 200, corpo, .. } => Some(html::decodificar_bytes(&corpo)),
                    _ => None,
                };
                p.scripts.retain(|s| s.no != no);
                match fonte {
                    Some(f) => {
                        self.com_web(t, largura, altura, |w, q| w.rodar_script(q, no, &f));
                        self.disparar(t, largura, altura, no, "load", false, false, &[]);
                    }
                    None => {
                        self.disparar(t, largura, altura, no, "error", false, false, &[]);
                    }
                }
            }
        }
        t.redesenhar();
    }

    /// O que baixar ou rodar agora: folhas, depois scripts em ordem, depois imagens.
    fn proximo_recurso(&mut self, t: &mut Tela, largura: i32, altura: i32) {
        let excecoes = core::mem::take(&mut self.excecoes);
        let cookies_de = |u: &Url, jar: &Vec<Cookie>, pagina: &Url| -> Vec<(String, String)> {
            let c = cabecalho_cookie(jar, u, false);
            if !c.is_empty() && web::mesma_origem(&pagina.texto(), &u.texto()) { vec![(String::from("Cookie"), c)] } else { Vec::new() }
        };
        let mut rodar_scripts = false;
        if let Some(p) = self.pagina.as_mut() {
            p.sincronizar_folhas();
            let pagina_url = p.url.clone();
            let get = PedidoRede { metodo: "GET", cabecalhos: &[], corpo: None };
            if let Some(i) = p.folhas.iter().position(|f| matches!(f.estado, EstadoFolha::Esperando(_))) {
                let EstadoFolha::Esperando(url) = &p.folhas[i].estado else { unreachable!() };
                if url.esquema == "file" && matches!(pagina_url.esquema.as_str(), "http" | "https") {
                    p.folhas[i].estado = EstadoFolha::Falhou;
                } else if url.esquema == "file" {
                    let f = sys::ler_arquivo(&url.caminho).ok().map(|d| css::Folha::ler(&html::decodificar_bytes(&d)));
                    p.folhas[i].estado = f.map_or(EstadoFolha::Falhou, EstadoFolha::Pronta);
                    p.estilos_validos = false;
                    p.largura = 0;
                } else {
                    let cab = cookies_de(url, &self.cookies, &pagina_url);
                    match Busca::comecar(url, &excecoes, PedidoRede { metodo: "GET", cabecalhos: &cab, corpo: None }) {
                        Some(b) => self.busca_recurso = Some((b, Recurso::Folha(i))),
                        None => p.folhas[i].estado = EstadoFolha::Falhou,
                    }
                }
                t.redesenhar();
            } else if p.fase < 3 {
                rodar_scripts = true;
            } else if let Some(i) = p.imagens.iter().position(|im| matches!(im.estado, EstadoImagem::Esperando)) {
                let url = p.imagens[i].url.clone();
                match url.esquema.as_str() {
                    "http" | "https" => match Busca::comecar(&url, &excecoes, get) {
                        Some(b) => self.busca_recurso = Some((b, Recurso::Imagem(i))),
                        None => p.imagens[i].estado = EstadoImagem::Falhou,
                    },
                    "file" if !matches!(pagina_url.esquema.as_str(), "http" | "https") => {
                        p.imagens[i].estado = match sys::ler_arquivo(&url.caminho).ok().and_then(|d| png::decodificar_imagem(&d)) {
                            Some(img) => EstadoImagem::Pronta(img),
                            None => EstadoImagem::Falhou,
                        };
                        p.largura = 0;
                    }
                    _ => p.imagens[i].estado = EstadoImagem::Falhou,
                }
                t.redesenhar();
            }
        }
        self.excecoes = excecoes;
        if rodar_scripts {
            self.progredir_scripts(t, largura, altura);
        }
        // Scripts que o JavaScript acrescentou ao documento.
        let novos = if let Some(p) = self.pagina.as_mut() { p.web.as_mut().map_or(Vec::new(), |w| w.tomar_scripts_novos()) } else { Vec::new() };
        if !novos.is_empty() {
            self.enfileirar_scripts(novos, t, largura, altura);
        }
        // Um script dinâmico à espera?
        if self.busca_recurso.is_none() {
            if let Some(p) = self.pagina.as_mut() {
                if p.fase >= 2 {
                    if let Some(s) = p.scripts.iter().find(|s| matches!(s.estado, EstadoScript::Esperando(_))) {
                        let EstadoScript::Esperando(u) = &s.estado else { unreachable!() };
                        let no = s.no;
                        if let Some(b) = Busca::comecar(u, &self.excecoes, PedidoRede { metodo: "GET", cabecalhos: &[], corpo: None }) {
                            self.busca_recurso = Some((b, Recurso::Script(no)));
                        } else {
                            p.scripts.retain(|x| x.no != no);
                        }
                    }
                }
            }
        }
    }

    fn enfileirar_scripts(&mut self, nos: Vec<NoId>, t: &mut Tela, largura: i32, altura: i32) {
        let Some(p) = self.pagina.as_mut() else { return };
        let base = p.base();
        let mut para_rodar = Vec::new();
        for no in nos {
            if !web::script_executavel(&p.dom, no) {
                continue;
            }
            match p.dom.atributo(no, "src").filter(|s| !s.is_empty()) {
                Some(src) => {
                    if let Some(u) = base.juntar(src) {
                        p.scripts.push(ScriptPagina { no, estado: EstadoScript::Esperando(u) });
                    }
                }
                None => {
                    let f = p.dom.texto(no);
                    if !f.trim().is_empty() {
                        para_rodar.push((no, f));
                    }
                }
            }
        }
        for (no, f) in para_rodar {
            self.com_web(t, largura, altura, |w, q| w.rodar_script(q, no, &f));
        }
    }

    /// A carga inicial: cria a máquina, roda os `<script>` na ordem, avisa.
    fn progredir_scripts(&mut self, t: &mut Tela, largura: i32, altura: i32) {
        let js = self.js_ligado;
        let Some(p) = self.pagina.as_mut() else { return };
        if p.fase == 0 {
            p.fase = 1;
            let mut nos = Vec::new();
            p.dom.todos(0, "script", &mut nos);
            let base = p.base();
            let tem_on = {
                let d = &p.dom;
                let mut todos = Vec::new();
                d.todos(0, "*", &mut todos);
                todos.iter().any(|n| d.atributos(*n).iter().any(|(k, _)| k.starts_with("on"))) || d.todos_href_js()
            };
            if !js || (nos.is_empty() && !tem_on) {
                p.fase = 3;
                return;
            }
            for no in nos {
                if !web::script_executavel(&p.dom, no) {
                    continue;
                }
                let estado = match p.dom.atributo(no, "src").filter(|s| !s.is_empty()) {
                    Some(src) => match base.juntar(src) {
                        Some(u) => EstadoScript::Esperando(u),
                        None => continue,
                    },
                    None => EstadoScript::Pronto(p.dom.texto(no)),
                };
                p.scripts.push(ScriptPagina { no, estado });
            }
            // Cria a máquina: a página fica sem resposta só neste instante.
            let mut caixa = self.pagina.take().unwrap();
            {
                let mut ponte = Ponte { p: &mut caixa, t, folha_padrao: &self.folha_padrao, folha_sem_js: None, cookies: &mut self.cookies, largura, altura };
                let w = Web::novo(&mut ponte, sys::tempo_ms);
                ponte.p.web = Some(w);
            }
            self.pagina = Some(caixa);
            t.redesenhar();
            return;
        }
        if p.fase == 1 {
            // O próximo script da fila.
            if let Some(s) = p.scripts.first() {
                match &s.estado {
                    EstadoScript::Pronto(_) => {
                        let s = p.scripts.remove(0);
                        let EstadoScript::Pronto(f) = s.estado else { unreachable!() };
                        let no = s.no;
                        self.com_web(t, largura, altura, |w, q| {
                            w.marcar_script_rodado(no);
                            w.rodar_script(q, no, &f)
                        });
                    }
                    EstadoScript::Esperando(u) => {
                        let (u, no) = (u.clone(), s.no);
                        let pagina = p.url.clone();
                        if let Some(w) = p.web.as_mut() {
                            w.marcar_script_rodado(no);
                        }
                        if u.esquema == "file" {
                            let d = if matches!(pagina.esquema.as_str(), "http" | "https") { None } else { sys::ler_arquivo(&u.caminho).ok() };
                            p.scripts.remove(0);
                            if let Some(d) = d {
                                let f = html::decodificar_bytes(&d);
                                self.com_web(t, largura, altura, |w, q| w.rodar_script(q, no, &f));
                            }
                        } else {
                            match Busca::comecar(&u, &self.excecoes, PedidoRede { metodo: "GET", cabecalhos: &[], corpo: None }) {
                                Some(b) => self.busca_recurso = Some((b, Recurso::Script(no))),
                                None => {
                                    p.scripts.remove(0);
                                }
                            }
                        }
                    }
                }
                t.redesenhar();
                return;
            }
            // Acabaram os scripts da carga.
            p.fase = 2;
            if let Some(w) = p.web.as_mut() {
                w.marcar_estado(1);
            }
            self.com_web(t, largura, altura, |w, q| {
                let doc = 0;
                w.disparar(q, doc, "DOMContentLoaded", true, false, &[]);
            });
            t.redesenhar();
            return;
        }
        if p.fase == 2 && !p.scripts.iter().any(|s| matches!(s.estado, EstadoScript::Esperando(_))) {
            p.fase = 3;
            if let Some(w) = p.web.as_mut() {
                w.marcar_estado(2);
            }
            self.com_web(t, largura, altura, |w, q| {
                w.disparar(q, NADA, "load", false, false, &[]);
            });
            t.redesenhar();
        }
    }

    fn iniciar_pedido_js(&mut self, q: PedidoJs, t: &mut Tela, largura: i32, altura: i32) {
        let Some(u) = Url::ler(&q.url) else {
            let r = Resultado::Erro(String::from("endereço inválido"));
            let l = Url::ler("sobre:inicio").unwrap();
            return self.resposta_js(t, largura, altura, q.id, r, &l, q.credenciais);
        };
        let mut cab = q.cabecalhos;
        if q.credenciais {
            let c = cabecalho_cookie(&self.cookies, &u, false);
            if !c.is_empty() {
                cab.push((String::from("Cookie"), c));
            }
        }
        match Busca::comecar(&u, &self.excecoes, PedidoRede { metodo: &q.metodo, cabecalhos: &cab, corpo: q.corpo.as_deref() }) {
            Some(b) => self.buscas_js.push((q.id, b, q.credenciais)),
            None => self.resposta_js(t, largura, altura, q.id, Resultado::Erro(String::from("sem processo")), &u, q.credenciais),
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn resposta_js(&mut self, t: &mut Tela, largura: i32, altura: i32, id: u32, r: Resultado, pedida: &Url, credenciais: bool) {
        match r {
            Resultado::Resposta { status, url, corpo, cabecalhos, .. } => {
                if credenciais {
                    for (k, v) in &cabecalhos {
                        if k.eq_ignore_ascii_case("set-cookie") {
                            guardar_cookie(&mut self.cookies, &url, v, false);
                        }
                    }
                }
                // Um cabeçalho que os scripts nunca veem.
                let cab: Vec<(String, String)> = cabecalhos.into_iter().filter(|(k, _)| !k.eq_ignore_ascii_case("set-cookie")).collect();
                let u = url.texto();
                self.com_web(t, largura, altura, |w, q| {
                    let outra = !web::mesma_origem(&q.url(), &u);
                    w.resposta(q, id, status, &u, &cab, corpo, None, outra);
                });
            }
            Resultado::Erro(e) => {
                let _ = pedida;
                self.com_web(t, largura, altura, |w, q| w.resposta(q, id, 0, "", &[], Vec::new(), Some(&e), false));
            }
        }
        t.redesenhar();
    }

    /// Envia um formulário (`botao`, o botão usado, também vai junto).
    fn enviar(&mut self, t: &mut Tela, largura: i32, altura: i32, form: NoId, botao: Option<NoId>, com_evento: bool) {
        if com_evento && !self.disparar(t, largura, altura, form, "submit", true, true, &[]) {
            return;
        }
        let Some(p) = self.pagina.as_mut() else { return };
        p.sincronizar_campos();
        let mut pares = Vec::new();
        let mut todos = Vec::new();
        p.dom.todos(0, "*", &mut todos);
        for n in todos {
            if !matches!(p.dom.nome(n), "input" | "select" | "textarea" | "button") || p.formulario_de(n) != Some(form) {
                continue;
            }
            let Some(nome) = p.dom.atributo(n, "name").filter(|s| !s.is_empty()).map(String::from) else { continue };
            if p.dom.atributo(n, "disabled").is_some() {
                continue;
            }
            let eh_botao = p.dom.e(n, "button") || matches!(p.dom.atributo(n, "type").map(|t| t.to_ascii_lowercase()).as_deref(), Some("submit" | "button" | "reset" | "image" | "file"));
            if eh_botao {
                if Some(n) == botao {
                    pares.push((nome, p.dom.atributo(n, "value").unwrap_or("").to_string()));
                }
                continue;
            }
            let oculto = p.dom.atributo(n, "type").is_some_and(|t| t.eq_ignore_ascii_case("hidden"));
            if oculto {
                let v = p.campos.iter().find(|c| c.no == n).map_or_else(|| p.dom.atributo(n, "value").unwrap_or("").to_string(), |c| c.valor.clone());
                pares.push((nome, v));
                continue;
            }
            let Some(i) = p.campo_de(n) else { continue };
            let c = &p.campos[i];
            let valor = match c.tipo {
                x if x.de_texto() => Some(c.texto.texto.clone()),
                TipoCampo::Caixa | TipoCampo::Radio if c.marcado => Some(if c.valor.is_empty() { "on".into() } else { c.valor.clone() }),
                TipoCampo::Lista => c.opcoes.get(c.escolhida).map(|(v, _)| v.clone()),
                _ => None,
            };
            if let Some(v) = valor {
                pares.push((nome, v));
            }
        }
        let dados = http::codificar_formulario(&pares);
        let base = p.base();
        let acao = p.dom.atributo(form, "action").filter(|a| !a.trim().is_empty()).and_then(|a| base.juntar(a.trim()));
        let post = p.dom.atributo(form, "method").is_some_and(|m| m.eq_ignore_ascii_case("post"));
        let mut url = acao.unwrap_or_else(|| p.url.clone());
        url.fragmento.clear();
        if !p.pode_ir_para(&url) {
            return;
        }
        if post {
            self.envio = Some(dados.into_bytes());
        } else {
            // GET: os dados vão no endereço, no lugar da "query" que houver.
            let caminho = url.caminho.split('?').next().unwrap_or("/").to_string();
            url.caminho = format!("{}?{}", caminho, dados);
        }
        self.ir(url, true);
    }

    fn limpar_form(&mut self, form: NoId) {
        let Some(p) = self.pagina.as_mut() else { return };
        for i in 0..p.campos.len() {
            let n = p.campos[i].no;
            if p.formulario_de(n) == Some(form) {
                let d = &p.dom;
                let (ini, marc, esc) = match d.nome(n) {
                    "textarea" => (d.texto(n).trim().to_string(), false, 0),
                    "select" => {
                        let mut o = Vec::new();
                        d.todos(n, "option", &mut o);
                        (String::new(), false, o.iter().position(|x| d.atributo(*x, "selected").is_some()).unwrap_or(0))
                    }
                    _ => (d.atributo(n, "value").unwrap_or("").to_string(), d.atributo(n, "checked").is_some(), 0),
                };
                let c = &mut p.campos[i];
                c.texto.mudar(&ini);
                c.marcado = marc;
                c.escolhida = esc;
            }
        }
        p.estilos_validos = false;
    }

    /// A ação de um clique que ninguém cancelou.
    fn acao_padrao(&mut self, t: &mut Tela, largura: i32, altura: i32, no: NoId) {
        let Some(p) = self.pagina.as_mut() else { return };
        let mut x = no;
        while x != NADA {
            let nome = p.dom.nome(x).to_string();
            match nome.as_str() {
                "a" | "area" => {
                    if let Some(h) = p.dom.atributo(x, "href").map(str::trim).map(String::from) {
                        if let Some(codigo) = h.strip_prefix("javascript:").or_else(|| h.strip_prefix("JavaScript:")) {
                            let c = http::decodificar_porcento(codigo);
                            self.com_web(t, largura, altura, |w, q| w.avaliar(q, &c));
                        } else if let Some(u) = p.base().juntar(&h) {
                            if p.pode_ir_para(&u) {
                                self.ir(u, true);
                            } else {
                                self.aviso = Some((String::from("Bloqueado: esta página não pode abrir esse endereço."), sys::tempo_ms()));
                            }
                        }
                        t.redesenhar();
                    }
                    return;
                }
                "button" | "input" => {
                    let tipo = p.dom.atributo(x, "type").map(|t| t.to_ascii_lowercase());
                    let padrao = if nome == "button" { "submit" } else { "text" };
                    match tipo.as_deref().unwrap_or(padrao) {
                        "submit" | "image" => {
                            if let Some(f) = p.formulario_de(x) {
                                self.enviar(t, largura, altura, f, Some(x), true);
                            }
                        }
                        "reset" => {
                            if let Some(f) = p.formulario_de(x) {
                                if self.disparar(t, largura, altura, f, "reset", true, true, &[]) {
                                    self.limpar_form(f);
                                }
                            }
                        }
                        _ => {}
                    }
                    t.redesenhar();
                    return;
                }
                "label" => {
                    let alvo = match p.dom.atributo(x, "for") {
                        Some(i) => p.dom.por_id(0, i),
                        None => ["input", "select", "textarea"].iter().find_map(|n| p.dom.achar(x, n)),
                    };
                    if let Some(a) = alvo {
                        if a != no {
                            match p.campo_de(a).map(|i| p.campos[i].tipo) {
                                Some(TipoCampo::Caixa | TipoCampo::Radio) => self.alternar_caixa(t, largura, altura, a),
                                Some(k) if k.de_texto() => p.pedir_foco = Some(a),
                                _ => {}
                            }
                        }
                    }
                    t.redesenhar();
                    return;
                }
                "summary" => {
                    let d = p.dom.pai(x);
                    if d != NADA && p.dom.e(d, "details") {
                        if p.dom.atributo(d, "open").is_some() {
                            p.dom.remover_atributo(d, "open");
                        } else {
                            p.dom.definir_atributo(d, "open", "");
                        }
                    }
                    t.redesenhar();
                    return;
                }
                _ => {}
            }
            x = p.dom.pai(x);
        }
    }

    fn alternar_caixa(&mut self, t: &mut Tela, largura: i32, altura: i32, no: NoId) {
        let Some(p) = self.pagina.as_mut() else { return };
        let Some(i) = p.campo_de(no) else { return };
        let radio = p.campos[i].tipo == TipoCampo::Radio;
        let antes = p.campos[i].marcado;
        if radio {
            if antes {
                return;
            }
            desmarcar_grupo(p, no);
            p.campos[i].marcado = true;
        } else {
            p.campos[i].marcado = !antes;
        }
        p.estilos_validos = false;
        // O clique pode ser cancelado (e aí a caixa volta como estava).
        if !self.disparar(t, largura, altura, no, "click", true, true, &[]) {
            if let Some(p) = self.pagina.as_mut() {
                p.campos[i].marcado = antes;
            }
            return;
        }
        self.disparar(t, largura, altura, no, "input", true, false, &[]);
        self.disparar(t, largura, altura, no, "change", true, false, &[]);
    }

    fn fazer_acoes(&mut self, t: &mut Tela, largura: i32, altura: i32) {
        let acoes = self.pagina.as_mut().map_or(Vec::new(), |p| core::mem::take(&mut p.acoes));
        for a in acoes {
            match a {
                Acao::Navegar(u, subst) => {
                    let destino = Url::ler(&u);
                    let permitido = match (&destino, self.pagina.as_ref()) {
                        (Some(d), Some(p)) => p.pode_ir_para(d),
                        _ => false,
                    };
                    if let (Some(d), true) = (destino, permitido) {
                        self.ir(d, !subst);
                        t.redesenhar();
                    }
                    return; // a página acabou
                }
                Acao::MudarEndereco(u, subst) => {
                    if let Some(d) = Url::ler(&u) {
                        self.endereco.mudar(&d.texto());
                        if subst {
                            if let Some(h) = self.historico.get_mut(self.atual) {
                                h.0 = d;
                            }
                        } else {
                            self.lembrar_rolagem();
                            self.historico.truncate(self.atual + 1);
                            self.historico.push((d, 0));
                            self.atual = self.historico.len() - 1;
                        }
                    }
                }
                Acao::Voltar(n) => {
                    if n < 0 {
                        self.voltar();
                    } else if n > 0 {
                        self.avancar();
                    } else {
                        self.recarregar();
                    }
                    return;
                }
                Acao::Clique(no) => self.acao_padrao(t, largura, altura, no),
                Acao::Enviar(f) => self.enviar(t, largura, altura, f, None, false),
                Acao::Limpar(f) => self.limpar_form(f),
            }
        }
    }

    fn voltar(&mut self) {
        if self.atual > 0 {
            self.lembrar_rolagem();
            self.atual -= 1;
            let url = self.historico[self.atual].0.clone();
            self.ir(url, false);
        }
    }

    fn avancar(&mut self) {
        if self.atual + 1 < self.historico.len() {
            self.lembrar_rolagem();
            self.atual += 1;
            let url = self.historico[self.atual].0.clone();
            self.ir(url, false);
        }
    }

    fn recarregar(&mut self) {
        if let Some(url) = self.pagina.as_ref().map(|p| p.url.clone()) {
            self.lembrar_rolagem();
            self.ir(url, false);
        }
    }

    fn atualizar_pagina(&mut self, t: &mut Tela, largura: i32, altura: i32) {
        let sem_js = !self.js_ligado;
        if let Some(p) = self.pagina.as_mut() {
            p.atualizar(t, &self.folha_padrao, sem_js.then_some(&self.folha_sem_js), largura, altura);
        }
    }

    fn desenhar(&mut self, t: &mut Tela) {
        let tema = t.tema();
        let largura_janela = t.q.largura;
        // A barra de ferramentas.
        let mut ir_para = None;
        let mut mudou_js = false;
        t.linha(|t| {
            if t.botao_compacto("←") {
                self.voltar();
            }
            if t.botao_compacto("→") {
                self.avancar();
            }
            if self.busca.is_some() {
                if t.botao_compacto("✕") {
                    if let Some(b) = self.busca.take() {
                        b.cancelar();
                    }
                }
            } else if t.botao_compacto("↻") {
                self.recarregar();
            }
            if t.botao_compacto("⌂") {
                ir_para = Url::ler(INICIO);
            }
            if t.botao_compacto(if self.js_ligado { "JS" } else { "—" }) {
                self.js_ligado = !self.js_ligado;
                mudou_js = true;
            }
            let (x, _) = t.posicao();
            let largura = largura_janela - x - 12;
            if t.campo(&mut self.endereco, largura) {
                ir_para = Navegador::interpretar_endereco(&self.endereco.texto);
                t.soltar_foco();
            }
        });
        if mudou_js {
            self.recarregar();
        }
        if let Some(url) = ir_para {
            self.ir(url, true);
        }
        if t.atalho('l') {
            // um endereço novo: a barra começa vazia
            self.endereco.mudar("");
            self.endereco.pedir_foco = true;
            t.redesenhar();
        }
        if t.atalho('r') {
            self.recarregar();
        }
        // A página.
        let (_, topo) = t.posicao();
        let altura_area = t.q.altura - topo - BARRA_ESTADO;
        t.em(0, topo);
        let area = t.area(largura_janela, altura_area);
        self.acompanhar(t, largura_janela, altura_area);
        // Os timers.
        let mut proximo = None;
        self.com_web(t, largura_janela, altura_area, |w, q| proximo = w.passo(q));
        if let Some(ms) = proximo {
            t.acordar_em((ms as i32).clamp(4, 1000));
        }
        self.fazer_acoes(t, largura_janela, altura_area);
        self.atualizar_pagina(t, largura_janela, altura_area);
        if let Some(p) = self.pagina.as_ref() {
            let titulo = if p.titulo.is_empty() { p.url.texto() } else { p.titulo.clone() };
            t.mudar_titulo(&format!("{} — Navegador", titulo));
        }
        // O elemento embaixo do mouse.
        let rolagem_agora = self.pagina.as_ref().map_or(0, |p| p.rolagem);
        let mouse = area.mouse.map(|(x, y)| (x, y + rolagem_agora));
        let sobre_novo = match (mouse.filter(|_| self.lista_aberta.is_none()), self.pagina.as_ref()) {
            (Some((mx, my)), Some(p)) => p.elemento_em(mx, my, rolagem_agora),
            _ => None,
        };
        if sobre_novo != self.sobre {
            let velho = self.sobre;
            self.sobre = sobre_novo;
            if let Some(v) = velho {
                self.disparar(t, largura_janela, altura_area, v, "mouseout", true, true, &[]);
            }
            if let Some(v) = sobre_novo {
                let (mx, my) = area.mouse.unwrap_or((0, 0));
                self.disparar(t, largura_janela, altura_area, v, "mouseover", true, true, &[("clientX", mx as f64), ("clientY", my as f64)]);
            }
            self.mudar_sobre(sobre_novo);
        }
        self.aplicar_sobre(t);
        // Apertar e soltar o botão.
        if let (Some(alvo), Some((mx, my))) = (self.sobre, area.mouse) {
            if area.apertado != self.apertado_antes {
                let tipo = if area.apertado { "mousedown" } else { "mouseup" };
                self.disparar(t, largura_janela, altura_area, alvo, tipo, true, true, &[("clientX", mx as f64), ("clientY", my as f64), ("pageX", mx as f64), ("pageY", (my + rolagem_agora) as f64)]);
            }
        }
        self.apertado_antes = area.apertado;
        self.teclado(t, &area, largura_janela, altura_area);
        self.fazer_acoes(t, largura_janela, altura_area);
        self.atualizar_pagina(t, largura_janela, altura_area);
        let altura_pagina = self.pagina.as_ref().map_or(0, |p| p.lista.altura);
        let mut rolagem = self.pagina.as_ref().map_or(0, |p| p.rolagem);
        if let Some(r) = self.rolagem_pendente.take() {
            rolagem = r;
        }
        if let Some(p) = self.pagina.as_mut() {
            if let Some(r) = p.rolar_para.take() {
                rolagem = r;
            }
        }
        if let (Some(a), Some(p)) = (self.ancora.take(), self.pagina.as_ref()) {
            let d = &p.dom;
            let mut alvo = d.por_id(0, &a);
            if alvo.is_none() {
                let mut as_ = Vec::new();
                d.todos(0, "a", &mut as_);
                alvo = as_.into_iter().find(|n| d.atributo(*n, "name") == Some(a.as_str()));
            }
            if let Some((_, y, _, _)) = alvo.and_then(|n| p.lista.retangulos.get(&n).copied()) {
                rolagem = y - 8;
            }
        }
        // Rolar: roda, teclado.
        let pagina_inteira = altura_area - 40;
        rolagem += area.roda * 48;
        for (tecla, passo) in [
            (Tecla::Seta(Seta::Baixo), 40),
            (Tecla::Seta(Seta::Cima), -40),
            (Tecla::Seta(Seta::PaginaAbaixo), pagina_inteira),
            (Tecla::Seta(Seta::PaginaAcima), -pagina_inteira),
            (Tecla::Letra(' '), pagina_inteira),
            (Tecla::Seta(Seta::Inicio), -1 << 24),
            (Tecla::Seta(Seta::Fim), 1 << 24),
        ] {
            while t.tecla_da_area(&area, tecla.clone()) {
                rolagem += passo;
            }
        }
        if t.tecla_da_area(&area, Tecla::Apagar) {
            self.voltar();
        }
        rolagem = rolagem.clamp(0, (altura_pagina - altura_area).max(0));
        let antes_rolagem = self.pagina.as_ref().map_or(0, |p| p.rolagem);
        if let Some(p) = self.pagina.as_mut() {
            p.rolagem = rolagem;
        }
        if rolagem != antes_rolagem {
            self.disparar(t, largura_janela, altura_area, NADA, "scroll", false, false, &[]);
        }
        // Desenha a parte visível.
        let antes = t.q.recorte;
        t.q.recortar(area.x, area.y, area.largura, area.altura);
        let fundo = self.pagina.as_ref().map_or(COR_PAGINA, |p| p.lista.fundo & 0xFFFFFF);
        t.q.retangulo(area.x, area.y, area.largura, area.altura, fundo);
        let mouse = area.mouse.map(|(x, y)| (x, y + rolagem));
        if let Some(p) = self.pagina.as_ref() {
            pintar_lista(t, p, (area.x, area.y, area.largura, area.altura), rolagem, area.apertado);
            // A lista aberta, por cima de tudo.
            if let Some((i, x, y, largura)) = self.lista_aberta {
                let c = &p.campos[i];
                let (sx, sy) = (area.x + x, area.y + y - rolagem);
                let altura = c.opcoes.len() as i32 * 22 + 4;
                t.q.retangulo(sx, sy, largura, altura, 0xFFFFFF);
                t.q.moldura(sx, sy, largura, altura, 0x909090);
                for (n, (_, texto)) in c.opcoes.iter().enumerate() {
                    let oy = sy + 2 + n as i32 * 22;
                    let sobre = area.mouse.is_some_and(|(mx, my)| {
                        let (mx, my) = (mx + area.x, my + area.y);
                        mx >= sx && mx < sx + largura && my >= oy && my < oy + 22
                    });
                    if sobre || n == c.escolhida && area.mouse.is_none() {
                        t.q.retangulo(sx + 2, oy, largura - 4, 22, if sobre { 0xD6E4F5 } else { 0xEEEEEE });
                    }
                    t.escrever(sx + 8, oy + 3, texto, COR_TEXTO, n == c.escolhida, 13);
                }
            }
            // A barra de rolagem.
            if altura_pagina > altura_area {
                let alca = (altura_area * altura_area / altura_pagina).max(24);
                let y = area.y + (altura_area - alca) * rolagem / (altura_pagina - altura_area).max(1);
                t.q.retangulo_redondo(area.x + area.largura - 8, y + 2, 5, alca - 4, 2, 0xB0B0B0);
            }
        }
        // Os campos de texto são componentes de verdade (foco, cursor,
        // teclado); sempre todos, na mesma ordem (o foco é pela ordem). Os
        // que não estão inteiros à vista ficam longe, fora da tela.
        let mut enviar_de = None;
        let mut rolar_para = None;
        let mut mudancas: Vec<NoId> = Vec::new();
        let mut foco_novo = None;
        let aberta = self.lista_aberta.is_some();
        if let Some(p) = self.pagina.as_mut() {
            let pedir = p.pedir_foco.take();
            let controles: Vec<(i32, i32, i32, i32, NoId)> = p
                .lista
                .itens
                .iter()
                .filter_map(|it| match it.d {
                    Desenho::Controle { x, y, l, a, no } => Some((x, y, l, a, no)),
                    _ => None,
                })
                .collect();
            for (x, y, largura, altura, no) in controles {
                let Some(i) = p.campos.iter().position(|c| c.no == no) else { continue };
                let c = &mut p.campos[i];
                if !c.tipo.de_texto() {
                    continue;
                }
                if pedir == Some(no) {
                    c.texto.pedir_foco = true;
                }
                let (sx, sy) = (area.x + x, area.y + y - rolagem + (altura - ui::ALTURA) / 2);
                let inteiro = sy >= area.y && sy + ui::ALTURA <= area.y + area.altura && !aberta;
                let antes_txt = c.texto.texto.clone();
                let (enter, focado) = if inteiro { t.campo_solto(&mut c.texto, sx, sy, largura) } else { t.campo_solto(&mut c.texto, -10000, -10000, largura) };
                if c.texto.texto != antes_txt {
                    mudancas.push(no);
                }
                if inteiro && !focado && c.texto.texto.is_empty() && !c.rotulo.is_empty() {
                    let recorte = t.q.recorte;
                    t.q.recortar(sx + 3, sy, largura - 6, ui::ALTURA);
                    t.escrever(sx + 7, sy + 5, &c.rotulo, 0x9A9A9A, false, 13);
                    t.q.recorte = recorte;
                }
                if focado {
                    foco_novo = Some(no);
                    if !inteiro {
                        rolar_para = Some(y - altura_area / 3); // o Tab foi para um campo fora da vista
                    }
                }
                if enter && c.tipo != TipoCampo::AreaTexto {
                    enviar_de = Some(no);
                }
            }
        }
        t.q.recorte = antes;
        if let Some(r) = rolar_para {
            if let Some(p) = self.pagina.as_mut() {
                p.rolagem = r.max(0);
            }
            t.redesenhar();
        }
        // O foco mudou?
        let (foco_velho, valor_velho, valor_atual) = match self.pagina.as_ref() {
            Some(p) => {
                let f = p.foco;
                let c = f.and_then(|n| p.campos.iter().find(|c| c.no == n));
                (f, c.map(|c| c.ao_focar.clone()), c.map(|c| c.texto.texto.clone()))
            }
            None => (None, None, None),
        };
        // Um elemento que recebeu foco por JavaScript e não é campo não perde por aqui.
        if foco_novo != foco_velho && (foco_novo.is_some() || foco_velho.is_some_and(|f| self.pagina.as_ref().is_some_and(|p| p.campos.iter().any(|c| c.no == f && c.tipo.de_texto())))) {
            if let Some(v) = foco_velho {
                if valor_velho != valor_atual {
                    self.disparar(t, largura_janela, altura_area, v, "change", true, false, &[]);
                }
                self.disparar(t, largura_janela, altura_area, v, "blur", false, false, &[]);
                self.disparar(t, largura_janela, altura_area, v, "focusout", true, false, &[]);
            }
            if let Some(p) = self.pagina.as_mut() {
                p.foco = foco_novo;
                p.estilos_validos = false;
                if let Some(c) = foco_novo.and_then(|n| p.campos.iter_mut().find(|c| c.no == n)) {
                    c.ao_focar = c.texto.texto.clone();
                }
            }
            if let Some(v) = foco_novo {
                self.disparar(t, largura_janela, altura_area, v, "focus", false, false, &[]);
                self.disparar(t, largura_janela, altura_area, v, "focusin", true, false, &[]);
            }
        }
        for no in mudancas {
            self.disparar(t, largura_janela, altura_area, no, "input", true, false, &[]);
        }
        if let Some(no) = enviar_de {
            // Enter num campo envia o formulário (como o primeiro botão de enviar).
            let f = self.pagina.as_ref().and_then(|p| p.formulario_de(no));
            if let Some(f) = f {
                let botao = self.pagina.as_ref().and_then(|p| primeiro_botao(p, f));
                self.enviar(t, largura_janela, altura_area, f, botao, true);
                t.redesenhar();
            }
        }
        // Os cliques: na lista aberta, num controle, num elemento.
        if area.clique {
            if let Some((i, x, y, largura)) = self.lista_aberta.take() {
                let mut mudou = None;
                if let (Some((mx, my)), Some(p)) = (mouse, self.pagina.as_mut()) {
                    let n = (my - y - 2).div_euclid(22);
                    if mx >= x && mx < x + largura && n >= 0 && (n as usize) < p.campos[i].opcoes.len() {
                        if p.campos[i].escolhida != n as usize {
                            mudou = Some(p.campos[i].no);
                        }
                        p.campos[i].escolhida = n as usize;
                    }
                }
                if let Some(no) = mudou {
                    self.disparar(t, largura_janela, altura_area, no, "input", true, false, &[]);
                    self.disparar(t, largura_janela, altura_area, no, "change", true, false, &[]);
                }
                t.redesenhar();
            } else if let Some(alvo) = self.sobre {
                self.clique_em(t, alvo, &area, largura_janela, altura_area, rolagem);
                t.redesenhar();
            }
        }
        self.fazer_acoes(t, largura_janela, altura_area);
        // A barra de estado.
        let y = t.q.altura - BARRA_ESTADO;
        t.q.retangulo(0, y, largura_janela, BARRA_ESTADO, tema.painel);
        t.q.retangulo(0, y, largura_janela, 1, tema.borda);
        let link_sobre = match (self.sobre, self.pagina.as_ref()) {
            (Some(n), Some(p)) => p.link_em(n),
            _ => None,
        };
        // `alert`: uma linha na barra (sem caixa que trava a página).
        if let Some(p) = self.pagina.as_mut() {
            if let Some(m) = p.avisos.pop() {
                self.aviso = Some((m, sys::tempo_ms()));
                p.avisos.clear();
            }
        }
        let agora = sys::tempo_ms();
        if self.aviso.as_ref().is_some_and(|a| agora > a.1 + 6000) {
            self.aviso = None;
        }
        let texto = if let Some((m, _)) = &self.aviso {
            t.acordar_em(1000);
            format!("Aviso da página: {}", m)
        } else if let Some(u) = link_sobre {
            u.texto()
        } else if let Some(b) = &self.busca {
            format!("Carregando {}… {}", b.url.host, if b.recebidos > 0 { tamanho_legivel(b.recebidos) } else { String::new() })
        } else if let Some((b, recurso)) = &self.busca_recurso {
            let nome = b.url.caminho.split('?').next().unwrap_or("").rsplit('/').next().unwrap_or("");
            match recurso {
                Recurso::Folha(_) => format!("Carregando estilo {}…", nome),
                Recurso::Imagem(_) => format!("Carregando imagem {}…", nome),
                Recurso::Script(_) => format!("Carregando script {}…", nome),
            }
        } else {
            self.pagina.as_ref().map_or(String::new(), |p| {
                let erros = p.web.as_ref().map_or(0, |w| w.erros());
                if erros > 0 { format!("{} · JS: {} erro(s), veja sobre:console", p.info, erros) } else { p.info.clone() }
            })
        };
        t.escrever(10, y + 4, &texto, tema.texto_fraco, false, 12);
    }

    fn mudar_sobre(&mut self, novo: Option<NoId>) {
        let Some(p) = self.pagina.as_mut() else { return };
        let mut cadeia = Vec::new();
        let mut x = novo.unwrap_or(NADA);
        while x != NADA {
            if p.dom.e_elemento(x) {
                cadeia.push(x);
            }
            x = p.dom.pai(x);
        }
        p.sobre_pendente = Some(cadeia);
    }

    /// `:hover`: refazer os estilos é caro; no máximo 4 vezes por segundo, e
    /// só se alguma regra se importar com os elementos que mudaram.
    fn aplicar_sobre(&mut self, t: &mut Tela) {
        let Some(p) = self.pagina.as_mut() else { return };
        let Some(cadeia) = p.sobre_pendente.as_ref() else { return };
        let agora = sys::tempo_ms();
        if agora < p.ultimo_sobre + 250 {
            t.acordar_em(100);
            return;
        }
        let velha = &p.estado.sobre;
        let mudados: Vec<NoId> = cadeia.iter().filter(|n| !velha.contains(n)).chain(velha.iter().filter(|n| !cadeia.contains(n))).copied().collect();
        let importa = mudados.iter().any(|n| {
            matches!(p.dom.nome(*n), "button" | "input") || p.folhas.iter().any(|f| matches!(&f.estado, EstadoFolha::Pronta(x) if x.sobre_importa(&p.dom, *n)))
        });
        p.estado.sobre = p.sobre_pendente.take().unwrap();
        if importa {
            p.ultimo_sobre = agora;
            p.estilos_validos = false;
            t.redesenhar();
        }
    }

    fn teclado(&mut self, t: &mut Tela, area: &ui::Area, largura: i32, altura: i32) {
        if self.pagina.as_ref().is_none_or(|p| p.web.is_none()) || !area.focada && self.pagina.as_ref().is_some_and(|p| p.foco.is_none()) {
            return;
        }
        let alvo = self.pagina.as_ref().and_then(|p| p.foco).unwrap_or(NADA);
        let mut i = 0;
        while i < t.teclas_pendentes().len() {
            let (nome, codigo): (String, i32) = match &t.teclas_pendentes()[i] {
                Tecla::Letra(c) => (c.to_string(), c.to_ascii_uppercase() as i32),
                Tecla::Enter => (String::from("Enter"), 13),
                Tecla::Apagar => (String::from("Backspace"), 8),
                Tecla::Tab => (String::from("Tab"), 9),
                Tecla::Esc => (String::from("Escape"), 27),
                Tecla::Seta(Seta::Esquerda) => (String::from("ArrowLeft"), 37),
                Tecla::Seta(Seta::Cima) => (String::from("ArrowUp"), 38),
                Tecla::Seta(Seta::Direita) => (String::from("ArrowRight"), 39),
                Tecla::Seta(Seta::Baixo) => (String::from("ArrowDown"), 40),
                Tecla::Seta(Seta::Inicio) => (String::from("Home"), 36),
                Tecla::Seta(Seta::Fim) => (String::from("End"), 35),
                Tecla::Seta(Seta::PaginaAcima) => (String::from("PageUp"), 33),
                Tecla::Seta(Seta::PaginaAbaixo) => (String::from("PageDown"), 34),
                Tecla::Seta(Seta::Delete) => (String::from("Delete"), 46),
                _ => {
                    i += 1;
                    continue;
                }
            };
            let mut seguir = true;
            self.com_web(t, largura, altura, |w, q| {
                seguir = w.disparar_tecla(q, alvo, "keydown", &nome, codigo);
                if seguir && nome.chars().count() == 1 {
                    seguir = w.disparar_tecla(q, alvo, "keypress", &nome, codigo);
                }
                w.disparar_tecla(q, alvo, "keyup", &nome, codigo);
            });
            if seguir {
                i += 1;
            } else {
                t.consumir_tecla(i);
            }
            t.redesenhar();
        }
    }

    fn clique_em(&mut self, t: &mut Tela, alvo: NoId, area: &ui::Area, largura: i32, altura: i32, rolagem: i32) {
        let (mx, my) = area.mouse.unwrap_or((0, 0));
        // Um controle embaixo do mouse?
        let controle = self.pagina.as_mut().and_then(|p| {
            let i = p.campo_de(alvo)?;
            Some((i, p.campos[i].tipo))
        });
        match controle {
            Some((_, TipoCampo::Caixa | TipoCampo::Radio)) => return self.alternar_caixa(t, largura, altura, alvo),
            Some((i, TipoCampo::Lista)) => {
                let caixa = self.pagina.as_ref().and_then(|p| p.lista.retangulos.get(&alvo).copied());
                if self.disparar(t, largura, altura, alvo, "click", true, true, &[]) {
                    if let Some((x, y, l, a)) = caixa {
                        self.lista_aberta = Some((i, x, y + a, l.max(120)));
                    }
                }
                return;
            }
            _ => {}
        }
        let extra = [("clientX", mx as f64), ("clientY", my as f64), ("pageX", mx as f64), ("pageY", (my + rolagem) as f64)];
        if self.disparar(t, largura, altura, alvo, "click", true, true, &extra) {
            self.acao_padrao(t, largura, altura, alvo);
        }
    }
}

fn primeiro_botao(p: &Pagina, f: NoId) -> Option<NoId> {
    let mut todos = Vec::new();
    p.dom.todos(f, "*", &mut todos);
    todos.into_iter().find(|n| match p.dom.nome(*n) {
        "button" => !matches!(p.dom.atributo(*n, "type").map(|t| t.to_ascii_lowercase()).as_deref(), Some("button" | "reset")),
        "input" => matches!(p.dom.atributo(*n, "type").map(|t| t.to_ascii_lowercase()).as_deref(), Some("submit" | "image")),
        _ => false,
    })
}

fn marcar_visitados(p: &mut Pagina, visitados: &[String]) {
    let mut links = Vec::new();
    p.dom.todos(0, "a", &mut links);
    let base = p.base();
    for n in links {
        if let Some(u) = p.dom.atributo(n, "href").and_then(|h| base.juntar(h)) {
            if visitados.contains(&u.texto()) {
                p.estado.visitados.insert(n);
            }
        }
    }
}

/// Desenha a parte visível da lista.
fn pintar_lista(t: &mut Tela, p: &Pagina, (ax, ay, al, aa): (i32, i32, i32, i32), rolagem: i32, apertado: bool) {
    let _ = apertado;
    let (y0, y1) = (rolagem, rolagem + aa);
    let recorte_area = t.q.recorte;
    let mut recorte_atual = 0;
    for it in &p.lista.itens {
        let (x, y, l, a) = caixa_de(&it.d);
        let topo = if it.fixo { y + rolagem } else { y };
        if topo + a < y0 || topo > y1 || x > al {
            continue;
        }
        let desl = if it.fixo { 0 } else { rolagem };
        if it.recorte != recorte_atual {
            recorte_atual = it.recorte;
            t.q.recorte = recorte_area;
            if recorte_atual != 0 {
                let (rx, ry, rl, ra) = p.lista.recortes[recorte_atual as usize];
                let (x0, y00) = ((ax + rx).max(recorte_area.0), (ay + ry - desl).max(recorte_area.1));
                let (x1, y11) = ((ax + rx + rl).min(recorte_area.0 + recorte_area.2), (ay + ry + ra - desl).min(recorte_area.1 + recorte_area.3));
                t.q.recorte = (x0, y00, (x1 - x0).max(0), (y11 - y00).max(0));
            }
        }
        t.q.opacidade = it.opac;
        let (sx, sy) = (ax + x, ay + y - desl);
        match &it.d {
            Desenho::Ret { cor, .. } => t.q.retangulo_alfa(sx, sy, l, a, *cor),
            Desenho::Redondo { raios, cor, .. } => t.q.redondo_alfa(sx, sy, l, a, *raios, *cor),
            Desenho::Borda { raios, larguras, cores, estilos, .. } => t.q.borda_caixa(sx, sy, l, a, *raios, *larguras, *cores, *estilos),
            Desenho::Texto { texto, tam, negrito, italico, mono, cor, .. } => {
                if *mono {
                    t.escrever_mono(sx, sy, texto, *cor, *tam);
                } else if *italico {
                    t.escrever_italico(sx, sy, texto, *cor, *negrito, *tam);
                } else {
                    t.escrever(sx, sy, texto, *cor, *negrito, *tam);
                }
            }
            Desenho::Imagem { indice, .. } => {
                if let Some(ImagemPagina { estado: EstadoImagem::Pronta(img), .. }) = p.imagens.get(*indice) {
                    t.q.desenhar_imagem(&img.pixels, img.largura as i32, img.altura as i32, sx, sy, l, a);
                }
            }
            Desenho::Gradiente { angulo, paradas, .. } => t.q.gradiente(sx, sy, l, a, *angulo, paradas),
            Desenho::Sombra { x: bx, y: by, l: bl, a: ba, raios, borrao, cor } => t.q.sombra(ax + bx, ay + by - desl, *bl, *ba, *raios, *borrao, *cor),
            Desenho::Controle { no, .. } => {
                if let Some(c) = p.campos.iter().find(|c| c.no == *no) {
                    desenhar_controle(t, c, (sx, sy, l, a));
                }
            }
        }
    }
    t.q.opacidade = 255;
    t.q.recorte = recorte_area;
}

fn caixa_de(d: &Desenho) -> (i32, i32, i32, i32) {
    match *d {
        Desenho::Ret { x, y, l, a, .. }
        | Desenho::Redondo { x, y, l, a, .. }
        | Desenho::Borda { x, y, l, a, .. }
        | Desenho::Imagem { x, y, l, a, .. }
        | Desenho::Gradiente { x, y, l, a, .. }
        | Desenho::Controle { x, y, l, a, .. } => (x, y, l, a),
        Desenho::Sombra { x, y, l, a, borrao, .. } => (x - borrao, y - borrao, l + 2 * borrao, a + 2 * borrao),
        Desenho::Texto { x, y, tam, .. } => (x, y, 4000, tam as i32 * 2),
    }
}

/// Um controle de formulário, sem interação (os campos de texto, quando
/// estão inteiros à vista, são desenhados pelo componente da `ui`).
fn desenhar_controle(t: &mut Tela, c: &Campo, (x, y, l, a): (i32, i32, i32, i32)) {
    let borda = 0x9A9A9A;
    match c.tipo {
        TipoCampo::Caixa => {
            t.q.retangulo_redondo(x, y, l, a, 3, borda);
            t.q.retangulo_redondo(x + 1, y + 1, l - 2, a - 2, 2, if c.marcado { COR_LINK } else { 0xFFFFFF });
            if c.marcado {
                t.escrever(x + 2, y, "✓", 0xFFFFFF, true, 13);
            }
        }
        TipoCampo::Radio => {
            t.q.retangulo_redondo(x, y, l, a, l / 2, borda);
            t.q.retangulo_redondo(x + 1, y + 1, l - 2, a - 2, l / 2 - 1, 0xFFFFFF);
            if c.marcado {
                t.q.retangulo_redondo(x + 4, y + 4, l - 8, a - 8, l / 2 - 4, COR_LINK);
            }
        }
        TipoCampo::Lista => {
            t.q.retangulo_arredondado(x, y, l, a, borda);
            t.q.retangulo_arredondado(x + 1, y + 1, l - 2, a - 2, 0xFFFFFF);
            let antes = t.q.recorte;
            let (ax, ay, al, aa) = antes;
            let (rx, ry) = ((x + 2).max(ax), y.max(ay));
            let (fx, fy) = ((x + l - 20).min(ax + al), (y + a).min(ay + aa));
            t.q.recorte = (rx, ry, (fx - rx).max(0), (fy - ry).max(0));
            let texto = c.opcoes.get(c.escolhida).map_or("", |(_, s)| s.as_str());
            t.escrever(x + 8, y + 5, texto, COR_TEXTO, false, 13);
            t.q.recorte = antes;
            t.escrever(x + l - 18, y + 4, "▾", COR_FRACA, false, 13);
        }
        TipoCampo::Oculto => {}
        _ => {
            // Um campo de texto meio fora da vista.
            let cy = y + (a - ui::ALTURA) / 2;
            t.q.retangulo_arredondado(x, cy, l, ui::ALTURA, 0x9A9A9A);
            t.q.retangulo_arredondado(x + 1, cy + 1, l - 2, ui::ALTURA - 2, 0xFFFFFF);
            let antes = t.q.recorte;
            let (ax, ay, al, aa) = antes;
            let (rx, ry) = ((x + 3).max(ax), cy.max(ay));
            let (fx, fy) = ((x + l - 3).min(ax + al), (cy + ui::ALTURA).min(ay + aa));
            t.q.recorte = (rx, ry, (fx - rx).max(0), (fy - ry).max(0));
            let (texto, cor) = if c.texto.texto.is_empty() { (c.rotulo.clone(), 0x9A9A9A) } else { (c.texto.visivel(), COR_TEXTO) };
            t.escrever(x + 7, cy + 5, &texto, cor, false, 13);
            t.q.recorte = antes;
        }
    }
}

fn principal(args: &[&str]) -> i32 {
    let mut janela = match Janela::abrir(900, 620, "Navegador") {
        Ok(j) => j,
        Err(e) => {
            eprintln!("navegador: {}", e);
            return 1;
        }
    };
    let mut ui = Ui::novo();
    let mut nav = Navegador {
        pagina: None,
        historico: Vec::new(),
        atual: 0,
        busca: None,
        busca_nova: true,
        busca_recurso: None,
        buscas_js: Vec::new(),
        endereco: CampoTexto::novo(""),
        visitados: Vec::new(),
        ancora: None,
        rolagem_pendente: None,
        refreshes: 0,
        excecoes: Vec::new(),
        envio: None,
        lista_aberta: None,
        js_ligado: true,
        folha_padrao: css::Folha::ler(css::FOLHA_PADRAO),
        folha_sem_js: css::Folha::ler(FOLHA_SEM_JS),
        cookies: Vec::new(),
        sobre: None,
        apertado_antes: false,
        console_anterior: Vec::new(),
        aviso: None,
    };
    let primeira = args.get(1).and_then(|a| Navegador::interpretar_endereco(a)).or_else(|| Url::ler(INICIO)).unwrap();
    nav.ir(primeira, true);
    ui::laco(&mut janela, &mut ui, |t| nav.desenhar(t));
    nav.cancelar_tudo();
    0
}
