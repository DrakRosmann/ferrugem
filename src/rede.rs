//! A pilha de rede: Ethernet, ARP, IPv4, ICMP e UDP (o TCP está em tcp.rs,
//! e as chamadas de sistema em inet.rs).
//!
//! ```text
//!   programa:   socket / connect / send / recv          (inet.rs)
//!                     │                  ▲
//!   transporte: TCP (tcp.rs), UDP, ICMP  │
//!                     │                  │
//!   rede:       IPv4 ── para quem? ── loopback (127.x e o nosso IP)
//!                     │                  │
//!   enlace:     ARP (IP → endereço MAC), Ethernet
//!                     │                  │
//!   placa:      RTL8139 (rtl8139.rs) ────┘ IRQ
//! ```
//!
//! A configuração é fixa, a da rede "user" do QEMU: nós somos 10.0.2.15, o
//! roteador é 10.0.2.2 (que também é o computador de fora) e o servidor de
//! DNS é 10.0.2.3.
//!
//! Tudo roda com a trava grande do kernel (smp.rs): os quadros que chegam são
//! tratados direto na IRQ da placa. Os pacotes para nós mesmos (loopback) vão
//! para uma fila, tratada no fim de cada chamada de sistema de rede e a cada
//! tique do relógio — assim quem envia nunca recebe no meio do próprio envio.

use crate::erro::*;
use crate::spin::Mutex;
use alloc::collections::{BTreeMap, VecDeque};
use alloc::vec::Vec;
use core::sync::atomic::{AtomicU16, Ordering};

/// Um endereço IPv4, como número (10.0.2.15 = 0x0A00020F).
pub type Ip = u32;

pub const MEU_IP: Ip = 0x0A00_020F; // 10.0.2.15
pub const MASCARA: Ip = 0xFFFF_FF00; // /24
pub const ROTEADOR: Ip = 0x0A00_0202; // 10.0.2.2
pub const LOOPBACK: Ip = 0x7F00_0001; // 127.0.0.1
const BROADCAST: Ip = 0xFFFF_FFFF;

pub const ICMP: u8 = 1;
pub const TCP: u8 = 6;
pub const UDP: u8 = 17;

const ETH_IP: u16 = 0x0800;
const ETH_ARP: u16 = 0x0806;

pub const DNS: Ip = 0x0A00_0203; // 10.0.2.3

pub fn texto_ip(ip: Ip) -> alloc::string::String {
    let b = ip.to_be_bytes();
    alloc::format!("{}.{}.{}.{}", b[0], b[1], b[2], b[3])
}

pub fn e_local(ip: Ip) -> bool {
    ip >> 24 == 127 || ip == MEU_IP
}

pub fn tem_placa() -> bool {
    crate::rtl8139::mac().is_some()
}

// -----------------------------------------------------------------------------
// A soma de verificação da Internet: a soma (em complemento de um) das
// palavras de 16 bits, invertida. Quem recebe soma tudo, inclusive o campo
// da soma, e tem que dar 0xFFFF.
// -----------------------------------------------------------------------------

pub fn somar(dados: &[u8], mut soma: u32) -> u32 {
    let mut pares = dados.chunks_exact(2);
    for p in &mut pares {
        soma += u16::from_be_bytes([p[0], p[1]]) as u32;
    }
    if let [ultimo] = pares.remainder() {
        soma += (*ultimo as u32) << 8;
    }
    soma
}

pub fn fechar_soma(mut soma: u32) -> u16 {
    while soma >> 16 != 0 {
        soma = (soma & 0xFFFF) + (soma >> 16);
    }
    !(soma as u16)
}

/// O "pseudo-cabeçalho" que o TCP e o UDP incluem na soma: os endereços, o
/// protocolo e o tamanho (assim um pacote entregue ao endereço errado não
/// passa por bom).
pub fn soma_pseudo(origem: Ip, destino: Ip, protocolo: u8, tamanho: usize) -> u32 {
    (origem >> 16) + (origem & 0xFFFF) + (destino >> 16) + (destino & 0xFFFF) + protocolo as u32 + tamanho as u32
}

