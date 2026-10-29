//! Processos e escalonador.
//!
//! Cada processo tem seu espaço de endereçamento, sua tabela de arquivos e
//! sua própria **pilha de kernel**: quando o programa faz uma syscall ou é
//! interrompido, a CPU passa a usar essa pilha (o endereço está na TSS).
//!
//! Trocar de processo = trocar de pilha de kernel (`troca_contexto`, em
//! assembly). O processo A para dentro de `escalonar()`; quando alguém
//! escolher A de novo, ele continua dali como se nada tivesse acontecido.
//!
//! ## A regra de ouro deste kernel
//!
//! O código do kernel roda sempre com interrupções **desligadas**: elas só
//! ficam ligadas em modo usuário e no processo ocioso (dentro do `hlt`).
//! Com uma CPU só, isso faz o kernel inteiro funcionar como uma grande seção
//! crítica — ninguém mexe na tabela de processos ao mesmo tempo que nós. É
//! a mesma ideia do "big kernel lock" das primeiras versões do Linux.

use crate::arquivo::Arquivo;
use crate::erro::*;
use crate::interrupcoes::Contexto;
use crate::paginacao::{self, ESCRITA, EspacoEnderecos, USUARIO};
use crate::sinal::{self, Sinais};
use crate::{elf, gdt, pit, println, syscall, vfs};
use alloc::boxed::Box;
use alloc::string::{String, ToString};
use alloc::sync::Arc;
use alloc::vec;
use alloc::vec::Vec;
use core::arch::asm;
use core::mem::size_of;
use core::ptr::null_mut;

pub type Pid = u32;

const TAMANHO_PILHA_KERNEL: usize = 64 * 1024;
pub const MAX_ARQUIVOS: usize = 32;

pub const PILHA_TOPO: u64 = 0x7F_0000_0000;
pub const PILHA_TAMANHO: u64 = 256 * 1024;
/// O `mmap` entrega memória daqui para baixo (o heap do `brk` cresce de baixo para cima).
pub const MMAP_TOPO: u64 = 0x70_0000_0000;

pub use crate::sinal::{SIGFPE, SIGILL, SIGINT, SIGKILL, SIGPIPE, SIGSEGV};

