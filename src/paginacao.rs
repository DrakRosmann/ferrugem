//! Memória virtual: um espaço de endereçamento separado para cada processo.
//!
//! O x86_64 traduz endereços com 4 níveis de tabelas (PML4 → PDPT → PD → PT),
//! cada uma com 512 entradas de 8 bytes. Um endereço virtual se divide assim:
//!
//! ```text
//!   | 47..39 PML4 | 38..30 PDPT | 29..21 PD | 20..12 PT | 11..0 deslocamento |
//! ```
//!
//! Mapa de cada processo:
//!
//! ```text
//!   0       .. 1 GiB     kernel (o mesmo em todos, em identidade, só anel 0)
//!   1 GiB   .. ...       programa (ELF) e, logo depois, o heap (brk)
//!   ...     .. 508 GiB   pilha do usuário, crescendo para baixo
//! ```
//!
//! Todos os processos apontam para a *mesma* tabela PD do kernel (o 1 GiB de
//! páginas de 2 MiB que o boot.asm montou). Ela não tem o bit "usuário": se um
//! programa tentar ler o kernel, a CPU gera page fault.

use crate::quadros::{self, TAMANHO_PAGINA};
use alloc::string::String;
use alloc::vec::Vec;
use core::arch::asm;

pub const PRESENTE: u64 = 1 << 0;
pub const ESCRITA: u64 = 1 << 1;
pub const USUARIO: u64 = 1 << 2;
/// Bit livre para o sistema operacional (a CPU ignora): "página compartilhada
/// depois de um fork; copie antes de deixar escrever".
const COPIA_NA_ESCRITA: u64 = 1 << 9;
const ENDERECO: u64 = 0x000F_FFFF_FFFF_F000;

pub const USUARIO_INICIO: u64 = 0x4000_0000; // 1 GiB
pub const USUARIO_FIM: u64 = 0x80_0000_0000; // 512 GiB: tudo cabe na entrada 0 da PML4

static mut PML4_KERNEL: u64 = 0;
static mut PD_KERNEL: u64 = 0;

/// Guarda as tabelas que o boot.asm montou: elas viram o "kernel" de todo processo.
pub fn init() {
    unsafe {
        let pml4 = ler_cr3() & ENDERECO;
        let pdpt = tabela(pml4)[0] & ENDERECO;
        PML4_KERNEL = pml4;
        PD_KERNEL = tabela(pdpt)[0] & ENDERECO;
    }
}

/// Graças ao mapeamento em identidade, o endereço físico de uma tabela também
/// é um endereço virtual válido para o kernel.
fn tabela(fisico: u64) -> &'static mut [u64; 512] {
    unsafe { &mut *(fisico as *mut [u64; 512]) }
}

fn ler_cr3() -> u64 {
    let valor: u64;
    unsafe { asm!("mov {}, cr3", out(reg) valor, options(nomem, nostack, preserves_flags)) };
    valor
}

/// Trocar o CR3 troca o espaço de endereçamento inteiro (e esvazia a TLB).
fn escrever_cr3(valor: u64) {
    unsafe { asm!("mov cr3, {}", in(reg) valor, options(nostack, preserves_flags)) };
}

pub fn ativar_kernel() {
    escrever_cr3(unsafe { PML4_KERNEL });
}

/// Tira uma página da TLB (o cache de traduções da CPU).
fn invalidar(virt: u64) {
    unsafe { asm!("invlpg [{}]", in(reg) virt, options(nostack, preserves_flags)) };
}

pub struct EspacoEnderecos {
    pml4: u64,
}

impl EspacoEnderecos {
    pub fn novo() -> Option<Self> {
        let pml4 = quadros::alocar_zerado()?;
        let Some(pdpt) = quadros::alocar_zerado() else {
            quadros::liberar(pml4);
            return None;
        };
        tabela(pdpt)[0] = unsafe { PD_KERNEL } | PRESENTE | ESCRITA;
        tabela(pml4)[0] = pdpt | PRESENTE | ESCRITA | USUARIO;
        Some(Self { pml4 })
    }

    pub fn ativar(&self) {
        escrever_cr3(self.pml4);
    }