// -----------------------------------------------------------------------------
// ARP: quem tem o IP x? (o endereço MAC de cada vizinho)
// -----------------------------------------------------------------------------

struct Arp {
    tabela: BTreeMap<Ip, [u8; 6]>,
    /// Pacotes IP esperando a resposta do ARP, por vizinho (e quando perguntamos).
    esperando: BTreeMap<Ip, (u64, Vec<Vec<u8>>)>,
}

static ARP: Mutex<Arp> = Mutex::new(Arp { tabela: BTreeMap::new(), esperando: BTreeMap::new() });

fn quadro(destino: [u8; 6], tipo: u16, carga: &[u8]) -> bool {
    let Some(mac) = crate::rtl8139::mac() else { return false };
    let mut q = Vec::with_capacity(14 + carga.len());
    q.extend_from_slice(&destino);
    q.extend_from_slice(&mac);
    q.extend_from_slice(&tipo.to_be_bytes());
    q.extend_from_slice(carga);
    crate::rtl8139::enviar(&q)
}

fn arp(operacao: u16, mac_alvo: [u8; 6], ip_alvo: Ip) {
    let Some(mac) = crate::rtl8139::mac() else { return };
    let mut p = Vec::with_capacity(28);
    p.extend_from_slice(&[0, 1, 0x08, 0x00, 6, 4]); // Ethernet, IPv4, tamanhos
    p.extend_from_slice(&operacao.to_be_bytes()); // 1 = pergunta, 2 = resposta
    p.extend_from_slice(&mac);
    p.extend_from_slice(&MEU_IP.to_be_bytes());
    p.extend_from_slice(&mac_alvo);
    p.extend_from_slice(&ip_alvo.to_be_bytes());
    let destino = if operacao == 1 { [0xFF; 6] } else { mac_alvo };
    quadro(destino, ETH_ARP, &p);
}

fn receber_arp(p: &[u8]) {
    if p.len() < 28 || p[..6] != [0, 1, 0x08, 0x00, 6, 4] {
        return;
    }
    let operacao = u16::from_be_bytes([p[6], p[7]]);
    let mac: [u8; 6] = p[8..14].try_into().unwrap();
    let ip = u32::from_be_bytes(p[14..18].try_into().unwrap());
    let alvo = u32::from_be_bytes(p[24..28].try_into().unwrap());
    let pendentes = {
        let mut a = ARP.lock();
        a.tabela.insert(ip, mac);
        a.esperando.remove(&ip).map(|(_, v)| v).unwrap_or_default()
    };
    for pacote in pendentes {
        quadro(mac, ETH_IP, &pacote);
    }
    if operacao == 1 && alvo == MEU_IP {
        arp(2, mac, ip);
    }
}

// -----------------------------------------------------------------------------
// IPv4
// -----------------------------------------------------------------------------

static ID_IP: AtomicU16 = AtomicU16::new(1);
static FILA_LOOPBACK: Mutex<VecDeque<Vec<u8>>> = Mutex::new(VecDeque::new());

/// O endereço de origem para falar com `destino`.
pub fn origem_para(destino: Ip) -> Ip {
    if destino >> 24 == 127 { destino } else { MEU_IP }
}