/// "Canais" onde processos dormem esperando um evento.
pub const CANAL_TERMINAL: usize = 1;
pub const CANAL_TEMPO: usize = 2;
pub const CANAL_PAUSA: usize = 3;
fn canal_filhos(pid: Pid) -> usize {
    0x1_0000 + pid as usize
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Estado {
    Pronto,
    Rodando,
    Dormindo(usize),
    Zumbi,
}

pub struct Processo {
    pub pid: Pid,
    pub pai: Pid,
    pub nome: String,
    pub estado: Estado,
    rsp_salvo: u64,
    pilha_kernel: Vec<u8>,
    pub espaco: Option<EspacoEnderecos>,
    pub arquivos: Vec<Option<Arc<Arquivo>>>,
    /// Um bit por descritor: fechar no `execve` (FD_CLOEXEC / O_CLOEXEC).
    pub fecha_no_exec: u64,
    /// Próximo endereço livre para o `mmap` (desce a cada mapeamento).
    pub mmap_topo: u64,
    pub diretorio: String,
    pub brk_inicio: u64,
    pub brk: u64,
    /// Formato do `wait` do Unix: (código << 8) se saiu, ou o número do sinal que o matou.
    pub status: i32,
    /// Identidade: o uid/gid *real* (quem é) e o *efetivo* (com que direitos
    /// age; difere do real num programa setuid, como o `passwd`).
    pub uid: u32,
    pub gid: u32,
    pub euid: u32,
    pub egid: u32,
    /// Bits de permissão que arquivos novos *não* recebem (022: sem escrita para os outros).
    pub umask: u32,
    /// Base do segmento FS (TLS da libc), trocada junto com o processo.
    pub fs_base: u64,
    /// Registradores x87/SSE (xmm0..15), salvos com `fxsave` na troca de processo.
    fpu: Box<EstadoFpu>,
    pub sinais: Sinais,
    /// Quando o SIGALRM deve chegar (em ms desde a partida); 0 = sem alarme.
    pub alarme_ms: u64,
    pub tiques: u64,
}

impl Processo {
    fn novo(pai: Pid, nome: String) -> Box<Processo> {
        let t = tabela();
        let pid = t.proximo_pid;
        t.proximo_pid += 1;
        Box::new(Processo {
            pid,
            pai,
            nome,
            estado: Estado::Pronto,
            rsp_salvo: 0,
            pilha_kernel: vec![0; TAMANHO_PILHA_KERNEL],
            espaco: None,
            arquivos: Vec::new(),
            fecha_no_exec: 0,
            mmap_topo: MMAP_TOPO,
            diretorio: String::from("/"),
            brk_inicio: 0,
            brk: 0,
            status: 0,
            uid: 0,
            gid: 0,
            euid: 0,
            egid: 0,
            umask: 0o022,
            fs_base: 0,
            fpu: Box::new(fpu_inicial()),
            sinais: Sinais::novo(),
            alarme_ms: 0,
            tiques: 0,
        })
    }

    pub fn cred(&self) -> vfs::Cred {
        vfs::Cred { uid: self.euid, gid: self.egid }
    }

    fn topo_pilha(&self) -> u64 {
        (self.pilha_kernel.as_ptr() as u64 + self.pilha_kernel.len() as u64) & !15
    }

    /// Onde fica o `Contexto` do programa quando ele entra no kernel: no topo
    /// da pilha de kernel, porque é ali que a CPU começa a empilhar.
    fn contexto(&self) -> *mut Contexto {
        (self.topo_pilha() - size_of::<Contexto>() as u64) as *mut Contexto
    }

    /// Monta a pilha de kernel de um processo que nunca rodou para que o
    /// primeiro `troca_contexto` para ele "volte" em `isr_sair`, que restaura
    /// o `Contexto` e salta para o programa com `iretq`.
    fn preparar_primeira_execucao(&mut self) {
        let quadro = self.contexto() as u64 - 7 * 8;
        unsafe {
            for i in 0..6 {
                (quadro as *mut u64).add(i).write(0); // r15, r14, r13, r12, rbx, rbp
            }
            (quadro as *mut u64).add(6).write(isr_sair as *const () as u64); // endereço de retorno
        }
        self.rsp_salvo = quadro;
    }
}

unsafe extern "C" {
    fn troca_contexto(salvar_rsp_em: *mut u64, novo_rsp: u64);
    fn isr_sair();
}

/// A área do `fxsave`: 512 bytes, alinhados em 16.
#[derive(Clone, Copy)]
#[repr(C, align(16))]
struct EstadoFpu([u8; 512]);

/// Estado da FPU de um programa que acabou de começar (gravado no boot).
static mut FPU_INICIAL: EstadoFpu = EstadoFpu([0; 512]);

fn fpu_inicial() -> EstadoFpu {
    unsafe { *(&raw const FPU_INICIAL) }
}

/// Os programas em C usam os registradores SSE (xmm) para `double`, para
/// copiar memória... O kernel, compilado sem SSE, nunca mexe neles: basta
/// salvar os do processo que sai e carregar os do que entra.
fn salvar_fpu(estado: &mut EstadoFpu) {
    unsafe { asm!("fxsave64 [{}]", in(reg) estado.0.as_mut_ptr(), options(nostack, preserves_flags)) };
}

fn restaurar_fpu(estado: &EstadoFpu) {
    unsafe { asm!("fxrstor64 [{}]", in(reg) estado.0.as_ptr(), options(nostack, preserves_flags)) };
}

/// Lido por `entrada_syscall` (asm): a pilha de kernel do processo atual.
#[unsafe(no_mangle)]
static mut PILHA_KERNEL_ATUAL: u64 = 0;

struct Tabela {
    processos: Vec<Box<Processo>>, // o índice 0 é sempre o ocioso
    atual: *mut Processo,
    proximo_pid: Pid,
}

static mut TABELA: Tabela = Tabela { processos: Vec::new(), atual: null_mut(), proximo_pid: 0 };

fn tabela() -> &'static mut Tabela {
    // Seguro pela regra de ouro: só o kernel mexe, e com interrupções desligadas.
    unsafe { &mut *(&raw mut TABELA) }
}

