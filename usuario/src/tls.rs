//! TLS 1.3 (RFC 8446), o lado do cliente: o "S" do HTTPS.
//!
//! ```text
//!   cliente                                   servidor
//!   ClientHello: versões, cifras, X25519 ───▶
//!     (a nossa metade da troca de chaves)
//!                                        ◀─── ServerHello: a metade dele
//!   ── daqui em diante, tudo cifrado com chaves derivadas do segredo ──
//!                                        ◀─── EncryptedExtensions
//!                                        ◀─── Certificate, CertificateVerify
//!                                        ◀─── Finished (prova que ele tem o segredo)
//!   Finished                              ───▶
//!   ── chaves novas: os dados da aplicação (o HTTP) ──
//! ```
//!
//! As chaves saem do "key schedule": HKDF sobre o segredo do X25519 e o
//! hash de tudo o que foi dito até aquele ponto (assim, mexer em qualquer
//! mensagem do aperto de mão estraga as chaves e o Finished não confere).
//!
//! **O que falta:** conferir o certificado do servidor (a cadeia até uma
//! autoridade conhecida, o nome, a assinatura do CertificateVerify). Sem
//! isso, a conexão é cifrada (quem só escuta não entende nada), mas quem
//! estiver no meio do caminho pode se passar pelo site.

use crate::cripto::{self, Aes128};
use crate::http::Transporte;
use crate::rede::{self, Conexao};
use crate::sha256::{sha256, Sha256};
use crate::sys;
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

const HANDSHAKE: u8 = 22;
const DADOS: u8 = 23;
const ALERTA: u8 = 21;
const MUDAR_CIFRA: u8 = 20;

const AES_128_GCM: u16 = 0x1301;
const CHACHA20_POLY1305: u16 = 0x1303;

enum Algoritmo {
    ChaCha([u8; 32]),
    Aes(alloc::boxed::Box<Aes128>),
}

/// Um sentido da conexão: a cifra, a chave, o IV e o número do registro.
struct Cifra {
    algoritmo: Algoritmo,
    iv: [u8; 12],
    sequencia: u64,
}

impl Cifra {
    fn nova(suite: u16, segredo: &[u8]) -> Cifra {
        let iv: [u8; 12] = cripto::expandir_rotulo(segredo, "iv", &[], 12).try_into().unwrap();
        let algoritmo = if suite == AES_128_GCM {
            let chave: [u8; 16] = cripto::expandir_rotulo(segredo, "key", &[], 16).try_into().unwrap();
            Algoritmo::Aes(alloc::boxed::Box::new(Aes128::novo(&chave)))
        } else {
            Algoritmo::ChaCha(cripto::expandir_rotulo(segredo, "key", &[], 32).try_into().unwrap())
        };
        Cifra { algoritmo, iv, sequencia: 0 }
    }

    /// O nonce de cada registro: o IV com o número do registro misturado no fim.
    fn nonce(&mut self) -> [u8; 12] {
        let mut n = self.iv;
        for (b, s) in n[4..].iter_mut().zip(self.sequencia.to_be_bytes()) {
            *b ^= s;
        }
        self.sequencia += 1;
        n
    }

    /// Um registro cifrado: o tipo verdadeiro vai dentro, por fora é sempre "dados".
    fn cifrar(&mut self, tipo: u8, dados: &[u8]) -> Vec<u8> {
        let mut interno = dados.to_vec();
        interno.push(tipo);
        let tamanho = interno.len() + 16;
        let cabecalho = [DADOS, 3, 3, (tamanho >> 8) as u8, tamanho as u8];
        let nonce = self.nonce();
        let cifrado = match &self.algoritmo {
            Algoritmo::ChaCha(k) => cripto::chacha20_poly1305_cifrar(k, &nonce, &cabecalho, &interno),
            Algoritmo::Aes(aes) => aes.gcm_cifrar(&nonce, &cabecalho, &interno),
        };
        let mut registro = cabecalho.to_vec();
        registro.extend_from_slice(&cifrado);
        registro
    }

    /// Decifra um registro: (tipo verdadeiro, conteúdo).
    fn decifrar(&mut self, cabecalho: &[u8], corpo: &[u8]) -> Result<(u8, Vec<u8>), String> {
        let nonce = self.nonce();
        let interno = match &self.algoritmo {
            Algoritmo::ChaCha(k) => cripto::chacha20_poly1305_decifrar(k, &nonce, cabecalho, corpo),
            Algoritmo::Aes(aes) => aes.gcm_decifrar(&nonce, cabecalho, corpo),
        }
        .ok_or_else(|| String::from("TLS: um registro chegou adulterado (a etiqueta não confere)"))?;
        // O fim pode ter zeros de enchimento; o último byte não-zero é o tipo.
        let fim = interno.iter().rposition(|&b| b != 0).ok_or("TLS: registro vazio")?;
        Ok((interno[fim], interno[..fim].to_vec()))
    }
}

