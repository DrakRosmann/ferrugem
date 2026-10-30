//! TCP: um fluxo de bytes confiável e em ordem por cima do IP, que perde,
//! duplica e embaralha pacotes.
//!
//! Cada byte tem um número de sequência. Quem recebe confirma (ACK) até onde
//! já recebeu tudo; quem envia guarda o que mandou até ser confirmado e, se a
//! confirmação demorar, manda de novo (retransmissão, com o prazo dobrando a
//! cada tentativa). Quem recebe também diz quanto espaço ainda tem (a
//! janela), e quem envia nunca passa disso.
//!
//! ```text
//!   cliente                                 servidor (ouvindo)
//!   SYN seq=x            ───────────────▶
//!                        ◀───────────────   SYN+ACK seq=y ack=x+1
//!   ACK ack=y+1          ───────────────▶   (vai para a fila do accept)
//!   dados, ACKs          ◀──────────────▶   dados, ACKs
//!   FIN (close)          ───────────────▶   o read de lá devolve 0
//!                        ◀───────────────   ACK, e depois o FIN de lá
//!   ACK; espera 2 s (TIME_WAIT) para não confundir uma conexão nova com
//!   pacotes atrasados da velha
//! ```
//!
//! Simplificações: segmentos fora de ordem são descartados (quem envia
//! retransmite), não há controle de congestionamento (só a janela de quem
//! recebe), nem "window scaling" (a janela vai até 64 KiB).

use crate::erro::*;
use crate::processo;
use crate::rede::{self, Ip};
use crate::spin::Mutex;
use alloc::collections::VecDeque;
use alloc::sync::{Arc, Weak};
use alloc::vec::Vec;

/// Um lado de uma conexão: (endereço, porta).
pub type Ponta = (Ip, u16);

const FIN: u8 = 0x01;
const SYN: u8 = 0x02;
const RST: u8 = 0x04;
const PSH: u8 = 0x08;
const ACK: u8 = 0x10;

/// O espaço para o que chegou e o programa ainda não leu (a janela máxima
/// sem "window scaling").
const CAPACIDADE_ENTRADA: usize = 65535;
/// O que o programa escreveu e ainda não foi confirmado.
pub const CAPACIDADE_SAIDA: usize = 64 * 1024;
const MEU_MSS: usize = 1460;
/// Em tiques de 10 ms.
const RTO_INICIAL: u32 = 100;
const RTO_MAXIMO: u32 = 3000;
const TENTATIVAS: u32 = 8;
const TEMPO_ESPERA: u32 = 200;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Estado {
    SynEnviado,
    SynRecebido,
    Estabelecido,
    /// Nós fechamos; esperando o ACK do nosso FIN.
    FinEspera1,
    /// O nosso FIN foi confirmado; esperando o FIN de lá.
    FinEspera2,
    /// Os dois fecharam ao mesmo tempo.
    Fechando,
    TempoEspera,
    /// O outro lado fechou; nós ainda podemos escrever.
    EsperaFechar,
    UltimoAck,
    Fechado,
}

pub struct Tcb {
    pub local: Ponta,
    pub remoto: Ponta,
    pub estado: Estado,
    iss: u32,
    /// O primeiro número de sequência ainda não confirmado.
    snd_una: u32,
    /// A janela de quem recebe.
    snd_wnd: u32,
    /// O nosso SYN ainda não foi confirmado (ele ocupa um número de sequência).
    syn_pendente: bool,
    /// Os bytes a partir de `snd_una`: os já enviados (`enviados`) e os que faltam.
    saida: VecDeque<u8>,
    enviados: usize,
    /// O programa fechou: depois dos dados vem o FIN.
    fin_na_fila: bool,
    fin_transmitido: bool,
    fin_confirmado: bool,
    /// O próximo número de sequência que esperamos receber.
    rcv_nxt: u32,
    pub entrada: VecDeque<u8>,
    pub fin_recebido: bool,
    pub erro: i64,
    mss: usize,
    /// Tiques até retransmitir (0 = nada esperando confirmação).
    relogio: u32,
    rto: u32,
    tentativas: u32,
    espera: u32,
    /// Quem vai fazer o accept (enquanto o aperto de mão não termina).
    ouvinte: Option<Weak<Ouvinte>>,
    pub leitura_fechada: bool,
}

pub struct Ouvinte {
    pub local: Ponta,
    pub fila: Mutex<VecDeque<Arc<Mutex<Tcb>>>>,
    pub maximo: usize,
}

