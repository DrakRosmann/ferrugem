//! Memória virtual: tabelas de página de 4 níveis e espaços de endereçamento.
//!
//! Layout virtual (igual em todos os processos na metade alta):
//!
//! ```text
//! 0x0000_0000_0000_0000 .. 0x0000_8000_0000_0000   processo de usuário
//! 0xFFFF_8000_0000_0000 .. +4 GiB                   mapa direto da RAM física
//! 0xFFFF_FFFF_8000_0000 .. +1 GiB                   imagem do kernel
//! ```
//!
//! Cada processo tem sua própria tabela P4; as entradas 256..512 (metade
//! alta) apontam para as mesmas tabelas do kernel, então o kernel está
//! sempre mapeado, mas sem o bit USER, inacessível ao anel 3.

use alloc::vec::Vec;
use core::sync::atomic::{AtomicU64, Ordering};

use crate::cpu;
use crate::memory::{self, FRAME_SIZE};

pub const PHYS_OFFSET: u64 = 0xFFFF_8000_0000_0000;
pub const KERNEL_VMA: u64 = 0xFFFF_FFFF_8000_0000;
/// Primeiro endereço que não pertence mais ao usuário.
pub const USER_TOP: u64 = 0x0000_8000_0000_0000;

pub const PRESENT: u64 = 1 << 0;
pub const WRITABLE: u64 = 1 << 1;
pub const USER: u64 = 1 << 2;
pub const NO_EXECUTE: u64 = 1 << 63;
/// Bit livre para o sistema (a CPU o ignora): página de cópia sob escrita.
pub const COPY_ON_WRITE: u64 = 1 << 9;
const ADDR_MASK: u64 = 0x000F_FFFF_FFFF_F000;

type Table = [u64; 512];

static KERNEL_P4: AtomicU64 = AtomicU64::new(0);

/// Endereço virtual (no mapa direto) de um endereço físico.
pub fn phys_to_virt<T>(phys: u64) -> *mut T {
    (phys + PHYS_OFFSET) as *mut T
}

fn table(phys: u64) -> &'static mut Table {
    unsafe { &mut *phys_to_virt::<Table>(phys) }
}

fn index(virt: u64, level: u32) -> usize {
    ((virt >> (12 + 9 * (level - 1))) & 0x1FF) as usize
}

/// Adota a P4 montada pelo boot como a do kernel e desfaz o mapeamento
/// 1:1 da metade baixa: dali em diante ela pertence aos processos, e um
/// ponteiro nulo no kernel passa a causar falha de página.
pub fn init() {
    let p4 = cpu::read_cr3() & ADDR_MASK;
    KERNEL_P4.store(p4, Ordering::Relaxed);
    table(p4)[0] = 0;
    unsafe { cpu::write_cr3(p4) };
}

pub fn kernel_p4() -> u64 {
    KERNEL_P4.load(Ordering::Relaxed)
}

/// O espaço de endereçamento de um processo. Libera todas as suas páginas
/// de usuário e tabelas ao ser descartado.
pub struct AddressSpace {
    p4: u64,
    /// Quadros de usuário mapeados (para o `ps`).
    pages: usize,
}

impl AddressSpace {
    pub fn new() -> Option<Self> {
        let p4 = memory::alloc_zeroed_frame()?;
        let kernel = table(kernel_p4());
        table(p4)[256..].copy_from_slice(&kernel[256..]);
        Some(Self { p4, pages: 0 })
    }

    pub fn p4(&self) -> u64 {
        self.p4
    }

    pub fn resident_bytes(&self) -> u64 {
        self.pages as u64 * FRAME_SIZE
    }

    fn is_active(&self) -> bool {
        cpu::read_cr3() & ADDR_MASK == self.p4
    }

