//! Interrupções e exceções: a IDT e o tratador central.
//!
//! Quando algo acontece — uma exceção da CPU (divisão por zero, page fault),
//! uma IRQ de hardware (timer, teclado) ou uma instrução `int n` — a CPU
//! consulta a IDT (Interrupt Descriptor Table), uma tabela de 256 entradas,
//! e pula para o endereço guardado na entrada daquele vetor.
//!
//! Os 256 pontos de entrada estão em `interrupcoes.asm`. Todos salvam os
//! registradores e chamam `tratar_interrupcao` com um ponteiro para eles.
//!
//! ```text
//!   vetores  0..=31   exceções da CPU (fixos, definidos pela Intel)
//!   vetores 32..=47   IRQs do PIC (remapeadas em pic.rs)
//!   vetor  0x80       chamadas de sistema (syscall.rs)
//! ```

use crate::{gdt, pic, pit, println, processo, serial, smp, syscall, teclado};
use core::arch::asm;
use core::fmt;
use core::mem::size_of;

/// Os registradores da tarefa interrompida, do jeito que `isr_comum` os deixou
/// na pilha. A ordem dos campos precisa bater com a ordem dos `push`.
#[derive(Clone, Copy)]
#[repr(C)]
pub struct Contexto {
    pub r15: u64,
    pub r14: u64,
    pub r13: u64,
    pub r12: u64,
    pub r11: u64,
    pub r10: u64,
    pub r9: u64,
    pub r8: u64,
    pub rbp: u64,
    pub rdi: u64,
    pub rsi: u64,
    pub rdx: u64,
    pub rcx: u64,
    pub rbx: u64,
    pub rax: u64,
    pub vetor: u64,
    pub codigo_erro: u64,
    // Daqui para baixo, empilhado pela própria CPU:
    pub rip: u64,
    pub cs: u64,
    pub rflags: u64,
    pub rsp: u64,
    pub ss: u64,
}

impl Contexto {
    /// Estado inicial de um programa: tudo zerado, salta para `entrada` com a
    /// pilha em `rsp`, no anel 3 e com interrupções ligadas.
    pub fn usuario(entrada: u64, rsp: u64) -> Self {
        Self {
            r15: 0, r14: 0, r13: 0, r12: 0, r11: 0, r10: 0, r9: 0, r8: 0,
            rbp: 0, rdi: 0, rsi: 0, rdx: 0, rcx: 0, rbx: 0, rax: 0,
            vetor: 0,
            codigo_erro: 0,
            rip: entrada,
            cs: gdt::SELETOR_CODIGO_USUARIO as u64,
            rflags: 1 << 9, // IF
            rsp,
            ss: gdt::SELETOR_DADOS_USUARIO as u64,
        }
    }

    fn veio_do_usuario(&self) -> bool {
        self.cs & 3 == 3
    }
}

impl fmt::Debug for Contexto {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        writeln!(f, "  RIP {:016x}  RSP {:016x}  RFLAGS {:016x}", self.rip, self.rsp, self.rflags)?;
        writeln!(f, "  RAX {:016x}  RBX {:016x}  RCX    {:016x}", self.rax, self.rbx, self.rcx)?;
        writeln!(f, "  RDX {:016x}  RSI {:016x}  RDI    {:016x}", self.rdx, self.rsi, self.rdi)?;
        write!(f, "  RBP {:016x}  CS  {:#06x}            SS     {:#06x}", self.rbp, self.cs, self.ss)
    }
}

// -----------------------------------------------------------------------------
// A IDT
// -----------------------------------------------------------------------------

/// Uma entrada da IDT (16 bytes). O endereço do tratador vem picado em 3 partes
/// por razões históricas (a entrada de 32 bits tinha 8 bytes e foi "esticada").
#[derive(Clone, Copy)]
#[repr(C)]
struct Entrada {
    endereco_baixo: u16,
    seletor: u16,
    ist: u8,
    atributos: u8,
    endereco_meio: u16,
    endereco_alto: u32,
    reservado: u32,
}

impl Entrada {
    const VAZIA: Self = Self {
        endereco_baixo: 0,
        seletor: 0,
        ist: 0,
        atributos: 0,
        endereco_meio: 0,
        endereco_alto: 0,
        reservado: 0,
    };

    fn nova(endereco: u64, ist: u8, nivel: u8) -> Self {
        Self {
            endereco_baixo: endereco as u16,
            seletor: gdt::SELETOR_CODIGO,
            ist,
            // presente | DPL | tipo 0xE = "interrupt gate": a CPU desliga as
            // interrupções (IF = 0) ao entrar, então um tratador nunca é
            // interrompido por outra IRQ. O DPL diz de qual anel dá para
            // chamar o vetor com `int n`: só o 0x80 aceita o anel 3.
            atributos: 0x80 | (nivel << 5) | 0xE,
            endereco_meio: (endereco >> 16) as u16,
            endereco_alto: (endereco >> 32) as u32,
            reservado: 0,
        }
    }
}