    /// A entrada da tabela de último nível (PT) que traduz `virt`.
    /// Com `criar`, aloca as tabelas intermediárias que faltarem.
    fn entrada(&self, virt: u64, criar: bool) -> Option<&'static mut u64> {
        if !(USUARIO_INICIO..USUARIO_FIM).contains(&virt) {
            return None;
        }
        let mut atual = self.pml4;
        for deslocamento in [39, 30, 21] {
            let e = &mut tabela(atual)[((virt >> deslocamento) & 511) as usize];
            if *e & PRESENTE == 0 {
                if !criar {
                    return None;
                }
                *e = quadros::alocar_zerado()? | PRESENTE | ESCRITA | USUARIO;
            }
            atual = *e & ENDERECO;
        }
        Some(&mut tabela(atual)[((virt >> 12) & 511) as usize])
    }

    pub fn mapear(&mut self, virt: u64, fisico: u64, flags: u64) -> bool {
        match self.entrada(virt, true) {
            Some(e) => {
                *e = (fisico & ENDERECO) | flags | PRESENTE;
                true
            }
            None => false,
        }
    }

    /// Garante páginas zeradas em `[inicio, fim)`. Páginas já mapeadas só ganham `flags`.
    pub fn alocar_regiao(&mut self, inicio: u64, fim: u64, flags: u64) -> bool {
        let mut virt = inicio & !(TAMANHO_PAGINA - 1);
        while virt < fim {
            let Some(e) = self.entrada(virt, true) else { return false };
            if *e & PRESENTE != 0 {
                *e |= flags;
            } else {
                let Some(quadro) = quadros::alocar_zerado() else { return false };
                *e = quadro | flags | PRESENTE;
            }
            virt += TAMANHO_PAGINA;
        }
        true
    }

    /// Devolve as páginas de `[inicio, fim)` (o `munmap`). As que não estavam mapeadas são ignoradas.
    pub fn desalocar_regiao(&mut self, inicio: u64, fim: u64) {
        let mut virt = inicio & !(TAMANHO_PAGINA - 1);
        while virt < fim {
            if let Some(e) = self.entrada(virt, false) {
                if *e & PRESENTE != 0 {
                    quadros::liberar(*e & ENDERECO);
                    *e = 0;
                    invalidar(virt);
                }
            }
            virt += TAMANHO_PAGINA;
        }
    }

    /// Endereço físico por trás de `virt`, se for uma página de usuário
    /// (e gravável, se `escrita`).
    fn fisico(&self, virt: u64, escrita: bool) -> Option<u64> {
        let entrada = self.entrada(virt, false)?;
        // O kernel escreve na memória do programa pelo endereço físico (sem
        // passar pela MMU), então ele mesmo precisa respeitar o copy-on-write.
        if escrita && *entrada & COPIA_NA_ESCRITA != 0 && !self.copiar_pagina(entrada, virt) {
            return None;
        }
        let e = *entrada;
        let exigido = PRESENTE | USUARIO | if escrita { ESCRITA } else { 0 };
        if e & exigido != exigido {
            return None;
        }
        Some((e & ENDERECO) + (virt & (TAMANHO_PAGINA - 1)))
    }

    /// Copia de/para a memória do processo página por página, pelo endereço
    /// físico. Funciona com qualquer CR3 ativo e nunca causa page fault no
    /// kernel: um ponteiro inválido de um programa vira só `false` (EFAULT).
    fn copiar(&self, mut virt: u64, tamanho: usize, escrita: bool, mut f: impl FnMut(*mut u8, usize, usize)) -> bool {
        let mut feito = 0;
        while feito < tamanho {
            let Some(fisico) = self.fisico(virt, escrita) else { return false };
            let n = ((TAMANHO_PAGINA - (virt & (TAMANHO_PAGINA - 1))) as usize).min(tamanho - feito);
            f(fisico as *mut u8, feito, n);
            feito += n;
            virt += n as u64;
        }
        true
    }

    /// Escreve na memória do processo (para chamadas como `read`). Exige página gravável.
    pub fn copiar_para(&self, virt: u64, dados: &[u8]) -> bool {
        self.copiar(virt, dados.len(), true, |destino, i, n| unsafe {
            core::ptr::copy_nonoverlapping(dados[i..].as_ptr(), destino, n)
        })
    }

    /// Como `copiar_para`, mas também em páginas só de leitura (para carregar o programa).
    pub fn gravar(&self, virt: u64, dados: &[u8]) -> bool {
        self.copiar(virt, dados.len(), false, |destino, i, n| unsafe {
            core::ptr::copy_nonoverlapping(dados[i..].as_ptr(), destino, n)
        })
    }

    pub fn copiar_de(&self, virt: u64, tamanho: usize) -> Option<Vec<u8>> {
        let mut dados = alloc::vec![0u8; tamanho];
        let ok = self.copiar(virt, tamanho, false, |origem, i, n| unsafe {
            core::ptr::copy_nonoverlapping(origem, dados[i..].as_mut_ptr(), n)
        });
        ok.then_some(dados)
    }

    /// Lê uma string terminada em zero (estilo C) da memória do processo.
    pub fn ler_string(&self, mut virt: u64, maximo: usize) -> Option<String> {
        let mut bytes = Vec::new();
        loop {
            let fisico = self.fisico(virt, false)?;
            let byte = unsafe { *(fisico as *const u8) };
            if byte == 0 {
                return String::from_utf8(bytes).ok();
            }
            if bytes.len() >= maximo {
                return None;
            }
            bytes.push(byte);
            virt += 1;
        }
    }

    /// Chama `f(endereço virtual, entrada)` para cada página de usuário mapeada.
    fn para_cada_pagina(&self, mut f: impl FnMut(u64, &mut u64)) {
        let pdpt = tabela(self.pml4)[0] & ENDERECO;
        // A entrada 0 da PDPT é o kernel: pulamos.
        for i3 in 1..512 {
            let e3 = tabela(pdpt)[i3];
            if e3 & PRESENTE == 0 {
                continue;
            }
            for i2 in 0..512 {
                let e2 = tabela(e3 & ENDERECO)[i2];
                if e2 & PRESENTE == 0 {
                    continue;
                }
                let pt = tabela(e2 & ENDERECO);
                for (i1, e) in pt.iter_mut().enumerate() {
                    if *e & PRESENTE != 0 {
                        f(((i3 as u64) << 30) | ((i2 as u64) << 21) | ((i1 as u64) << 12), e);
                    }
                }
            }
        }
    }

    /// O `fork`: o filho recebe as *mesmas* páginas do pai. As graváveis
    /// viram só leitura nos dois, marcadas como copy-on-write; a primeira
    /// escrita de qualquer um deles causa um page fault, e só então a página
    /// é copiada (`tratar_escrita`). Um `fork` seguido de `exec` — o caso
    /// mais comum — quase não copia nada.
    pub fn clonar(&self) -> Option<Self> {
        let mut novo = Self::novo()?;
        let mut ok = true;
        self.para_cada_pagina(|virt, e| {
            if !ok {
                return;
            }
            if *e & ESCRITA != 0 {
                *e = (*e & !ESCRITA) | COPIA_NA_ESCRITA;
            }
            let quadro = *e & ENDERECO;
            quadros::compartilhar(quadro);
            ok = novo.mapear(virt, quadro, *e & 0xFFF);
            if !ok {
                quadros::liberar(quadro);
            }
        });
        // As páginas do pai acabaram de perder a permissão de escrita: a CPU
        // não pode continuar usando as traduções antigas que estão na TLB.
        escrever_cr3(ler_cr3());
        ok.then_some(novo) // se falhou, `novo` é destruído e devolve o que pegou
    }

    /// Dá a uma página copy-on-write uma cópia só dela (ou, se ninguém mais
    /// a usa, só devolve a permissão de escrita).
    fn copiar_pagina(&self, entrada: &mut u64, virt: u64) -> bool {
        let quadro = *entrada & ENDERECO;
        let flags = (*entrada & 0xFFF & !COPIA_NA_ESCRITA) | ESCRITA;
        if quadros::donos(quadro) == 1 {
            *entrada = quadro | flags;
        } else {
            let Some(novo) = quadros::alocar() else { return false };
            unsafe {
                core::ptr::copy_nonoverlapping(quadro as *const u8, novo as *mut u8, TAMANHO_PAGINA as usize)
            };
            *entrada = novo | flags;
            quadros::liberar(quadro);
        }
        invalidar(virt);
        true
    }

    /// Chamada no page fault de escrita: se era copy-on-write, resolve e
    /// devolve `true` (a instrução roda de novo e agora funciona).
    pub fn tratar_escrita(&self, virt: u64) -> bool {
        match self.entrada(virt, false) {
            Some(e) if *e & PRESENTE != 0 && *e & COPIA_NA_ESCRITA != 0 => self.copiar_pagina(e, virt & !0xFFF),
            _ => false,
        }
    }

    /// Quantas páginas de usuário estão mapeadas.
    pub fn paginas(&self) -> usize {
        let mut n = 0;
        self.para_cada_pagina(|_, _| n += 1);
        n
    }
}

/// Devolve todos os quadros do processo: páginas e tabelas (menos a PD do kernel).
/// O espaço não pode estar ativo no CR3 quando isso acontece.
impl Drop for EspacoEnderecos {
    fn drop(&mut self) {
        let pdpt = tabela(self.pml4)[0] & ENDERECO;
        for i3 in 1..512 {
            let e3 = tabela(pdpt)[i3];
            if e3 & PRESENTE == 0 {
                continue;
            }
            let pd = e3 & ENDERECO;
            for i2 in 0..512 {
                let e2 = tabela(pd)[i2];
                if e2 & PRESENTE == 0 {
                    continue;
                }
                let pt = e2 & ENDERECO;
                for &e in tabela(pt).iter() {
                    if e & PRESENTE != 0 {
                        quadros::liberar(e & ENDERECO);
                    }
                }
                quadros::liberar(pt);
            }
            quadros::liberar(pd);
        }
        quadros::liberar(pdpt);
        quadros::liberar(self.pml4);
    }
}
