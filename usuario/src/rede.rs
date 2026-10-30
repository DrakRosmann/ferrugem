//! Rede: endereços IPv4, nomes (DNS) e conexões TCP.
//!
//! Para achar o IP de um nome, olhamos o /etc/hosts e, se não estiver lá,
//! perguntamos ao servidor de DNS do /etc/resolv.conf (na rede do QEMU,
//! 10.0.2.3): uma pergunta num datagrama UDP para a porta 53, a resposta em
//! outro.

use crate::sys;
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

/// "10.0.2.15" → 0x0A00020F
pub fn ler_ip(texto: &str) -> Option<u32> {
    let partes: Vec<&str> = texto.split('.').collect();
    if partes.len() != 4 {
        return None;
    }
    let mut ip = 0u32;
    for p in partes {
        ip = ip << 8 | p.parse::<u8>().ok()? as u32;
    }
    Some(ip)
}

pub fn texto_ip(ip: u32) -> String {
    let b = ip.to_be_bytes();
    format!("{}.{}.{}.{}", b[0], b[1], b[2], b[3])
}

fn ler_texto(caminho: &str) -> String {
    String::from_utf8_lossy(&sys::ler_arquivo(caminho).unwrap_or_default()).into_owned()
}

/// O IP de um nome (ou de um IP escrito como texto).
pub fn resolver(nome: &str) -> Result<u32, String> {
    if let Some(ip) = ler_ip(nome) {
        return Ok(ip);
    }
    let nome = nome.trim_end_matches('.').to_ascii_lowercase();
    if nome == "localhost" {
        return Ok(0x7F00_0001);
    }
    for linha in ler_texto("/etc/hosts").lines() {
        let mut partes = linha.split('#').next().unwrap_or("").split_whitespace();
        if let Some(ip) = partes.next().and_then(ler_ip) {
            if partes.any(|n| n.eq_ignore_ascii_case(&nome)) {
                return Ok(ip);
            }
        }
    }
    let servidor = ler_texto("/etc/resolv.conf")
        .lines()
        .find_map(|l| l.strip_prefix("nameserver").and_then(|ip| ler_ip(ip.trim())))
        .unwrap_or(0x0A00_0203);
    perguntar_dns(&nome, servidor)
}

/// Uma pergunta "tipo A" (o endereço IPv4) ao servidor de DNS.
fn perguntar_dns(nome: &str, servidor: u32) -> Result<u32, String> {
    let id = sys::tempo_ms() as u16 ^ 0x5A5A;
    let mut pergunta = Vec::new();
    pergunta.extend_from_slice(&id.to_be_bytes());
    pergunta.extend_from_slice(&[0x01, 0x00]); // "pode perguntar a outros por mim" (recursão)
    pergunta.extend_from_slice(&[0, 1, 0, 0, 0, 0, 0, 0]); // uma pergunta
    for parte in nome.split('.') {
        if parte.is_empty() || parte.len() > 63 {
            return Err(format!("nome inválido: {}", nome));
        }
        pergunta.push(parte.len() as u8);
        pergunta.extend_from_slice(parte.as_bytes());
    }
    pergunta.extend_from_slice(&[0, 0, 1, 0, 1]); // fim do nome; tipo A; classe IN
    let fd = sys::socket_inet(sys::SOCK_DGRAM, 0).map_err(|e| format!("socket: {}", e))?;
    let mut resposta = [0u8; 1500];
    let mut resultado = Err(format!("{}: o servidor de DNS ({}) não respondeu", nome, texto_ip(servidor)));
    'tentativas: for _ in 0..3 {
        if let Err(e) = sys::enviar_para_inet(fd, &pergunta, servidor, 53) {
            resultado = Err(format!("DNS: {}", e));
            break;
        }
        let prazo = sys::tempo_ms() + 2000;
        while sys::tempo_ms() < prazo {
            let falta = (prazo - sys::tempo_ms()) as i32;
            if sys::poll(&[(fd, sys::POLLIN)], falta).map_or(true, |r| r[0] & sys::POLLIN == 0) {
                continue 'tentativas;
            }
            let Ok((n, _, _)) = sys::receber_de_inet(fd, &mut resposta) else { continue };
            if n >= 12 && resposta[..2] == id.to_be_bytes() {
                resultado = ler_resposta_dns(&resposta[..n], nome);
                break 'tentativas;
            }
        }
    }
    let _ = sys::fechar(fd);
    resultado
}