pub fn atual() -> &'static mut Processo {
    unsafe { &mut *tabela().atual }
}

fn buscar(pid: Pid) -> Option<&'static mut Processo> {
    tabela().processos.iter_mut().find(|p| p.pid == pid).map(|p| &mut **p)
}

/// Transforma o fluxo do boot no processo 0, o "ocioso".
pub fn init() {
    unsafe {
        // FPU zerada, MXCSR padrão (todas as exceções de ponto flutuante mascaradas).
        asm!("fninit", "ldmxcsr [{}]", in(reg) &0x1F80u32, options(nostack));
        salvar_fpu(&mut *(&raw mut FPU_INICIAL));
    }
    let mut ocioso = Processo::novo(0, "ocioso".to_string());
    ocioso.pilha_kernel = Vec::new(); // ele usa a pilha do boot
    ocioso.estado = Estado::Rodando;
    let t = tabela();
    t.atual = &mut *ocioso;
    t.processos.push(ocioso);
}

/// O que o processo 0 faz para sempre: escalonar e, se não houver ninguém
/// para rodar, dormir até a próxima interrupção.
pub fn ocioso() -> ! {
    loop {
        escalonar();
        // `sti; hlt` é atômico (o sti só vale depois da instrução seguinte):
        // nenhuma interrupção se perde entre os dois.
        unsafe { asm!("sti", "hlt", "cli", options(nostack)) };
    }
}

/// Escolhe o próximo processo pronto (rodízio, ou "round-robin") e troca para ele.
pub fn escalonar() {
    let t = tabela();
    let atual = t.atual;
    unsafe {
        if (*atual).estado == Estado::Rodando {
            (*atual).estado = Estado::Pronto;
        }
        let n = t.processos.len();
        let posicao = t.processos.iter().position(|p| core::ptr::eq(&**p, atual)).unwrap_or(0);
        let mut proximo: *mut Processo = &mut *t.processos[0];
        for k in 1..=n {
            let i = (posicao + k) % n;
            if i != 0 && t.processos[i].estado == Estado::Pronto {
                proximo = &mut *t.processos[i];
                break;
            }
        }
        (*proximo).estado = Estado::Rodando;
        if proximo == atual {
            return;
        }

        t.atual = proximo;
        if !(*proximo).pilha_kernel.is_empty() {
            let topo = (*proximo).topo_pilha();
            gdt::definir_rsp0(topo);
            PILHA_KERNEL_ATUAL = topo;
        }
        match &(*proximo).espaco {
            Some(espaco) => espaco.ativar(),
            None => paginacao::ativar_kernel(),
        }
        salvar_fpu(&mut (*atual).fpu);
        restaurar_fpu(&(*proximo).fpu);
        syscall::definir_fs((*proximo).fs_base);
        troca_contexto(&raw mut (*atual).rsp_salvo, (*proximo).rsp_salvo);
    }
}

pub fn dormir(canal: usize) {
    atual().estado = Estado::Dormindo(canal);
    escalonar();
}

pub fn acordar(canal: usize) {
    for p in tabela().processos.iter_mut() {
        if p.estado == Estado::Dormindo(canal) {
            p.estado = Estado::Pronto;
        }
    }
}

/// A cada tique do timer.
pub fn tique() {
    if tabela().atual.is_null() {
        return;
    }
    atual().tiques += 1;
    acordar(CANAL_TEMPO);
    let agora = pit::milissegundos();
    let vencidos: Vec<Pid> = tabela()
        .processos
        .iter()
        .filter(|p| p.alarme_ms != 0 && agora >= p.alarme_ms)
        .map(|p| p.pid)
        .collect();
    for pid in vencidos {
        if let Some(p) = buscar(pid) {
            p.alarme_ms = 0;
        }
        let _ = enviar_sinal(pid, sinal::SIGALRM);
    }
}