pub struct Tls {
    conexao: Conexao,
    bruto: Vec<u8>,
    saida: Cifra,
    entrada: Cifra,
    /// Os segredos da aplicação (para o KeyUpdate).
    segredo_saida: Vec<u8>,
    segredo_entrada: Vec<u8>,
    /// Dados da aplicação já decifrados e ainda não lidos.
    pendentes: Vec<u8>,
    fechada: bool,
    /// O nome da cifra combinada (para mostrar).
    pub cifra: &'static str,
}

fn descricao_alerta(codigo: u8) -> &'static str {
    match codigo {
        40 => "o aperto de mão falhou (o servidor não aceitou nada do que oferecemos)",
        42 | 43 | 44 | 45 | 46 => "problema com o certificado",
        47 => "parâmetro inválido",
        50 => "mensagem malformada",
        51 => "falha ao decifrar",
        70 => "versão do protocolo não suportada (o servidor não fala TLS 1.3?)",
        71 => "segurança insuficiente",
        80 => "erro interno do servidor",
        109 => "extensão obrigatória faltando",
        112 => "nome do servidor desconhecido (SNI)",
        120 => "nenhum protocolo de aplicação em comum (ALPN)",
        _ => "alerta",
    }
}

/// Lê um registro da conexão: (tipo, cabeçalho, corpo).
fn ler_registro(conexao: &mut Conexao, bruto: &mut Vec<u8>) -> Result<Option<(u8, [u8; 5], Vec<u8>)>, String> {
    loop {
        if bruto.len() >= 5 {
            let tamanho = u16::from_be_bytes([bruto[3], bruto[4]]) as usize;
            if tamanho > 16384 + 256 {
                return Err(String::from("TLS: registro grande demais"));
            }
            if bruto.len() >= 5 + tamanho {
                let cabecalho: [u8; 5] = bruto[..5].try_into().unwrap();
                let corpo = bruto[5..5 + tamanho].to_vec();
                bruto.drain(..5 + tamanho);
                return Ok(Some((cabecalho[0], cabecalho, corpo)));
            }
        }
        let mut buf = [0u8; 8192];
        match conexao.ler(&mut buf) {
            Ok(0) => return if bruto.is_empty() { Ok(None) } else { Err(String::from("TLS: a conexão caiu no meio de um registro")) },
            Ok(n) => bruto.extend_from_slice(&buf[..n]),
            Err(e) => return Err(format!("TLS: {}", e)),
        }
    }
}

fn extensao(tipo: u16, dados: &[u8]) -> Vec<u8> {
    let mut e = tipo.to_be_bytes().to_vec();
    e.extend_from_slice(&(dados.len() as u16).to_be_bytes());
    e.extend_from_slice(dados);
    e
}

/// Uma mensagem de handshake: tipo (1 byte) + tamanho (3) + corpo.
fn mensagem(tipo: u8, corpo: &[u8]) -> Vec<u8> {
    let mut m = alloc::vec![tipo, (corpo.len() >> 16) as u8, (corpo.len() >> 8) as u8, corpo.len() as u8];
    m.extend_from_slice(corpo);
    m
}

