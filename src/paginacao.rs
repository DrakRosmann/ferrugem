//! Memória virtual: um espaço de endereçamento separado para cada processo.
//!
//! O x86_64 traduz endereços com 4 níveis de tabelas (PML4 → PDPT → PD → PT),
//! cada uma com 512 entradas de 8 bytes. Um endereço virtual se divide assim:
//!
//! ```text
//!   | 47..39 PML4 | 38..30 PDPT | 29..21 PD | 20..12 PT | 11..0 deslocamento |
//! ```
//!
//! Mapa de cada processo (as posições marcadas com * mudam a cada execução: ASLR):
//!
//! ```text
//!   0       .. 1 GiB     kernel (o mesmo em todos, em identidade, só anel 0)
//!   1 GiB   .. ...       programa (ELF) e, logo depois*, o heap (brk) crescendo para cima
//!   ...*    .. ...       mmap, crescendo para baixo
//!   ...*    .. ~508 GiB  pilha (até 8 MiB), crescendo para baixo
//! ```
//!
//! ## Regiões
//!
//! O processo é descrito por uma lista de *regiões* (as "VMAs" do Linux):
//! faixas de endereços com permissões (ler/escrever/executar) e uma origem
//! (memória zerada ou um pedaço de arquivo). As páginas de uma região só
//! ganham um quadro de verdade quando são tocadas pela primeira vez: o page
//! fault cai aqui (`tratar_falha`), a região diz o que pôr na página, e a
//! instrução roda de novo. É a *paginação sob demanda*: uma pilha de 8 MiB
//! que só usa 12 KiB custa 12 KiB.
//!
//! Todos os processos apontam para a *mesma* tabela PD do kernel (o 1 GiB de
//! páginas de 2 MiB que o boot.asm montou). Ela não tem o bit "usuário": se um
//! programa tentar ler o kernel, a CPU gera page fault.

use crate::quadros::{self, TAMANHO_PAGINA};
use crate::spin::Mutex;
use crate::vfs::Vnode;
use alloc::string::String;
use alloc::sync::Arc;
use alloc::vec;
use alloc::vec::Vec;
use core::arch::asm;

pub const PRESENTE: u64 = 1 << 0;
pub const ESCRITA: u64 = 1 << 1;
pub const USUARIO: u64 = 1 << 2;
/// A CPU marca sozinha quando a página é escrita (usado no `msync`).
const SUJA: u64 = 1 << 6;
/// Bit livre para o sistema operacional (a CPU ignora): "página compartilhada
/// depois de um fork; copie antes de deixar escrever".
const COPIA_NA_ESCRITA: u64 = 1 << 9;
/// NX: a CPU se recusa a executar código desta página (precisa do EFER.NXE).
pub const NAO_EXECUTAR: u64 = 1 << 63;
const ENDERECO: u64 = 0x000F_FFFF_FFFF_F000;

pub const USUARIO_INICIO: u64 = 0x4000_0000; // 1 GiB
pub const USUARIO_FIM: u64 = 0x80_0000_0000; // 512 GiB: tudo cabe na entrada 0 da PML4

/// Permissões de uma região, com os mesmos bits do `mmap` do Linux.
pub const PROT_LER: u8 = 1;
pub const PROT_ESCREVER: u8 = 2;
pub const PROT_EXECUTAR: u8 = 4;

static mut PML4_KERNEL: u64 = 0;
static mut PD_KERNEL: u64 = 0;

/// Guarda as tabelas que o boot.asm montou (elas viram o "kernel" de todo
/// processo) e liga as proteções que a CPU oferecer.
pub fn init() -> &'static str {
    unsafe {
        let pml4 = ler_cr3() & ENDERECO;
        let pdpt = tabela(pml4)[0] & ENDERECO;
        PML4_KERNEL = pml4;
        PD_KERNEL = tabela(pdpt)[0] & ENDERECO;
    }
    ligar_protecoes()
}