impl Drop for Ouvinte {
    fn drop(&mut self) {
        // Conexões que ninguém vai aceitar.
        for c in self.fila.lock().drain(..) {
            abandonar(&c);
        }
    }
}

static CONEXOES: Mutex<Vec<Arc<Mutex<Tcb>>>> = Mutex::new(Vec::new());
static OUVINTES: Mutex<Vec<Weak<Ouvinte>>> = Mutex::new(Vec::new());

/// `a` vem antes de `b` no espaço (circular) dos números de sequência.
fn antes(a: u32, b: u32) -> bool {
    (a.wrapping_sub(b) as i32) < 0
}

pub fn canal(c: &Arc<Mutex<Tcb>>) -> usize {
    Arc::as_ptr(c) as usize
}

pub fn canal_ouvinte(o: &Arc<Ouvinte>) -> usize {
    Arc::as_ptr(o) as usize
}

#[allow(clippy::too_many_arguments)]
fn enviar_segmento(local: Ponta, remoto: Ponta, seq: u32, ack: u32, flags: u8, janela: u16, dados: &[u8]) {
    let opcoes: &[u8] = if flags & SYN != 0 { &[2, 4, (MEU_MSS >> 8) as u8, MEU_MSS as u8] } else { &[] };
    let cabecalho = 20 + opcoes.len();
    let mut s = Vec::with_capacity(cabecalho + dados.len());
    s.extend_from_slice(&local.1.to_be_bytes());
    s.extend_from_slice(&remoto.1.to_be_bytes());
    s.extend_from_slice(&seq.to_be_bytes());
    s.extend_from_slice(&(if flags & ACK != 0 { ack } else { 0 }).to_be_bytes());
    s.push(((cabecalho / 4) as u8) << 4);
    s.push(flags);
    s.extend_from_slice(&janela.to_be_bytes());
    s.extend_from_slice(&[0, 0, 0, 0]); // soma e ponteiro de urgência
    s.extend_from_slice(opcoes);
    s.extend_from_slice(dados);
    let soma = rede::fechar_soma(rede::somar(&s, rede::soma_pseudo(local.0, remoto.0, rede::TCP, s.len())));
    s[16..18].copy_from_slice(&soma.to_be_bytes());
    let _ = rede::enviar_ip(local.0, remoto.0, rede::TCP, &s);
}

struct Segmento<'a> {
    seq: u32,
    ack: u32,
    flags: u8,
    janela: u16,
    mss: Option<usize>,
    dados: &'a [u8],
}

impl Tcb {
    fn novo(local: Ponta, remoto: Ponta, estado: Estado) -> Tcb {
        let iss = crate::aleatorio::proximo() as u32;
        Tcb {
            local,
            remoto,
            estado,
            iss,
            snd_una: iss,
            snd_wnd: 0,
            syn_pendente: true,
            saida: VecDeque::new(),
            enviados: 0,
            fin_na_fila: false,
            fin_transmitido: false,
            fin_confirmado: false,
            rcv_nxt: 0,
            entrada: VecDeque::new(),
            fin_recebido: false,
            erro: 0,
            mss: 536,
            relogio: RTO_INICIAL,
            rto: RTO_INICIAL,
            tentativas: 0,
            espera: 0,
            ouvinte: None,
            leitura_fechada: false,
        }
    }

    /// O número de sequência do primeiro byte de `saida`.
    fn seq_dados(&self) -> u32 {
        self.snd_una.wrapping_add(self.syn_pendente as u32)
    }

    fn seq_proxima(&self) -> u32 {
        self.seq_dados().wrapping_add(self.enviados as u32).wrapping_add(self.fin_transmitido as u32)
    }

    fn janela(&self) -> u16 {
        (CAPACIDADE_ENTRADA - self.entrada.len()) as u16
    }

    fn segmento(&self, seq: u32, flags: u8, dados: &[u8]) {
        enviar_segmento(self.local, self.remoto, seq, self.rcv_nxt, flags, self.janela(), dados);
    }

    fn mandar_ack(&self) {
        self.segmento(self.seq_proxima(), ACK, &[]);
    }

    fn mandar_syn(&self) {
        let flags = if self.estado == Estado::SynEnviado { SYN } else { SYN | ACK };
        self.segmento(self.iss, flags, &[]);
    }

    fn resetar(&mut self, erro: i64) {
        if !matches!(self.estado, Estado::SynEnviado | Estado::Fechado) {
            self.segmento(self.seq_proxima(), RST | ACK, &[]);
        }
        self.erro = erro;
        self.estado = Estado::Fechado;
    }

