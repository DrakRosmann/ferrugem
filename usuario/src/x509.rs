//! Certificados X.509 (RFC 5280): ler, e conferir a cadeia até uma
//! autoridade conhecida.
//!
//! Um certificado diz "esta chave pública é de www.exemplo.com", assinado
//! por uma autoridade. O servidor manda o dele e os das autoridades
//! intermediárias; cada um tem que estar assinado pelo seguinte, e o último
//! por uma das autoridades "raiz" que o sistema conhece de antemão
//! (/etc/ssl/certs.pem, copiado do Linux onde o FerrugemOS foi compilado).
//!
//! Conferimos: as assinaturas da cadeia, as datas de validade, que os
//! intermediários são autoridades (basicConstraints) e o nome do site
//! (subjectAltName, com curingas "*.exemplo.com"). Não conferimos se o
//! certificado foi revogado, nem as restrições de nomes e de uso.

use crate::assinatura::{self, NomeCurva, Resumo};
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

/// Onde ficam as autoridades raiz em que o sistema confia.
pub const RAIZES: &str = "/etc/ssl/certs.pem";

// -----------------------------------------------------------------------------
// DER: tipo (1 byte), tamanho, conteúdo — e conteúdos dentro de conteúdos
// -----------------------------------------------------------------------------

pub struct Der<'a> {
    dados: &'a [u8],
}

impl<'a> Der<'a> {
    pub fn novo(dados: &'a [u8]) -> Der<'a> {
        Der { dados }
    }

    pub fn vazio(&self) -> bool {
        self.dados.is_empty()
    }

    /// O próximo elemento: (tipo, conteúdo, o elemento inteiro com cabeçalho).
    pub fn proximo(&mut self) -> Option<(u8, &'a [u8], &'a [u8])> {
        let d = self.dados;
        let tipo = *d.first()?;
        let primeiro = *d.get(1)? as usize;
        let (tamanho, cabecalho) = if primeiro < 0x80 {
            (primeiro, 2)
        } else {
            let n = primeiro & 0x7F;
            if n == 0 || n > 4 {
                return None;
            }
            let mut t = 0usize;
            for &b in d.get(2..2 + n)? {
                t = t << 8 | b as usize;
            }
            (t, 2 + n)
        };
        let fim = cabecalho.checked_add(tamanho)?;
        let inteiro = d.get(..fim)?;
        self.dados = &d[fim..];
        Some((tipo, &inteiro[cabecalho..], inteiro))
    }

    /// O próximo elemento, que tem que ser do tipo `tipo`.
    pub fn esperar(&mut self, tipo: u8) -> Option<&'a [u8]> {
        match self.proximo()? {
            (t, conteudo, _) if t == tipo => Some(conteudo),
            _ => None,
        }
    }

    /// Pula o próximo elemento se ele for do tipo `tipo`.
    fn opcional(&mut self, tipo: u8) -> Option<&'a [u8]> {
        if self.dados.first() == Some(&tipo) { self.esperar(tipo) } else { None }
    }
}

// Os identificadores (OIDs) que interessam, já codificados.
const RSA: &[u8] = &[0x2a, 0x86, 0x48, 0x86, 0xf7, 0x0d, 0x01, 0x01, 0x01];
const RSA_SHA256: &[u8] = &[0x2a, 0x86, 0x48, 0x86, 0xf7, 0x0d, 0x01, 0x01, 0x0b];
const RSA_SHA384: &[u8] = &[0x2a, 0x86, 0x48, 0x86, 0xf7, 0x0d, 0x01, 0x01, 0x0c];
const RSA_SHA512: &[u8] = &[0x2a, 0x86, 0x48, 0x86, 0xf7, 0x0d, 0x01, 0x01, 0x0d];
const EC: &[u8] = &[0x2a, 0x86, 0x48, 0xce, 0x3d, 0x02, 0x01];
const P256: &[u8] = &[0x2a, 0x86, 0x48, 0xce, 0x3d, 0x03, 0x01, 0x07];
const P384: &[u8] = &[0x2b, 0x81, 0x04, 0x00, 0x22];
const ECDSA_SHA256: &[u8] = &[0x2a, 0x86, 0x48, 0xce, 0x3d, 0x04, 0x03, 0x02];
const ECDSA_SHA384: &[u8] = &[0x2a, 0x86, 0x48, 0xce, 0x3d, 0x04, 0x03, 0x03];
const ECDSA_SHA512: &[u8] = &[0x2a, 0x86, 0x48, 0xce, 0x3d, 0x04, 0x03, 0x04];
const NOME_COMUM: &[u8] = &[0x55, 0x04, 0x03];
const ORGANIZACAO: &[u8] = &[0x55, 0x04, 0x0a];
const NOMES_ALTERNATIVOS: &[u8] = &[0x55, 0x1d, 0x11];
const RESTRICOES_BASICAS: &[u8] = &[0x55, 0x1d, 0x13];

