//! Mouse PS/2 (IRQ 12).
//!
//! O mouse fica pendurado no mesmo controlador 8042 do teclado, na "porta
//! auxiliar": para falar com ele, mandamos 0xD4 ao controlador antes de cada
//! byte. Depois de ligado, a cada movimento ou clique ele manda um pacote de
//! 3 bytes:
//!
//! ```text
//!   byte 0: bits 0-2 = botões (esquerdo, direito, meio), bit 3 = sempre 1,
//!           bits 4-5 = sinal de dx e dy
//!   byte 1: dx (quanto andou para a direita)
//!   byte 2: dy (quanto andou para CIMA)
//!   byte 3: a roda (só no modo IntelliMouse): -1 = para cima, +1 = para baixo
//! ```
//!
//! O modo IntelliMouse (com a roda) é ligado por uma "senha": mudar a taxa de
//! amostragem para 200, 100 e 80, nessa ordem. Se o mouse passar a dizer que
//! é do tipo 3, os pacotes ganham o 4º byte.
//!
//! ## O mouse absoluto (vmmouse)
//!
//! Numa máquina virtual, o ponteiro do sistema de fora já sabe onde está: o
//! QEMU (como o VMware) oferece uma "porta dos fundos", a porta de E/S
//! 0x5658, pela qual perguntamos a posição *absoluta* do mouse (0 a 65535 em
//! cada eixo). Com ela, o cursor acompanha o do computador de fora, sem
//! precisar "capturar" o mouse na janela do QEMU. O mouse PS/2 continua
//! gerando a IRQ 12; os dados é que vêm pela porta dos fundos.
//!
//! ## /dev/mouse
//!
//! Pacotes de 8 bytes:
//!
//! ```text
//!   [0] tipo: 0 = relativo (PS/2), 1 = absoluto (vmmouse)
//!   [1] botões: bit 0 = esquerdo, bit 1 = direito, bit 2 = meio
//!   [2..4] x: quanto andou (relativo) ou onde está, de 0 a 65535 (absoluto)
//!   [4..6] y: idem; positivo = PARA BAIXO
//!   [6] roda (com sinal): negativo = para cima
//! ```

use crate::erro::*;
use crate::porta::{inb, outb};
use crate::processo;
use crate::spin::Mutex;
use alloc::collections::VecDeque;
use core::sync::atomic::{AtomicUsize, Ordering};

const DADOS: u16 = 0x60;
const STATUS: u16 = 0x64;

struct Estado {
    pacote: [u8; 4],
    recebidos: usize,
    /// 3 bytes por pacote, ou 4 com a roda.
    tamanho: usize,
    /// Os dados vêm do vmmouse (posição absoluta).
    absoluto: bool,
    fila: VecDeque<u8>,
}

static ESTADO: Mutex<Estado> =
    Mutex::new(Estado { pacote: [0; 4], recebidos: 0, tamanho: 3, absoluto: false, fila: VecDeque::new() });

pub const TAMANHO_PACOTE: usize = 8;

// ----------------------------------------------------------- a porta dos fundos

const MAGICO: u32 = 0x564D_5868; // "VMXh"
const PORTA_VMWARE: u32 = 0x5658;
const VERSAO: u32 = 10;
const ABS_DADOS: u32 = 39;
const ABS_ESTADO: u32 = 40;
const ABS_COMANDO: u32 = 41;
const LIGAR: u32 = 0x4541_4552;
const PEDIR_ABSOLUTO: u32 = 0x5342_4152;

/// Uma chamada à porta dos fundos: `in eax, dx` com EAX = mágico, ECX =
/// comando e EBX = argumento; a resposta volta em EAX, EBX, ECX e EDX. (O
/// Rust não deixa usar o RBX direto no asm: ele passa por outro registrador.)
fn porta_dos_fundos(comando: u32, argumento: u32) -> (u32, u32, u32, u32) {
    let (a, b, c, d): (u32, u64, u32, u32);
    unsafe {
        core::arch::asm!(
            "xchg {b}, rbx",
            "in eax, dx",
            "xchg {b}, rbx",
            b = inout(reg) argumento as u64 => b,
            inout("eax") MAGICO => a,
            inout("ecx") comando => c,
            inout("edx") PORTA_VMWARE => d,
            options(nostack),
        );
    }
    (a, b as u32, c, d)
}