/// Chamada antes de voltar para o modo usuário (fim de syscall ou interrupção).
/// É aqui que a preempção acontece e que os sinais pendentes são entregues.
pub fn ao_voltar_para_usuario(ctx: &mut Contexto, foi_o_timer: bool) {
    if foi_o_timer {
        escalonar(); // preempção: o programa perde a vez, mesmo que não queira
    }
    while let Some(numero) = atual().sinais.retirar() {
        let acao = atual().sinais.acoes[numero as usize];
        if acao.tem_tratador() {
            if !entregar(ctx, numero, acao) {
                sair(SIGSEGV as i32); // a pilha do programa está estragada
            }
            return; // um por vez: os outros saem depois do rt_sigreturn
        }
        if acao.tratador == sinal::SIG_DFL && !sinal::padrao_e_ignorar(numero) || numero == SIGKILL {
            sair(numero as i32);
        }
    }
}

const TAMANHO_QUADRO_SINAL: u64 = 8 + 128 + size_of::<Contexto>() as u64 + 8 + 512;

/// Monta o quadro do sinal na pilha do usuário e desvia para o tratador.
fn entregar(ctx: &mut Contexto, numero: u32, acao: sinal::Acao) -> bool {
    let p = atual();
    // Pula a red zone e deixa RSP+8 alinhado em 16, como numa chamada de função.
    let base = ((ctx.rsp.wrapping_sub(128 + TAMANHO_QUADRO_SINAL)) & !15) - 8;
    let mut quadro = Vec::with_capacity(TAMANHO_QUADRO_SINAL as usize);
    quadro.extend(acao.restaurador.to_le_bytes());
    let mut info = [0u8; 128];
    info[..4].copy_from_slice(&numero.to_le_bytes()); // si_signo
    quadro.extend(info);
    let salvo = unsafe { core::slice::from_raw_parts(ctx as *const Contexto as *const u8, size_of::<Contexto>()) };
    quadro.extend(salvo);
    quadro.extend(p.sinais.bloqueados.to_le_bytes());
    // O tratador pode usar os registradores xmm: guardamos os de antes também.
    let mut fpu = EstadoFpu([0; 512]);
    salvar_fpu(&mut fpu);
    quadro.extend(fpu.0);
    let Some(espaco) = &p.espaco else { return false };
    if !espaco.copiar_para(base, &quadro) {
        return false;
    }
    p.sinais.ao_entregar(numero);
    ctx.rip = acao.tratador;
    ctx.rsp = base;
    ctx.rdi = numero as u64;
    ctx.rsi = base + 8; // siginfo
    ctx.rdx = 0;
    ctx.rax = 0;
    true
}

/// `rt_sigreturn`: desfaz o que `entregar` fez. Devolve o RAX restaurado
/// (para que o tratador de syscalls não o sobrescreva).
pub fn retornar_de_sinal(ctx: &mut Contexto) -> u64 {
    let p = atual();
    // O tratador fez `ret` (tirou o endereço de retorno) e o restaurador fez a
    // syscall: RSP aponta para o siginfo.
    let tamanho = size_of::<Contexto>() + 8 + 512;
    let dados = p.espaco.as_ref().and_then(|e| e.copiar_de(ctx.rsp + 128, tamanho));
    let Some(dados) = dados else { sair(SIGSEGV as i32) };
    let mut salvo: Contexto = unsafe { core::ptr::read_unaligned(dados.as_ptr() as *const Contexto) };
    // Nada de deixar o programa voltar para o anel 0 ou mexer em flags do sistema.
    salvo.cs = gdt::SELETOR_CODIGO_USUARIO as u64;
    salvo.ss = gdt::SELETOR_DADOS_USUARIO as u64;
    salvo.rflags = (salvo.rflags & 0xCD5) | 0x202;
    let fim_contexto = size_of::<Contexto>();
    let mascara = u64::from_le_bytes(dados[fim_contexto..fim_contexto + 8].try_into().unwrap());
    p.sinais.bloqueados = mascara & !sinal::INBLOQUEAVEIS;
    let mut fpu = EstadoFpu([0; 512]);
    fpu.0.copy_from_slice(&dados[fim_contexto + 8..]);
    // Um MXCSR com bits reservados ligados faria o fxrstor gerar #GP no kernel.
    let permitidos = u32::from_le_bytes(fpu_inicial().0[28..32].try_into().unwrap());
    let mxcsr = u32::from_le_bytes(fpu.0[24..28].try_into().unwrap()) & permitidos;
    fpu.0[24..28].copy_from_slice(&mxcsr.to_le_bytes());
    restaurar_fpu(&fpu);
    *ctx = salvo;
    ctx.rax
}

