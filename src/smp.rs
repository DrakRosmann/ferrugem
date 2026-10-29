//! Várias CPUs (SMP): acordar as outras, e cada uma com o seu estado.
//!
//! No boot, só uma CPU roda (a "BSP"); as outras ("APs") ficam paradas
//! esperando um sinal. Para achá-las, lemos a tabela MADT do ACPI, que a
//! BIOS deixa na memória: ela lista o APIC de cada CPU. Para acordar uma,
//! mandamos pelo nosso APIC a sequência INIT, SIPI, SIPI: ela começa em
//! modo real no endereço que o SIPI diz (0x8000, onde copiamos o
//! `trampolim.asm`) e sobe sozinha até o modo longo e `ap_principal`.
//!
//! ## O que é de cada CPU
//!
//! Cada CPU tem uma `Cpu`: o processo que ela está rodando, a pilha de
//! kernel dele, a TSS, um processo ocioso só dela. O registrador GS aponta
//! para a `Cpu` enquanto ela está no kernel (a instrução `swapgs`, na entrada
//! e na saída do kernel, troca o GS do programa pelo do kernel).
//!
//! ## Uma trava para o kernel inteiro
//!
//! O kernel foi escrito para uma CPU: em vez de uma trava para cada
//! estrutura, há uma só (a "big kernel lock", como no Linux 2.0): uma CPU
//! a pega ao entrar no kernel e a solta ao voltar para o programa. Só uma
//! CPU por vez roda código do kernel; as outras rodam programas ao mesmo
//! tempo — é aí que está o ganho. E um mesmo espaço de endereçamento (as
//! threads de um processo) roda numa CPU por vez: assim, quando o kernel
//! muda as tabelas de páginas de um processo, nenhuma outra CPU pode estar
//! com traduções velhas dele na TLB.
//!
//! ## O APIC
//!
//! Usamos o x2APIC, programado por MSRs (o APIC antigo fica num endereço
//! físico que, aqui, cai no meio da memória dos programas). Cada AP tem o
//! seu timer (o do APIC, calibrado contra o PIT) para a preempção; a BSP
//! continua com o PIT, que também mantém o relógio do sistema.

use crate::gdt;
use crate::pit;
use crate::processo::Processo;
use crate::syscall::{escrever_msr, ler_msr};
use alloc::boxed::Box;
use alloc::vec;
use alloc::vec::Vec;
use core::arch::asm;
use core::arch::x86_64::__cpuid;
use core::ptr::null_mut;
use core::sync::atomic::{AtomicBool, AtomicU32, AtomicUsize, Ordering};

pub const MAX_CPUS: usize = 16;
pub const VETOR_TIMER: u64 = 0x30;
pub const VETOR_ESPURIO: u64 = 0xFF;

const GS_BASE: u32 = 0xC000_0101;
const KERNEL_GS_BASE: u32 = 0xC000_0102;
const EFER: u32 = 0xC000_0080;

/// O estado de uma CPU. Os três primeiros campos são lidos pelo assembly
/// (`gs:0`, `gs:8`, `gs:16`): não mude a ordem.
#[repr(C)]
pub struct Cpu {
    /// O topo da pilha de kernel do processo que esta CPU está rodando (para o `syscall`).
    pub pilha_kernel: u64,
    /// Onde o `syscall` guarda o RSP do programa por um instante.
    rsp_usuario: u64,
    eu: *mut Cpu,
    pub id: usize,
    pub apic: u32,
    pub atual: *mut Processo,
    pub ocioso: *mut Processo,
    pub tss: *mut gdt::Tss,
    gdt: *mut gdt::Gdt,
    segura_trava: bool,
}

impl Cpu {
    const fn nova(id: usize) -> Cpu {
        Cpu {
            pilha_kernel: 0,
            rsp_usuario: 0,
            eu: null_mut(),
            id,
            apic: 0,
            atual: null_mut(),
            ocioso: null_mut(),
            tss: null_mut(),
            gdt: null_mut(),
            segura_trava: false,
        }
    }
}