    /// Há algo esperando confirmação (ou uma janela fechada a sondar)?
    fn algo_no_ar(&self) -> bool {
        self.syn_pendente
            || self.enviados > 0
            || self.fin_transmitido && !self.fin_confirmado
            || !self.saida.is_empty() && self.snd_wnd == 0
    }

    fn pode_enviar(&self) -> bool {
        matches!(
            self.estado,
            Estado::Estabelecido | Estado::EsperaFechar | Estado::FinEspera1 | Estado::Fechando | Estado::UltimoAck
        )
    }

    /// Manda o que der (dentro da janela de lá) e, no fim dos dados, o FIN.
    fn transmitir(&mut self, janela_minima: usize) {
        if !self.pode_enviar() || self.syn_pendente {
            return;
        }
        let janela = (self.snd_wnd as usize).max(janela_minima);
        while self.enviados < self.saida.len() && self.enviados < janela {
            let n = (self.saida.len() - self.enviados).min(self.mss).min(janela - self.enviados);
            let dados: Vec<u8> = self.saida.range(self.enviados..self.enviados + n).copied().collect();
            let seq = self.seq_dados().wrapping_add(self.enviados as u32);
            self.enviados += n;
            let flags = if self.enviados == self.saida.len() { ACK | PSH } else { ACK };
            self.segmento(seq, flags, &dados);
        }
        if self.fin_na_fila && !self.fin_transmitido && self.enviados == self.saida.len() {
            self.segmento(self.seq_dados().wrapping_add(self.enviados as u32), FIN | ACK, &[]);
            self.fin_transmitido = true;
        }
        if self.relogio == 0 && self.algo_no_ar() {
            self.relogio = self.rto;
        }
    }

    /// O prazo de confirmação venceu: manda tudo de novo, a partir do
    /// primeiro byte não confirmado.
    fn expirou(&mut self) {
        if !self.algo_no_ar() {
            return;
        }
        // Sondar uma janela fechada não conta como tentativa perdida.
        if !(self.snd_wnd == 0 && self.enviados == 0 && !self.syn_pendente) {
            self.tentativas += 1;
        }
        if self.tentativas > TENTATIVAS {
            self.resetar(ETIMEDOUT);
            return;
        }
        self.rto = (self.rto * 2).min(RTO_MAXIMO);
        if self.syn_pendente {
            self.mandar_syn();
            self.relogio = self.rto;
            return;
        }
        self.enviados = 0;
        self.fin_transmitido = self.fin_confirmado;
        self.transmitir(1);
    }

    /// Um ACK chegou: libera o que foi confirmado.
    fn confirmar(&mut self, ack: u32, janela: u16) {
        let maximo = self.seq_dados().wrapping_add(self.saida.len() as u32).wrapping_add(self.fin_na_fila as u32);
        if antes(ack, self.snd_una) {
            return; // um ACK velho
        }
        if antes(maximo, ack) {
            self.mandar_ack(); // confirma o que nunca mandamos
            return;
        }
        self.snd_wnd = janela as u32;
        let mut n = ack.wrapping_sub(self.snd_una) as usize;
        if n == 0 {
            if self.snd_wnd > 0 {
                self.transmitir(0); // talvez a janela tenha aberto
            }
            return;
        }
        if self.syn_pendente {
            self.syn_pendente = false;
            n -= 1;
        }
        let dados = n.min(self.saida.len());
        self.saida.drain(..dados);
        self.enviados = self.enviados.saturating_sub(dados);
        if n > dados && self.fin_na_fila {
            self.fin_transmitido = true;
            self.fin_confirmado = true;
        }
        self.snd_una = ack;
        self.tentativas = 0;
        self.rto = RTO_INICIAL;
        self.relogio = if self.algo_no_ar() { self.rto } else { 0 };
        if self.fin_confirmado {
            self.estado = match self.estado {
                Estado::FinEspera1 => Estado::FinEspera2,
                Estado::Fechando => {
                    self.espera = TEMPO_ESPERA;
                    Estado::TempoEspera
                }
                Estado::UltimoAck => Estado::Fechado,
                e => e,
            };
        }
    }