static mut IDT: [Entrada; 256] = [Entrada::VAZIA; 256];

unsafe extern "C" {
    /// Endereços dos 256 stubs, definidos em interrupcoes.asm.
    safe static tabela_isr: [u64; 256];
}

#[repr(C, packed)]
struct Ponteiro {
    limite: u16,
    base: u64,
}

/// As outras CPUs usam a mesma IDT (smp.rs).
pub fn carregar_idt() {
    let idt = unsafe { &*(&raw const IDT) };
    let ponteiro = Ponteiro {
        limite: (size_of::<[Entrada; 256]>() - 1) as u16,
        base: idt.as_ptr() as u64,
    };
    unsafe { asm!("lidt [{}]", in(reg) &ponteiro, options(readonly, nostack, preserves_flags)) };
}

pub fn init() {
    let idt = unsafe { &mut *(&raw mut IDT) };
    for (vetor, entrada) in idt.iter_mut().enumerate() {
        let ist = if vetor == 8 { gdt::IST_DOUBLE_FAULT } else { 0 };
        // O DPL diz de qual anel dá para chamar o vetor com uma instrução:
        // int3 (breakpoint, 3) e int 0x80 (syscall) valem para os programas.
        let nivel = if vetor == 0x80 || vetor == 3 { 3 } else { 0 };
        *entrada = Entrada::nova(tabela_isr[vetor], ist, nivel);
    }

    let ponteiro = Ponteiro {
        limite: (size_of::<[Entrada; 256]>() - 1) as u16,
        base: idt.as_ptr() as u64,
    };
    unsafe { asm!("lidt [{}]", in(reg) &ponteiro, options(readonly, nostack, preserves_flags)) };
}

// -----------------------------------------------------------------------------
// Ligar e desligar interrupções
//
// Sem a opção `nomem`, o compilador trata esses `asm!` como barreiras: não move
// leituras e escritas de memória para fora da região protegida.
// -----------------------------------------------------------------------------

pub fn habilitar() {
    unsafe { asm!("sti", options(nostack)) };
}

pub fn desabilitar() {
    unsafe { asm!("cli", options(nostack)) };
}

pub fn habilitadas() -> bool {
    let rflags: u64;
    unsafe { asm!("pushfq", "pop {}", out(reg) rflags, options(preserves_flags)) };
    rflags & (1 << 9) != 0 // bit IF
}

/// Roda `f` com as interrupções desligadas e depois as devolve ao estado anterior.
///
/// Todo lock que um tratador de interrupção também usa precisa disto: se o
/// código normal estiver segurando o lock e a interrupção chegar, ela ficaria
/// girando para sempre esperando um lock que nunca vai ser solto (deadlock).
pub fn sem_interrupcoes<R>(f: impl FnOnce() -> R) -> R {
    let estavam_habilitadas = habilitadas();
    if estavam_habilitadas {
        desabilitar();
    }
    let resultado = f();
    if estavam_habilitadas {
        habilitar();
    }
    resultado
}

// -----------------------------------------------------------------------------
// O tratador central
// -----------------------------------------------------------------------------

const NOMES_EXCECOES: [&str; 32] = [
    "divisão por zero",
    "depuração",
    "NMI",
    "breakpoint",
    "overflow",
    "limite excedido (BOUND)",
    "instrução inválida",
    "FPU indisponível",
    "double fault",
    "coprocessor segment overrun",
    "TSS inválida",
    "segmento ausente",
    "falha no segmento de pilha",
    "proteção geral",
    "page fault",
    "reservada",
    "erro de ponto flutuante (x87)",
    "acesso desalinhado",
    "machine check",
    "erro de ponto flutuante (SIMD)",
    "virtualização",
    "proteção de controle",
    "reservada",
    "reservada",
    "reservada",
    "reservada",
    "reservada",
    "reservada",
    "injeção do hypervisor",
    "comunicação com o VMM",
    "segurança",
    "reservada",
];