fn ola_do_cliente(host: &str, aleatorio: &[u8; 32], sessao: &[u8; 32], publica: &[u8; 32]) -> Vec<u8> {
    let mut c = alloc::vec![3, 3]; // "TLS 1.2" (o 1.3 vai numa extensão)
    c.extend_from_slice(aleatorio);
    c.push(32);
    c.extend_from_slice(sessao); // finge retomar uma sessão (alguns firewalls exigem)
    c.extend_from_slice(&[0, 4]);
    c.extend_from_slice(&CHACHA20_POLY1305.to_be_bytes());
    c.extend_from_slice(&AES_128_GCM.to_be_bytes());
    c.extend_from_slice(&[1, 0]); // sem compressão
    let mut ext = Vec::new();
    // server_name: o nome do site (um servidor pode ter vários)
    if rede::ler_ip(host).is_none() {
        let mut sni = ((host.len() + 3) as u16).to_be_bytes().to_vec();
        sni.push(0);
        sni.extend_from_slice(&(host.len() as u16).to_be_bytes());
        sni.extend_from_slice(host.as_bytes());
        ext.extend(extensao(0x0000, &sni));
    }
    ext.extend(extensao(0x000a, &[0, 2, 0x00, 0x1d])); // supported_groups: x25519
    // signature_algorithms: os que o servidor pode usar no CertificateVerify
    let assinaturas: [u16; 9] = [0x0403, 0x0804, 0x0401, 0x0503, 0x0805, 0x0501, 0x0806, 0x0601, 0x0807];
    let mut sa = ((assinaturas.len() * 2) as u16).to_be_bytes().to_vec();
    for a in assinaturas {
        sa.extend_from_slice(&a.to_be_bytes());
    }
    ext.extend(extensao(0x000d, &sa));
    ext.extend(extensao(0x0010, b"\x00\x09\x08http/1.1")); // ALPN
    ext.extend(extensao(0x002b, &[2, 3, 4])); // supported_versions: TLS 1.3
    ext.extend(extensao(0x002d, &[1, 1])); // psk_key_exchange_modes
    let mut ks = alloc::vec![0, 36, 0x00, 0x1d, 0, 32]; // key_share: x25519
    ks.extend_from_slice(publica);
    ext.extend(extensao(0x0033, &ks));
    c.extend_from_slice(&(ext.len() as u16).to_be_bytes());
    c.extend_from_slice(&ext);
    mensagem(1, &c)
}

/// O ServerHello: (cifra escolhida, chave pública do servidor).
fn ler_ola_do_servidor(m: &[u8]) -> Result<(u16, [u8; 32]), String> {
    let erro = || String::from("TLS: ServerHello malformado");
    const RETRY: [u8; 32] = [
        0xCF, 0x21, 0xAD, 0x74, 0xE5, 0x9A, 0x61, 0x11, 0xBE, 0x1D, 0x8C, 0x02, 0x1E, 0x65, 0xB8, 0x91, 0xC2, 0xA2,
        0x11, 0x16, 0x7A, 0xBB, 0x8C, 0x5E, 0x07, 0x9E, 0x09, 0xE2, 0xC8, 0xA8, 0x33, 0x9C,
    ];
    if m.get(2..34) == Some(&RETRY) {
        return Err(String::from("o servidor pediu outra curva para a troca de chaves (só temos X25519)"));
    }
    let sessao = *m.get(34).ok_or_else(erro)? as usize;
    let mut i = 35 + sessao;
    let suite = u16::from_be_bytes(m.get(i..i + 2).ok_or_else(erro)?.try_into().unwrap());
    i += 3;
    let total = u16::from_be_bytes(m.get(i..i + 2).ok_or_else(erro)?.try_into().unwrap()) as usize;
    i += 2;
    let fim = i + total;
    let (mut versao, mut chave) = (0u16, None);
    while i + 4 <= fim.min(m.len()) {
        let tipo = u16::from_be_bytes([m[i], m[i + 1]]);
        let n = u16::from_be_bytes([m[i + 2], m[i + 3]]) as usize;
        let d = m.get(i + 4..i + 4 + n).ok_or_else(erro)?;
        match tipo {
            0x002b if n == 2 => versao = u16::from_be_bytes([d[0], d[1]]),
            0x0033 if n == 36 && d[..2] == [0, 0x1d] => chave = Some(<[u8; 32]>::try_from(&d[4..36]).unwrap()),
            _ => {}
        }
        i += 4 + n;
    }
    if versao != 0x0304 {
        return Err(String::from("o servidor não fala TLS 1.3 (este navegador ainda não fala o 1.2)"));
    }
    if suite != AES_128_GCM && suite != CHACHA20_POLY1305 {
        return Err(format!("TLS: o servidor escolheu uma cifra que não oferecemos ({:#06x})", suite));
    }
    Ok((suite, chave.ok_or("TLS: o servidor não mandou a chave X25519")?))
}