static mut CPU_PRINCIPAL: Cpu = Cpu::nova(0);
static mut CPUS: [*mut Cpu; MAX_CPUS] = [null_mut(); MAX_CPUS];
static QUANTAS: AtomicUsize = AtomicUsize::new(1);
/// Quantas APs já chegaram ao modo longo (a BSP espera por cada uma).
static ACORDADAS: AtomicUsize = AtomicUsize::new(0);
/// Quanto o timer do APIC conta em 10 ms (calibrado no boot).
static CONTAGEM_10MS: AtomicU32 = AtomicU32::new(0);

/// A CPU que está rodando este código.
pub fn cpu() -> &'static mut Cpu {
    let p: *mut Cpu;
    unsafe {
        asm!("mov {}, gs:[16]", out(reg) p, options(nostack, readonly, preserves_flags));
        &mut *p
    }
}

pub fn quantas() -> usize {
    QUANTAS.load(Ordering::Relaxed)
}

/// As CPUs ligadas (para o escalonador e o /proc/cpuinfo).
pub fn todas() -> impl Iterator<Item = &'static mut Cpu> {
    (0..quantas()).filter_map(|i| unsafe { CPUS[i].as_mut() })
}

/// Aponta o GS desta CPU para a `Cpu` dela (e o GS "do programa" para 0).
fn instalar(c: &'static mut Cpu) {
    c.eu = c;
    unsafe { CPUS[c.id] = c };
    escrever_msr(GS_BASE, c as *mut Cpu as u64);
    escrever_msr(KERNEL_GS_BASE, 0);
}

/// No começo do boot, logo depois da GDT: a CPU principal já segura a trava.
pub fn init_principal(tss: *mut gdt::Tss) {
    let c = unsafe { &mut *(&raw mut CPU_PRINCIPAL) };
    c.tss = tss;
    c.segura_trava = true;
    TRAVA.store(true, Ordering::Relaxed);
    instalar(c);
}

// -----------------------------------------------------------------------------
// A trava do kernel
// -----------------------------------------------------------------------------

static TRAVA: AtomicBool = AtomicBool::new(false);

/// Pega a trava do kernel (esperando, se outra CPU estiver com ela).
/// Devolve se pegou agora (false: esta CPU já estava com ela).
pub fn travar() -> bool {
    let c = cpu();
    if c.segura_trava {
        return false;
    }
    while TRAVA.compare_exchange_weak(false, true, Ordering::Acquire, Ordering::Relaxed).is_err() {
        core::hint::spin_loop();
    }
    c.segura_trava = true;
    true
}

pub fn destravar() {
    let c = cpu();
    if c.segura_trava {
        c.segura_trava = false;
        TRAVA.store(false, Ordering::Release);
    }
}

/// Um processo novo começa a vida voltando para o modo usuário (veja
/// `comeco_de_processo` no assembly): solta a trava que herdou de quem o escalonou.
#[unsafe(no_mangle)]
extern "C" fn processo_comecou() {
    destravar();
}

// -----------------------------------------------------------------------------
// O x2APIC
// -----------------------------------------------------------------------------

fn tem_x2apic() -> bool {
    #[allow(unused_unsafe)]
    let r = unsafe { __cpuid(1) };
    r.ecx & (1 << 21) != 0
}

fn ligar_apic(principal: bool) {
    escrever_msr(0x1B, ler_msr(0x1B) | 1 << 11 | 1 << 10); // ligado, modo x2APIC
    escrever_msr(0x808, 0); // prioridade 0: aceita tudo
    escrever_msr(0x80F, 0x100 | VETOR_ESPURIO); // liga o APIC; vetor das interrupções espúrias
    // LINT0: na principal, é por onde chegam as IRQs do PIC (modo "ExtINT");
    // nas outras, desligado. LINT1: NMI.
    escrever_msr(0x835, if principal { 0x700 } else { 1 << 16 });
    escrever_msr(0x836, 0x400);
}

/// Fim de interrupção para o APIC (o timer dele e as IPIs; as do PIC não precisam).
pub fn fim_de_interrupcao() {
    escrever_msr(0x80B, 0);
}

fn meu_apic() -> u32 {
    ler_msr(0x802) as u32
}