/// Pula um nome (que pode terminar num "ponteiro" para outro lugar da mensagem).
fn pular_nome(m: &[u8], mut i: usize) -> Option<usize> {
    loop {
        let n = *m.get(i)? as usize;
        if n == 0 {
            return Some(i + 1);
        }
        if n & 0xC0 == 0xC0 {
            return Some(i + 2);
        }
        i += 1 + n;
    }
}

fn ler_resposta_dns(m: &[u8], nome: &str) -> Result<u32, String> {
    match m[3] & 0xF {
        0 => {}
        3 => return Err(format!("{}: esse nome não existe", nome)),
        codigo => return Err(format!("{}: o servidor de DNS respondeu com erro {}", nome, codigo)),
    }
    let perguntas = u16::from_be_bytes([m[4], m[5]]);
    let respostas = u16::from_be_bytes([m[6], m[7]]);
    let erro = || format!("{}: resposta de DNS malformada", nome);
    let mut i = 12;
    for _ in 0..perguntas {
        i = pular_nome(m, i).ok_or_else(erro)? + 4;
    }
    // A primeira resposta tipo A (antes dela pode vir um CNAME: um apelido).
    for _ in 0..respostas {
        i = pular_nome(m, i).ok_or_else(erro)?;
        let campos = m.get(i..i + 10).ok_or_else(erro)?;
        let tipo = u16::from_be_bytes([campos[0], campos[1]]);
        let tamanho = u16::from_be_bytes([campos[8], campos[9]]) as usize;
        i += 10;
        let dados = m.get(i..i + tamanho).ok_or_else(erro)?;
        if tipo == 1 && tamanho == 4 {
            return Ok(u32::from_be_bytes(dados.try_into().unwrap()));
        }
        i += tamanho;
    }
    Err(format!("{}: o nome não tem endereço IPv4", nome))
}

/// Uma conexão TCP aberta (fecha sozinha no fim).
pub struct Conexao {
    pub fd: i32,
}

impl Conexao {
    pub fn ler(&mut self, buf: &mut [u8]) -> sys::Resultado<usize> {
        sys::ler(self.fd, buf)
    }

    pub fn escrever(&mut self, dados: &[u8]) -> sys::Resultado<()> {
        sys::escrever_tudo(self.fd, dados)
    }

    /// Lê até o outro lado fechar.
    pub fn ler_tudo(&mut self) -> sys::Resultado<Vec<u8>> {
        let mut tudo = Vec::new();
        let mut buf = [0u8; 4096];
        loop {
            match self.ler(&mut buf)? {
                0 => return Ok(tudo),
                n => tudo.extend_from_slice(&buf[..n]),
            }
        }
    }
}

impl Drop for Conexao {
    fn drop(&mut self) {
        let _ = sys::fechar(self.fd);
    }
}

/// Conecta a `host:porta` (o host pode ser um nome ou um IP).
pub fn conectar(host: &str, porta: u16) -> Result<Conexao, String> {
    let ip = resolver(host)?;
    let fd = sys::socket_inet(sys::SOCK_STREAM, 0).map_err(|e| format!("socket: {}", e))?;
    let conexao = Conexao { fd };
    sys::conectar_inet(fd, ip, porta).map_err(|e| format!("{} ({}:{}): {}", host, texto_ip(ip), porta, e))?;
    Ok(conexao)
}
