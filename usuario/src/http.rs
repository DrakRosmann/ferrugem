//! URLs e HTTP/1.1 (o cliente: GET, redirecionamentos, "chunked").
//!
//! Um pedido HTTP é texto: a linha do pedido, cabeçalhos e uma linha vazia.
//! A resposta também: a linha de estado ("HTTP/1.1 200 OK"), cabeçalhos,
//! uma linha vazia e o corpo. O corpo pode vir inteiro (com o tamanho no
//! Content-Length, ou até a conexão fechar) ou em pedaços ("chunked": cada
//! pedaço precedido do tamanho em hexadecimal, até um pedaço de tamanho 0).

use crate::rede;
use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

/// Desfaz os "%XX" (e "+" vira espaço, como nos formulários).
pub fn decodificar_porcento(s: &str) -> String {
    let b = s.as_bytes();
    let mut saida = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            b'%' if i + 2 < b.len() => match u8::from_str_radix(&s[i + 1..i + 3], 16) {
                Ok(v) => {
                    saida.push(v);
                    i += 3;
                    continue;
                }
                Err(_) => saida.push(b'%'),
            },
            b'+' => saida.push(b' '),
            c => saida.push(c),
        }
        i += 1;
    }
    String::from_utf8_lossy(&saida).into_owned()
}

/// Um endereço: `esquema://host:porta/caminho?consulta#fragmento`.
#[derive(Clone, PartialEq, Debug)]
pub struct Url {
    pub esquema: String,
    pub host: String,
    pub porta: u16,
    /// O caminho e a consulta (começa com "/").
    pub caminho: String,
    pub fragmento: String,
}

/// Resolve "." e ".." num caminho (sem mexer na consulta).
fn normalizar(caminho: &str) -> String {
    let (caminho, consulta) = match caminho.find('?') {
        Some(i) => caminho.split_at(i),
        None => (caminho, ""),
    };
    let mut partes: Vec<&str> = Vec::new();
    let pedacos: Vec<&str> = caminho.split('/').collect();
    for (i, p) in pedacos.iter().enumerate() {
        let ultimo = i == pedacos.len() - 1;
        match *p {
            "." => {
                if ultimo {
                    partes.push("");
                }
            }
            ".." => {
                if partes.len() > 1 {
                    partes.pop();
                }
                if ultimo {
                    partes.push("");
                }
            }
            p => partes.push(p),
        }
    }
    let mut r = partes.join("/");
    if !r.starts_with('/') {
        r.insert(0, '/');
    }
    r + consulta
}

impl Url {
    pub fn ler(texto: &str) -> Option<Url> {
        let texto = texto.trim();
        let (texto, fragmento) = match texto.split_once('#') {
            Some((t, f)) => (t, f.to_string()),
            None => (texto, String::new()),
        };
        if let Some(caminho) = texto.strip_prefix("sobre:") {
            return Some(Url {
                esquema: "sobre".into(),
                host: String::new(),
                porta: 0,
                caminho: caminho.into(),
                fragmento,
            });
        }
        let (esquema, resto) = texto.split_once("://")?;
        let esquema = esquema.to_ascii_lowercase();
        if esquema == "file" {
            return Some(Url { esquema, host: String::new(), porta: 0, caminho: normalizar(resto), fragmento });
        }
        let padrao = match esquema.as_str() {
            "http" => 80,
            "https" => 443,
            _ => return None,
        };
        let fim = resto.find(['/', '?']).unwrap_or(resto.len());
        let (autoridade, caminho) = resto.split_at(fim);
        let autoridade = autoridade.rsplit('@').next().unwrap_or(""); // sem "usuário@"
        let (host, porta) = match autoridade.rsplit_once(':') {
            Some((h, p)) => (h, p.parse().ok()?),
            None => (autoridade, padrao),
        };
        if host.is_empty() {
            return None;
        }
        let caminho = if caminho.is_empty() { "/".to_string() } else { normalizar(caminho) };
        Some(Url { esquema, host: host.to_ascii_lowercase(), porta, caminho, fragmento })
    }

    /// Um endereço relativo a este (o `href` de um link, o `src` de uma imagem).
    pub fn juntar(&self, relativo: &str) -> Option<Url> {
        let r = relativo.trim();
        if r.contains("://") || r.starts_with("sobre:") {
            return Url::ler(r);
        }
        // Outro esquema ("mailto:", "javascript:", "data:"...): letras,
        // dígitos, "+", "-" e "." antes do ":" (e começando com letra).
        if let Some((esquema, _)) = r.split_once(':') {
            let valido = esquema.starts_with(|c: char| c.is_ascii_alphabetic())
                && esquema.chars().all(|c| c.is_ascii_alphanumeric() || "+-.".contains(c));
            if valido {
                return None;
            }
        }
        let mut u = self.clone();
        u.fragmento.clear();
        if let Some(resto) = r.strip_prefix("//") {
            return Url::ler(&format!("{}://{}", self.esquema, resto));
        }
        let (r, fragmento) = match r.split_once('#') {
            Some((r, f)) => (r, f.to_string()),
            None => (r, String::new()),
        };
        u.fragmento = fragmento;
        if r.is_empty() {
            return Some(u);
        }
        let sem_consulta = self.caminho.split('?').next().unwrap_or("/");
        u.caminho = if r.starts_with('/') {
            normalizar(r)
        } else if r.starts_with('?') {
            format!("{}{}", sem_consulta, r)
        } else {
            let pasta = &sem_consulta[..sem_consulta.rfind('/').map_or(0, |i| i + 1)];
            normalizar(&format!("{}{}", pasta, r))
        };
        Some(u)
    }