/// Uma syscall interrompida por um sinal deve recomeçar sozinha? (SA_RESTART)
pub fn reiniciar_syscall() -> bool {
    let s = &atual().sinais;
    let entregaveis = s.entregaveis();
    entregaveis != 0 && {
        let acao = s.acoes[entregaveis.trailing_zeros() as usize];
        acao.tem_tratador() && acao.flags & sinal::SA_RESTART != 0
    }
}

// -----------------------------------------------------------------------------
// Criar, duplicar, substituir e encerrar processos
// -----------------------------------------------------------------------------

struct Imagem {
    espaco: EspacoEnderecos,
    entrada: u64,
    rsp: u64,
    brk: u64,
    meta: vfs::Meta,
}

fn carregar_programa(caminho: &str, args: &[String], ambiente: &[String], cred: vfs::Cred) -> Result<Imagem, i64> {
    let no = vfs::buscar_como(caminho, cred)?;
    let dados = match no.tipo()? {
        vfs::Tipo::Arquivo => {
            no.permite(cred, vfs::EXECUTAR)?;
            no.ler_tudo()?
        }
        vfs::Tipo::Diretorio => return Err(EACCES),
        vfs::Tipo::Dispositivo(_) => return Err(EACCES),
    };
    let mut espaco = EspacoEnderecos::novo().ok_or(ENOMEM)?;
    let programa = elf::carregar(&dados, &mut espaco)?;
    if !espaco.alocar_regiao(PILHA_TOPO - PILHA_TAMANHO, PILHA_TOPO, USUARIO | ESCRITA) {
        return Err(ENOMEM);
    }
    let meta = no.meta()?;
    let rsp = montar_pilha(&espaco, args, ambiente, &programa, cred, meta)?;
    let brk = (programa.fim + 0xFFF) & !0xFFF;
    Ok(Imagem { espaco, entrada: programa.entrada, rsp, brk, meta })
}