/// Liga o vmmouse no modo absoluto, se a máquina tiver um.
fn ligar_vmmouse() -> bool {
    if porta_dos_fundos(VERSAO, !MAGICO).1 != MAGICO {
        return false; // não é uma máquina virtual com a porta dos fundos
    }
    porta_dos_fundos(ABS_COMANDO, LIGAR);
    // Ele responde com uma palavra: a versão do protocolo.
    if porta_dos_fundos(ABS_ESTADO, 0).0 & 0xFFFF != 1 || porta_dos_fundos(ABS_DADOS, 1).0 != 0x3442_554A {
        return false;
    }
    porta_dos_fundos(ABS_COMANDO, PEDIR_ABSOLUTO);
    true
}

/// Lê os pacotes que o vmmouse tem na fila (4 palavras cada).
fn ler_vmmouse(e: &mut Estado) {
    loop {
        let estado = porta_dos_fundos(ABS_ESTADO, 0).0;
        if estado == 0xFFFF_0000 {
            ligar_vmmouse(); // ele se perdeu: liga de novo
            return;
        }
        if estado & 0xFFFF < 4 {
            return;
        }
        let (botoes, x, y, roda) = porta_dos_fundos(ABS_DADOS, 4);
        // No vmmouse: 0x20 = esquerdo, 0x10 = direito, 0x08 = meio.
        let b = (botoes & 0x20 != 0) as u8 | ((botoes & 0x10 != 0) as u8) << 1 | ((botoes & 0x08 != 0) as u8) << 2;
        empilhar(e, 1, b, x as i16, y as i16, roda as i8);
    }
}

fn empilhar(e: &mut Estado, tipo: u8, botoes: u8, x: i16, y: i16, roda: i8) {
    if LEITORES.load(Ordering::Relaxed) == 0 || e.fila.len() >= TAMANHO_PACOTE * 128 {
        return;
    }
    let [x0, x1] = x.to_le_bytes();
    let [y0, y1] = y.to_le_bytes();
    e.fila.extend([tipo, botoes, x0, x1, y0, y1, roda as u8, 0]);
}
static LEITORES: AtomicUsize = AtomicUsize::new(0);

fn canal() -> usize {
    &raw const ESTADO as usize
}

fn esperar_para_escrever() {
    for _ in 0..100_000 {
        if unsafe { inb(STATUS) } & 2 == 0 {
            return;
        }
    }
}

fn esperar_resposta() -> Option<u8> {
    for _ in 0..100_000 {
        if unsafe { inb(STATUS) } & 1 != 0 {
            return Some(unsafe { inb(DADOS) });
        }
    }
    None
}

fn mandar_ao_mouse(byte: u8) -> Option<u8> {
    esperar_para_escrever();
    unsafe { outb(STATUS, 0xD4) };
    esperar_para_escrever();
    unsafe { outb(DADOS, byte) };
    esperar_resposta() // 0xFA = entendido
}

