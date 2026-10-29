//! PIC 8259: o controlador de interrupções "clássico" do PC.
//!
//! São dois chips em cascata, cada um com 8 linhas de IRQ:
//!
//! ```text
//!   mestre  IRQ 0-7    0 timer (PIT)  1 teclado  2 escravo  4 COM1 ...
//!   escravo IRQ 8-15   8 relógio (RTC)  12 mouse PS/2  14-15 discos ...
//! ```
//!
//! Por padrão a BIOS deixa o mestre entregando as IRQs 0-7 nos vetores 8-15,
//! que em modo protegido são exceções da CPU (o 8 é o double fault!). Então a
//! primeira coisa é *remapear*: IRQ 0-15 passam para os vetores 32-47.

use crate::porta::{inb, outb};

const MESTRE_COMANDO: u16 = 0x20;
const MESTRE_DADOS: u16 = 0x21;
const ESCRAVO_COMANDO: u16 = 0xA0;
const ESCRAVO_DADOS: u16 = 0xA1;

pub const VETOR_BASE: u8 = 32;

const FIM_DE_INTERRUPCAO: u8 = 0x20;

pub fn init() {
    unsafe {
        // ICW1: começar a inicialização; vamos mandar a ICW4 também
        outb(MESTRE_COMANDO, 0x11);
        esperar();
        outb(ESCRAVO_COMANDO, 0x11);
        esperar();
        // ICW2: vetor da primeira IRQ de cada chip
        outb(MESTRE_DADOS, VETOR_BASE);
        esperar();
        outb(ESCRAVO_DADOS, VETOR_BASE + 8);
        esperar();
        // ICW3: o escravo está pendurado na linha 2 do mestre
        outb(MESTRE_DADOS, 1 << 2);
        esperar();
        outb(ESCRAVO_DADOS, 2);
        esperar();
        // ICW4: modo 8086
        outb(MESTRE_DADOS, 0x01);
        esperar();
        outb(ESCRAVO_DADOS, 0x01);
        esperar();

        // Máscaras (bit 1 = IRQ bloqueada). Liberamos só o que sabemos tratar:
        // timer (0), teclado (1) e COM1 (4).
        outb(MESTRE_DADOS, !((1 << 0) | (1 << 1) | (1 << 4)));
        outb(ESCRAVO_DADOS, 0xFF);
    }
}

/// Avisa o PIC que terminamos de tratar a IRQ. Sem isso, ele não entrega
/// mais nenhuma IRQ de prioridade igual ou menor.
pub fn fim_de_interrupcao(irq: u8) {
    unsafe {
        if irq >= 8 {
            outb(ESCRAVO_COMANDO, FIM_DE_INTERRUPCAO);
        }
        outb(MESTRE_COMANDO, FIM_DE_INTERRUPCAO);
    }
}

/// IRQ 7 e 15 podem ser "espúrias": ruído na linha que some antes do PIC
/// dizer qual IRQ era. Nesse caso o bit no registrador ISR (In-Service) não
/// está ligado e *não* devemos mandar fim de interrupção para aquele chip.
pub fn espuria(irq: u8) -> bool {
    let (comando, bit) = match irq {
        7 => (MESTRE_COMANDO, 7),
        15 => (ESCRAVO_COMANDO, 7),
        _ => return false,
    };
    let em_servico = unsafe {
        outb(comando, 0x0B); // OCW3: próxima leitura devolve o ISR
        inb(comando)
    };
    if em_servico & (1 << bit) != 0 {
        return false;
    }
    if irq == 15 {
        // O mestre viu uma IRQ 2 legítima (vinda do escravo): ele precisa do aviso.
        unsafe { outb(MESTRE_COMANDO, FIM_DE_INTERRUPCAO) };
    }
    true
}

/// Uma escrita na porta 0x80 (não usada) leva ~1 µs: tempo para o PIC antigo
/// processar o comando anterior.
fn esperar() {
    unsafe { outb(0x80, 0) };
}