    pub fn texto(&self) -> String {
        let mut t = match self.esquema.as_str() {
            "sobre" => format!("sobre:{}", self.caminho),
            "file" => format!("file://{}", self.caminho),
            _ => {
                let padrao = if self.esquema == "https" { 443 } else { 80 };
                if self.porta == padrao {
                    format!("{}://{}{}", self.esquema, self.host, self.caminho)
                } else {
                    format!("{}://{}:{}{}", self.esquema, self.host, self.porta, self.caminho)
                }
            }
        };
        if !self.fragmento.is_empty() {
            t.push('#');
            t.push_str(&self.fragmento);
        }
        t
    }
}

pub struct Resposta {
    pub status: u16,
    pub cabecalhos: Vec<(String, String)>,
    pub corpo: Vec<u8>,
    /// O endereço final (depois dos redirecionamentos).
    pub url: Url,
    /// Como a conexão foi protegida (None: HTTP, sem cifra).
    pub seguranca: Option<String>,
}

impl Resposta {
    pub fn cabecalho(&self, nome: &str) -> Option<&str> {
        self.cabecalhos.iter().find(|(n, _)| n.eq_ignore_ascii_case(nome)).map(|(_, v)| v.as_str())
    }

    /// O Content-Type sem os parâmetros ("text/html; charset=utf-8" → "text/html").
    pub fn tipo(&self) -> String {
        let t = self.cabecalho("content-type").unwrap_or("");
        t.split(';').next().unwrap_or("").trim().to_ascii_lowercase()
    }
}

/// Como os bytes chegam: uma conexão TCP, ou (no futuro) TLS por cima dela.
pub trait Transporte {
    fn escrever(&mut self, dados: &[u8]) -> Result<(), String>;
    fn ler(&mut self, buf: &mut [u8]) -> Result<usize, String>;
    /// Uma descrição da proteção da conexão, para mostrar ao usuário.
    fn seguranca(&self) -> Option<String> {
        None
    }
}

impl Transporte for rede::Conexao {
    fn escrever(&mut self, dados: &[u8]) -> Result<(), String> {
        rede::Conexao::escrever(self, dados).map_err(|e| e.to_string())
    }
    fn ler(&mut self, buf: &mut [u8]) -> Result<usize, String> {
        rede::Conexao::ler(self, buf).map_err(|e| e.to_string())
    }
}

/// Como abrir a conexão de um endereço (um teste pode trocar).
pub type Abrir<'a> = &'a dyn Fn(&Url) -> Result<alloc::boxed::Box<dyn Transporte>, String>;

/// TCP para http://, TLS 1.3 por cima do TCP para https://, conferindo o
/// certificado com as autoridades `raizes` (None: sem conferir).
pub fn abrir(url: &Url, raizes: Option<&crate::x509::Raizes>) -> Result<alloc::boxed::Box<dyn Transporte>, String> {
    match url.esquema.as_str() {
        "http" => Ok(alloc::boxed::Box::new(rede::conectar(&url.host, url.porta)?)),
        "https" => Ok(alloc::boxed::Box::new(crate::tls::Tls::conectar(&url.host, url.porta, raizes)?)),
        e => Err(format!("esquema não suportado: {}", e)),
    }
}

/// `abrir` com as autoridades do sistema (/etc/ssl/certs.pem).
pub fn abrir_simples(url: &Url) -> Result<alloc::boxed::Box<dyn Transporte>, String> {
    abrir(url, Some(&crate::x509::Raizes::do_sistema()))
}

/// GET, seguindo até 8 redirecionamentos. `progresso` recebe os bytes já
/// recebidos (para mostrar ao usuário).
pub fn obter(url: &Url, abrir: Abrir<'_>, progresso: &mut dyn FnMut(usize)) -> Result<Resposta, String> {
    obter_com(url, abrir, None, progresso)
}

/// Os pares (nome, valor) de um formulário, como o navegador os envia
/// ("application/x-www-form-urlencoded"): `a=1&b=dois+tr%C3%AAs`.
pub fn codificar_formulario(pares: &[(String, String)]) -> String {
    let codificar = |t: &str| -> String {
        t.bytes()
            .map(|b| match b {
                b'a'..=b'z' | b'A'..=b'Z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'*' => String::from(b as char),
                b' ' => String::from("+"),
                b => format!("%{:02X}", b),
            })
            .collect()
    };
    pares.iter().map(|(n, v)| format!("{}={}", codificar(n), codificar(v))).collect::<Vec<_>>().join("&")
}