/// Liga a porta auxiliar, a IRQ 12 e o envio de pacotes. Devolve se achou um mouse.
pub fn init() -> bool {
    unsafe {
        esperar_para_escrever();
        outb(STATUS, 0xA8); // liga a porta auxiliar
        esperar_para_escrever();
        outb(STATUS, 0x20); // lê o byte de configuração do controlador
        let Some(config) = esperar_resposta() else { return false };
        esperar_para_escrever();
        outb(STATUS, 0x60); // ...e o grava com a IRQ 12 ligada e o relógio do mouse ativo
        esperar_para_escrever();
        outb(DADOS, (config | 0x02) & !0x20);
    }
    let mut ok = mandar_ao_mouse(0xF6) == Some(0xFA); // configuração padrão
    // A "senha" do IntelliMouse; depois, quem é você? (0xF2: o tipo)
    for taxa in [200, 100, 80] {
        mandar_ao_mouse(0xF3);
        mandar_ao_mouse(taxa);
    }
    if mandar_ao_mouse(0xF2) == Some(0xFA) && esperar_resposta() == Some(3) {
        ESTADO.lock().tamanho = 4;
    }
    ok &= mandar_ao_mouse(0xF4) == Some(0xFA); // começa a mandar pacotes
    if ok && ligar_vmmouse() {
        ESTADO.lock().absoluto = true;
    }
    // descarta o que tenha sobrado
    while unsafe { inb(STATUS) } & 1 != 0 {
        unsafe { inb(DADOS) };
    }
    ok
}

/// Um byte do mouse (da IRQ 12, ou da IRQ 1 quando o byte era do mouse).
pub fn receber(byte: u8) {
    let mut e = ESTADO.lock();
    // O byte 0 sempre tem o bit 3 ligado: se não tiver, perdemos a sincronia.
    // Um 0xFA ali é a confirmação ("entendido") de um comando que chegou
    // atrasada; os bits 6 e 7 (estouro) também nunca vêm num pacote útil.
    if e.recebidos == 0 && (byte & 0x08 == 0 || byte == 0xFA || byte & 0xC0 != 0) {
        return;
    }
    let i = e.recebidos;
    e.pacote[i] = byte;
    e.recebidos += 1;
    if e.recebidos == e.tamanho {
        e.recebidos = 0;
        let antes = e.fila.len();
        // No modo absoluto, o pacote PS/2 em geral é só o aviso de que há
        // dados na porta dos fundos. Se lá não houver nada, ele é um
        // movimento relativo de verdade (de outra fonte): vale como tal.
        if e.absoluto {
            ler_vmmouse(&mut e);
        }
        if e.fila.len() == antes {
            let p = e.pacote;
            let dx = p[1] as i16 - if p[0] & 0x10 != 0 { 256 } else { 0 };
            let dy = p[2] as i16 - if p[0] & 0x20 != 0 { 256 } else { 0 };
            let roda = if e.tamanho == 4 { p[3] as i8 } else { 0 };
            empilhar(&mut e, 0, p[0] & 7, dx, -dy, roda);
        }
        if e.fila.len() != antes {
            drop(e);
            processo::acordar(canal());
        }
    }
}

pub fn tratar_irq() {
    let byte = unsafe { inb(DADOS) };
    receber(byte);
}

pub fn abrir() {
    LEITORES.fetch_add(1, Ordering::Relaxed);
}

pub fn fechar() {
    if LEITORES.fetch_sub(1, Ordering::Relaxed) == 1 {
        ESTADO.lock().fila.clear();
    }
}

/// Entrega pacotes inteiros (múltiplos de 8 bytes).
pub fn ler(buf: &mut [u8], bloquear: bool) -> Result<usize, i64> {
    loop {
        {
            let mut e = ESTADO.lock();
            let n = (buf.len() / TAMANHO_PACOTE * TAMANHO_PACOTE).min(e.fila.len());
            if n > 0 {
                for (destino, b) in buf.iter_mut().zip(e.fila.drain(..n)) {
                    *destino = b;
                }
                return Ok(n);
            }
        }
        if !bloquear {
            return Err(EAGAIN);
        }
        if processo::tem_sinal_pendente() {
            return Err(EINTR);
        }
        processo::dormir(canal());
    }
}

pub fn tem_roda() -> bool {
    ESTADO.lock().tamanho == 4
}

pub fn absoluto() -> bool {
    ESTADO.lock().absoluto
}

pub fn pode_ler() -> bool {
    !ESTADO.lock().fila.is_empty()
}
