//! Vídeo gráfico: a placa "Bochs VBE" (a placa padrão do QEMU, `-vga std`).
//!
//! No boot, a tela está no modo texto da VGA (80×25, veja vga.rs). Esta
//! placa também tem um modo gráfico simples de programar: escrevemos a
//! resolução e a profundidade de cor em dois registradores (portas 0x1CE e
//! 0x1CF) e ela passa a mostrar um *framebuffer* linear — a imagem, pixel a
//! pixel, num pedaço da memória física. Onde fica esse pedaço é a placa que
//! diz, no espaço de configuração PCI dela (BAR 0).
//!
//! O kernel não desenha nada no modo gráfico: quem abre `/dev/fb0` (o
//! servidor de janelas) muda o modo, mapeia o framebuffer na própria
//! memória com `mmap` e desenha lá. Quando ele fecha o `/dev/fb0` (ou morre),
//! voltamos ao modo texto.
//!
//! Para escrever texto no modo gráfico, o servidor precisa de uma fonte: a
//! mesma do console (vga.rs), em `/dev/fonte`. Ao voltar ao modo texto, ela
//! é gravada de novo na placa, já que o modo gráfico escreve por cima.
//! Do mesmo jeito, guardamos os registradores da VGA do modo texto (a placa
//! os reprograma para o modo gráfico) e os repomos na volta.

use crate::erro::*;
use crate::porta::{inb, outb, outw};
use crate::processo;
use crate::spin::Mutex;
use crate::vga;
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicU32, Ordering};

const INDICE: u16 = 0x01CE;
const DADOS: u16 = 0x01CF;
const REG_ID: u16 = 0;
const REG_LARGURA: u16 = 1;
const REG_ALTURA: u16 = 2;
const REG_BPP: u16 = 3;
const REG_LIGADO: u16 = 4;
const LIGADO: u16 = 0x01;
const LINEAR: u16 = 0x40;

pub const LARGURA: u32 = 1024;
pub const ALTURA: u32 = 768;
const BPP: u32 = 32;

/// Onde está o framebuffer na memória física, e quanto a placa tem de memória.
static PLACA: Mutex<Option<(u64, u64)>> = Mutex::new(None);
/// Quem está com a tela no modo gráfico (0 = modo texto).
static DONO: AtomicU32 = AtomicU32::new(0);

/// Os registradores da VGA no modo texto, como a BIOS deixou.
struct RegistradoresVga {
    misc: u8,
    sequenciador: [u8; 5],
    crtc: [u8; 25],
    grafico: [u8; 9],
    atributos: [u8; 21],
}

static MODO_TEXTO: Mutex<Option<RegistradoresVga>> = Mutex::new(None);

fn ler_indexado(indice: u16, dados: u16, i: u8) -> u8 {
    unsafe {
        outb(indice, i);
        inb(dados)
    }
}

fn guardar_modo_texto() -> RegistradoresVga {
    let mut r = RegistradoresVga { misc: 0, sequenciador: [0; 5], crtc: [0; 25], grafico: [0; 9], atributos: [0; 21] };
    unsafe {
        r.misc = inb(0x3CC);
        for (i, v) in r.sequenciador.iter_mut().enumerate() {
            *v = ler_indexado(0x3C4, 0x3C5, i as u8);
        }
        for (i, v) in r.crtc.iter_mut().enumerate() {
            *v = ler_indexado(0x3D4, 0x3D5, i as u8);
        }
        for (i, v) in r.grafico.iter_mut().enumerate() {
            *v = ler_indexado(0x3CE, 0x3CF, i as u8);
        }
        // O controlador de atributos tem uma porta só (0x3C0) que alterna entre
        // índice e dado; ler 0x3DA a põe de volta em "índice".
        for (i, v) in r.atributos.iter_mut().enumerate() {
            inb(0x3DA);
            outb(0x3C0, i as u8);
            *v = inb(0x3C1);
        }
        inb(0x3DA);
        outb(0x3C0, 0x20); // a tela volta a mostrar a imagem
    }
    r
}

fn repor_modo_texto(r: &RegistradoresVga) {
    unsafe {
        outb(0x3C2, r.misc);
        for (i, &v) in r.sequenciador.iter().enumerate() {
            outb(0x3C4, i as u8);
            outb(0x3C5, v);
        }
        // Os registradores 0..7 do CRTC vêm travados (bit 7 do 0x11).
        outb(0x3D4, 0x11);
        outb(0x3D5, r.crtc[0x11] & !0x80);
        for (i, &v) in r.crtc.iter().enumerate() {
            outb(0x3D4, i as u8);
            outb(0x3D5, if i == 0x11 { v & !0x80 } else { v });
        }
        outb(0x3D4, 0x11);
        outb(0x3D5, r.crtc[0x11]);
        for (i, &v) in r.grafico.iter().enumerate() {
            outb(0x3CE, i as u8);
            outb(0x3CF, v);
        }
        for (i, &v) in r.atributos.iter().enumerate() {
            inb(0x3DA);
            outb(0x3C0, i as u8);
            outb(0x3C0, v);
        }
        inb(0x3DA);
        outb(0x3C0, 0x20);
    }
}