    /// Entrada P1 de `virt`, criando as tabelas intermediárias se `create`.
    fn entry(&mut self, virt: u64, create: bool) -> Option<&'static mut u64> {
        let mut current = self.p4;
        for level in [4, 3, 2] {
            let entry = &mut table(current)[index(virt, level)];
            if *entry & PRESENT == 0 {
                if !create {
                    return None;
                }
                *entry = memory::alloc_zeroed_frame()? | PRESENT | WRITABLE | USER;
            }
            current = *entry & ADDR_MASK;
        }
        Some(&mut table(current)[index(virt, 1)])
    }

    /// Mapeia uma página de usuário em um quadro físico já reservado, que
    /// passa a pertencer a este espaço.
    pub fn map(&mut self, virt: u64, phys: u64, flags: u64) -> Result<(), ()> {
        debug_assert!(virt < USER_TOP && virt % FRAME_SIZE == 0);
        let entry = self.entry(virt, true).ok_or(())?;
        let replaced = *entry & PRESENT != 0;
        *entry = phys | flags | PRESENT | USER;
        if replaced {
            self.flush(virt);
        } else {
            self.pages += 1;
        }
        Ok(())
    }

    /// Reserva um quadro zerado e o mapeia em `virt`.
    pub fn map_new(&mut self, virt: u64, flags: u64) -> Result<u64, ()> {
        let phys = memory::alloc_zeroed_frame().ok_or(())?;
        match self.map(virt, phys, flags) {
            Ok(()) => Ok(phys),
            Err(()) => {
                memory::free_frame(phys);
                Err(())
            }
        }
    }

    /// (endereço físico, flags) da página que contém `virt`.
    pub fn translate(&mut self, virt: u64) -> Option<(u64, u64)> {
        let entry = *self.entry(virt, false)?;
        (entry & PRESENT != 0).then_some((entry & ADDR_MASK, entry & !ADDR_MASK))
    }

    pub fn set_flags(&mut self, virt: u64, flags: u64) {
        if let Some(entry) = self.entry(virt, false)
            && *entry & PRESENT != 0
        {
            *entry = (*entry & ADDR_MASK) | flags | PRESENT | USER;
            self.flush(virt);
        }
    }

    /// Desfaz o mapeamento de uma página e libera o quadro.
    pub fn unmap(&mut self, virt: u64) {
        if let Some(entry) = self.entry(virt, false)
            && *entry & PRESENT != 0
        {
            memory::free_frame(*entry & ADDR_MASK);
            *entry = 0;
            self.pages -= 1;
            self.flush(virt);
        }
    }

    /// Todas as páginas de usuário mapeadas: (endereço virtual, entrada P1).
    fn user_entries(&mut self) -> Vec<(u64, &'static mut u64)> {
        let mut entries = Vec::new();
        for (i4, &e4) in table(self.p4)[..256].iter().enumerate() {
            if e4 & PRESENT == 0 {
                continue;
            }
            for (i3, &e3) in table(e4 & ADDR_MASK).iter().enumerate() {
                if e3 & PRESENT == 0 {
                    continue;
                }
                for (i2, &e2) in table(e3 & ADDR_MASK).iter().enumerate() {
                    if e2 & PRESENT == 0 {
                        continue;
                    }
                    for (i1, e1) in table(e2 & ADDR_MASK).iter_mut().enumerate() {
                        if *e1 & PRESENT != 0 {
                            let virt = (i4 as u64) << 39 | (i3 as u64) << 30 | (i2 as u64) << 21 | (i1 as u64) << 12;
                            entries.push((virt, e1));
                        }
                    }
                }
            }
        }
        entries
    }

    /// Cria a cópia de um espaço para o `fork`. Nada é copiado agora: as
    /// páginas graváveis viram somente-leitura nos dois espaços, marcadas como
    /// cópia sob escrita, e quem escrever primeiro ganha sua cópia particular.
    pub fn fork(&mut self) -> Option<AddressSpace> {
        let mut child = AddressSpace::new()?;
        let mut ok = true;
        for (virt, entry) in self.user_entries() {
            if *entry & (WRITABLE | COPY_ON_WRITE) != 0 {
                *entry = (*entry & !WRITABLE) | COPY_ON_WRITE;
            }
            let phys = *entry & ADDR_MASK;
            memory::share_frame(phys);
            if child.map(virt, phys, *entry & !ADDR_MASK).is_err() {
                memory::free_frame(phys);
                ok = false;
                break;
            }
        }
        if self.is_active() {
            unsafe { cpu::write_cr3(self.p4) }; // descarta as permissões antigas do TLB
        }
        ok.then_some(child)
    }

    /// Trata uma escrita numa página de cópia sob escrita. Devolve `false` se
    /// a página não era desse tipo (o acesso é mesmo inválido).
    pub fn resolve_copy_on_write(&mut self, virt: u64) -> bool {
        let page = virt & !(FRAME_SIZE - 1);
        let Some(entry) = self.entry(page, false) else { return false };
        if *entry & PRESENT == 0 || *entry & COPY_ON_WRITE == 0 {
            return false;
        }
        let phys = *entry & ADDR_MASK;
        let flags = (*entry & !ADDR_MASK & !COPY_ON_WRITE) | WRITABLE;
        if memory::frame_refs(phys) > 1 {
            let Some(copy) = memory::alloc_zeroed_frame() else { return false };
            unsafe {
                core::ptr::copy_nonoverlapping(phys_to_virt::<u8>(phys), phys_to_virt::<u8>(copy), FRAME_SIZE as usize)
            };
            *entry = copy | flags;
            memory::free_frame(phys);
        } else {
            *entry = phys | flags; // o outro processo já saiu: a página é só nossa
        }
        self.flush(page);
        true
    }

    fn flush(&self, virt: u64) {
        if self.is_active() {
            cpu::invlpg(virt);
        }
    }

    /// Copia bytes para a memória deste espaço (que pode não estar ativo),
    /// pelo mapa direto. As páginas precisam estar mapeadas.
    pub fn write(&mut self, mut virt: u64, mut data: &[u8]) -> Result<(), ()> {
        while !data.is_empty() {
            let (phys, _) = self.translate(virt & !(FRAME_SIZE - 1)).ok_or(())?;
            let offset = virt % FRAME_SIZE;
            let count = data.len().min((FRAME_SIZE - offset) as usize);
            unsafe {
                core::ptr::copy_nonoverlapping(data.as_ptr(), phys_to_virt::<u8>(phys + offset), count);
            }
            data = &data[count..];
            virt += count as u64;
        }
        Ok(())
    }
}

impl Drop for AddressSpace {
    fn drop(&mut self) {
        fn free_level(phys: u64, level: u32) {
            for &entry in table(phys).iter() {
                if entry & PRESENT == 0 {
                    continue;
                }
                if level > 1 {
                    free_level(entry & ADDR_MASK, level - 1);
                } else {
                    memory::free_frame(entry & ADDR_MASK);
                }
            }
            memory::free_frame(phys);
        }
        assert!(!self.is_active(), "liberando o espaço de endereçamento ativo");
        for &entry in &table(self.p4)[..256] {
            if entry & PRESENT != 0 {
                free_level(entry & ADDR_MASK, 3);
            }
        }
        memory::free_frame(self.p4);
    }
}