#[derive(Clone, PartialEq)]
pub enum Chave {
    Rsa { n: Vec<u8>, e: Vec<u8> },
    Ec { curva: NomeCurva, ponto: Vec<u8> },
    /// Um tipo de chave que não sabemos usar (Ed25519, P-521…).
    Outra,
}

impl Chave {
    /// SubjectPublicKeyInfo: SEQUENCE { algoritmo, BIT STRING com a chave }.
    fn ler(spki: &[u8]) -> Option<Chave> {
        let mut d = Der::novo(Der::novo(spki).esperar(0x30)?);
        let mut alg = Der::novo(d.esperar(0x30)?);
        let oid = alg.esperar(0x06)?;
        let bits = d.esperar(0x03)?;
        let chave = bits.get(1..)?; // o primeiro byte diz quantos bits sobram (0)
        Some(if oid == RSA {
            let mut k = Der::novo(Der::novo(chave).esperar(0x30)?);
            let n = k.esperar(0x02)?.to_vec();
            let e = k.esperar(0x02)?.to_vec();
            Chave::Rsa { n, e }
        } else if oid == EC {
            let curva = match alg.esperar(0x06)? {
                P256 => NomeCurva::P256,
                P384 => NomeCurva::P384,
                _ => return Some(Chave::Outra),
            };
            Chave::Ec { curva, ponto: chave.to_vec() }
        } else {
            Chave::Outra
        })
    }

    pub fn descricao(&self) -> String {
        match self {
            Chave::Rsa { n, .. } => format!("RSA de {} bits", n.iter().skip_while(|&&b| b == 0).count() * 8),
            Chave::Ec { curva: NomeCurva::P256, .. } => String::from("ECDSA P-256"),
            Chave::Ec { curva: NomeCurva::P384, .. } => String::from("ECDSA P-384"),
            Chave::Outra => String::from("de um tipo desconhecido"),
        }
    }
}

pub struct Certificado<'a> {
    /// A parte assinada (tbsCertificate), com cabeçalho: é sobre ela que vale a assinatura.
    pub assinado: &'a [u8],
    pub algoritmo: &'a [u8],
    pub assinatura: &'a [u8],
    /// Quem emitiu e de quem é (os "Name" em DER, comparados byte a byte).
    pub emissor: &'a [u8],
    pub sujeito: &'a [u8],
    /// Validade, em segundos desde 1970.
    pub inicio: u64,
    pub fim: u64,
    pub chave_der: &'a [u8],
    pub chave: Chave,
    /// subjectAltName: nomes (dNSName) e endereços IP.
    pub nomes: Vec<&'a [u8]>,
    pub ips: Vec<&'a [u8]>,
    pub tem_nomes_alternativos: bool,
    /// basicConstraints cA: pode assinar outros certificados?
    pub autoridade: bool,
}

/// UTCTime ("AAMMDDhhmmssZ") ou GeneralizedTime ("AAAAMMDDhhmmssZ").
fn ler_data(tipo: u8, t: &[u8]) -> Option<u64> {
    let digitos = |de: usize, n: usize| -> Option<i64> {
        let s = core::str::from_utf8(t.get(de..de + n)?).ok()?;
        s.bytes().all(|b| b.is_ascii_digit()).then(|| s.parse().ok())?
    };
    let (ano, i) = match tipo {
        0x17 => (digitos(0, 2).map(|a| if a >= 50 { 1900 + a } else { 2000 + a })?, 2),
        0x18 => (digitos(0, 4)?, 4),
        _ => return None,
    };
    let (mes, dia, h, m, s) = (digitos(i, 2)?, digitos(i + 2, 2)?, digitos(i + 4, 2)?, digitos(i + 6, 2)?, digitos(i + 8, 2)?);
    // "days_from_civil" (o inverso de sys::data)
    let a = if mes <= 2 { ano - 1 } else { ano };
    let era = a.div_euclid(400);
    let ano_da_era = a - era * 400;
    let dia_do_ano = (153 * (if mes > 2 { mes - 3 } else { mes + 9 }) + 2) / 5 + dia - 1;
    let dia_da_era = ano_da_era * 365 + ano_da_era / 4 - ano_da_era / 100 + dia_do_ano;
    let dias = era * 146097 + dia_da_era - 719468;
    u64::try_from(dias * 86400 + h * 3600 + m * 60 + s).ok()
}