fn escrever(registrador: u16, valor: u16) {
    unsafe {
        outw(INDICE, registrador);
        outw(DADOS, valor);
    }
}

fn ler(registrador: u16) -> u16 {
    unsafe {
        outw(INDICE, registrador);
        crate::porta::inw(DADOS)
    }
}

/// Procura a placa no barramento PCI 0 (vendedor 0x1234, dispositivo 0x1111).
fn achar_placa() -> Option<u64> {
    let placa = crate::pci::achar(0x1234, 0x1111)?;
    Some((crate::pci::ler(placa, 0x10) & !0xF) as u64)
}

/// No boot (ainda no modo texto): acha a placa e guarda os registradores da VGA.
pub fn init() -> Option<String> {
    if ler(REG_ID) < 0xB0C0 {
        return None; // não é uma placa Bochs VBE
    }
    let fisico = achar_placa()?;
    // A memória de vídeo, em blocos de 64 KiB (registrador 0xA).
    let memoria = ler(0xA) as u64 * 64 * 1024;
    *PLACA.lock() = Some((fisico, memoria.max((LARGURA * ALTURA * 4) as u64)));
    *MODO_TEXTO.lock() = Some(guardar_modo_texto());
    Some(format!("vídeo: placa Bochs VBE, framebuffer em {:#x} ({} MiB)", fisico, memoria >> 20))
}

pub fn existe() -> bool {
    PLACA.lock().is_some()
}

/// Abrir /dev/fb0: a tela vai para o modo gráfico (um dono por vez).
pub fn abrir() -> Result<(), i64> {
    if !existe() {
        return Err(ENODEV);
    }
    let pid = processo::atual().tgid;
    if DONO.compare_exchange(0, pid, Ordering::Relaxed, Ordering::Relaxed).is_err() {
        return Err(EBUSY);
    }
    vga::modo_grafico(true);
    escrever(REG_LIGADO, 0);
    escrever(REG_LARGURA, LARGURA as u16);
    escrever(REG_ALTURA, ALTURA as u16);
    escrever(REG_BPP, BPP as u16);
    escrever(REG_LIGADO, LIGADO | LINEAR);
    Ok(())
}

/// O último descritor do /dev/fb0 fechou: volta ao modo texto, com a fonte
/// e o conteúdo do console que estava na tela.
pub fn fechar() {
    if DONO.swap(0, Ordering::Relaxed) == 0 {
        return;
    }
    escrever(REG_LIGADO, 0);
    if let Some(r) = &*MODO_TEXTO.lock() {
        repor_modo_texto(r);
    }
    vga::repor_fonte();
    vga::modo_grafico(false);
}

/// Ctrl+Alt+Backspace: mata quem está com a tela (e o modo texto volta).
pub fn emergencia() {
    let dono = DONO.load(Ordering::Relaxed);
    if dono != 0 {
        let _ = processo::enviar_sinal(dono, crate::sinal::SIGKILL);
    }
}

/// (endereço físico, tamanho) do framebuffer, para o mmap.
pub fn memoria() -> Option<(u64, u64)> {
    *PLACA.lock()
}

pub fn fonte() -> Vec<u8> {
    vga::fonte()
}

const FBIOGET_VSCREENINFO: u64 = 0x4600;
const FBIOGET_FSCREENINFO: u64 = 0x4602;

/// Os ioctl de framebuffer do Linux: o tamanho da tela e o formato dos pixels.
pub fn ioctl(pedido: u64) -> Result<Vec<u8>, i64> {
    let (fisico, memoria) = memoria().ok_or(ENODEV)?;
    match pedido {
        FBIOGET_VSCREENINFO => {
            // struct fb_var_screeninfo (160 bytes)
            let mut v = [0u8; 160];
            let mut por = |i: usize, valor: u32| v[i..i + 4].copy_from_slice(&valor.to_le_bytes());
            por(0, LARGURA);
            por(4, ALTURA);
            por(8, LARGURA);
            por(12, ALTURA);
            por(24, BPP);
            // vermelho, verde e azul: (deslocamento, bits) — pixels 0x00RRGGBB
            por(32, 16);
            por(36, 8);
            por(44, 8);
            por(48, 8);
            por(56, 0);
            por(60, 8);
            Ok(v.to_vec())
        }
        FBIOGET_FSCREENINFO => {
            // struct fb_fix_screeninfo (80 bytes)
            let mut f = [0u8; 80];
            f[..9].copy_from_slice(b"bochs-vbe");
            f[16..24].copy_from_slice(&fisico.to_le_bytes());
            f[24..28].copy_from_slice(&(memoria as u32).to_le_bytes());
            f[36..40].copy_from_slice(&2u32.to_le_bytes()); // FB_VISUAL_TRUECOLOR
            f[48..52].copy_from_slice(&(LARGURA * 4).to_le_bytes()); // bytes por linha
            Ok(f.to_vec())
        }
        _ => Err(ENOTTY),
    }
}
