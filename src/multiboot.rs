//! Leitura da estrutura de informações que o bootloader Multiboot nos entrega.
//!
//! O ponteiro chega em EBX no boot (e vira o 2º argumento de `kmain`).
//! O campo `flags` diz quais campos são válidos. Referência:
//! https://www.gnu.org/software/grub/manual/multiboot/multiboot.html

use core::ptr::read_unaligned;

pub const MAGICO: u32 = 0x2BADB002;

#[derive(Clone, Copy)]
pub struct Info {
    base: usize,
}

/// Uma região de memória física, como informada pelo firmware (BIOS e820).
pub struct RegiaoMemoria {
    pub inicio: u64,
    pub tamanho: u64,
    pub tipo: u32,
}

impl Info {
    /// # Safety
    /// `endereco` precisa ser o ponteiro válido recebido do bootloader,
    /// e a memória precisa estar mapeada (estamos com identidade em 1 GiB).
    pub unsafe fn de_endereco(endereco: u32) -> Self {
        Self { base: endereco as usize }
    }

    fn u32_em(&self, deslocamento: usize) -> u32 {
        unsafe { read_unaligned((self.base + deslocamento) as *const u32) }
    }

    fn flags(&self) -> u32 {
        self.u32_em(0)
    }

    pub fn nome_bootloader(&self) -> Option<&'static str> {
        if self.flags() & (1 << 9) == 0 {
            return None;
        }
        let ptr = self.u32_em(64) as usize as *const u8;
        // String no estilo C: vai até o primeiro byte zero.
        let mut tamanho = 0;
        unsafe {
            while *ptr.add(tamanho) != 0 && tamanho < 256 {
                tamanho += 1;
            }
            core::str::from_utf8(core::slice::from_raw_parts(ptr, tamanho)).ok()
        }
    }

    /// Itera sobre o mapa de memória. Esse mapa é a base para, no próximo passo,
    /// escrever um alocador de páginas físicas.
    pub fn mapa_memoria(&self) -> Option<IterMapa> {
        if self.flags() & (1 << 6) == 0 {
            return None;
        }
        let tamanho = self.u32_em(44) as usize;
        let inicio = self.u32_em(48) as usize;
        Some(IterMapa { atual: inicio, fim: inicio + tamanho })
    }
}

/// Um arquivo que o bootloader carregou junto com o kernel (o nosso initrd).
pub struct Modulo {
    pub inicio: u64,
    pub fim: u64,
}

impl Info {
    pub fn modulos(&self) -> impl Iterator<Item = Modulo> + '_ {
        let quantidade = if self.flags() & (1 << 3) != 0 { self.u32_em(20) as usize } else { 0 };
        let tabela = self.u32_em(24) as usize;
        (0..quantidade).map(move |i| unsafe {
            Modulo {
                inicio: read_unaligned((tabela + i * 16) as *const u32) as u64,
                fim: read_unaligned((tabela + i * 16 + 4) as *const u32) as u64,
            }
        })
    }

    /// Chama `f(início, fim)` para cada área de memória que o bootloader usou
    /// e que o alocador de quadros não pode entregar para ninguém.
    pub fn reservas(&self, mut f: impl FnMut(u64, u64)) {
        let base = self.base as u64;
        f(base, base + 120);
        if self.flags() & (1 << 6) != 0 {
            let mapa = self.u32_em(48) as u64;
            f(mapa, mapa + self.u32_em(44) as u64);
        }
        if self.flags() & (1 << 3) != 0 {
            let tabela = self.u32_em(24) as u64;
            f(tabela, tabela + self.u32_em(20) as u64 * 16);
            for m in self.modulos() {
                f(m.inicio, m.fim);
            }
        }
    }
}

pub struct IterMapa {
    atual: usize,
    fim: usize,
}

impl Iterator for IterMapa {
    type Item = RegiaoMemoria;

    fn next(&mut self) -> Option<RegiaoMemoria> {
        if self.atual >= self.fim {
            return None;
        }
        // Formato de cada entrada: size(u32) base(u64) len(u64) type(u32)
        // `size` não conta a si mesmo, por isso o "+ 4" no avanço.
        let p = self.atual;
        let entrada = unsafe {
            let size = read_unaligned(p as *const u32) as usize;
            let regiao = RegiaoMemoria {
                inicio: read_unaligned((p + 4) as *const u64),
                tamanho: read_unaligned((p + 12) as *const u64),
                tipo: read_unaligned((p + 20) as *const u32),
            };
            self.atual += size + 4;
            regiao
        };
        Some(entrada)
    }
}