    /// Um segmento desta conexão chegou. Devolve o ouvinte a avisar, se o
    /// aperto de mão terminou agora.
    fn processar(&mut self, seg: &Segmento) -> Option<Arc<Ouvinte>> {
        if self.estado == Estado::SynEnviado {
            let ack_certo = seg.ack == self.iss.wrapping_add(1);
            if seg.flags & ACK != 0 && !ack_certo {
                if seg.flags & RST == 0 {
                    enviar_segmento(self.local, self.remoto, seg.ack, 0, RST, 0, &[]);
                }
                return None;
            }
            if seg.flags & RST != 0 {
                if seg.flags & ACK != 0 {
                    self.erro = ECONNREFUSED;
                    self.estado = Estado::Fechado;
                }
                return None;
            }
            if seg.flags & SYN != 0 && seg.flags & ACK != 0 {
                self.rcv_nxt = seg.seq.wrapping_add(1);
                self.snd_una = seg.ack;
                self.syn_pendente = false;
                self.snd_wnd = seg.janela as u32;
                self.mss = seg.mss.unwrap_or(536).min(MEU_MSS);
                self.estado = Estado::Estabelecido;
                (self.relogio, self.tentativas, self.rto) = (0, 0, RTO_INICIAL);
                self.mandar_ack();
                self.transmitir(0);
            }
            return None;
        }
        if seg.flags & RST != 0 {
            // Só vale se estiver dentro da janela (senão seria fácil derrubar
            // conexões dos outros mandando RSTs a esmo).
            let dentro = !antes(seg.seq, self.rcv_nxt) && antes(seg.seq, self.rcv_nxt.wrapping_add(65536));
            if dentro {
                self.erro = if self.estado == Estado::SynRecebido { 0 } else { ECONNRESET };
                self.estado = Estado::Fechado;
            }
            return None;
        }
        if seg.flags & SYN != 0 {
            // O SYN de novo: o nosso SYN+ACK (ou ACK) se perdeu.
            if self.estado == Estado::SynRecebido {
                self.mandar_syn();
            } else {
                self.mandar_ack();
            }
            return None;
        }
        if seg.flags & ACK == 0 {
            return None;
        }
        let mut avisar = None;
        if self.estado == Estado::SynRecebido {
            if seg.ack != self.iss.wrapping_add(1) {
                enviar_segmento(self.local, self.remoto, seg.ack, 0, RST, 0, &[]);
                return None;
            }
            self.syn_pendente = false;
            self.snd_una = seg.ack;
            self.snd_wnd = seg.janela as u32;
            self.estado = Estado::Estabelecido;
            (self.relogio, self.tentativas, self.rto) = (0, 0, RTO_INICIAL);
            match self.ouvinte.take().and_then(|o| o.upgrade()) {
                Some(o) => avisar = Some(o),
                None => {
                    self.resetar(0);
                    return None;
                }
            }
        } else {
            self.confirmar(seg.ack, seg.janela);
        }
        let fin = seg.flags & FIN != 0;
        if !seg.dados.is_empty() || fin {
            let (mut seq, mut dados) = (seg.seq, seg.dados);
            // Um pedaço já recebido antes (retransmitido)? Cortamos.
            if antes(seq, self.rcv_nxt) {
                let corte = (self.rcv_nxt.wrapping_sub(seq) as usize).min(dados.len());
                dados = &dados[corte..];
                seq = seq.wrapping_add(corte as u32);
            }
            let aceita = matches!(self.estado, Estado::Estabelecido | Estado::FinEspera1 | Estado::FinEspera2);
            if seq == self.rcv_nxt && aceita {
                let cabe = (CAPACIDADE_ENTRADA - self.entrada.len()).min(dados.len());
                if !self.leitura_fechada {
                    self.entrada.extend(&dados[..cabe]);
                }
                self.rcv_nxt = self.rcv_nxt.wrapping_add(cabe as u32);
                if fin && cabe == dados.len() {
                    self.rcv_nxt = self.rcv_nxt.wrapping_add(1);
                    self.fin_recebido = true;
                    self.estado = match self.estado {
                        Estado::Estabelecido => Estado::EsperaFechar,
                        Estado::FinEspera1 if self.fin_confirmado => Estado::TempoEspera,
                        Estado::FinEspera1 => Estado::Fechando,
                        Estado::FinEspera2 => Estado::TempoEspera,
                        e => e,
                    };
                    if self.estado == Estado::TempoEspera {
                        self.espera = TEMPO_ESPERA;
                        self.relogio = 0;
                    }
                }
            }
            // Em ordem ou não, confirmamos até onde temos tudo (fora de
            // ordem, isso é um "ACK duplicado": quem envia percebe a falta).
            self.mandar_ack();
        }
        self.transmitir(0);
        avisar
    }

