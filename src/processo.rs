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
use crate::paginacao::{self, EspacoEnderecos, Falha, Fonte, PROT_ESCREVER, PROT_LER, Regiao};
use crate::sinal::{self, Sinais};
use crate::{elf, gdt, pit, println, syscall, vfs};
use alloc::boxed::Box;
use alloc::string::{String, ToString};
use alloc::sync::Arc;
use alloc::vec;
use alloc::vec::Vec;
use core::arch::asm;
use core::cell::UnsafeCell;
use core::mem::size_of;
use core::ptr::null_mut;

pub type Pid = u32;

/// Algo que as threads de um processo dividem (memória, arquivos abertos,
/// diretório, tratadores de sinal). Não tem lock: pela regra de ouro, só o
/// kernel mexe, com as interrupções desligadas, numa CPU só.
pub struct Dividido<T>(UnsafeCell<T>);

unsafe impl<T> Sync for Dividido<T> {}
unsafe impl<T> Send for Dividido<T> {}

impl<T> Dividido<T> {
    pub fn novo(valor: T) -> Arc<Self> {
        Arc::new(Dividido(UnsafeCell::new(valor)))
    }

    #[allow(clippy::mut_from_ref)]
    pub fn get(&self) -> &'static mut T {
        unsafe { &mut *self.0.get() }
    }
}

/// A memória de um processo: o espaço de endereçamento e onde estão o heap e o mmap.
pub struct Memoria {
    pub espaco: EspacoEnderecos,
    pub brk_inicio: u64,
    pub brk: u64,
    /// Próximo endereço livre para o `mmap` (desce a cada mapeamento).
    pub mmap_topo: u64,
}

/// A tabela de descritores de arquivo.
#[derive(Default)]
pub struct Arquivos {
    pub lista: Vec<Option<Arc<Arquivo>>>,
    /// Um bit por descritor: fechar no `execve` (FD_CLOEXEC / O_CLOEXEC).
    pub fecha_no_exec: u64,
}

pub struct InfoFs {
    pub diretorio: String,
    /// Bits de permissão que arquivos novos *não* recebem (022: sem escrita para os outros).
    pub umask: u32,
}

const TAMANHO_PILHA_KERNEL: usize = 64 * 1024;
pub const MAX_ARQUIVOS: usize = 32;

/// A pilha começa até 1 GiB abaixo daqui (o deslocamento é sorteado: ASLR).
const PILHA_TOPO_MAXIMO: u64 = 0x7F_0000_0000;
/// Quanto a pilha pode crescer (as páginas só existem quando são usadas).
const PILHA_MAXIMA: u64 = 8 * 1024 * 1024;
/// Um número de páginas sorteado, de 0 até `maximo` - 1, para o ASLR.
fn paginas_aleatorias(maximo: u64) -> u64 {
    (crate::aleatorio::proximo() % maximo) * 4096
}

pub use crate::sinal::{SIGFPE, SIGILL, SIGINT, SIGKILL, SIGPIPE, SIGSEGV};