/// O timer do APIC: periódico, a cada 10 ms (100 Hz, como o PIT).
fn ligar_timer() {
    escrever_msr(0x83E, 0x3); // divide o relógio por 16
    escrever_msr(0x832, VETOR_TIMER | 1 << 17); // periódico
    escrever_msr(0x838, CONTAGEM_10MS.load(Ordering::Relaxed) as u64);
}

/// Mede quanto o timer do APIC conta em 50 ms do PIT.
fn calibrar() {
    escrever_msr(0x83E, 0x3);
    escrever_msr(0x832, 1 << 16); // mascarado: só contamos
    escrever_msr(0x838, 0xFFFF_FFFF);
    let inicio = pit::milissegundos();
    esperar_ms(50);
    let contado = 0xFFFF_FFFF - ler_msr(0x839) as u32;
    let ms = (pit::milissegundos() - inicio).max(1);
    CONTAGEM_10MS.store((contado as u64 * 10 / ms) as u32, Ordering::Relaxed);
}

/// Espera com as interrupções ligadas (o relógio anda pelo PIT). Só no boot.
fn esperar_ms(ms: u64) {
    let fim = pit::milissegundos() + ms;
    while pit::milissegundos() < fim {
        unsafe { asm!("sti", "hlt", "cli", options(nostack)) };
    }
}

/// Manda uma IPI (interrupção entre CPUs) para o APIC `destino`.
fn ipi(destino: u32, comando: u64) {
    escrever_msr(0x830, (destino as u64) << 32 | comando);
}

// -----------------------------------------------------------------------------
// ACPI: a lista de CPUs
// -----------------------------------------------------------------------------

fn ler<T: Copy>(endereco: u64) -> T {
    unsafe { core::ptr::read_unaligned(endereco as *const T) }
}

/// Procura a assinatura "RSD PTR " (a raiz do ACPI) na área da BIOS.
fn achar_rsdp() -> Option<u64> {
    let ebda = (ler::<u16>(0x40E) as u64) << 4;
    let areas = [(ebda, ebda + 1024), (0xE0000, 0x100000)];
    for (inicio, fim) in areas {
        let mut e = inicio;
        while e + 20 <= fim {
            if ler::<[u8; 8]>(e) == *b"RSD PTR " {
                return Some(e);
            }
            e += 16;
        }
    }
    None
}

/// Os IDs de APIC das CPUs ligadas, pela MADT ("APIC") do ACPI.
fn cpus_do_acpi() -> Vec<u32> {
    let mut apics = Vec::new();
    let Some(rsdp) = achar_rsdp() else { return apics };
    // RSDT: cabeçalho de 36 bytes e endereços de 32 bits das outras tabelas.
    let rsdt = ler::<u32>(rsdp + 16) as u64;
    if rsdt == 0 || rsdt >= 1 << 30 {
        return apics;
    }
    let tamanho = ler::<u32>(rsdt + 4) as u64;
    for i in 0..(tamanho.saturating_sub(36) / 4) {
        let tabela = ler::<u32>(rsdt + 36 + i * 4) as u64;
        if tabela == 0 || tabela >= 1 << 30 || ler::<[u8; 4]>(tabela) != *b"APIC" {
            continue;
        }
        let fim = tabela + ler::<u32>(tabela + 4) as u64;
        let mut e = tabela + 44;
        while e + 2 <= fim {
            let (tipo, comprimento) = (ler::<u8>(e), ler::<u8>(e + 1) as u64);
            if comprimento < 2 {
                break;
            }
            match tipo {
                // APIC local: id do ACPI, id do APIC, flags (bit 0 = ligada)
                0 if ler::<u32>(e + 4) & 1 != 0 => apics.push(ler::<u8>(e + 3) as u32),
                // x2APIC local: id de 32 bits
                9 if ler::<u32>(e + 8) & 1 != 0 => apics.push(ler::<u32>(e + 4)),
                _ => {}
            }
            e += comprimento;
        }
    }
    apics
}

// -----------------------------------------------------------------------------
// Acordar as outras CPUs
// -----------------------------------------------------------------------------

