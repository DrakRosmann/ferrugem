//! Tabela global de descritores: segmentos do kernel e do usuário, e um TSS
//! com a pilha do kernel usada ao entrar vindo do anel 3 (rsp0) e uma
//! pilha separada (IST) para o tratador de falta dupla.

use core::arch::asm;
use core::mem::size_of;
use core::ptr::addr_of_mut;

pub const KERNEL_CODE: u16 = 0x08;
pub const KERNEL_DATA: u16 = 0x10;
// A ordem dados-antes-de-código do usuário é exigida pelo `sysret` (STAR).
pub const USER_DATA: u16 = 0x18 | 3;
pub const USER_CODE: u16 = 0x20 | 3;
const TSS_SELECTOR: u16 = 0x28;

/// Índice IST (1..=7) usado pela falta dupla.
pub const DOUBLE_FAULT_IST: u8 = 1;

#[repr(C, packed)]
struct Tss {
    _reserved0: u32,
    rsp: [u64; 3],
    _reserved1: u64,
    ist: [u64; 7],
    _reserved2: u64,
    _reserved3: u16,
    iomap_base: u16,
}

#[repr(C, packed)]
struct DescriptorPointer {
    limit: u16,
    base: u64,
}

#[repr(C, align(16))]
struct Stack([u8; 16 * 1024]);

static mut DOUBLE_FAULT_STACK: Stack = Stack([0; 16 * 1024]);
static mut TSS: Tss = Tss {
    _reserved0: 0,
    rsp: [0; 3],
    _reserved1: 0,
    ist: [0; 7],
    _reserved2: 0,
    _reserved3: 0,
    iomap_base: size_of::<Tss>() as u16,
};
static mut GDT: [u64; 7] = [
    0,
    0x00AF9A000000FFFF, // código 64 bits, anel 0
    0x00CF92000000FFFF, // dados, anel 0
    0x00CFF2000000FFFF, // dados, anel 3
    0x00AFFA000000FFFF, // código 64 bits, anel 3
    0,                  // TSS (parte baixa)
    0,                  // TSS (parte alta)
];

pub fn init() {
    unsafe {
        let stack = addr_of_mut!(DOUBLE_FAULT_STACK);
        let stack_top = stack as u64 + size_of::<Stack>() as u64;
        let tss = addr_of_mut!(TSS);
        let mut ist = [0u64; 7];
        ist[DOUBLE_FAULT_IST as usize - 1] = stack_top;
        (*tss).ist = ist;

        let base = tss as u64;
        let limit = (size_of::<Tss>() - 1) as u64;
        let low = (limit & 0xFFFF)
            | (base & 0xFF_FFFF) << 16
            | 0x89 << 40 // presente, TSS de 64 bits disponível
            | ((limit >> 16) & 0xF) << 48
            | ((base >> 24) & 0xFF) << 56;
        let gdt = addr_of_mut!(GDT);
        (*gdt)[5] = low;
        (*gdt)[6] = base >> 32;

        let pointer = DescriptorPointer { limit: (size_of::<[u64; 7]>() - 1) as u16, base: gdt as u64 };
        asm!("lgdt [{}]", in(reg) &pointer, options(readonly, nostack, preserves_flags));

        // Recarrega CS com um "retorno distante" e os demais seletores.
        asm!(
            "push {code}",
            "lea {tmp}, [rip + 2f]",
            "push {tmp}",
            "retfq",
            "2:",
            "mov ds, {data:x}",
            "mov es, {data:x}",
            "mov ss, {data:x}",
            code = const KERNEL_CODE as u64,
            data = in(reg) KERNEL_DATA as u64,
            tmp = out(reg) _,
        );
        asm!("ltr {:x}", in(reg) TSS_SELECTOR, options(nomem, nostack, preserves_flags));
    }
}

/// Define a pilha que a CPU usa ao receber uma interrupção no anel 3.
pub fn set_kernel_stack(top: u64) {
    unsafe { (*addr_of_mut!(TSS)).rsp = [top, 0, 0] }
}