impl Tls {
    /// Conecta e faz o aperto de mão.
    pub fn conectar(host: &str, porta: u16) -> Result<Tls, String> {
        let mut conexao = rede::conectar(host, porta)?;
        let mut sorte = [0u8; 96];
        sys::aleatorio(&mut sorte);
        let privada: [u8; 32] = sorte[..32].try_into().unwrap();
        let publica = cripto::x25519_publica(&privada);
        let ola = ola_do_cliente(host, sorte[32..64].try_into().unwrap(), sorte[64..].try_into().unwrap(), &publica);
        let mut registro = alloc::vec![HANDSHAKE, 3, 1, (ola.len() >> 8) as u8, ola.len() as u8];
        registro.extend_from_slice(&ola);
        conexao.escrever(&registro).map_err(|e| format!("TLS: {}", e))?;
        let mut conversa = Sha256::novo();
        conversa.atualizar(&ola);

        let mut bruto = Vec::new();
        let mut mensagens: Vec<u8> = Vec::new(); // handshake ainda não processado
        let mut cifras: Option<(Cifra, Cifra, Vec<u8>, Vec<u8>, u16)> = None; // (servidor, cliente, segredos, suite)
        let mut segredo_handshake = Vec::new();
        let mut pediu_certificado = false;
        loop {
            let (tipo, cabecalho, corpo) = ler_registro(&mut conexao, &mut bruto)?.ok_or("TLS: o servidor fechou a conexão no aperto de mão")?;
            let (tipo, corpo) = match (tipo, cifras.as_mut()) {
                (MUDAR_CIFRA, _) => continue, // só por compatibilidade: ignorado no 1.3
                (DADOS, Some((servidor, ..))) => servidor.decifrar(&cabecalho, &corpo)?,
                (t, _) => (t, corpo),
            };
            if tipo == ALERTA {
                let codigo = corpo.get(1).copied().unwrap_or(0);
                return Err(format!("TLS: o servidor recusou: {} ({})", descricao_alerta(codigo), codigo));
            }
            if tipo != HANDSHAKE {
                return Err(format!("TLS: registro inesperado (tipo {}) no aperto de mão", tipo));
            }
            mensagens.extend_from_slice(&corpo);
            // Cada mensagem completa que chegou.
            while mensagens.len() >= 4 {
                let n = (mensagens[1] as usize) << 16 | (mensagens[2] as usize) << 8 | mensagens[3] as usize;
                if mensagens.len() < 4 + n {
                    break;
                }
                let m: Vec<u8> = mensagens.drain(..4 + n).collect();
                match m[0] {
                    2 => {
                        let (suite, chave_do_servidor) = ler_ola_do_servidor(&m[4..])?;
                        conversa.atualizar(&m);
                        let compartilhado = cripto::x25519(&privada, &chave_do_servidor);
                        if compartilhado == [0; 32] {
                            return Err(String::from("TLS: chave do servidor inválida"));
                        }
                        let vazio = sha256(&[]);
                        let cedo = cripto::hkdf_extrair(&[0; 32], &[0; 32]);
                        let derivado = cripto::expandir_rotulo(&cedo, "derived", &vazio, 32);
                        segredo_handshake = cripto::hkdf_extrair(&derivado, &compartilhado).to_vec();
                        let h = conversa.terminar();
                        let s = cripto::expandir_rotulo(&segredo_handshake, "s hs traffic", &h, 32);
                        let c = cripto::expandir_rotulo(&segredo_handshake, "c hs traffic", &h, 32);
                        cifras = Some((Cifra::nova(suite, &s), Cifra::nova(suite, &c), s, c, suite));
                    }
                    13 => {
                        pediu_certificado = true;
                        conversa.atualizar(&m);
                    }
                    20 => {
                        let Some((_, mut cliente, segredo_s, segredo_c, suite)) = cifras.take() else {
                            return Err(String::from("TLS: Finished antes do ServerHello"));
                        };
                        // O Finished do servidor: HMAC da conversa com uma chave
                        // que só quem tem o segredo calcula.
                        let chave = cripto::expandir_rotulo(&segredo_s, "finished", &[], 32);
                        let esperado = cripto::hmac_sha256(&chave, &[&conversa.terminar()]);
                        if !cripto::iguais(&esperado, &m[4..]) {
                            return Err(String::from("TLS: o Finished do servidor não confere"));
                        }
                        conversa.atualizar(&m);
                        // As chaves da aplicação (com a conversa até o Finished do servidor).
                        let h = conversa.terminar();
                        let derivado = cripto::expandir_rotulo(&segredo_handshake, "derived", &sha256(&[]), 32);
                        let mestre = cripto::hkdf_extrair(&derivado, &[0; 32]);
                        let app_c = cripto::expandir_rotulo(&mestre, "c ap traffic", &h, 32);
                        let app_s = cripto::expandir_rotulo(&mestre, "s ap traffic", &h, 32);
                        // A nossa resposta: (um certificado vazio, se pedido) e o nosso Finished.
                        let mut resposta = alloc::vec![MUDAR_CIFRA, 3, 3, 0, 1, 1];
                        if pediu_certificado {
                            let vazio = mensagem(11, &[0, 0, 0, 0]);
                            conversa.atualizar(&vazio);
                            resposta.extend(cliente.cifrar(HANDSHAKE, &vazio));
                        }
                        let chave = cripto::expandir_rotulo(&segredo_c, "finished", &[], 32);
                        let fim = mensagem(20, &cripto::hmac_sha256(&chave, &[&conversa.terminar()]));
                        resposta.extend(cliente.cifrar(HANDSHAKE, &fim));
                        conexao.escrever(&resposta).map_err(|e| format!("TLS: {}", e))?;
                        return Ok(Tls {
                            conexao,
                            bruto,
                            saida: Cifra::nova(suite, &app_c),
                            entrada: Cifra::nova(suite, &app_s),
                            segredo_saida: app_c,
                            segredo_entrada: app_s,
                            pendentes: Vec::new(),
                            fechada: false,
                            cifra: if suite == AES_128_GCM { "AES-128-GCM" } else { "ChaCha20-Poly1305" },
                        });
                    }
                    // EncryptedExtensions, Certificate, CertificateVerify: entram na conversa.
                    // (Conferir o certificado é o que falta: veja o começo do arquivo.)
                    _ => conversa.atualizar(&m),
                }
            }
        }
    }