/// "Canais" onde processos dormem esperando um evento.
pub const CANAL_TERMINAL: usize = 1;
pub const CANAL_TEMPO: usize = 2;
pub const CANAL_PAUSA: usize = 3;
/// Quem está num `poll`/`select` dorme aqui; qualquer evento de E/S acorda.
pub const CANAL_POLL: usize = 4;
fn canal_filhos(pid: Pid) -> usize {
    0x1_0000 + pid as usize
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Estado {
    Pronto,
    Rodando,
    Dormindo(usize),
    /// Parado por um sinal (Ctrl+Z, SIGSTOP...) até alguém mandar SIGCONT.
    Parado,
    Zumbi,
}

/// Uma tarefa: uma thread. Um processo de uma thread só é uma tarefa com
/// `pid == tgid`; as threads extras de um processo têm o mesmo `tgid` e
/// dividem com ele memória, arquivos, diretório e tratadores de sinal.
pub struct Processo {
    /// O número desta tarefa (o "tid"). Na primeira thread, é também o pid do processo.
    pub pid: Pid,
    /// O processo (grupo de threads) a que ela pertence: é o que `getpid` devolve.
    pub tgid: Pid,
    pub pai: Pid,
    /// Grupo de processos: um "job" do shell (todos os programas de um pipeline).
    pub pgid: Pid,
    /// Sessão: tudo o que nasceu de um login.
    pub sid: Pid,
    /// Parou e o pai ainda não soube (o `wait` com WUNTRACED conta para ele).
    parada_a_relatar: Option<u32>,
    pub nome: String,
    /// A linha de comando e o caminho do programa (para o /proc).
    pub argumentos: Vec<String>,
    pub executavel: String,
    /// Quando nasceu (ms desde a partida).
    pub inicio_ms: u64,
    /// Até quando um `poll` com prazo pode dormir (ms desde a partida; 0 = sem prazo).
    pub prazo_ms: u64,
    pub estado: Estado,
    rsp_salvo: u64,
    pilha_kernel: Vec<u8>,
    memoria: Option<Arc<Dividido<Memoria>>>,
    arquivos: Arc<Dividido<Arquivos>>,
    fs: Arc<Dividido<InfoFs>>,
    /// Ao sair, zerar este endereço e acordar quem espera nele (é como o
    /// `pthread_join` sabe que a thread terminou: CLONE_CHILD_CLEARTID).
    pub limpar_tid: u64,
    /// Uma thread que terminou: some da tabela sozinha (ninguém faz `wait` nela).
    thread_morta: bool,
    /// O código de saída do processo, se alguma thread chamou `exit_group`.
    saida_do_grupo: Arc<Dividido<Option<i32>>>,
    /// Acordado por um FUTEX_WAKE (e não por prazo vencido ou sinal).
    acordado_por_futex: bool,
    /// Formato do `wait` do Unix: (código << 8) se saiu, ou o número do sinal que o matou.
    pub status: i32,
    /// Identidade: o uid/gid *real* (quem é) e o *efetivo* (com que direitos
    /// age; difere do real num programa setuid, como o `passwd`).
    pub uid: u32,
    pub gid: u32,
    pub euid: u32,
    pub egid: u32,
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
            tgid: pid,
            pai,
            pgid: 0,
            sid: 0,
            parada_a_relatar: None,
            argumentos: Vec::new(),
            executavel: String::new(),
            inicio_ms: pit::milissegundos(),
            prazo_ms: 0,
            nome,
            estado: Estado::Pronto,
            rsp_salvo: 0,
            pilha_kernel: vec![0; TAMANHO_PILHA_KERNEL],
            memoria: None,
            arquivos: Dividido::novo(Arquivos::default()),
            fs: Dividido::novo(InfoFs { diretorio: String::from("/"), umask: 0o022 }),
            limpar_tid: 0,
            thread_morta: false,
            saida_do_grupo: Dividido::novo(None),
            acordado_por_futex: false,
            status: 0,
            uid: 0,
            gid: 0,
            euid: 0,
            egid: 0,
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

    pub fn mem(&self) -> Option<&'static mut Memoria> {
        self.memoria.as_ref().map(|m| m.get())
    }

    pub fn espaco(&self) -> Option<&'static mut EspacoEnderecos> {
        self.mem().map(|m| &mut m.espaco)
    }

    pub fn arquivos(&self) -> &'static mut Arquivos {
        self.arquivos.get()
    }

    pub fn fs(&self) -> &'static mut InfoFs {
        self.fs.get()
    }

    /// A primeira thread do processo (a que o pai espera com `wait`).
    fn eh_lider(&self) -> bool {
        self.pid == self.tgid
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
    // Threads que já terminaram (de outras pilhas: a atual ainda está em uso).
    t.processos.retain(|p| !(p.thread_morta && p.estado == Estado::Zumbi && !core::ptr::eq(&**p, atual)));
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
        match (*proximo).espaco() {
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
    // Todo evento de E/S (pipe, terminal, filho...) também acorda quem está
    // num poll: ele confere de novo os descritores que está vigiando.
    let tambem_poll = canal != CANAL_TEMPO;
    for p in tabela().processos.iter_mut() {
        if p.estado == Estado::Dormindo(canal) || tambem_poll && p.estado == Estado::Dormindo(CANAL_POLL) {
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
    for p in tabela().processos.iter_mut() {
        if matches!(p.estado, Estado::Dormindo(_)) && p.prazo_ms != 0 && agora >= p.prazo_ms {
            p.estado = Estado::Pronto; // o prazo (de um poll, futex...) venceu
        }
    }
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
        let acao = atual().sinais.acoes()[numero as usize];
        if numero == SIGKILL {
            sair(numero as i32);
        }
        if acao.tem_tratador() && numero != sinal::SIGSTOP {
            if !entregar(ctx, numero, acao) {
                sair(SIGSEGV as i32); // a pilha do programa está estragada
            }
            return; // um por vez: os outros saem depois do rt_sigreturn
        }
        if acao.tratador == sinal::SIG_IGN && numero != sinal::SIGSTOP || sinal::padrao_e_ignorar(numero) {
            continue;
        }
        if sinal::padrao_e_parar(numero) {
            // Para aqui mesmo; quando vier um SIGCONT, segue deste ponto.
            parar(atual(), numero);
            escalonar();
            continue;
        }
        sair(numero as i32);
    }
}

/// Coloca o processo em estado Parado e avisa o pai (que pode estar num `wait`).
fn parar(p: &mut Processo, numero: u32) {
    p.estado = Estado::Parado;
    p.parada_a_relatar = Some(numero);
    let pai = p.pai;
    acordar(canal_filhos(pai));
    let _ = enviar_sinal(pai, sinal::SIGCHLD);
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
    let Some(espaco) = p.espaco() else { return false };
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
    let dados = p.espaco().and_then(|e| e.copiar_de(ctx.rsp + 128, tamanho));
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
        let acao = s.acoes()[entregaveis.trailing_zeros() as usize];
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
    mmap_topo: u64,
    meta: vfs::Meta,
}

fn carregar_programa(caminho: &str, args: &[String], ambiente: &[String], cred: vfs::Cred) -> Result<Imagem, i64> {
    let no = vfs::buscar_como(caminho, cred)?;
    let dados = match no.tipo()? {
        vfs::Tipo::Arquivo => {
            no.permite(cred, vfs::EXECUTAR)?;
            no.ler_tudo()?
        }
        vfs::Tipo::Diretorio | vfs::Tipo::Dispositivo(_) | vfs::Tipo::Link => return Err(EACCES),
    };
    let mut espaco = EspacoEnderecos::novo().ok_or(ENOMEM)?;
    let programa = elf::carregar(&dados, &mut espaco)?;

    // ASLR: pilha, mmap e heap em posições sorteadas a cada execução, para que
    // um ataque não saiba de antemão onde estão.
    let pilha_topo = PILHA_TOPO_MAXIMO - paginas_aleatorias(1 << 18);
    espaco.criar_regiao(Regiao {
        inicio: pilha_topo - PILHA_MAXIMA,
        fim: pilha_topo,
        prot: PROT_LER | PROT_ESCREVER,
        compartilhada: false,
        fonte: Fonte::Anonima,
    });
    let mmap_topo = pilha_topo - PILHA_MAXIMA - 0x1000_0000 - paginas_aleatorias(1 << 18);
    let brk = ((programa.fim + 0xFFF) & !0xFFF) + paginas_aleatorias(8192);

    let meta = no.meta()?;
    let rsp = montar_pilha(&espaco, pilha_topo, args, ambiente, &programa, cred, meta)?;
    Ok(Imagem { espaco, entrada: programa.entrada, rsp, brk, mmap_topo, meta })
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
    topo: u64,
    args: &[String],
    ambiente: &[String],
    programa: &elf::Programa,
    cred: vfs::Cred,
    meta: vfs::Meta,
) -> Result<u64, i64> {
    let mut sp = topo;
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
    (p.pgid, p.sid) = (p.pid, p.pid);
    p.argumentos = alloc::vec![caminho.to_string()];
    p.executavel = caminho.to_string();
    unsafe { *p.contexto() = Contexto::usuario(imagem.entrada, imagem.rsp) };
    p.memoria = Some(Dividido::novo(Memoria {
        espaco: imagem.espaco,
        brk_inicio: imagem.brk,
        brk: imagem.brk,
        mmap_topo: imagem.mmap_topo,
    }));
    p.preparar_primeira_execucao();
    tabela().processos.push(p);
    Ok(())
}

/// `fork`: um processo novo, cópia exata do atual. No filho, a syscall devolve 0.
pub fn fork(ctx: &Contexto) -> Result<Pid, i64> {
    let pai = atual();
    let m = pai.mem().ok_or(EINVAL)?;
    let espaco = m.espaco.clonar().ok_or(ENOMEM)?;
    let mut filho = Processo::novo(pai.tgid, pai.nome.clone());
    filho.argumentos = pai.argumentos.clone();
    filho.executavel = pai.executavel.clone();
    (filho.pgid, filho.sid) = (pai.pgid, pai.sid);
    filho.memoria =
        Some(Dividido::novo(Memoria { espaco, brk_inicio: m.brk_inicio, brk: m.brk, mmap_topo: m.mmap_topo }));
    let a = pai.arquivos();
    filho.arquivos = Dividido::novo(Arquivos { lista: a.lista.clone(), fecha_no_exec: a.fecha_no_exec });
    filho.fs = Dividido::novo(InfoFs { diretorio: pai.fs().diretorio.clone(), umask: pai.fs().umask });
    (filho.uid, filho.gid, filho.euid, filho.egid) = (pai.uid, pai.gid, pai.euid, pai.egid);
    filho.fs_base = pai.fs_base;
    salvar_fpu(&mut filho.fpu); // os registradores do pai, como estão agora
    filho.sinais = pai.sinais.duplicar();
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
    // Um programa novo não leva as outras threads junto.
    matar_outras_threads(p);
    imagem.espaco.ativar();
    // O espaço antigo é liberado aqui (se ninguém mais o usar).
    p.memoria = Some(Dividido::novo(Memoria {
        espaco: imagem.espaco,
        brk_inicio: imagem.brk,
        brk: imagem.brk,
        mmap_topo: imagem.mmap_topo,
    }));
    p.nome = caminho.rsplit('/').next().unwrap_or(caminho).to_string();
    p.executavel = caminho.to_string();
    p.argumentos = args;
    p.sinais.ao_executar();
    // Descritores marcados com close-on-exec não passam para o programa novo.
    let a = p.arquivos();
    let fecha = a.fecha_no_exec;
    for (fd, arquivo) in a.lista.iter_mut().enumerate() {
        if fecha & (1 << fd) != 0 {
            *arquivo = None;
        }
    }
    a.fecha_no_exec = 0;
    // Programa novo: FPU limpa e sem TLS.
    *p.fpu = fpu_inicial();
    restaurar_fpu(&p.fpu);
    p.fs_base = 0;
    syscall::definir_fs(0);
    *ctx = Contexto::usuario(imagem.entrada, imagem.rsp);
    Ok(())
}

/// As outras threads do processo de `p`, vivas.
fn outras_threads(p: &Processo) -> Vec<Pid> {
    tabela()
        .processos
        .iter()
        .filter(|q| q.tgid == p.tgid && q.pid != p.pid && q.estado != Estado::Zumbi)
        .map(|q| q.pid)
        .collect()
}

/// Manda as outras threads embora (exit_group, execve): SIGKILL direto, sem volta.
fn matar_outras_threads(p: &Processo) {
    for tid in outras_threads(p) {
        if let Some(q) = buscar(tid) {
            q.sinais.pendentes |= 1 << SIGKILL;
            if matches!(q.estado, Estado::Dormindo(_) | Estado::Parado) {
                q.estado = Estado::Pronto;
            }
        }
    }
}

/// Encerra o processo atual (todas as threads: é o `exit_group`). A primeira
/// thread vira "zumbi" até o pai recolher o status com `wait`.
pub fn sair(status: i32) -> ! {
    let p = atual();
    if p.pid == 1 {
        panic!("o init terminou (status {:#x})", status);
    }
    if !outras_threads(p).is_empty() {
        let saida = p.saida_do_grupo.get();
        if saida.is_none() {
            *saida = Some(status);
        }
        matar_outras_threads(p);
    }
    if !p.eh_lider() {
        sair_thread();
    }
    // Se uma thread chamou exit_group, o código é o dela (a líder morreu de SIGKILL).
    let status = p.saida_do_grupo.get().unwrap_or(status);
    limpar_tid(p);
    p.arquivos = Dividido::novo(Arquivos::default()); // fecha tudo (pipes avisam a outra ponta)
    paginacao::ativar_kernel();
    p.memoria = None; // devolve a memória (se for o último a usá-la)
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
pub const WUNTRACED: u64 = 2;

/// CLONE_CHILD_CLEARTID: zera o tid guardado na memória do programa e acorda
/// quem espera nele com futex (o `pthread_join`).
fn limpar_tid(p: &mut Processo) {
    if p.limpar_tid != 0 {
        if let Some(espaco) = p.espaco() {
            if espaco.copiar_para(p.limpar_tid, &0u32.to_le_bytes()) {
                let _ = futex_acordar(p.limpar_tid, 1);
            }
        }
        p.limpar_tid = 0;
    }
}

/// Uma thread (que não a primeira) termina: some sem deixar zumbi.
fn sair_thread() -> ! {
    let p = atual();
    limpar_tid(p);
    p.arquivos = Dividido::novo(Arquivos::default());
    paginacao::ativar_kernel();
    p.memoria = None;
    p.thread_morta = true;
    p.estado = Estado::Zumbi;
    escalonar();
    unreachable!("uma thread morta voltou a rodar");
}

/// `exit` (60): numa thread extra, termina só ela; na primeira, o processo todo.
pub fn sair_da_thread(status: i32) -> ! {
    if atual().eh_lider() { sair(status) } else { sair_thread() }
}

/// `wait`: espera um filho terminar (ou parar, com WUNTRACED) e devolve (pid, status).
/// `pid` > 0: aquele filho; -1: qualquer um; 0: do meu grupo; < -1: do grupo -pid.
pub fn esperar(pid: i64, opcoes: u64) -> Result<(Pid, i32), i64> {
    loop {
        let (eu, meu_grupo) = (atual().tgid, atual().pgid);
        let t = tabela();
        let serve = |q: &Processo| match pid {
            -1 => true,
            0 => q.pgid == meu_grupo,
            p if p < 0 => q.pgid as i64 == -p,
            p => q.pid as i64 == p,
        };
        let mut tem_filho = false;
        let mut zumbi = None;
        let mut parado = None;
        for (i, q) in t.processos.iter().enumerate().skip(1) {
            if q.pai == eu && q.eh_lider() && serve(q) {
                tem_filho = true;
                if q.estado == Estado::Zumbi {
                    zumbi = Some(i);
                    break;
                }
                if opcoes & WUNTRACED != 0 && q.parada_a_relatar.is_some() {
                    parado = Some(i);
                }
            }
        }
        if let Some(i) = zumbi {
            let q = t.processos.remove(i); // aqui a pilha de kernel dele é liberada
            return Ok((q.pid, q.status));
        }
        if let Some(i) = parado {
            // No formato do Unix: 0x7f no byte baixo = "parado", e o sinal no de cima.
            let q = &mut t.processos[i];
            let numero = q.parada_a_relatar.take().unwrap_or(0);
            return Ok((q.pid, 0x7f | (numero as i32) << 8));
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

/// Manda um sinal para o processo `pid`. Parar, continuar e matar valem
/// para todas as threads dele; os outros vão para uma thread só.
pub fn enviar_sinal(pid: Pid, sinal: u32) -> Result<(), i64> {
    if sinal >= 64 {
        return Err(EINVAL);
    }
    let tgid = buscar(pid).ok_or(ESRCH)?.tgid;
    let todas = sinal::PARADAS & (1 << sinal) != 0 || sinal == sinal::SIGCONT || sinal == SIGKILL;
    if todas {
        let threads: Vec<Pid> = tabela().processos.iter().filter(|q| q.tgid == tgid).map(|q| q.pid).collect();
        for tid in threads {
            enviar_a_tarefa(tid, sinal)?;
        }
        return Ok(());
    }
    enviar_a_tarefa(pid, sinal)
}

/// Sinal para uma thread específica (`tgkill`).
pub fn enviar_a_thread(tgid: Pid, tid: Pid, sinal: u32) -> Result<(), i64> {
    match buscar(tid) {
        Some(t) if t.tgid == tgid || tgid == 0 => enviar_a_tarefa(tid, sinal),
        _ => Err(ESRCH),
    }
}

fn enviar_a_tarefa(pid: Pid, sinal: u32) -> Result<(), i64> {
    let p = buscar(pid).ok_or(ESRCH)?;
    if p.pid == 0 {
        return Err(EPERM);
    }
    if sinal == 0 || p.estado == Estado::Zumbi {
        return Ok(());
    }
    let bit = 1u64 << sinal;
    if sinal == sinal::SIGCONT {
        p.sinais.pendentes &= !sinal::PARADAS;
        if p.estado == Estado::Parado {
            p.estado = Estado::Pronto; // as chamadas em que ele dormia conferem tudo de novo
            p.parada_a_relatar = None;
        }
    } else if sinal::PARADAS & bit != 0 {
        p.sinais.pendentes &= !(1 << sinal::SIGCONT);
    }
    if sinal == SIGKILL && p.estado == Estado::Parado {
        p.estado = Estado::Pronto; // acorda só para morrer
    }
    // Parar outro processo é imediato, mesmo que ele esteja dormindo numa
    // chamada de sistema: ao continuar, ela segue de onde estava (sem EINTR).
    // O processo atual para na volta para o modo usuário.
    if !core::ptr::eq(p, atual()) && p.sinais.pararia(sinal) {
        if p.estado != Estado::Parado {
            parar(p, sinal);
        }
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

/// Os processos (vivos) de um grupo.
fn membros(pgid: Pid) -> Vec<Pid> {
    tabela()
        .processos
        .iter()
        .skip(1)
        .filter(|p| p.pgid == pgid && p.eh_lider() && p.estado != Estado::Zumbi)
        .map(|p| p.pid)
        .collect()
}

/// Antes de alguém definir um grupo em primeiro plano: o sinal vai para todos
/// (menos o ocioso). Quem ignora o sinal (o init) sobrevive.
pub fn sinal_para_grupo_todos(sinal: u32) {
    let pids: Vec<Pid> = tabela().processos.iter().skip(1).filter(|p| p.eh_lider()).map(|p| p.pid).collect();
    for pid in pids {
        let _ = enviar_sinal(pid, sinal);
    }
}

/// Sinal para um grupo inteiro, sem conferir permissão (é o terminal mandando).
pub fn sinal_para_grupo(pgid: Pid, sinal: u32) {
    for pid in membros(pgid) {
        let _ = enviar_sinal(pid, sinal);
    }
}

/// O `kill` do Unix: `pid` > 0 é um processo; 0, o meu grupo; -1, todos
/// (menos o init); < -1, o grupo -pid. Confere permissão em cada um.
pub fn sinalizar(pid: i64, sinal: u32) -> Result<(), i64> {
    if pid > 0 {
        pode_sinalizar(pid as Pid)?;
        return enviar_sinal(pid as Pid, sinal);
    }
    let alvos: Vec<Pid> = match pid {
        0 => membros(atual().pgid),
        -1 => tabela().processos.iter().skip(1).filter(|p| p.pid != 1).map(|p| p.pid).collect(),
        p => membros((-p) as Pid),
    };
    let mut algum = false;
    let mut erro = ESRCH;
    for alvo in alvos {
        match pode_sinalizar(alvo) {
            Ok(()) => {
                let _ = enviar_sinal(alvo, sinal);
                algum = true;
            }
            Err(e) => erro = e,
        }
    }
    if algum { Ok(()) } else { Err(erro) }
}

/// `setpgid`: põe um processo (eu ou um filho) num grupo. 0 = "o próprio pid".
pub fn definir_grupo(pid: Pid, pgid: Pid) -> Result<(), i64> {
    let (eu, minha_sessao) = (atual().pid, atual().sid);
    let pid = if pid == 0 { eu } else { pid };
    let pgid = if pgid == 0 { pid } else { pgid };
    let grupo_existe = tabela().processos.iter().any(|q| q.pgid == pgid && q.sid == minha_sessao);
    let alvo = buscar(pid).ok_or(ESRCH)?;
    if alvo.pid != eu && alvo.pai != eu {
        return Err(ESRCH);
    }
    if alvo.sid == alvo.pid || alvo.sid != minha_sessao {
        return Err(EPERM); // líder de sessão não muda de grupo
    }
    if pgid != pid && !grupo_existe {
        return Err(EPERM);
    }
    alvo.pgid = pgid;
    Ok(())
}

/// `setsid`: começa uma sessão nova (e um grupo novo), com este processo como líder.
pub fn nova_sessao() -> Result<Pid, i64> {
    let p = atual();
    if p.pgid == p.pid {
        return Err(EPERM);
    }
    (p.sid, p.pgid) = (p.pid, p.pid);
    Ok(p.pid)
}

/// (pgid, sid) de um processo (0 = o atual).
pub fn grupo_e_sessao(pid: Pid) -> Result<(Pid, Pid), i64> {
    let p = if pid == 0 { atual() } else { buscar(pid).ok_or(ESRCH)? };
    Ok((p.pgid, p.sid))
}

pub fn tem_sinal_pendente() -> bool {
    atual().sinais.entregaveis() != 0
}

/// Um page fault do programa: página ainda não materializada, copy-on-write
/// ou acesso inválido?
pub fn falha_de_pagina(endereco: u64, escrita: bool, execucao: bool) -> Falha {
    match atual().espaco() {
        Some(espaco) => espaco.tratar_falha(endereco, escrita, execucao),
        None => Falha::Invalida,
    }
}

/// Acabou a memória física no meio de um page fault: quem pediu é encerrado
/// (um "OOM killer" bem simples), e o resto do sistema segue.
pub fn sem_memoria() -> ! {
    let p = atual();
    println!("[{}] {}: sem memória: encerrado pelo kernel", p.pid, p.nome);
    sair(SIGKILL as i32)
}

/// Uma exceção da CPU em modo usuário. Se o programa tiver um tratador para
/// o sinal, ele é chamado; senão, o programa morre (e o kernel segue em pé).
pub fn falha_do_programa(descricao: core::fmt::Arguments, numero: u32) {
    let p = atual();
    let acao = p.sinais.acoes()[numero as usize];
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
    pub tgid: Pid,
    pub pai: Pid,
    pub pgid: Pid,
    pub sid: Pid,
    pub uid: u32,
    pub gid: u32,
    pub euid: u32,
    pub egid: u32,
    pub threads: u32,
    pub estado: u32,
    pub memoria_kib: u64,
    pub paginas: u64,
    pub tiques: u64,
    pub inicio_ms: u64,
    pub nome: String,
    pub argumentos: Vec<String>,
    pub diretorio: String,
    pub executavel: String,
}

fn resumir(p: &Processo) -> Resumo {
    let paginas = p.espaco().map_or(0, |e| e.paginas() as u64);
    let threads = tabela().processos.iter().filter(|q| q.tgid == p.tgid && !q.thread_morta).count();
    Resumo {
        pid: p.pid,
        tgid: p.tgid,
        pai: p.pai,
        pgid: p.pgid,
        sid: p.sid,
        uid: p.uid,
        gid: p.gid,
        euid: p.euid,
        egid: p.egid,
        threads: threads as u32,
        estado: match p.estado {
            Estado::Pronto => 0,
            Estado::Rodando => 1,
            Estado::Dormindo(_) => 2,
            Estado::Zumbi => 3,
            Estado::Parado => 4,
        },
        memoria_kib: paginas * 4 + p.pilha_kernel.len() as u64 / 1024,
        paginas,
        tiques: p.tiques,
        inicio_ms: p.inicio_ms,
        nome: p.nome.clone(),
        argumentos: p.argumentos.clone(),
        diretorio: p.fs().diretorio.clone(),
        executavel: p.executavel.clone(),
    }
}

/// Os processos (só a primeira thread de cada um).
pub fn listar() -> Vec<Resumo> {
    tabela().processos.iter().filter(|p| p.eh_lider()).map(|p| resumir(p)).collect()
}

pub fn resumo(pid: Pid) -> Option<Resumo> {
    tabela().processos.iter().find(|p| p.pid == pid && p.eh_lider()).map(|p| resumir(p))
}

// -----------------------------------------------------------------------------
// Threads e futex
// -----------------------------------------------------------------------------

pub const CLONE_VM: u64 = 0x100;
const CLONE_FS: u64 = 0x200;
const CLONE_FILES: u64 = 0x400;
const CLONE_SIGHAND: u64 = 0x800;
const CLONE_SETTLS: u64 = 0x0008_0000;
const CLONE_PARENT_SETTID: u64 = 0x0010_0000;
const CLONE_CHILD_CLEARTID: u64 = 0x0020_0000;
const CLONE_CHILD_SETTID: u64 = 0x0100_0000;

/// `clone` com CLONE_THREAD: uma tarefa nova no mesmo processo, começando
/// em `pilha`, dividindo o que as flags pedirem (o `pthread_create` pede tudo).
pub fn criar_thread(ctx: &Contexto, flags: u64, pilha: u64, ptid: u64, ctid: u64, tls: u64) -> Result<Pid, i64> {
    if flags & CLONE_VM == 0 || flags & CLONE_SIGHAND == 0 {
        return Err(EINVAL); // uma thread sem a memória e os tratadores do processo não faz sentido
    }
    let pai = atual();
    let mut t = Processo::novo(pai.pai, pai.nome.clone());
    t.tgid = pai.tgid;
    (t.pgid, t.sid) = (pai.pgid, pai.sid);
    (t.uid, t.gid, t.euid, t.egid) = (pai.uid, pai.gid, pai.euid, pai.egid);
    t.argumentos = pai.argumentos.clone();
    t.executavel = pai.executavel.clone();
    t.memoria = pai.memoria.clone();
    t.arquivos = if flags & CLONE_FILES != 0 {
        pai.arquivos.clone()
    } else {
        let a = pai.arquivos();
        Dividido::novo(Arquivos { lista: a.lista.clone(), fecha_no_exec: a.fecha_no_exec })
    };
    t.fs = if flags & CLONE_FS != 0 {
        pai.fs.clone()
    } else {
        Dividido::novo(InfoFs { diretorio: pai.fs().diretorio.clone(), umask: pai.fs().umask })
    };
    t.saida_do_grupo = pai.saida_do_grupo.clone();
    t.sinais = pai.sinais.para_thread();
    t.fs_base = if flags & CLONE_SETTLS != 0 { tls } else { pai.fs_base };
    salvar_fpu(&mut t.fpu);
    unsafe {
        *t.contexto() = *ctx;
        (*t.contexto()).rax = 0;
        if pilha != 0 {
            (*t.contexto()).rsp = pilha;
        }
    }
    let tid = t.pid;
    let espaco = pai.espaco().ok_or(EINVAL)?;
    if flags & CLONE_PARENT_SETTID != 0 {
        espaco.copiar_para(ptid, &tid.to_le_bytes());
    }
    if flags & CLONE_CHILD_SETTID != 0 {
        espaco.copiar_para(ctid, &tid.to_le_bytes()); // a memória é a mesma
    }
    if flags & CLONE_CHILD_CLEARTID != 0 {
        t.limpar_tid = ctid;
    }
    t.preparar_primeira_execucao();
    tabela().processos.push(t);
    Ok(tid)
}

/// A chave de um futex é o endereço *físico* da palavra: duas threads (ou
/// dois processos com memória compartilhada) que olham a mesma palavra
/// caem no mesmo canal, mesmo vendo-a em endereços virtuais diferentes.
fn canal_do_futex(endereco: u64) -> Result<usize, i64> {
    if endereco & 3 != 0 {
        return Err(EINVAL);
    }
    let espaco = atual().espaco().ok_or(EFAULT)?;
    let fisico = espaco.traduzir(endereco).ok_or(EFAULT)?;
    Ok((1usize << 63) | fisico as usize)
}

/// FUTEX_WAIT: se a palavra ainda vale `esperado`, dorme até um FUTEX_WAKE
/// (ou até o prazo, ou um sinal). É a base dos mutexes e variáveis de
/// condição da libc: na maior parte do tempo eles nem chamam o kernel.
pub fn futex_esperar(endereco: u64, esperado: u32, prazo_ms: Option<u64>) -> Result<u64, i64> {
    let canal = canal_do_futex(endereco)?;
    let palavra = atual().espaco().and_then(|e| e.copiar_de(endereco, 4)).ok_or(EFAULT)?;
    if u32::from_le_bytes(palavra.try_into().unwrap()) != esperado {
        return Err(EAGAIN);
    }
    if tem_sinal_pendente() {
        return Err(EINTR);
    }
    let limite = prazo_ms.map(|ms| pit::milissegundos() + ms.max(1));
    let p = atual();
    p.acordado_por_futex = false;
    p.prazo_ms = limite.unwrap_or(0);
    dormir(canal);
    let p = atual();
    p.prazo_ms = 0;
    if p.acordado_por_futex {
        Ok(0)
    } else if tem_sinal_pendente() {
        Err(EINTR)
    } else if limite.is_some_and(|l| pit::milissegundos() >= l) {
        Err(ETIMEDOUT)
    } else {
        Ok(0) // acordou por outro motivo: a libc confere a palavra e tenta de novo
    }
}

/// FUTEX_WAKE: acorda até `quantos` que esperam nesta palavra. Devolve quantos acordou.
pub fn futex_acordar(endereco: u64, quantos: u64) -> Result<u64, i64> {
    let canal = canal_do_futex(endereco)?;
    let mut acordados = 0;
    for p in tabela().processos.iter_mut() {
        if acordados >= quantos {
            break;
        }
        if p.estado == Estado::Dormindo(canal) {
            p.estado = Estado::Pronto;
            p.acordado_por_futex = true;
            acordados += 1;
        }
    }
    Ok(acordados)
}

/// FUTEX_REQUEUE: acorda até `acordar` e muda até `mover` dos outros para
/// esperar noutra palavra (evita acordar todo mundo num broadcast).
pub fn futex_mudar_fila(endereco: u64, acordar: u64, mover: u64, destino: u64, esperado: Option<u32>) -> Result<u64, i64> {
    if let Some(v) = esperado {
        let palavra = atual().espaco().and_then(|e| e.copiar_de(endereco, 4)).ok_or(EFAULT)?;
        if u32::from_le_bytes(palavra.try_into().unwrap()) != v {
            return Err(EAGAIN);
        }
    }
    let (origem, novo) = (canal_do_futex(endereco)?, canal_do_futex(destino)?);
    let acordados = futex_acordar(endereco, acordar)?;
    let mut movidos = 0;
    for p in tabela().processos.iter_mut() {
        if movidos >= mover {
            break;
        }
        if p.estado == Estado::Dormindo(origem) {
            p.estado = Estado::Dormindo(novo);
            movidos += 1;
        }
    }
    Ok(acordados + movidos)
}