static TRAMPOLIM: &[u8] = include_bytes!("../build/trampolim.bin");
const ENDERECO_TRAMPOLIM: u64 = 0x8000;
const TAMANHO_PILHA: usize = 64 * 1024;

fn ler_cr(n: u8) -> u64 {
    let v: u64;
    unsafe {
        match n {
            0 => asm!("mov {}, cr0", out(reg) v, options(nomem, nostack, preserves_flags)),
            3 => asm!("mov {}, cr3", out(reg) v, options(nomem, nostack, preserves_flags)),
            _ => asm!("mov {}, cr4", out(reg) v, options(nomem, nostack, preserves_flags)),
        }
    }
    v
}

/// Liga o APIC da principal e acorda as outras. Devolve quantas CPUs há.
/// Chamada no fim do boot (as interrupções ligam e desligam durante as esperas).
pub fn iniciar() -> usize {
    if !tem_x2apic() {
        return 1;
    }
    ligar_apic(true);
    let principal = cpu();
    principal.apic = meu_apic();
    principal.ocioso = principal.atual;
    calibrar();
    let apics = cpus_do_acpi();

    for &apic in apics.iter().filter(|&&a| a != principal.apic) {
        let id = quantas();
        if id >= MAX_CPUS {
            break;
        }
        // Tudo o que a CPU nova vai usar é preparado aqui, por nós.
        let c = Box::leak(Box::new(Cpu::nova(id)));
        c.apic = apic;
        c.gdt = Box::leak(Box::new(gdt::Gdt::nova()));
        c.ocioso = crate::processo::novo_ocioso(id);
        c.atual = c.ocioso;
        let pilha = Box::leak(vec![0u8; TAMANHO_PILHA].into_boxed_slice());
        let topo = (pilha.as_ptr() as u64 + TAMANHO_PILHA as u64) & !15;
        unsafe { CPUS[id] = c };

        // O trampolim, com os parâmetros nos primeiros bytes (veja trampolim.asm).
        unsafe {
            let destino = ENDERECO_TRAMPOLIM as *mut u8;
            core::ptr::copy_nonoverlapping(TRAMPOLIM.as_ptr(), destino, TRAMPOLIM.len());
            let parametros = [ler_cr(3), ler_cr(4), ler_cr(0), ler_msr(EFER), topo, ap_principal as *const () as u64, id as u64];
            for (i, &v) in parametros.iter().enumerate() {
                (destino.add(8 + i * 8) as *mut u64).write_volatile(v);
            }
        }
        let antes = ACORDADAS.load(Ordering::Acquire);
        ipi(apic, 0x4500); // INIT
        esperar_ms(10);
        for _ in 0..2 {
            ipi(apic, 0x4600 | (ENDERECO_TRAMPOLIM >> 12)); // SIPI: comece em 0x8000
            esperar_ms(1);
        }
        let limite = pit::milissegundos() + 500;
        while ACORDADAS.load(Ordering::Acquire) == antes && pit::milissegundos() < limite {
            esperar_ms(1);
        }
        if ACORDADAS.load(Ordering::Acquire) == antes {
            crate::println!("  [ -- ] a CPU com APIC {} não acordou", apic);
            unsafe { CPUS[id] = null_mut() };
            continue;
        }
        QUANTAS.store(id + 1, Ordering::Release);
    }
    quantas()
}

/// Onde cada AP chega, já em 64 bits, na pilha que preparamos.
extern "C" fn ap_principal(id: u64) -> ! {
    let c = unsafe { &mut *CPUS[id as usize] };
    c.tss = gdt::carregar(unsafe { &mut *c.gdt });
    crate::interrupcoes::carregar_idt();
    instalar(c);
    crate::syscall::init();
    crate::processo::preparar_fpu();
    ligar_apic(false);
    ligar_timer();
    ACORDADAS.fetch_add(1, Ordering::Release);
    travar();
    crate::processo::ocioso()
}

/// O timer do APIC de uma AP: conta o tempo do processo dela (a preempção
/// acontece na volta para o programa, como com o PIT).
pub fn tique_local() {
    crate::processo::contar_tique();
    fim_de_interrupcao();
}

/// `getcpu`: em qual CPU estou.
pub fn numero() -> usize {
    cpu().id
}