pub fn ler(der: &[u8]) -> Result<Certificado<'_>, String> {
    ler_opcao(der).ok_or_else(|| String::from("certificado malformado"))
}

fn ler_opcao(der: &[u8]) -> Option<Certificado<'_>> {
    let mut externo = Der::novo(der);
    let mut cert = Der::novo(externo.esperar(0x30)?);
    let (_, tbs, assinado) = cert.proximo()?;
    let algoritmo = Der::novo(cert.esperar(0x30)?).esperar(0x06)?;
    let assinatura = cert.esperar(0x03)?.get(1..)?;

    let mut t = Der::novo(tbs);
    t.opcional(0xa0); // versão
    t.esperar(0x02)?; // número de série
    t.esperar(0x30)?; // algoritmo (repetido)
    let (_, _, emissor) = t.proximo()?;
    let mut validade = Der::novo(t.esperar(0x30)?);
    let (ti, vi, _) = validade.proximo()?;
    let (tf, vf, _) = validade.proximo()?;
    let (_, _, sujeito) = t.proximo()?;
    let (_, _, chave_der) = t.proximo()?;
    t.opcional(0x81);
    t.opcional(0x82);
    let mut c = Certificado {
        assinado,
        algoritmo,
        assinatura,
        emissor,
        sujeito,
        inicio: ler_data(ti, vi)?,
        fim: ler_data(tf, vf)?,
        chave_der,
        chave: Chave::ler(chave_der)?,
        nomes: Vec::new(),
        ips: Vec::new(),
        tem_nomes_alternativos: false,
        autoridade: false,
    };
    if let Some(extensoes) = t.opcional(0xa3) {
        let mut lista = Der::novo(Der::novo(extensoes).esperar(0x30)?);
        while !lista.vazio() {
            let mut e = Der::novo(lista.esperar(0x30)?);
            let oid = e.esperar(0x06)?;
            e.opcional(0x01); // "crítica"
            let valor = e.esperar(0x04)?;
            if oid == NOMES_ALTERNATIVOS {
                c.tem_nomes_alternativos = true;
                let mut nomes = Der::novo(Der::novo(valor).esperar(0x30)?);
                while let Some((tipo, v, _)) = nomes.proximo() {
                    match tipo {
                        0x82 => c.nomes.push(v),
                        0x87 => c.ips.push(v),
                        _ => {}
                    }
                }
            } else if oid == RESTRICOES_BASICAS {
                let mut r = Der::novo(Der::novo(valor).esperar(0x30)?);
                c.autoridade = r.opcional(0x01).is_some_and(|v| v.first().is_some_and(|&b| b != 0));
            }
        }
    }
    Some(c)
}

/// Um campo de um Name (o nome comum, a organização…), como texto.
fn campo(nome: &[u8], qual: &[u8]) -> Option<String> {
    let mut d = Der::novo(Der::novo(nome).esperar(0x30)?);
    while let Some(conjunto) = d.esperar(0x31) {
        let mut c = Der::novo(conjunto);
        while let Some(par) = c.esperar(0x30) {
            let mut p = Der::novo(par);
            if p.esperar(0x06)? == qual {
                return Some(String::from_utf8_lossy(p.proximo()?.1).into_owned());
            }
        }
    }
    None
}

/// Um nome para mostrar: o nome comum (ou a organização).
pub fn nome_legivel(nome: &[u8]) -> String {
    campo(nome, NOME_COMUM).or_else(|| campo(nome, ORGANIZACAO)).unwrap_or_else(|| String::from("?"))
}