    // ------------------------------------------------------- o lado do programa

    /// Põe dados na fila de saída (o quanto couber) e manda o que der.
    pub fn escrever(&mut self, dados: &[u8]) -> usize {
        let n = (CAPACIDADE_SAIDA - self.saida.len()).min(dados.len());
        self.saida.extend(&dados[..n]);
        self.transmitir(0);
        n
    }

    pub fn cabe_escrever(&self) -> bool {
        self.saida.len() < CAPACIDADE_SAIDA
    }

    /// Tira dados da entrada. Se a janela estava quase fechada, avisa que abriu.
    pub fn ler(&mut self, buf: &mut [u8]) -> usize {
        let livre_antes = CAPACIDADE_ENTRADA - self.entrada.len();
        let n = buf.len().min(self.entrada.len());
        for (destino, b) in buf.iter_mut().zip(self.entrada.drain(..n)) {
            *destino = b;
        }
        let livre = CAPACIDADE_ENTRADA - self.entrada.len();
        if livre_antes < 2 * self.mss && livre >= 2 * self.mss && self.pode_receber() {
            self.mandar_ack();
        }
        n
    }

    fn pode_receber(&self) -> bool {
        matches!(self.estado, Estado::Estabelecido | Estado::FinEspera1 | Estado::FinEspera2)
    }

    /// O programa não vai mais escrever (close, ou shutdown): FIN depois dos dados.
    pub fn fechar_escrita(&mut self) {
        match self.estado {
            Estado::SynEnviado => self.estado = Estado::Fechado,
            Estado::SynRecebido | Estado::Estabelecido => {
                self.fin_na_fila = true;
                self.estado = Estado::FinEspera1;
                self.transmitir(0);
            }
            Estado::EsperaFechar => {
                self.fin_na_fila = true;
                self.estado = Estado::UltimoAck;
                self.transmitir(0);
            }
            _ => {}
        }
    }

    pub fn conectando(&self) -> bool {
        matches!(self.estado, Estado::SynEnviado | Estado::SynRecebido)
    }

    /// Ainda dá para escrever (não fechamos o nosso lado).
    pub fn escrita_aberta(&self) -> bool {
        matches!(self.estado, Estado::Estabelecido | Estado::EsperaFechar) && !self.fin_na_fila
    }
}

/// Começa uma conexão (o `connect`): manda o SYN.
pub fn conectar(local: Ponta, remoto: Ponta) -> Arc<Mutex<Tcb>> {
    let t = Tcb::novo(local, remoto, Estado::SynEnviado);
    t.mandar_syn();
    let c = Arc::new(Mutex::new(t));
    CONEXOES.lock().push(c.clone());
    c
}

/// O programa fechou o socket: a conexão termina sozinha (manda o que
/// falta, o FIN...). Se ficou coisa sem ler, é um fim abrupto (RST).
pub fn abandonar(c: &Arc<Mutex<Tcb>>) {
    let mut t = c.lock();
    if !t.entrada.is_empty() || t.estado == Estado::SynEnviado {
        t.resetar(0);
    } else {
        t.fechar_escrita();
    }
}

pub fn ouvir(local: Ponta, maximo: usize) -> Result<Arc<Ouvinte>, i64> {
    let mut ouvintes = OUVINTES.lock();
    ouvintes.retain(|o| o.strong_count() > 0);
    if ouvintes.iter().filter_map(Weak::upgrade).any(|o| o.local.1 == local.1) {
        return Err(EADDRINUSE);
    }
    let o = Arc::new(Ouvinte { local, fila: Mutex::new(VecDeque::new()), maximo: maximo.clamp(1, 128) });
    ouvintes.push(Arc::downgrade(&o));
    Ok(o)
}

/// Alguma conexão (ou ouvinte) usa esta porta local?
pub fn porta_em_uso(porta: u16) -> bool {
    OUVINTES.lock().iter().filter_map(Weak::upgrade).any(|o| o.local.1 == porta)
        || CONEXOES.lock().iter().any(|c| c.lock().local.1 == porta)
}

fn ler_mss(opcoes: &[u8]) -> Option<usize> {
    let mut i = 0;
    while i < opcoes.len() {
        match opcoes[i] {
            0 => break,
            1 => i += 1,
            tipo => {
                let n = *opcoes.get(i + 1)? as usize;
                if n < 2 {
                    break;
                }
                if tipo == 2 && n == 4 && i + 4 <= opcoes.len() {
                    return Some(u16::from_be_bytes([opcoes[i + 2], opcoes[i + 3]]) as usize);
                }
                i += n;
            }
        }
    }
    None
}