/// Monta a pilha inicial no formato da ABI System V (a mesma do Linux):
///
/// ```text
///   rsp → argc | argv[0..n] | NULL | envp[0..m] | NULL | auxv (pares tipo, valor) | AT_NULL
///         ... e, lá em cima, as próprias strings e 16 bytes aleatórios.
/// ```
///
/// O "vetor auxiliar" (auxv) é como o kernel conta coisas para a libc: onde
/// estão os program headers do executável, o tamanho da página, quem é o usuário...
fn montar_pilha(
    espaco: &EspacoEnderecos,
    args: &[String],
    ambiente: &[String],
    programa: &elf::Programa,
    cred: vfs::Cred,
    meta: vfs::Meta,
) -> Result<u64, i64> {
    let mut sp = PILHA_TOPO;
    let mut empilhar = |bytes: &[u8]| -> Result<u64, i64> {
        sp -= bytes.len() as u64;
        if espaco.gravar(sp, bytes) { Ok(sp) } else { Err(EFAULT) }
    };
    let mut ponteiros_args = Vec::new();
    let mut ponteiros_ambiente = Vec::new();
    for (lista, ponteiros) in [(args, &mut ponteiros_args), (ambiente, &mut ponteiros_ambiente)] {
        for texto in lista {
            let mut bytes = texto.as_bytes().to_vec();
            bytes.push(0);
            ponteiros.push(empilhar(&bytes)?);
        }
    }
    // 16 bytes "aleatórios" (a libc usa como semente do canário da pilha).
    let tsc = unsafe { core::arch::x86_64::_rdtsc() };
    let mut aleatorio = [0u8; 16];
    aleatorio[..8].copy_from_slice(&tsc.to_le_bytes());
    aleatorio[8..].copy_from_slice(&(tsc.rotate_left(29) ^ 0x9E37_79B9_7F4A_7C15).to_le_bytes());
    let ptr_aleatorio = empilhar(&aleatorio)?;

    let mut palavras = vec![args.len() as u64];
    palavras.extend(&ponteiros_args);
    palavras.push(0);
    palavras.extend(&ponteiros_ambiente);
    palavras.push(0);
    let seguro = (meta.permissoes & (vfs::SETUID | vfs::SETGID) != 0) as u64;
    let auxv: [(u64, u64); 12] = [
        (3, programa.cabecalhos),            // AT_PHDR
        (4, 56),                             // AT_PHENT
        (5, programa.quantidade_cabecalhos), // AT_PHNUM
        (6, 4096),                           // AT_PAGESZ
        (9, programa.entrada),               // AT_ENTRY
        (11, atual().uid as u64),            // AT_UID
        (12, cred.uid as u64),               // AT_EUID
        (13, atual().gid as u64),            // AT_GID
        (14, cred.gid as u64),               // AT_EGID
        (23, seguro),                        // AT_SECURE
        (25, ptr_aleatorio),                 // AT_RANDOM
        (0, 0),                              // AT_NULL
    ];
    for (tipo, valor) in auxv {
        palavras.push(tipo);
        palavras.push(valor);
    }
    let sp = (sp - palavras.len() as u64 * 8) & !15;
    let bytes: Vec<u8> = palavras.iter().flat_map(|p| p.to_le_bytes()).collect();
    if !espaco.gravar(sp, &bytes) {
        return Err(EFAULT);
    }
    Ok(sp)
}

/// Cria o processo 1. Todos os outros nascem dele, por `fork`.
pub fn criar_init(caminho: &str) -> Result<(), i64> {
    let ambiente = ["PATH=/bin".to_string(), "HOME=/".to_string(), "TERM=linux".to_string()];
    let imagem = carregar_programa(caminho, &[caminho.to_string()], &ambiente, vfs::Cred { uid: 0, gid: 0 })?;
    let mut p = Processo::novo(0, "init".to_string());
    unsafe { *p.contexto() = Contexto::usuario(imagem.entrada, imagem.rsp) };
    p.espaco = Some(imagem.espaco);
    p.brk_inicio = imagem.brk;
    p.brk = imagem.brk;
    p.preparar_primeira_execucao();
    tabela().processos.push(p);
    Ok(())
}

/// `fork`: um processo novo, cópia exata do atual. No filho, a syscall devolve 0.
pub fn fork(ctx: &Contexto) -> Result<Pid, i64> {
    let pai = atual();
    let espaco = pai.espaco.as_ref().ok_or(EINVAL)?.clonar().ok_or(ENOMEM)?;
    let mut filho = Processo::novo(pai.pid, pai.nome.clone());
    filho.espaco = Some(espaco);
    filho.arquivos = pai.arquivos.clone();
    filho.fecha_no_exec = pai.fecha_no_exec;
    filho.mmap_topo = pai.mmap_topo;
    filho.diretorio = pai.diretorio.clone();
    filho.brk_inicio = pai.brk_inicio;
    filho.brk = pai.brk;
    (filho.uid, filho.gid, filho.euid, filho.egid) = (pai.uid, pai.gid, pai.euid, pai.egid);
    filho.umask = pai.umask;
    filho.fs_base = pai.fs_base;
    salvar_fpu(&mut filho.fpu); // os registradores do pai, como estão agora
    filho.sinais = pai.sinais.clone();
    filho.sinais.pendentes = 0;
    unsafe {
        *filho.contexto() = *ctx;
        (*filho.contexto()).rax = 0;
    }
    filho.preparar_primeira_execucao();
    let pid = filho.pid;
    tabela().processos.push(filho);
    Ok(pid)
}