impl Certificado<'_> {
    /// A assinatura deste certificado confere com a chave de quem o emitiu?
    pub fn assinado_por(&self, chave: &Chave) -> bool {
        let resumo = match self.algoritmo {
            RSA_SHA256 | ECDSA_SHA256 => Resumo::Sha256,
            RSA_SHA384 | ECDSA_SHA384 => Resumo::Sha384,
            RSA_SHA512 | ECDSA_SHA512 => Resumo::Sha512,
            _ => return false, // SHA-1 (quebrado), RSA-PSS em certificado (raro)…
        };
        let hash = resumo.calcular(self.assinado);
        match (chave, matches!(self.algoritmo, RSA_SHA256 | RSA_SHA384 | RSA_SHA512)) {
            (Chave::Rsa { n, e }, true) => assinatura::rsa_pkcs1(n, e, resumo, &hash, self.assinatura),
            (Chave::Ec { curva, ponto }, false) => assinatura::ecdsa(*curva, ponto, &hash, self.assinatura),
            _ => false,
        }
    }

    /// O certificado vale para `host` (um nome ou um IP)?
    pub fn vale_para(&self, host: &str) -> bool {
        let host = host.trim_end_matches('.').to_ascii_lowercase();
        if let Some(ip) = crate::rede::ler_ip(&host) {
            return self.ips.iter().any(|&i| i == ip.to_be_bytes());
        }
        let confere = |padrao: &str| {
            let padrao = padrao.trim_end_matches('.').to_ascii_lowercase();
            if padrao == host {
                return true;
            }
            // "*.exemplo.com" vale para "www.exemplo.com", não para "exemplo.com"
            // nem para "a.b.exemplo.com".
            match (padrao.strip_prefix("*."), host.split_once('.')) {
                (Some(resto), Some((primeiro, depois))) => !primeiro.is_empty() && depois == resto && resto.contains('.'),
                _ => false,
            }
        };
        if self.tem_nomes_alternativos {
            self.nomes.iter().any(|n| confere(&String::from_utf8_lossy(n)))
        } else {
            campo(self.sujeito, NOME_COMUM).is_some_and(|cn| confere(&cn)) // certificados antigos
        }
    }

    fn nomes_texto(&self) -> String {
        let mut v: Vec<String> = self.nomes.iter().map(|n| String::from_utf8_lossy(n).into_owned()).collect();
        v.extend(self.ips.iter().filter(|i| i.len() == 4).map(|i| format!("{}.{}.{}.{}", i[0], i[1], i[2], i[3])));
        if v.is_empty() {
            v.push(nome_legivel(self.sujeito));
        }
        if v.len() > 3 {
            let mais = v.len() - 3;
            v.truncate(3);
            v.push(format!("mais {}", mais));
        }
        v.join(", ")
    }
}

// -----------------------------------------------------------------------------
// As autoridades raiz e a cadeia
// -----------------------------------------------------------------------------

struct Raiz {
    sujeito: Vec<u8>,
    chave_der: Vec<u8>,
    chave: Chave,
}

/// As autoridades em que confiamos.
pub struct Raizes {
    lista: Vec<Raiz>,
}

/// Base64 (o miolo de um arquivo PEM). Ignora espaços e quebras de linha.
pub fn base64(texto: &[u8]) -> Option<Vec<u8>> {
    let mut saida = Vec::new();
    let (mut acumulado, mut bits) = (0u32, 0);
    for &c in texto {
        let v = match c {
            b'A'..=b'Z' => c - b'A',
            b'a'..=b'z' => c - b'a' + 26,
            b'0'..=b'9' => c - b'0' + 52,
            b'+' => 62,
            b'/' => 63,
            b'=' => break,
            b' ' | b'\t' | b'\r' | b'\n' => continue,
            _ => return None,
        };
        acumulado = acumulado << 6 | v as u32;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            saida.push((acumulado >> bits) as u8);
        }
    }
    Some(saida)
}

/// Os certificados de um arquivo PEM ("-----BEGIN CERTIFICATE-----" …), em DER.
pub fn ler_pem(texto: &[u8]) -> Vec<Vec<u8>> {
    const INICIO: &[u8] = b"-----BEGIN CERTIFICATE-----";
    const FIM: &[u8] = b"-----END CERTIFICATE-----";
    let mut certificados = Vec::new();
    let mut resto = texto;
    while let Some(i) = resto.windows(INICIO.len()).position(|j| j == INICIO) {
        resto = &resto[i + INICIO.len()..];
        let Some(f) = resto.windows(FIM.len()).position(|j| j == FIM) else { break };
        if let Some(der) = base64(&resto[..f]) {
            certificados.push(der);
        }
        resto = &resto[f + FIM.len()..];
    }
    certificados
}