    fn suite(&self) -> u16 {
        if matches!(self.saida.algoritmo, Algoritmo::Aes(_)) { AES_128_GCM } else { CHACHA20_POLY1305 }
    }

    /// Mensagens de handshake depois do aperto de mão (bilhetes de sessão,
    /// troca de chaves).
    fn depois_do_handshake(&mut self, m: &[u8]) -> Result<(), String> {
        let mut i = 0;
        while i + 4 <= m.len() {
            let n = (m[i + 1] as usize) << 16 | (m[i + 2] as usize) << 8 | m[i + 3] as usize;
            if m[i] == 24 {
                // KeyUpdate: o servidor passa a usar chaves novas (e talvez peça o mesmo).
                let suite = self.suite();
                self.segredo_entrada = cripto::expandir_rotulo(&self.segredo_entrada, "traffic upd", &[], 32);
                self.entrada = Cifra::nova(suite, &self.segredo_entrada);
                if m.get(i + 4) == Some(&1) {
                    let pedido = self.saida.cifrar(HANDSHAKE, &mensagem(24, &[0]));
                    self.conexao.escrever(&pedido).map_err(|e| format!("TLS: {}", e))?;
                    self.segredo_saida = cripto::expandir_rotulo(&self.segredo_saida, "traffic upd", &[], 32);
                    self.saida = Cifra::nova(suite, &self.segredo_saida);
                }
            }
            i += 4 + n; // NewSessionTicket (4) e o resto: ignorados
        }
        Ok(())
    }
}

impl Transporte for Tls {
    fn escrever(&mut self, dados: &[u8]) -> Result<(), String> {
        for pedaco in dados.chunks(16384) {
            let registro = self.saida.cifrar(DADOS, pedaco);
            self.conexao.escrever(&registro).map_err(|e| format!("TLS: {}", e))?;
        }
        Ok(())
    }

    fn ler(&mut self, buf: &mut [u8]) -> Result<usize, String> {
        while self.pendentes.is_empty() && !self.fechada {
            let Some((_, cabecalho, corpo)) = ler_registro(&mut self.conexao, &mut self.bruto)? else {
                self.fechada = true; // fechou sem avisar: o fim, de todo modo
                break;
            };
            if cabecalho[0] == MUDAR_CIFRA {
                continue;
            }
            let (tipo, conteudo) = self.entrada.decifrar(&cabecalho, &corpo)?;
            match tipo {
                DADOS => self.pendentes = conteudo,
                HANDSHAKE => self.depois_do_handshake(&conteudo)?,
                ALERTA if conteudo.get(1) == Some(&0) => self.fechada = true, // close_notify
                ALERTA => {
                    let codigo = conteudo.get(1).copied().unwrap_or(0);
                    return Err(format!("TLS: alerta do servidor: {} ({})", descricao_alerta(codigo), codigo));
                }
                _ => {}
            }
        }
        let n = buf.len().min(self.pendentes.len());
        buf[..n].copy_from_slice(&self.pendentes[..n]);
        self.pendentes.drain(..n);
        Ok(n)
    }
}