/// Muda o TLS de outro processo (usado pelo `clone` com CLONE_SETTLS).
pub fn definir_fs_de(pid: Pid, valor: u64) {
    if let Some(p) = buscar(pid) {
        p.fs_base = valor;
    }
}

/// `execve`: troca o programa do processo atual por outro.
pub fn exec(ctx: &mut Contexto, caminho: &str, args: Vec<String>, ambiente: Vec<String>) -> Result<(), i64> {
    let p = atual();
    let imagem = carregar_programa(caminho, &args, &ambiente, p.cred())?;
    // setuid/setgid: o programa roda com os direitos do dono do arquivo.
    if imagem.meta.permissoes & vfs::SETUID != 0 {
        p.euid = imagem.meta.uid;
    }
    if imagem.meta.permissoes & vfs::SETGID != 0 {
        p.egid = imagem.meta.gid;
    }
    imagem.espaco.ativar();
    p.espaco = Some(imagem.espaco); // o espaço antigo é liberado aqui
    p.brk_inicio = imagem.brk;
    p.brk = imagem.brk;
    p.nome = caminho.rsplit('/').next().unwrap_or(caminho).to_string();
    p.sinais.ao_executar();
    p.mmap_topo = MMAP_TOPO;
    // Descritores marcados com close-on-exec não passam para o programa novo.
    for (fd, arquivo) in p.arquivos.iter_mut().enumerate() {
        if p.fecha_no_exec & (1 << fd) != 0 {
            *arquivo = None;
        }
    }
    p.fecha_no_exec = 0;
    // Programa novo: FPU limpa e sem TLS.
    *p.fpu = fpu_inicial();
    restaurar_fpu(&p.fpu);
    p.fs_base = 0;
    syscall::definir_fs(0);
    *ctx = Contexto::usuario(imagem.entrada, imagem.rsp);
    Ok(())
}

/// Encerra o processo atual. Ele vira "zumbi" até o pai recolher o status com `wait`.
pub fn sair(status: i32) -> ! {
    let p = atual();
    if p.pid == 1 {
        panic!("o init terminou (status {:#x})", status);
    }
    p.arquivos.clear(); // fecha tudo (pipes avisam a outra ponta)
    paginacao::ativar_kernel();
    p.espaco = None; // devolve a memória
    p.status = status;
    p.estado = Estado::Zumbi;
    let (pid, pai) = (p.pid, p.pai);

    // Os filhos órfãos são adotados pelo init.
    let mut orfao_zumbi = false;
    for q in tabela().processos.iter_mut() {
        if q.pai == pid {
            q.pai = 1;
            orfao_zumbi |= q.estado == Estado::Zumbi;
        }
    }
    acordar(canal_filhos(pai));
    let _ = enviar_sinal(pai, sinal::SIGCHLD);
    if orfao_zumbi {
        acordar(canal_filhos(1));
    }
    escalonar();
    unreachable!("um zumbi voltou a rodar");
}

pub const WNOHANG: u64 = 1;