/// Monta um pacote IP e o envia (pela placa ou pelo loopback).
pub fn enviar_ip(origem: Ip, destino: Ip, protocolo: u8, carga: &[u8]) -> Result<(), i64> {
    if carga.len() > 1480 {
        return Err(EMSGSIZE); // sem fragmentação
    }
    let mut p = Vec::with_capacity(20 + carga.len());
    p.extend_from_slice(&[0x45, 0]); // versão 4, cabeçalho de 5 palavras
    p.extend_from_slice(&((20 + carga.len()) as u16).to_be_bytes());
    p.extend_from_slice(&ID_IP.fetch_add(1, Ordering::Relaxed).to_be_bytes());
    p.extend_from_slice(&[0x40, 0, 64, protocolo, 0, 0]); // não fragmentar, TTL 64
    p.extend_from_slice(&origem.to_be_bytes());
    p.extend_from_slice(&destino.to_be_bytes());
    let soma = fechar_soma(somar(&p, 0));
    p[10..12].copy_from_slice(&soma.to_be_bytes());
    p.extend_from_slice(carga);
    if e_local(destino) {
        FILA_LOOPBACK.lock().push_back(p);
        return Ok(());
    }
    if !tem_placa() {
        return Err(ENETUNREACH);
    }
    // O vizinho a quem entregar: o próprio destino, se estiver na nossa rede;
    // senão, o roteador.
    let vizinho = if destino == BROADCAST || destino & MASCARA == MEU_IP & MASCARA { destino } else { ROTEADOR };
    if vizinho == BROADCAST {
        quadro([0xFF; 6], ETH_IP, &p);
        return Ok(());
    }
    let mut a = ARP.lock();
    if let Some(&mac) = a.tabela.get(&vizinho) {
        drop(a);
        quadro(mac, ETH_IP, &p);
        return Ok(());
    }
    // Não sabemos o MAC: o pacote espera a resposta do ARP.
    let agora = crate::pit::milissegundos();
    let (perguntado, fila) = a.esperando.entry(vizinho).or_insert((0, Vec::new()));
    if fila.len() < 32 {
        fila.push(p);
    }
    let perguntar = agora >= *perguntado + 500;
    if perguntar {
        *perguntado = agora;
    }
    drop(a);
    if perguntar {
        arp(1, [0; 6], vizinho);
    }
    Ok(())
}

fn receber_ip(p: &[u8]) {
    if p.len() < 20 || p[0] >> 4 != 4 {
        return;
    }
    let cabecalho = (p[0] & 0xF) as usize * 4;
    let total = u16::from_be_bytes([p[2], p[3]]) as usize;
    if cabecalho < 20 || total < cabecalho || total > p.len() || fechar_soma(somar(&p[..cabecalho], 0)) != 0 {
        return;
    }
    // Fragmentos (mais fragmentos, ou deslocamento) não são remontados.
    if u16::from_be_bytes([p[6], p[7]]) & 0x3FFF != 0 {
        return;
    }
    let origem = u32::from_be_bytes(p[12..16].try_into().unwrap());
    let destino = u32::from_be_bytes(p[16..20].try_into().unwrap());
    if !e_local(destino) && destino != BROADCAST {
        return;
    }
    let carga = &p[cabecalho..total];
    match p[9] {
        ICMP => receber_icmp(origem, destino, carga),
        UDP => receber_udp(origem, destino, carga),
        TCP => crate::tcp::receber(origem, destino, carga),
        _ => {}
    }
}

/// Um quadro Ethernet que a placa recebeu.
pub fn receber_quadro(q: &[u8]) {
    if q.len() < 14 {
        return;
    }
    match u16::from_be_bytes([q[12], q[13]]) {
        ETH_ARP => receber_arp(&q[14..]),
        ETH_IP => receber_ip(&q[14..]),
        _ => {}
    }
}

/// Entrega os pacotes que mandamos para nós mesmos.
pub fn processar_loopback() {
    for _ in 0..10_000 {
        let Some(p) = FILA_LOOPBACK.lock().pop_front() else { return };
        receber_ip(&p);
    }
}

/// A cada tique do relógio: o loopback e os prazos do TCP.
pub fn tique() {
    processar_loopback();
    crate::tcp::tique();
}

// -----------------------------------------------------------------------------
// ICMP (o ping) e UDP: os datagramas vão para a caixa do socket certo
// -----------------------------------------------------------------------------

fn receber_icmp(origem: Ip, destino: Ip, m: &[u8]) {
    if m.len() < 8 || fechar_soma(somar(m, 0)) != 0 {
        return;
    }
    match m[0] {
        8 => {
            // Um pedido de eco: devolvemos a mesma mensagem, com tipo 0.
            let mut resposta = m.to_vec();
            resposta[0] = 0;
            resposta[2..4].fill(0);
            let soma = fechar_soma(somar(&resposta, 0));
            resposta[2..4].copy_from_slice(&soma.to_be_bytes());
            let _ = enviar_ip(destino, origem, ICMP, &resposta);
        }
        0 => {
            // Uma resposta de eco: vai para o socket de ping cujo número é o id.
            let id = u16::from_be_bytes([m[4], m[5]]);
            crate::inet::entregar_datagrama(ICMP, id, m, (origem, 0));
        }
        _ => {}
    }
}

