//! Leitura da estrutura de informação Multiboot (v1) entregue pelo carregador.
//! Tudo é copiado para cá, porque a memória original será reaproveitada.

use crate::paging::phys_to_virt;

const MAX_REGIONS: usize = 32;

#[derive(Clone, Copy, Default)]
pub struct MemoryRegion {
    pub base: u64,
    pub length: u64,
    /// 1 = utilizável; os demais valores são reservados.
    pub kind: u32,
}

pub struct BootInfo {
    regions: [MemoryRegion; MAX_REGIONS],
    region_count: usize,
    loader_name: [u8; 64],
    loader_name_len: usize,
    cmdline: [u8; 128],
    cmdline_len: usize,
}

/// Lê da memória física, pelo mapa direto.
unsafe fn read<T: Copy>(addr: u64) -> T {
    unsafe { phys_to_virt::<T>(addr).read_unaligned() }
}

unsafe fn copy_cstr(addr: u64, out: &mut [u8]) -> usize {
    let mut len = 0;
    while len < out.len() {
        let byte = unsafe { read::<u8>(addr + len as u64) };
        if byte == 0 {
            break;
        }
        out[len] = byte;
        len += 1;
    }
    len
}

impl BootInfo {
    /// # Safety
    /// `addr` precisa apontar para uma estrutura Multiboot válida.
    pub unsafe fn parse(addr: u32) -> Self {
        let addr = addr as u64;
        let mut info = Self {
            regions: [MemoryRegion::default(); MAX_REGIONS],
            region_count: 0,
            loader_name: [0; 64],
            loader_name_len: 0,
            cmdline: [0; 128],
            cmdline_len: 0,
        };
        unsafe {
            let flags: u32 = read(addr);
            if flags & (1 << 2) != 0 {
                let ptr: u32 = read(addr + 16);
                info.cmdline_len = copy_cstr(ptr as u64, &mut info.cmdline);
            }
            if flags & (1 << 6) != 0 {
                let length: u32 = read(addr + 44);
                let mut entry = read::<u32>(addr + 48) as u64;
                let end = entry + length as u64;
                while entry < end && info.region_count < MAX_REGIONS {
                    let size: u32 = read(entry);
                    info.regions[info.region_count] = MemoryRegion {
                        base: read(entry + 4),
                        length: read(entry + 12),
                        kind: read(entry + 20),
                    };
                    info.region_count += 1;
                    entry += size as u64 + 4;
                }
            } else if flags & 1 != 0 {
                // Sem mapa de memória: usa só a memória "alta" a partir de 1 MiB.
                let upper_kib: u32 = read(addr + 8);
                info.regions[0] = MemoryRegion { base: 0x100000, length: upper_kib as u64 * 1024, kind: 1 };
                info.region_count = 1;
            }
            if flags & (1 << 9) != 0 {
                let ptr: u32 = read(addr + 64);
                info.loader_name_len = copy_cstr(ptr as u64, &mut info.loader_name);
            }
        }
        info
    }

    pub fn regions(&self) -> &[MemoryRegion] {
        &self.regions[..self.region_count]
    }

    pub fn loader_name(&self) -> &str {
        core::str::from_utf8(&self.loader_name[..self.loader_name_len]).unwrap_or("?")
    }

    pub fn cmdline(&self) -> &str {
        core::str::from_utf8(&self.cmdline[..self.cmdline_len]).unwrap_or("")
    }
}