/// Um segmento TCP que chegou (de `rede.rs`).
pub fn receber(origem: Ip, destino: Ip, s: &[u8]) {
    if s.len() < 20 || rede::fechar_soma(rede::somar(s, rede::soma_pseudo(origem, destino, rede::TCP, s.len()))) != 0 {
        return;
    }
    let cabecalho = (s[12] >> 4) as usize * 4;
    if cabecalho < 20 || cabecalho > s.len() {
        return;
    }
    let seg = Segmento {
        seq: u32::from_be_bytes(s[4..8].try_into().unwrap()),
        ack: u32::from_be_bytes(s[8..12].try_into().unwrap()),
        flags: s[13],
        janela: u16::from_be_bytes([s[14], s[15]]),
        mss: ler_mss(&s[20..cabecalho]),
        dados: &s[cabecalho..],
    };
    let local = (destino, u16::from_be_bytes([s[2], s[3]]));
    let remoto = (origem, u16::from_be_bytes([s[0], s[1]]));
    let achada = CONEXOES.lock().iter().find(|c| {
        let t = c.lock();
        t.local == local && t.remoto == remoto && t.estado != Estado::Fechado
    }).cloned();
    if let Some(c) = achada {
        let avisar = c.lock().processar(&seg);
        if let Some(o) = avisar {
            o.fila.lock().push_back(c.clone());
            processo::acordar(canal_ouvinte(&o));
        }
        processo::acordar(canal(&c));
        return;
    }
    if seg.flags & RST != 0 {
        return;
    }
    if seg.flags & SYN != 0 && seg.flags & ACK == 0 {
        let ouvinte = OUVINTES
            .lock()
            .iter()
            .filter_map(Weak::upgrade)
            .find(|o| o.local.1 == local.1 && (o.local.0 == 0 || o.local.0 == local.0));
        if let Some(o) = ouvinte {
            if o.fila.lock().len() >= o.maximo {
                return; // fila cheia: o cliente tenta de novo mais tarde
            }
            let mut t = Tcb::novo(local, remoto, Estado::SynRecebido);
            t.rcv_nxt = seg.seq.wrapping_add(1);
            t.snd_wnd = seg.janela as u32;
            t.mss = seg.mss.unwrap_or(536).min(MEU_MSS);
            t.ouvinte = Some(Arc::downgrade(&o));
            t.mandar_syn();
            CONEXOES.lock().push(Arc::new(Mutex::new(t)));
            return;
        }
    }
    // Ninguém nesta porta: "conexão recusada".
    if seg.flags & ACK != 0 {
        enviar_segmento(local, remoto, seg.ack, 0, RST, 0, &[]);
    } else {
        let tamanho = seg.dados.len() as u32 + (seg.flags & SYN != 0) as u32 + (seg.flags & FIN != 0) as u32;
        enviar_segmento(local, remoto, 0, seg.seq.wrapping_add(tamanho), RST | ACK, 0, &[]);
    }
}

/// A cada tique (10 ms): retransmissões e o fim do TIME_WAIT.
pub fn tique() {
    let conexoes: Vec<Arc<Mutex<Tcb>>> = CONEXOES.lock().clone();
    if conexoes.is_empty() {
        return;
    }
    for c in &conexoes {
        let mut t = c.lock();
        let antes = t.estado;
        if t.estado == Estado::TempoEspera {
            t.espera = t.espera.saturating_sub(1);
            if t.espera == 0 {
                t.estado = Estado::Fechado;
            }
        } else if t.relogio > 0 {
            t.relogio -= 1;
            if t.relogio == 0 {
                t.expirou();
            }
        }
        if t.estado != antes {
            drop(t);
            processo::acordar(canal(c));
        }
    }
    CONEXOES.lock().retain(|c| c.lock().estado != Estado::Fechado);
}

/// Para o /proc/net/tcp: (local, remoto, estado); sem estado = ouvindo.
pub fn listar() -> Vec<(Ponta, Ponta, Option<Estado>)> {
    let mut v: Vec<_> = OUVINTES.lock().iter().filter_map(Weak::upgrade).map(|o| (o.local, (0, 0), None)).collect();
    v.extend(CONEXOES.lock().iter().map(|c| {
        let t = c.lock();
        (t.local, t.remoto, Some(t.estado))
    }));
    v
}