/// NX (páginas não executáveis), SMEP (o kernel não executa código do
/// usuário) e SMAP (o kernel não lê memória do usuário por engano: aqui ele
/// sempre passa pelo endereço físico, então pode ligar sem medo).
fn ligar_protecoes() -> &'static str {
    use crate::syscall::{escrever_msr, ler_msr};
    use core::arch::x86_64::__cpuid_count;
    const EFER: u32 = 0xC000_0080;
    escrever_msr(EFER, ler_msr(EFER) | (1 << 11)); // NXE
    #[allow(unused_unsafe)]
    let ebx = unsafe { __cpuid_count(7, 0) }.ebx;
    let (smep, smap) = (ebx & (1 << 7) != 0, ebx & (1 << 20) != 0);
    unsafe {
        let mut cr4: u64;
        asm!("mov {}, cr4", out(reg) cr4, options(nomem, nostack, preserves_flags));
        if smep {
            cr4 |= 1 << 20;
        }
        if smap {
            cr4 |= 1 << 21;
        }
        asm!("mov cr4, {}", in(reg) cr4, options(nostack, preserves_flags));
    }
    match (smep, smap) {
        (true, true) => "NX, SMEP e SMAP",
        (true, false) => "NX e SMEP",
        _ => "NX",
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

fn arredondar_cima(v: u64) -> u64 {
    (v + TAMANHO_PAGINA - 1) & !(TAMANHO_PAGINA - 1)
}

/// De onde vem o conteúdo das páginas de uma região.
#[derive(Clone)]
pub enum Fonte {
    /// Páginas zeradas.
    Anonima,
    /// Um arquivo, a partir de `deslocamento` (só até `tamanho` bytes; o resto é zero).
    Arquivo { no: Vnode, deslocamento: u64, tamanho: u64 },
    /// Memória compartilhada (MAP_SHARED anônimo, shmget): as páginas são as
    /// do segmento, as mesmas para todos que o mapeiam. `pagina`: a primeira.
    Segmento { segmento: Arc<Segmento>, pagina: usize },
    /// Memória física de um dispositivo (o framebuffer), a partir de `base`.
    Fisica { base: u64 },
}

/// Páginas que vários espaços de endereçamento enxergam ao mesmo tempo.
/// Cada quadro é alocado no primeiro acesso (de qualquer um) e fica aqui até
/// o segmento morrer; cada mapeamento conta como mais um dono do quadro.
pub struct Segmento {
    quadros: Mutex<Vec<u64>>, // 0 = ainda não foi tocada
}

impl Segmento {
    pub fn novo(tamanho: u64) -> Arc<Segmento> {
        let paginas = tamanho.div_ceil(TAMANHO_PAGINA) as usize;
        Arc::new(Segmento { quadros: Mutex::new(vec![0; paginas]) })
    }

    pub fn tamanho(&self) -> u64 {
        self.quadros.lock().len() as u64 * TAMANHO_PAGINA
    }

    /// O quadro da página `i` (alocado agora, se preciso), já com mais um dono.
    fn quadro(&self, i: usize) -> Option<u64> {
        let mut q = self.quadros.lock();
        let lugar = q.get_mut(i)?;
        if *lugar == 0 {
            *lugar = quadros::alocar_zerado()?;
        }
        quadros::compartilhar(*lugar);
        Some(*lugar)
    }
}

impl Drop for Segmento {
    fn drop(&mut self) {
        for &q in self.quadros.lock().iter().filter(|&&q| q != 0) {
            quadros::liberar(q);
        }
    }
}

#[derive(Clone)]
pub struct Regiao {
    pub inicio: u64,
    pub fim: u64,
    pub prot: u8,
    /// MAP_SHARED: as páginas são as mesmas depois de um fork, e as de
    /// arquivo voltam para o arquivo (no `msync`, no `munmap` e ao sair).
    pub compartilhada: bool,
    pub fonte: Fonte,
}

impl Regiao {
    fn flags(&self) -> u64 {
        let mut f = USUARIO | PRESENTE;
        if self.prot & PROT_ESCREVER != 0 {
            f |= ESCRITA;
        }
        if self.prot & PROT_EXECUTAR == 0 {
            f |= NAO_EXECUTAR;
        }
        f
    }

    /// O pedaço de `self` que sobra fora de `[inicio, fim)` (0, 1 ou 2 regiões).
    fn sem(&self, inicio: u64, fim: u64) -> Vec<Regiao> {
        let mut partes = Vec::new();
        if self.inicio < inicio {
            let mut r = self.clone();
            r.fim = inicio.min(self.fim);
            partes.push(r);
        }
        if self.fim > fim {
            let mut r = self.clone();
            r.inicio = fim.max(self.inicio);
            r.avancar_fonte(r.inicio - self.inicio);
            partes.push(r);
        }
        partes
    }

    fn avancar_fonte(&mut self, bytes: u64) {
        match &mut self.fonte {
            Fonte::Arquivo { deslocamento, tamanho, .. } => {
                *deslocamento += bytes;
                *tamanho = tamanho.saturating_sub(bytes);
            }
            Fonte::Segmento { pagina, .. } => *pagina += (bytes / TAMANHO_PAGINA) as usize,
            Fonte::Fisica { base } => *base += bytes,
            Fonte::Anonima => {}
        }
    }
}

/// O que aconteceu com um page fault.
#[derive(PartialEq, Eq)]
pub enum Falha {
    Resolvida,
    /// Acesso inválido: o programa leva SIGSEGV.
    Invalida,
    /// Faltou memória física para atender.
    SemMemoria,
}

pub struct EspacoEnderecos {
    pml4: u64,
    regioes: Vec<Regiao>,
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
        Some(Self { pml4, regioes: Vec::new() })
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

    // ------------------------------------------------------------- regiões

    pub fn regiao_de(&self, virt: u64) -> Option<&Regiao> {
        self.regioes.iter().find(|r| (r.inicio..r.fim).contains(&virt))
    }

    /// Acrescenta uma região (sem alocar nada: as páginas vêm sob demanda).
    /// O que já houver nessa faixa é desfeito antes, como no MAP_FIXED.
    pub fn criar_regiao(&mut self, regiao: Regiao) {
        self.remover_faixa(regiao.inicio, regiao.fim);
        self.regioes.push(regiao);
    }

    /// Só anota a região (as páginas já foram preparadas por quem chama: o
    /// carregador de ELF, que pode ter dois segmentos na mesma página).
    pub fn registrar_regiao(&mut self, regiao: Regiao) {
        self.regioes.push(regiao);
    }

    /// Cria a região e já aloca todas as páginas (usado para o MAP_SHARED,
    /// que precisa das páginas antes de um fork para compartilhá-las).
    pub fn criar_regiao_ja_alocada(&mut self, regiao: Regiao) -> bool {
        let (inicio, fim) = (regiao.inicio, regiao.fim);
        self.criar_regiao(regiao);
        let mut v = inicio;
        while v < fim {
            if self.materializar(v) != Falha::Resolvida {
                return false;
            }
            v += TAMANHO_PAGINA;
        }
        true
    }

    /// Estica (ou encolhe) a região que termina em `fim_antigo` (o heap do `brk`).
    pub fn redimensionar_regiao(&mut self, inicio: u64, fim_antigo: u64, fim_novo: u64) {
        let (fim_antigo, fim_novo) = (arredondar_cima(fim_antigo), arredondar_cima(fim_novo));
        if fim_novo < fim_antigo {
            self.remover_faixa(fim_novo, fim_antigo);
            return;
        }
        match self.regioes.iter_mut().find(|r| r.inicio == inicio && r.fim == fim_antigo) {
            Some(r) => r.fim = fim_novo,
            None if fim_novo > inicio => self.regioes.push(Regiao {
                inicio,
                fim: fim_novo,
                prot: PROT_LER | PROT_ESCREVER,
                compartilhada: false,
                fonte: Fonte::Anonima,
            }),
            None => {}
        }
    }

    /// Desfaz `[inicio, fim)`: devolve as páginas (gravando de volta as de
    /// arquivos compartilhados) e corta as regiões (o `munmap`).
    pub fn remover_faixa(&mut self, inicio: u64, fim: u64) {
        self.sincronizar(inicio, fim);
        let mut v = inicio;
        while v < fim {
            if let Some(e) = self.entrada(v, false) {
                if *e & PRESENTE != 0 {
                    quadros::liberar(*e & ENDERECO);
                    *e = 0;
                    invalidar(v);
                }
            }
            v += TAMANHO_PAGINA;
        }
        self.liberar_tabelas_vazias(inicio, fim);
        let antigas = core::mem::take(&mut self.regioes);
        for r in antigas {
            if r.fim <= inicio || r.inicio >= fim {
                self.regioes.push(r);
            } else {
                self.regioes.extend(r.sem(inicio, fim));
            }
        }
    }

    /// Depois de um `munmap`, as tabelas de páginas (PT e PD) que ficaram sem
    /// nenhuma entrada também voltam para o alocador de quadros.
    fn liberar_tabelas_vazias(&mut self, inicio: u64, fim: u64) {
        let pdpt = tabela(self.pml4)[0] & ENDERECO;
        let gib = 1u64 << 30;
        let mut liberou = false;
        let mut base = inicio & !(gib - 1);
        while base < fim {
            let i3 = ((base >> 30) & 511) as usize;
            let e3 = tabela(pdpt)[i3];
            if i3 != 0 && e3 & PRESENTE != 0 {
                let pd = e3 & ENDERECO;
                for i2 in 0..512 {
                    let (de, ate) = (base + ((i2 as u64) << 21), base + ((i2 as u64 + 1) << 21));
                    let e2 = tabela(pd)[i2];
                    if ate <= inicio || de >= fim || e2 & PRESENTE == 0 {
                        continue;
                    }
                    if tabela(e2 & ENDERECO).iter().all(|&e| e == 0) {
                        quadros::liberar(e2 & ENDERECO);
                        tabela(pd)[i2] = 0;
                        liberou = true;
                    }
                }
                if tabela(pd).iter().all(|&e| e == 0) {
                    quadros::liberar(pd);
                    tabela(pdpt)[i3] = 0;
                }
            }
            base += gib;
        }
        // A CPU também guarda em cache as próprias tabelas: se este espaço está ativo, recarrega.
        if liberou && ler_cr3() & ENDERECO == self.pml4 {
            escrever_cr3(self.pml4);
        }
    }

    /// `mprotect`: muda as permissões de `[inicio, fim)`, nas regiões e nas páginas.
    pub fn proteger(&mut self, inicio: u64, fim: u64, prot: u8) -> bool {
        // Toda a faixa precisa estar coberta por regiões.
        let mut v = inicio;
        while v < fim {
            if self.regiao_de(v).is_none() {
                return false;
            }
            v += TAMANHO_PAGINA;
        }
        let antigas = core::mem::take(&mut self.regioes);
        for r in antigas {
            if r.fim <= inicio || r.inicio >= fim {
                self.regioes.push(r);
                continue;
            }
            let mut meio = r.clone();
            meio.inicio = r.inicio.max(inicio);
            meio.fim = r.fim.min(fim);
            meio.avancar_fonte(meio.inicio - r.inicio);
            meio.prot = prot;
            self.regioes.extend(r.sem(inicio, fim));
            self.regioes.push(meio);
        }
        let mut v = inicio;
        while v < fim {
            if let Some(e) = self.entrada(v, false) {
                if *e & PRESENTE != 0 {
                    // Uma página copy-on-write continua sem escrita: o page
                    // fault é que vai copiá-la (agora que a região permite).
                    let escrita = prot & PROT_ESCREVER != 0 && *e & COPIA_NA_ESCRITA == 0;
                    *e = if escrita { *e | ESCRITA } else { *e & !ESCRITA };
                    *e = if prot & PROT_EXECUTAR != 0 { *e & !NAO_EXECUTAR } else { *e | NAO_EXECUTAR };
                    invalidar(v);
                }
            }
            v += TAMANHO_PAGINA;
        }
        true
    }

    /// `msync`: grava de volta no arquivo as páginas sujas de regiões
    /// compartilhadas de arquivo em `[inicio, fim)`.
    pub fn sincronizar(&self, inicio: u64, fim: u64) {
        for r in self.regioes.iter().filter(|r| r.compartilhada && r.fim > inicio && r.inicio < fim) {
            let Fonte::Arquivo { no, deslocamento, tamanho } = &r.fonte else { continue };
            let mut v = r.inicio.max(inicio);
            while v < r.fim.min(fim) {
                let dentro = v - r.inicio;
                if let Some(e) = self.entrada(v, false) {
                    if *e & PRESENTE != 0 && *e & SUJA != 0 && dentro < *tamanho {
                        let n = (*tamanho - dentro).min(TAMANHO_PAGINA) as usize;
                        let pagina = unsafe { core::slice::from_raw_parts((*e & ENDERECO) as *const u8, n) };
                        let _ = no.escrever(deslocamento + dentro, pagina);
                        *e &= !SUJA;
                        invalidar(v);
                    }
                }
                v += TAMANHO_PAGINA;
            }
        }
    }

    /// Dá um quadro à página de `virt`, conforme a região: zerado, ou com o
    /// pedaço do arquivo.
    fn materializar(&self, virt: u64) -> Falha {
        let pagina = virt & !(TAMANHO_PAGINA - 1);
        let Some(regiao) = self.regiao_de(pagina) else { return Falha::Invalida };
        let quadro = match &regiao.fonte {
            Fonte::Segmento { segmento, pagina: primeira } => {
                let i = primeira + ((pagina - regiao.inicio) / TAMANHO_PAGINA) as usize;
                segmento.quadro(i)
            }
            Fonte::Fisica { base } => Some(base + (pagina - regiao.inicio)),
            _ => quadros::alocar_zerado(),
        };
        let Some(quadro) = quadro else { return Falha::SemMemoria };
        if let Fonte::Arquivo { no, deslocamento, tamanho } = &regiao.fonte {
            let dentro = pagina - regiao.inicio;
            if dentro < *tamanho {
                let n = (*tamanho - dentro).min(TAMANHO_PAGINA) as usize;
                let destino = unsafe { core::slice::from_raw_parts_mut(quadro as *mut u8, n) };
                let _ = no.ler(deslocamento + dentro, destino);
            }
        }
        let flags = regiao.flags();
        match self.entrada(pagina, true) {
            Some(e) => {
                *e = quadro | flags;
                Falha::Resolvida
            }
            None => {
                quadros::liberar(quadro);
                Falha::SemMemoria
            }
        }
    }

    /// O page fault de um programa. Decide se foi um acesso legítimo (página
    /// ainda não materializada, ou copy-on-write) ou uma falha de verdade.
    pub fn tratar_falha(&self, virt: u64, escrita: bool, execucao: bool) -> Falha {
        let Some(regiao) = self.regiao_de(virt) else { return Falha::Invalida };
        let permitido = (!escrita || regiao.prot & PROT_ESCREVER != 0)
            && (!execucao || regiao.prot & PROT_EXECUTAR != 0)
            && regiao.prot != 0;
        if !permitido {
            return Falha::Invalida;
        }
        match self.entrada(virt, false) {
            Some(e) if *e & PRESENTE != 0 => {
                if escrita && *e & COPIA_NA_ESCRITA != 0 {
                    if self.copiar_pagina(e, virt & !0xFFF) { Falha::Resolvida } else { Falha::SemMemoria }
                } else {
                    Falha::Invalida
                }
            }
            _ => self.materializar(virt),
        }
    }

    /// Garante páginas zeradas em `[inicio, fim)` *agora* (para o carregador
    /// de ELF). Páginas que já existem só ganham as permissões novas.
    pub fn alocar_regiao(&mut self, inicio: u64, fim: u64, flags: u64) -> bool {
        let mut virt = inicio & !(TAMANHO_PAGINA - 1);
        while virt < fim {
            let Some(e) = self.entrada(virt, true) else { return false };
            if *e & PRESENTE != 0 {
                *e |= flags & !NAO_EXECUTAR;
                if flags & NAO_EXECUTAR == 0 {
                    *e &= !NAO_EXECUTAR; // dois segmentos na mesma página: vale o mais permissivo
                }
            } else {
                let Some(quadro) = quadros::alocar_zerado() else { return false };
                *e = quadro | flags | PRESENTE;
            }
            virt += TAMANHO_PAGINA;
        }
        true
    }

    /// Endereço físico por trás de `virt`, se for uma página de usuário
    /// (e gravável, se `escrita`).
    fn fisico(&self, virt: u64, escrita: bool) -> Option<u64> {
        self.fisico_com(virt, escrita, escrita)
    }

    /// `quebrar_cow`: antes de escrever, dá à página uma cópia só deste
    /// processo (mesmo sem permissão de escrita: é o caso do carregador e do
    /// ptrace pondo um breakpoint no código).
    fn fisico_com(&self, virt: u64, escrita: bool, quebrar_cow: bool) -> Option<u64> {
        // O kernel lê e escreve a memória do programa pelo endereço físico
        // (sem passar pela MMU), então ele mesmo faz o papel do page fault:
        // materializa a página e respeita o copy-on-write.
        if self.entrada(virt, false).is_none_or(|e| *e & PRESENTE == 0) {
            let regiao = self.regiao_de(virt)?;
            if escrita && regiao.prot & PROT_ESCREVER == 0 {
                return None;
            }
            if self.materializar(virt) != Falha::Resolvida {
                return None;
            }
        }
        let entrada = self.entrada(virt, false)?;
        // Uma página só de leitura (código) também é dividida depois de um
        // fork, só que sem a marca de copy-on-write: escrever nela à força
        // (um breakpoint) mudaria o código dos outros processos também.
        let dividida = *entrada & COPIA_NA_ESCRITA != 0
            || quadros::donos(*entrada & ENDERECO) > 1 && !self.regiao_de(virt).is_some_and(|r| r.compartilhada);
        if quebrar_cow && dividida {
            let regiao_permite = self.regiao_de(virt).is_some_and(|r| r.prot & PROT_ESCREVER != 0);
            if escrita && !regiao_permite || !self.copiar_pagina(entrada, virt & !0xFFF) {
                return None;
            }
            if !regiao_permite {
                *entrada &= !ESCRITA; // a cópia é privada, mas continua só de leitura
            }
        }
        let e = *entrada;
        let exigido = PRESENTE | USUARIO | if escrita { ESCRITA } else { 0 };
        if e & exigido != exigido {
            return None;
        }
        if escrita {
            *entrada |= SUJA; // o kernel escreveu: para o msync, conta como sujo
        }
        Some((e & ENDERECO) + (virt & (TAMANHO_PAGINA - 1)))
    }

    /// O endereço físico de `virt` (materializando a página se preciso).
    pub fn traduzir(&self, virt: u64) -> Option<u64> {
        self.fisico(virt, false)
    }

    /// Copia de/para a memória do processo página por página, pelo endereço
    /// físico. Funciona com qualquer CR3 ativo e nunca causa page fault no
    /// kernel: um ponteiro inválido de um programa vira só `false` (EFAULT).
    fn copiar(&self, virt: u64, tamanho: usize, escrita: bool, f: impl FnMut(*mut u8, usize, usize)) -> bool {
        self.copiar_com(virt, tamanho, escrita, escrita, f)
    }

    fn copiar_com(
        &self,
        mut virt: u64,
        tamanho: usize,
        escrita: bool,
        quebrar_cow: bool,
        mut f: impl FnMut(*mut u8, usize, usize),
    ) -> bool {
        let mut feito = 0;
        while feito < tamanho {
            let Some(fisico) = self.fisico_com(virt, escrita, quebrar_cow) else { return false };
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

    /// Como `copiar_para`, mas também em páginas só de leitura (para carregar
    /// o programa, ou para o ptrace pôr um breakpoint). Nunca escreve numa
    /// página dividida com outro processo: desfaz o copy-on-write antes.
    pub fn gravar(&self, virt: u64, dados: &[u8]) -> bool {
        self.copiar_com(virt, dados.len(), false, true, |destino, i, n| unsafe {
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
    /// privadas viram só leitura nos dois, marcadas como copy-on-write; a
    /// primeira escrita de qualquer um deles causa um page fault, e só então a
    /// página é copiada. As de regiões MAP_SHARED continuam as mesmas, graváveis.
    pub fn clonar(&self) -> Option<Self> {
        let mut novo = Self::novo()?;
        novo.regioes = self.regioes.clone();
        let mut ok = true;
        self.para_cada_pagina(|virt, e| {
            if !ok {
                return;
            }
            let compartilhada = self.regiao_de(virt).is_some_and(|r| r.compartilhada);
            if *e & ESCRITA != 0 && !compartilhada {
                *e = (*e & !ESCRITA) | COPIA_NA_ESCRITA;
            }
            let quadro = *e & ENDERECO;
            quadros::compartilhar(quadro);
            ok = novo.mapear(virt, quadro, *e & !ENDERECO);
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
        let flags = (*entrada & !ENDERECO & !COPIA_NA_ESCRITA) | ESCRITA;
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

    /// Quantas páginas de usuário estão mapeadas (a memória "residente").
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
        self.sincronizar(USUARIO_INICIO, USUARIO_FIM); // MAP_SHARED de arquivo volta para o arquivo
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