/// Chamada por `isr_comum` com os registradores salvos. Devolve o contexto
/// que deve ser restaurado na volta.
#[unsafe(no_mangle)]
extern "C" fn tratar_interrupcao(ctx: &mut Contexto) -> *mut Contexto {
    // Só uma CPU por vez no kernel (smp.rs). Viemos de um programa ou do
    // `hlt` do ocioso, sem a trava; ou do próprio kernel no boot, já com ela.
    let peguei = smp::travar();
    match ctx.vetor {
        // Passo a passo (#DB, com a flag TF) e breakpoint (`int3`) num programa
        // viram SIGTRAP: é assim que o depurador fica sabendo.
        1 | 3 if ctx.veio_do_usuario() => {
            ctx.rflags &= !(1 << 8);
            let _ = processo::enviar_sinal_a_mim(crate::sinal::SIGTRAP);
        }
        3 => println!("[int3] breakpoint no kernel em {:#x}; seguindo em frente", ctx.rip),
        0..=31 => excecao(ctx),
        32..=47 => irq((ctx.vetor - 32) as u8),
        0x80 => syscall::tratar(ctx),
        smp::VETOR_TIMER => smp::tique_local(),
        smp::VETOR_ESPURIO => {}
        vetor => println!("[interrupção] vetor {} inesperado", vetor),
    }
    // A troca de processo acontece dentro de `ao_voltar_para_usuario`, por
    // `troca_contexto`: quando voltarmos aqui, já é a vez deste processo de novo.
    if ctx.veio_do_usuario() {
        let foi_o_timer = ctx.vetor == 32 || ctx.vetor == smp::VETOR_TIMER;
        processo::ao_voltar_para_usuario(ctx, foi_o_timer);
    }
    // De volta ao programa (ou ao `hlt`): a trava fica livre para as outras CPUs.
    if peguei {
        smp::destravar();
    }
    ctx
}

fn irq(numero: u8) {
    if pic::espuria(numero) {
        return;
    }
    match numero {
        0 => {
            pit::tique();
            processo::tique();
        }
        1 => teclado::tratar_irq(),
        4 => serial::tratar_irq(),
        12 => crate::mouse::tratar_irq(),
        _ => {}
    }
    pic::fim_de_interrupcao(numero);
}

fn excecao(ctx: &Contexto) {
    let nome = NOMES_EXCECOES[ctx.vetor as usize];

    // Page fault de um programa: pode ser só uma página que ainda não existe
    // (paginação sob demanda) ou um copy-on-write. Resolvido, a instrução roda de novo.
    if ctx.vetor == 14 && ctx.veio_do_usuario() {
        let (escrita, execucao) = (ctx.codigo_erro & 2 != 0, ctx.codigo_erro & 16 != 0);
        match processo::falha_de_pagina(ler_cr2(), escrita, execucao) {
            crate::paginacao::Falha::Resolvida => return,
            crate::paginacao::Falha::SemMemoria => processo::sem_memoria(),
            crate::paginacao::Falha::Invalida => {}
        }
    }

    // Erro de um programa: só ele morre, com o sinal que o Unix usaria.
    if ctx.veio_do_usuario() {
        let sinal = match ctx.vetor {
            0 | 16 | 19 => processo::SIGFPE,
            6 => processo::SIGILL,
            _ => processo::SIGSEGV,
        };
        if ctx.vetor == 14 {
            let acao = if ctx.codigo_erro & 16 != 0 {
                "executar"
            } else if ctx.codigo_erro & 2 != 0 {
                "escrever em"
            } else {
                "ler"
            };
            processo::falha_do_programa(
                format_args!("falha de segmentação ao {} {:#x} (RIP {:#x})", acao, ler_cr2(), ctx.rip),
                sinal,
            );
        } else {
            processo::falha_do_programa(format_args!("exceção #{} ({}) em RIP {:#x}", ctx.vetor, nome, ctx.rip), sinal);
        }
        return;
    }

    if ctx.vetor == 14 {
        let endereco = ler_cr2();
        let erro = ctx.codigo_erro;
        let quem = if erro & (1 << 2) != 0 { "um programa" } else { "o kernel" };
        let acao = if erro & (1 << 4) != 0 {
            "executar"
        } else if erro & (1 << 1) != 0 {
            "escrever em"
        } else {
            "ler"
        };
        let motivo = if erro & 1 != 0 { "violação de proteção" } else { "página não mapeada" };
        crate::erro_fatal(format_args!(
            "exceção #14 ({})\n  {} tentou {} {:#x}: {}\n{:?}",
            nome, quem, acao, endereco, motivo, ctx
        ));
    }

    crate::erro_fatal(format_args!(
        "exceção #{} ({}), código de erro {:#x}\n{:?}",
        ctx.vetor, nome, ctx.codigo_erro, ctx
    ))
}

/// No page fault, a CPU guarda em CR2 o endereço que causou a falha.
fn ler_cr2() -> u64 {
    let valor: u64;
    unsafe { asm!("mov {}, cr2", out(reg) valor, options(nomem, nostack, preserves_flags)) };
    valor
}
