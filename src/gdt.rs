//! GDT (Global Descriptor Table) e TSS (Task State Segment).
//!
//! O boot.asm carregou uma GDT mínima, só com o segmento de código: o
//! suficiente para entrar em 64 bits. Aqui montamos a definitiva:
//!
//! ```text
//!   0x00  nulo (obrigatório)
//!   0x08  código do kernel   (anel 0)
//!   0x10  dados do kernel    (anel 0)
//!   0x18  dados de usuário   (anel 3)   ┐ nessa ordem por exigência da
//!   0x20  código de usuário  (anel 3)   ┘ instrução `syscall`
//!   0x28  TSS (ocupa 2 entradas)
//! ```
//!
//! Em 64 bits a TSS não serve para trocar de tarefa, apesar do nome. Ela
//! guarda pilhas: a que a CPU usa ao passar do anel 3 para o 0 (RSP0) e até
//! 7 pilhas de emergência (IST) que uma entrada da IDT pode pedir. Usamos a
//! IST 1 no double fault: se a pilha do kernel estiver corrompida, a CPU
//! ainda tem uma pilha boa para nos contar o que aconteceu.

use core::arch::asm;
use core::mem::size_of;

pub const SELETOR_CODIGO: u16 = 0x08;
pub const SELETOR_DADOS: u16 = 0x10;
pub const SELETOR_DADOS_USUARIO: u16 = 0x18 | 3; // o "| 3" é o nível de privilégio pedido
pub const SELETOR_CODIGO_USUARIO: u16 = 0x20 | 3;
const SELETOR_TSS: u16 = 0x28;

/// Índice da pilha de emergência do double fault (a IDT conta a partir de 1).
pub const IST_DOUBLE_FAULT: u8 = 1;

#[repr(C, packed(4))]
struct Tss {
    reservado1: u32,
    rsp: [u64; 3],
    reservado2: u64,
    ist: [u64; 7],
    reservado3: u64,
    reservado4: u16,
    iomap_base: u16,
}

#[repr(C, align(16))]
struct Pilha([u8; 16 * 1024]);

static mut PILHA_DOUBLE_FAULT: Pilha = Pilha([0; 16 * 1024]);

static mut TSS: Tss = Tss {
    reservado1: 0,
    rsp: [0; 3],
    reservado2: 0,
    ist: [0; 7],
    reservado3: 0,
    reservado4: 0,
    // Base do mapa de permissões de E/S além do fim da TSS = "sem mapa".
    iomap_base: size_of::<Tss>() as u16,
};

static mut GDT: [u64; 7] = [
    0,
    // Bits: 43 executável | 44 código/dados | 45-46 DPL | 47 presente | 53 64 bits
    // (o bit 41 permite leitura no código e escrita nos dados)
    0x00AF_9A00_0000_FFFF, // código do kernel
    0x00CF_9200_0000_FFFF, // dados do kernel
    0x00CF_F200_0000_FFFF, // dados de usuário  (DPL 3)
    0x00AF_FA00_0000_FFFF, // código de usuário (DPL 3)
    0,                     // TSS: preenchida em init(), pois depende do endereço
    0,
];

#[repr(C, packed)]
struct Ponteiro {
    limite: u16,
    base: u64,
}

/// A pilha que a CPU usa quando um programa (anel 3) é interrompido ou faz
/// uma syscall. O escalonador troca a cada processo.
pub fn definir_rsp0(topo: u64) {
    unsafe { (*(&raw mut TSS)).rsp[0] = topo };
}

pub fn init() {
    unsafe {
        let topo = (&raw const PILHA_DOUBLE_FAULT) as u64 + size_of::<Pilha>() as u64;
        (*(&raw mut TSS)).ist[(IST_DOUBLE_FAULT - 1) as usize] = topo;

        // O descritor de TSS em 64 bits tem 16 bytes, com a base espalhada em pedaços.
        let base = (&raw const TSS) as u64;
        let limite = size_of::<Tss>() as u64 - 1;
        let gdt = &mut *(&raw mut GDT);
        gdt[5] = (limite & 0xFFFF)
            | ((base & 0xFF_FFFF) << 16)
            | (0x89 << 40) // presente + tipo 9 (TSS de 64 bits disponível)
            | (((limite >> 16) & 0xF) << 48)
            | (((base >> 24) & 0xFF) << 56);
        gdt[6] = base >> 32;

        let ponteiro = Ponteiro {
            limite: (size_of::<[u64; 7]>() - 1) as u16,
            base: gdt.as_ptr() as u64,
        };
        asm!("lgdt [{}]", in(reg) &ponteiro, options(readonly, nostack, preserves_flags));

        // Não existe `mov cs, ...`. Para recarregar CS empilhamos seletor e
        // endereço de retorno e fazemos um retorno "far" (retfq).
        asm!(
            "push {sel}",
            "lea {tmp}, [rip + 2f]",
            "push {tmp}",
            "retfq",
            "2:",
            sel = in(reg) SELETOR_CODIGO as u64,
            tmp = lateout(reg) _,
            options(preserves_flags),
        );
        asm!(
            "mov ds, {0:x}",
            "mov es, {0:x}",
            "mov ss, {0:x}",
            in(reg) SELETOR_DADOS,
            options(nostack, preserves_flags),
        );
        asm!("ltr {0:x}", in(reg) SELETOR_TSS, options(nostack, preserves_flags));
    }
}