/// `wait`: espera um filho terminar e devolve (pid, status).
pub fn esperar(pid: i64, opcoes: u64) -> Result<(Pid, i32), i64> {
    loop {
        let eu = atual().pid;
        let t = tabela();
        let mut tem_filho = false;
        let mut zumbi = None;
        for (i, q) in t.processos.iter().enumerate().skip(1) {
            if q.pai == eu && (pid == -1 || q.pid as i64 == pid) {
                tem_filho = true;
                if q.estado == Estado::Zumbi {
                    zumbi = Some(i);
                    break;
                }
            }
        }
        if let Some(i) = zumbi {
            let q = t.processos.remove(i); // aqui a pilha de kernel dele é liberada
            return Ok((q.pid, q.status));
        }
        if !tem_filho {
            return Err(ECHILD);
        }
        if opcoes & WNOHANG != 0 {
            return Ok((0, 0));
        }
        if tem_sinal_pendente() {
            return Err(EINTR);
        }
        dormir(canal_filhos(eu));
    }
}

// -----------------------------------------------------------------------------
// Sinais
// -----------------------------------------------------------------------------

/// Só o root, ou o mesmo usuário, pode mandar sinais para um processo.
pub fn pode_sinalizar(pid: Pid) -> Result<(), i64> {
    let eu = atual();
    let (euid, uid) = (eu.euid, eu.uid);
    let alvo = buscar(pid).ok_or(ESRCH)?;
    if euid == 0 || euid == alvo.uid || uid == alvo.uid { Ok(()) } else { Err(EPERM) }
}

pub fn enviar_sinal(pid: Pid, sinal: u32) -> Result<(), i64> {
    if sinal >= 64 {
        return Err(EINVAL);
    }
    let p = buscar(pid).ok_or(ESRCH)?;
    if p.pid == 0 {
        return Err(EPERM);
    }
    if sinal == 0 || p.estado == Estado::Zumbi {
        return Ok(());
    }
    // Se ele estiver dormindo (num read, wait, sleep...), acorda: a chamada
    // devolve EINTR e o sinal é entregue na volta para o modo usuário.
    if p.sinais.postar(sinal) && p.sinais.entregaveis() != 0 {
        if let Estado::Dormindo(_) = p.estado {
            p.estado = Estado::Pronto;
        }
    }
    Ok(())
}

/// Ctrl+C: manda o sinal para todos os processos (menos o ocioso). Quem
/// ignora o sinal (o init e o shell) sobrevive.
pub fn sinal_para_o_terminal(sinal: u32) {
    let pids: Vec<Pid> = tabela().processos.iter().skip(1).map(|p| p.pid).collect();
    for pid in pids {
        let _ = enviar_sinal(pid, sinal);
    }
}

pub fn tem_sinal_pendente() -> bool {
    atual().sinais.entregaveis() != 0
}

/// Uma exceção da CPU em modo usuário. Se o programa tiver um tratador para
/// o sinal, ele é chamado; senão, o programa morre (e o kernel segue em pé).
pub fn falha_do_programa(descricao: core::fmt::Arguments, numero: u32) {
    let p = atual();
    let acao = p.sinais.acoes[numero as usize];
    if acao.tem_tratador() && p.sinais.bloqueados & (1 << numero) == 0 {
        p.sinais.pendentes |= 1 << numero;
        return;
    }
    println!("[{}] {}: {}", p.pid, p.nome, descricao);
    sair(numero as i32)
}

// -----------------------------------------------------------------------------
// Informações para o `ps`
// -----------------------------------------------------------------------------

pub struct Resumo {
    pub pid: Pid,
    pub pai: Pid,
    pub uid: u32,
    pub estado: u32,
    pub memoria_kib: u64,
    pub tiques: u64,
    pub nome: String,
}

pub fn listar() -> Vec<Resumo> {
    tabela()
        .processos
        .iter()
        .map(|p| Resumo {
            pid: p.pid,
            pai: p.pai,
            uid: p.uid,
            estado: match p.estado {
                Estado::Pronto => 0,
                Estado::Rodando => 1,
                Estado::Dormindo(_) => 2,
                Estado::Zumbi => 3,
            },
            memoria_kib: p.espaco.as_ref().map_or(0, |e| e.paginas() as u64 * 4)
                + p.pilha_kernel.len() as u64 / 1024,
            tiques: p.tiques,
            nome: p.nome.clone(),
        })
        .collect()
}