fn receber_udp(origem: Ip, destino: Ip, d: &[u8]) {
    if d.len() < 8 {
        return;
    }
    let tamanho = u16::from_be_bytes([d[4], d[5]]) as usize;
    if tamanho < 8 || tamanho > d.len() {
        return;
    }
    let d = &d[..tamanho];
    if d[6..8] != [0, 0] && fechar_soma(somar(d, soma_pseudo(origem, destino, UDP, tamanho))) != 0 {
        return;
    }
    let porta_origem = u16::from_be_bytes([d[0], d[1]]);
    let porta = u16::from_be_bytes([d[2], d[3]]);
    crate::inet::entregar_datagrama(UDP, porta, &d[8..], (origem, porta_origem));
}

pub fn enviar_udp(origem: (Ip, u16), destino: (Ip, u16), dados: &[u8]) -> Result<(), i64> {
    let tamanho = 8 + dados.len();
    let mut d = Vec::with_capacity(tamanho);
    d.extend_from_slice(&origem.1.to_be_bytes());
    d.extend_from_slice(&destino.1.to_be_bytes());
    d.extend_from_slice(&(tamanho as u16).to_be_bytes());
    d.extend_from_slice(&[0, 0]);
    d.extend_from_slice(dados);
    let soma = match fechar_soma(somar(&d, soma_pseudo(origem.0, destino.0, UDP, tamanho))) {
        0 => 0xFFFF, // 0 quer dizer "sem soma"
        s => s,
    };
    d[6..8].copy_from_slice(&soma.to_be_bytes());
    enviar_ip(origem.0, destino.0, UDP, &d)
}

/// Um pedido de eco (o ping): `m` é a mensagem ICMP que o programa montou;
/// o kernel põe o id (o "número de porta" do socket) e a soma.
pub fn enviar_eco(origem: Ip, destino: Ip, id: u16, m: &[u8]) -> Result<(), i64> {
    if m.len() < 8 || m[0] != 8 {
        return Err(EINVAL);
    }
    let mut m = m.to_vec();
    m[4..6].copy_from_slice(&id.to_be_bytes());
    m[2..4].fill(0);
    let soma = fechar_soma(somar(&m, 0));
    m[2..4].copy_from_slice(&soma.to_be_bytes());
    enviar_ip(origem, destino, ICMP, &m)
}

/// O /proc/rede: a placa, o endereço, os vizinhos conhecidos e as conexões.
pub fn descrever() -> alloc::string::String {
    use alloc::format;
    let mut t = alloc::string::String::new();
    match crate::rtl8139::mac() {
        Some(m) => {
            t += &format!("placa RTL8139 {:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x}\n", m[0], m[1], m[2], m[3], m[4], m[5]);
            t += &format!("ip {}/24 roteador {} dns {}\n", texto_ip(MEU_IP), texto_ip(ROTEADOR), texto_ip(DNS));
        }
        None => t += "placa nenhuma (só o loopback)\n",
    }
    for (ip, m) in ARP.lock().tabela.iter() {
        t += &format!("arp {} {:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x}\n", texto_ip(*ip), m[0], m[1], m[2], m[3], m[4], m[5]);
    }
    let ponta = |(ip, porta): (Ip, u16)| format!("{}:{}", texto_ip(ip), porta);
    for (local, remoto, estado) in crate::tcp::listar() {
        t += &match estado {
            None => format!("tcp {} - ouvindo\n", ponta(local)),
            Some(e) => format!("tcp {} {} {:?}\n", ponta(local), ponta(remoto), e),
        };
    }
    for (protocolo, local) in crate::inet::datagramas() {
        t += &format!("{} {}\n", if protocolo == UDP { "udp" } else { "icmp" }, ponta(local));
    }
    t
}