/// Como `obter`, mas com `envio` (um formulário já codificado) vira um POST.
pub fn obter_com(url: &Url, abrir: Abrir<'_>, envio: Option<&[u8]>, progresso: &mut dyn FnMut(usize)) -> Result<Resposta, String> {
    let mut url = url.clone();
    let mut envio = envio;
    for _ in 0..8 {
        let mut c = abrir(&url)?;
        let host = if (url.esquema == "http" && url.porta == 80) || (url.esquema == "https" && url.porta == 443) {
            url.host.clone()
        } else {
            format!("{}:{}", url.host, url.porta)
        };
        let corpo = match envio {
            Some(e) => format!("Content-Type: application/x-www-form-urlencoded\r\nContent-Length: {}\r\n", e.len()),
            None => String::new(),
        };
        let pedido = format!(
            "{} {} HTTP/1.1\r\nHost: {}\r\nUser-Agent: Ferrugem/0.1 (FerrugemOS)\r\n\
             Accept: text/html, text/plain, image/png, image/jpeg, */*;q=0.5\r\nAccept-Language: pt-BR, pt, en;q=0.5\r\n\
             Accept-Encoding: identity\r\n{}Connection: close\r\n\r\n",
            if envio.is_some() { "POST" } else { "GET" },
            url.caminho,
            host,
            corpo
        );
        let mut pedido = pedido.into_bytes();
        pedido.extend_from_slice(envio.unwrap_or(&[]));
        c.escrever(&pedido)?;
        let mut bruto = Vec::new();
        let mut buf = [0u8; 8192];
        loop {
            match c.ler(&mut buf)? {
                0 => break,
                n => {
                    bruto.extend_from_slice(&buf[..n]);
                    progresso(bruto.len());
                }
            }
        }
        let (status, cabecalhos, corpo) = interpretar(&bruto)?;
        let r = Resposta { status, cabecalhos, corpo, url: url.clone(), seguranca: c.seguranca() };
        if matches!(status, 301 | 302 | 303 | 307 | 308) {
            if let Some(destino) = r.cabecalho("location").and_then(|l| url.juntar(l)) {
                url = destino;
                if !matches!(status, 307 | 308) {
                    envio = None; // depois de um POST, o destino é pedido com GET
                }
                continue;
            }
        }
        return Ok(r);
    }
    Err(String::from("redirecionamentos demais"))
}

/// Separa uma resposta HTTP em (estado, cabeçalhos, corpo).
pub fn interpretar(bruto: &[u8]) -> Result<(u16, Vec<(String, String)>, Vec<u8>), String> {
    let fim = bruto
        .windows(4)
        .position(|j| j == b"\r\n\r\n")
        .ok_or_else(|| String::from("resposta HTTP incompleta (a conexão caiu?)"))?;
    let cabeca = String::from_utf8_lossy(&bruto[..fim]);
    let mut linhas = cabeca.split("\r\n");
    let primeira = linhas.next().unwrap_or("");
    let mut partes = primeira.splitn(3, ' ');
    if !partes.next().unwrap_or("").starts_with("HTTP/") {
        return Err(format!("isso não é uma resposta HTTP: {}", primeira));
    }
    let status = partes.next().and_then(|s| s.parse().ok()).ok_or_else(|| format!("estado inválido: {}", primeira))?;
    let cabecalhos: Vec<(String, String)> = linhas
        .filter_map(|l| l.split_once(':'))
        .map(|(n, v)| (n.trim().to_string(), v.trim().to_string()))
        .collect();
    let valor = |nome: &str| cabecalhos.iter().find(|(n, _)| n.eq_ignore_ascii_case(nome)).map(|(_, v)| v.as_str());
    let mut corpo = bruto[fim + 4..].to_vec();
    if valor("transfer-encoding").is_some_and(|v| v.to_ascii_lowercase().contains("chunked")) {
        corpo = juntar_pedacos(&corpo);
    } else if let Some(n) = valor("content-length").and_then(|v| v.parse::<usize>().ok()) {
        corpo.truncate(n);
    }
    Ok((status, cabecalhos, corpo))
}

/// "chunked": `tamanho em hexa\r\n` + dados + `\r\n`, até o tamanho 0.
fn juntar_pedacos(mut d: &[u8]) -> Vec<u8> {
    let mut corpo = Vec::new();
    loop {
        let Some(fim) = d.windows(2).position(|j| j == b"\r\n") else { break };
        let linha = String::from_utf8_lossy(&d[..fim]);
        let Ok(n) = usize::from_str_radix(linha.split(';').next().unwrap_or("").trim(), 16) else { break };
        if n == 0 {
            break;
        }
        let dados = &d[fim + 2..];
        let n = n.min(dados.len());
        corpo.extend_from_slice(&dados[..n]);
        d = dados.get(n + 2..).unwrap_or(&[]);
    }
    corpo
}