impl Raizes {
    pub fn vazia() -> Raizes {
        Raizes { lista: Vec::new() }
    }

    /// As do sistema (/etc/ssl/certs.pem).
    pub fn do_sistema() -> Raizes {
        let mut r = Raizes::vazia();
        if let Ok(texto) = crate::sys::ler_arquivo(RAIZES) {
            r.acrescentar_pem(&texto);
        }
        r
    }

    pub fn acrescentar_pem(&mut self, texto: &[u8]) {
        for der in ler_pem(texto) {
            if let Ok(c) = ler(&der) {
                self.lista.push(Raiz { sujeito: c.sujeito.to_vec(), chave_der: c.chave_der.to_vec(), chave: c.chave });
            }
        }
    }

    pub fn quantas(&self) -> usize {
        self.lista.len()
    }
}

/// O resultado de conferir uma cadeia.
pub struct Conferido {
    /// A chave do certificado do site (para conferir o CertificateVerify).
    pub chave: Chave,
    /// A autoridade raiz que garante o certificado.
    pub raiz: String,
}

/// Confere a cadeia que o servidor mandou (o certificado dele primeiro)
/// para `host`, no instante `agora` (segundos desde 1970).
pub fn conferir_cadeia(cadeia: &[Vec<u8>], host: &str, agora: u64, raizes: &Raizes) -> Result<Conferido, String> {
    let certificados: Vec<Certificado> = cadeia.iter().map(|c| ler(c)).collect::<Result<_, _>>()?;
    let site = certificados.first().ok_or("o servidor não mandou certificado")?;
    if !site.vale_para(host) {
        return Err(format!("o certificado é de outro site ({}), não de {}", site.nomes_texto(), host));
    }
    let mut atual = site;
    let mut usados = alloc::vec![0usize];
    for profundidade in 0..10 {
        if agora < atual.inicio || agora > atual.fim {
            let d = crate::sys::data(if agora < atual.inicio { atual.inicio } else { atual.fim });
            let quando = format!("{:02}/{:02}/{}", d.dia, d.mes, d.ano);
            return Err(if agora < atual.inicio {
                format!("o certificado de \"{}\" só vale a partir de {} (o relógio está certo?)", nome_legivel(atual.sujeito), quando)
            } else {
                format!("o certificado de \"{}\" venceu em {}", nome_legivel(atual.sujeito), quando)
            });
        }
        if profundidade > 0 && !atual.autoridade {
            return Err(format!("\"{}\" assinou um certificado sem ser uma autoridade", nome_legivel(atual.sujeito)));
        }
        // Ele mesmo é uma das raízes (o servidor mandou a raiz junto)?
        if let Some(r) = raizes.lista.iter().find(|r| r.sujeito == atual.sujeito && r.chave_der == atual.chave_der) {
            return Ok(Conferido { chave: site.chave.clone(), raiz: nome_legivel(&r.sujeito) });
        }
        // Uma raiz o assinou?
        if let Some(r) = raizes.lista.iter().find(|r| r.sujeito == atual.emissor && atual.assinado_por(&r.chave)) {
            return Ok(Conferido { chave: site.chave.clone(), raiz: nome_legivel(&r.sujeito) });
        }
        // Um dos intermediários que o servidor mandou?
        let proximo = (0..certificados.len())
            .find(|&i| !usados.contains(&i) && certificados[i].sujeito == atual.emissor && atual.assinado_por(&certificados[i].chave));
        match proximo {
            Some(i) => {
                usados.push(i);
                atual = &certificados[i];
            }
            None if raizes.quantas() == 0 => {
                return Err(format!("não há autoridades conhecidas em {} para conferir o certificado", RAIZES))
            }
            None if raizes.lista.iter().any(|r| r.sujeito == atual.emissor) || certificados.iter().any(|c| c.sujeito == atual.emissor) => {
                return Err(format!(
                    "a assinatura de \"{}\" no certificado de \"{}\" não confere (ou usa um algoritmo que não conheço)",
                    nome_legivel(atual.emissor),
                    nome_legivel(atual.sujeito)
                ))
            }
            None => return Err(format!("o certificado foi emitido por \"{}\", uma autoridade que não conheço", nome_legivel(atual.emissor))),
        }
    }
    Err(String::from("cadeia de certificados longa demais"))
}
