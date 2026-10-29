//! Chamadas de sistema. Mesmos números e convenções do Linux x86_64:
//! número em RAX, argumentos em RDI, RSI, RDX, R10, R8, R9, resposta em RAX
//! (negativa em caso de erro).

use alloc::string::String;
use alloc::vec::Vec;
use core::arch::asm;
use core::fmt;

#[inline(always)]
unsafe fn syscall(numero: u64, a: u64, b: u64, c: u64, d: u64) -> i64 {
    let resposta: i64;
    unsafe {
        asm!(
            "syscall",
            inlateout("rax") numero as i64 => resposta,
            in("rdi") a, in("rsi") b, in("rdx") c, in("r10") d,
            lateout("rcx") _, lateout("r11") _, // a instrução syscall usa os dois
            options(nostack),
        );
    }
    resposta
}

/// Um código de erro do kernel (errno).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Erro(pub i64);

pub const ENOENT: Erro = Erro(2);
pub const EPERM: Erro = Erro(1);
pub const EINTR: Erro = Erro(4);
pub const EACCES: Erro = Erro(13);
pub const ECHILD: Erro = Erro(10);
pub const EEXIST: Erro = Erro(17);
pub const EXDEV: Erro = Erro(18);
pub const ELOOP: Erro = Erro(40);

impl Erro {
    pub fn descricao(self) -> &'static str {
        match self.0 {
            1 => "operação não permitida",
            2 => "arquivo ou diretório não encontrado",
            3 => "processo não existe",
            4 => "interrompido",
            8 => "não é um executável válido",
            9 => "descritor de arquivo inválido",
            10 => "nenhum processo filho",
            5 => "erro de entrada/saída",
            12 => "sem memória",
            13 => "permissão negada",
            14 => "endereço inválido",
            16 => "recurso ocupado",
            17 => "já existe",
            18 => "não dá para mover entre sistemas de arquivos diferentes",
            20 => "não é um diretório",
            21 => "é um diretório",
            22 => "argumento inválido",
            24 => "arquivos abertos demais",
            25 => "não é um terminal",
            27 => "arquivo grande demais",
            28 => "sem espaço no disco",
            29 => "não dá para posicionar",
            32 => "pipe sem leitor",
            36 => "nome longo demais",
            38 => "não implementado",
            39 => "diretório não está vazio",
            40 => "links simbólicos demais (um laço?)",
            _ => "erro desconhecido",
        }
    }
}

impl fmt::Display for Erro {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.write_str(self.descricao())
    }
}

pub type Resultado<T> = Result<T, Erro>;

fn conferir(resposta: i64) -> Resultado<u64> {
    if resposta < 0 { Err(Erro(-resposta)) } else { Ok(resposta as u64) }
}

/// O kernel quer strings terminadas em zero, como em C.
fn string_c(s: &str) -> Vec<u8> {
    let mut v = Vec::with_capacity(s.len() + 1);
    v.extend_from_slice(s.as_bytes());
    v.push(0);
    v
}

// -----------------------------------------------------------------------------
// Arquivos
// -----------------------------------------------------------------------------

pub const O_RDONLY: u64 = 0;
pub const O_WRONLY: u64 = 1;
pub const O_RDWR: u64 = 2;
pub const O_CREAT: u64 = 0o100;
pub const O_TRUNC: u64 = 0o1000;
pub const O_APPEND: u64 = 0o2000;
pub const O_DIRECTORY: u64 = 0o200000;

pub fn ler(fd: i32, buf: &mut [u8]) -> Resultado<usize> {
    conferir(unsafe { syscall(0, fd as u64, buf.as_mut_ptr() as u64, buf.len() as u64, 0) }).map(|n| n as usize)
}

pub fn escrever(fd: i32, buf: &[u8]) -> Resultado<usize> {
    conferir(unsafe { syscall(1, fd as u64, buf.as_ptr() as u64, buf.len() as u64, 0) }).map(|n| n as usize)
}

/// Escreve tudo, mesmo que o kernel aceite aos pedaços.
pub fn escrever_tudo(fd: i32, mut buf: &[u8]) -> Resultado<()> {
    while !buf.is_empty() {
        let n = escrever(fd, buf)?;
        buf = &buf[n..];
    }
    Ok(())
}

pub fn abrir(caminho: &str, flags: u64) -> Resultado<i32> {
    abrir_com_permissoes(caminho, flags, 0o644)
}

/// Como `abrir`, dizendo as permissões do arquivo se ele for criado.
pub fn abrir_com_permissoes(caminho: &str, flags: u64, permissoes: u32) -> Resultado<i32> {
    let c = string_c(caminho);
    conferir(unsafe { syscall(2, c.as_ptr() as u64, flags, permissoes as u64, 0) }).map(|fd| fd as i32)
}

pub fn mudar_permissoes(caminho: &str, permissoes: u32) -> Resultado<()> {
    let c = string_c(caminho);
    conferir(unsafe { syscall(90, c.as_ptr() as u64, permissoes as u64, 0, 0) }).map(|_| ())
}

/// `None` mantém o atual.
pub fn mudar_dono(caminho: &str, uid: Option<u32>, gid: Option<u32>) -> Resultado<()> {
    let c = string_c(caminho);
    let (u, g) = (uid.unwrap_or(u32::MAX) as u64, gid.unwrap_or(u32::MAX) as u64);
    conferir(unsafe { syscall(92, c.as_ptr() as u64, u, g, 0) }).map(|_| ())
}

pub fn uid() -> u32 {
    unsafe { syscall(102, 0, 0, 0, 0) as u32 }
}
pub fn gid() -> u32 {
    unsafe { syscall(104, 0, 0, 0, 0) as u32 }
}
pub fn euid() -> u32 {
    unsafe { syscall(107, 0, 0, 0, 0) as u32 }
}
pub fn egid() -> u32 {
    unsafe { syscall(108, 0, 0, 0, 0) as u32 }
}
pub fn mudar_uid(uid: u32) -> Resultado<()> {
    conferir(unsafe { syscall(105, uid as u64, 0, 0, 0) }).map(|_| ())
}
pub fn mudar_gid(gid: u32) -> Resultado<()> {
    conferir(unsafe { syscall(106, gid as u64, 0, 0, 0) }).map(|_| ())
}

/// O descritor é um terminal? (o `isatty` do C)
pub fn eh_terminal(fd: i32) -> bool {
    let mut termios = [0u8; 60];
    unsafe { syscall(16, fd as u64, 0x5401, termios.as_mut_ptr() as u64, 0) >= 0 }
}

/// Liga ou desliga o eco do terminal (para digitar senhas).
pub fn eco(fd: i32, ligado: bool) -> Resultado<()> {
    let mut termios = [0u8; 60];
    conferir(unsafe { syscall(16, fd as u64, 0x5401, termios.as_mut_ptr() as u64, 0) })?; // TCGETS
    let mut lflag = u32::from_le_bytes(termios[12..16].try_into().unwrap());
    if ligado { lflag |= 0o10 } else { lflag &= !0o10 }
    termios[12..16].copy_from_slice(&lflag.to_le_bytes());
    conferir(unsafe { syscall(16, fd as u64, 0x5402, termios.as_ptr() as u64, 0) }).map(|_| ()) // TCSETS
}

pub fn fechar(fd: i32) -> Resultado<()> {
    conferir(unsafe { syscall(3, fd as u64, 0, 0, 0) }).map(|_| ())
}

pub fn posicionar(fd: i32, deslocamento: i64, referencia: u64) -> Resultado<u64> {
    conferir(unsafe { syscall(8, fd as u64, deslocamento as u64, referencia, 0) })
}

pub fn duplicar_para(antigo: i32, novo: i32) -> Resultado<()> {
    conferir(unsafe { syscall(33, antigo as u64, novo as u64, 0, 0) }).map(|_| ())
}

/// (ponta de leitura, ponta de escrita)
pub fn pipe() -> Resultado<(i32, i32)> {
    let mut fds = [0i32; 2];
    conferir(unsafe { syscall(22, fds.as_mut_ptr() as u64, 0, 0, 0) })?;
    Ok((fds[0], fds[1]))
}

pub struct Stat {
    pub inode: u64,
    /// Tipo (bits altos) + permissões (os 12 bits baixos).
    pub modo: u32,
    pub uid: u32,
    pub gid: u32,
    pub tamanho: u64,
    /// Tempo Unix da última modificação (0 = desconhecido).
    pub modificado: u64,
}

impl Stat {
    pub fn eh_diretorio(&self) -> bool {
        self.modo & 0o170000 == 0o040000
    }
    pub fn eh_dispositivo(&self) -> bool {
        self.modo & 0o170000 == 0o020000
    }
    pub fn eh_link(&self) -> bool {
        self.modo & 0o170000 == 0o120000
    }
}

fn decodificar_stat(b: &[u8; 144]) -> Stat {
    Stat {
        inode: u64::from_le_bytes(b[8..16].try_into().unwrap()),
        modo: u32::from_le_bytes(b[24..28].try_into().unwrap()),
        uid: u32::from_le_bytes(b[28..32].try_into().unwrap()),
        gid: u32::from_le_bytes(b[32..36].try_into().unwrap()),
        tamanho: u64::from_le_bytes(b[48..56].try_into().unwrap()),
        modificado: u64::from_le_bytes(b[88..96].try_into().unwrap()),
    }
}

pub fn stat(caminho: &str) -> Resultado<Stat> {
    let c = string_c(caminho);
    let mut b = [0u8; 144];
    conferir(unsafe { syscall(4, c.as_ptr() as u64, b.as_mut_ptr() as u64, 0, 0) })?;
    Ok(decodificar_stat(&b))
}

/// Como `stat`, mas se o caminho for um link simbólico, fala do próprio link.
pub fn lstat(caminho: &str) -> Resultado<Stat> {
    let c = string_c(caminho);
    let mut b = [0u8; 144];
    conferir(unsafe { syscall(6, c.as_ptr() as u64, b.as_mut_ptr() as u64, 0, 0) })?;
    Ok(decodificar_stat(&b))
}

/// Cria `caminho` como um link simbólico para `alvo`.
pub fn criar_link_simbolico(alvo: &str, caminho: &str) -> Resultado<()> {
    let (a, c) = (string_c(alvo), string_c(caminho));
    conferir(unsafe { syscall(88, a.as_ptr() as u64, c.as_ptr() as u64, 0, 0) }).map(|_| ())
}

/// Cria `novo` como mais um nome (link físico) para `existente`.
pub fn criar_link(existente: &str, novo: &str) -> Resultado<()> {
    let (a, c) = (string_c(existente), string_c(novo));
    conferir(unsafe { syscall(86, a.as_ptr() as u64, c.as_ptr() as u64, 0, 0) }).map(|_| ())
}

/// Para onde um link simbólico aponta.
pub fn ler_link(caminho: &str) -> Resultado<String> {
    let c = string_c(caminho);
    let mut buf = [0u8; 4096];
    let n = conferir(unsafe { syscall(89, c.as_ptr() as u64, buf.as_mut_ptr() as u64, buf.len() as u64, 0) })?;
    Ok(String::from_utf8_lossy(&buf[..n as usize]).into_owned())
}

pub fn fstat(fd: i32) -> Resultado<Stat> {
    let mut b = [0u8; 144];
    conferir(unsafe { syscall(5, fd as u64, b.as_mut_ptr() as u64, 0, 0) })?;
    Ok(decodificar_stat(&b))
}

pub struct EntradaDiretorio {
    pub nome: String,
    pub tipo: u8, // 4 = diretório, 8 = arquivo, 2 = dispositivo
}

/// Lista um diretório (sem `.` e `..`).
pub fn listar_diretorio(caminho: &str) -> Resultado<Vec<EntradaDiretorio>> {
    let fd = abrir(caminho, O_RDONLY | O_DIRECTORY)?;
    let mut entradas = Vec::new();
    let mut buf = [0u8; 2048];
    loop {
        let lidos = match conferir(unsafe { syscall(217, fd as u64, buf.as_mut_ptr() as u64, buf.len() as u64, 0) }) {
            Ok(0) => break,
            Ok(n) => n as usize,
            Err(e) => {
                let _ = fechar(fd);
                return Err(e);
            }
        };
        let mut i = 0;
        while i < lidos {
            let tamanho = u16::from_le_bytes([buf[i + 16], buf[i + 17]]) as usize;
            let tipo = buf[i + 18];
            let nome = &buf[i + 19..i + tamanho];
            let fim = nome.iter().position(|&b| b == 0).unwrap_or(nome.len());
            let nome = String::from_utf8_lossy(&nome[..fim]).into_owned();
            if nome != "." && nome != ".." {
                entradas.push(EntradaDiretorio { nome, tipo });
            }
            i += tamanho;
        }
    }
    let _ = fechar(fd);
    Ok(entradas)
}

pub fn criar_diretorio(caminho: &str) -> Resultado<()> {
    let c = string_c(caminho);
    conferir(unsafe { syscall(83, c.as_ptr() as u64, 0o755, 0, 0) }).map(|_| ())
}

pub fn remover_diretorio(caminho: &str) -> Resultado<()> {
    let c = string_c(caminho);
    conferir(unsafe { syscall(84, c.as_ptr() as u64, 0, 0, 0) }).map(|_| ())
}

pub fn remover(caminho: &str) -> Resultado<()> {
    let c = string_c(caminho);
    conferir(unsafe { syscall(87, c.as_ptr() as u64, 0, 0, 0) }).map(|_| ())
}

pub fn renomear(de: &str, para: &str) -> Resultado<()> {
    let (a, b) = (string_c(de), string_c(para));
    conferir(unsafe { syscall(82, a.as_ptr() as u64, b.as_ptr() as u64, 0, 0) }).map(|_| ())
}

pub fn mudar_diretorio(caminho: &str) -> Resultado<()> {
    let c = string_c(caminho);
    conferir(unsafe { syscall(80, c.as_ptr() as u64, 0, 0, 0) }).map(|_| ())
}

pub fn diretorio_atual() -> String {
    let mut buf = [0u8; 1024];
    match conferir(unsafe { syscall(79, buf.as_mut_ptr() as u64, buf.len() as u64, 0, 0) }) {
        Ok(n) => String::from_utf8_lossy(&buf[..(n as usize).saturating_sub(1)]).into_owned(),
        Err(_) => String::from("?"),
    }
}

/// Lê um arquivo inteiro.
pub fn ler_arquivo(caminho: &str) -> Resultado<Vec<u8>> {
    let fd = abrir(caminho, O_RDONLY)?;
    let mut dados = Vec::new();
    let mut buf = [0u8; 4096];
    loop {
        match ler(fd, &mut buf) {
            Ok(0) => break,
            Ok(n) => dados.extend_from_slice(&buf[..n]),
            Err(e) => {
                let _ = fechar(fd);
                return Err(e);
            }
        }
    }
    let _ = fechar(fd);
    Ok(dados)
}

// -----------------------------------------------------------------------------
// Processos
// -----------------------------------------------------------------------------

pub fn sair(codigo: i32) -> ! {
    unsafe { syscall(60, codigo as u64, 0, 0, 0) };
    unreachable!()
}

/// Devolve 0 no processo filho e o pid do filho no pai.
pub fn fork() -> Resultado<i32> {
    conferir(unsafe { syscall(57, 0, 0, 0, 0) }).map(|p| p as i32)
}

/// Troca o programa (repassando as variáveis de ambiente). Só volta se der errado.
pub fn executar(caminho: &str, args: &[&str]) -> Erro {
    executar_com_ambiente(caminho, args, crate::ambiente())
}

/// Como `executar`, escolhendo as variáveis de ambiente ("NOME=valor").
pub fn executar_com_ambiente<S: AsRef<str>>(caminho: &str, args: &[&str], ambiente: &[S]) -> Erro {
    let c = string_c(caminho);
    let vetor = |lista: Vec<Vec<u8>>| -> (Vec<Vec<u8>>, Vec<u64>) {
        let mut ponteiros: Vec<u64> = lista.iter().map(|s| s.as_ptr() as u64).collect();
        ponteiros.push(0);
        (lista, ponteiros)
    };
    let (_a, argv) = vetor(args.iter().map(|a| string_c(a)).collect());
    let (_e, envp) = vetor(ambiente.iter().map(|a| string_c(a.as_ref())).collect());
    let r = unsafe { syscall(59, c.as_ptr() as u64, argv.as_ptr() as u64, envp.as_ptr() as u64, 0) };
    Erro(-r)
}

/// Espera um filho (`-1` = qualquer um). Devolve (pid, status).
pub fn esperar(pid: i32) -> Resultado<(i32, i32)> {
    let mut status = 0i32;
    let p = conferir(unsafe { syscall(61, pid as u64, &raw mut status as u64, 0, 0) })?;
    Ok((p as i32, status))
}

/// Como `esperar(-1)`, mas não bloqueia: `Ok(None)` se nenhum filho terminou.
pub fn recolher_terminado() -> Resultado<Option<(i32, i32)>> {
    esperar_com(-1, WNOHANG)
}

pub const WNOHANG: u64 = 1;
/// Também avisa quando um filho *para* (Ctrl+Z), não só quando termina.
pub const WUNTRACED: u64 = 2;

/// O `waitpid` completo. `pid`: > 0 um filho, -1 qualquer, -N o grupo N.
/// `Ok(None)` só com WNOHANG, se ninguém mudou de estado.
pub fn esperar_com(pid: i32, opcoes: u64) -> Resultado<Option<(i32, i32)>> {
    let mut status = 0i32;
    let p = conferir(unsafe { syscall(61, pid as i64 as u64, &raw mut status as u64, opcoes, 0) })?;
    Ok(if p == 0 { None } else { Some((p as i32, status)) })
}

/// `Some(sinal)` se o status diz que o processo *parou* (WUNTRACED).
pub fn sinal_que_parou(status: i32) -> Option<i32> {
    if status & 0xFF == 0x7F { Some((status >> 8) & 0xFF) } else { None }
}

// ------------------------------------------------- grupos de processos e terminal

/// Põe o processo `pid` (0 = eu) no grupo `pgid` (0 = um grupo novo, com o número dele).
pub fn mudar_grupo(pid: i32, pgid: i32) -> Resultado<()> {
    conferir(unsafe { syscall(109, pid as u64, pgid as u64, 0, 0) }).map(|_| ())
}

pub fn grupo() -> i32 {
    unsafe { syscall(111, 0, 0, 0, 0) as i32 }
}

/// Começa uma sessão nova, com este processo como líder.
pub fn nova_sessao() -> Resultado<i32> {
    conferir(unsafe { syscall(112, 0, 0, 0, 0) }).map(|s| s as i32)
}

/// Entrega o terminal (teclado e sinais do teclado) a um grupo.
pub fn dar_terminal(fd: i32, pgid: i32) -> Resultado<()> {
    let grupo = pgid;
    conferir(unsafe { syscall(16, fd as u64, 0x5410, &raw const grupo as u64, 0) }).map(|_| ())
}

/// `Some(código)` se o processo terminou chamando `exit`.
pub fn codigo_de_saida(status: i32) -> Option<i32> {
    if status & 0x7F == 0 { Some((status >> 8) & 0xFF) } else { None }
}

/// `Some(sinal)` se o processo foi morto por um sinal.
pub fn sinal_que_matou(status: i32) -> Option<i32> {
    if status & 0x7F != 0 { Some(status & 0x7F) } else { None }
}

pub const SIGINT: i32 = 2;
pub const SIGKILL: i32 = 9;
pub const SIGUSR1: i32 = 10;
pub const SIGSEGV: i32 = 11;
pub const SIGUSR2: i32 = 12;
pub const SIGALRM: i32 = 14;
pub const SIGTERM: i32 = 15;
pub const SIGCHLD: i32 = 17;
pub const SIGCONT: i32 = 18;
pub const SIGSTOP: i32 = 19;
pub const SIGTSTP: i32 = 20;
pub const SIGTTIN: i32 = 21;
pub const SIGTTOU: i32 = 22;

pub fn nome_do_sinal(sinal: i32) -> &'static str {
    match sinal {
        2 => "interrompido",
        4 => "instrução ilegal",
        8 => "erro aritmético",
        9 => "morto",
        11 => "falha de segmentação",
        10 => "sinal do usuário 1",
        12 => "sinal do usuário 2",
        13 => "pipe quebrado",
        14 => "alarme",
        15 => "terminado",
        18 => "continuado",
        19 => "parado (SIGSTOP)",
        20 => "parado",
        21 => "parado (leu o terminal em segundo plano)",
        22 => "parado (escreveu no terminal em segundo plano)",
        _ => "sinal",
    }
}

pub fn matar(pid: i32, sinal: i32) -> Resultado<()> {
    conferir(unsafe { syscall(62, pid as u64, sinal as u64, 0, 0) }).map(|_| ())
}

const SA_RESTORER: u64 = 0x0400_0000;
const SA_RESTART: u64 = 0x1000_0000;

/// Para onde o tratador de sinal "retorna": pede ao kernel para restaurar
/// os registradores de antes do sinal (é o mesmo código da musl e da glibc).
#[unsafe(naked)]
unsafe extern "C" fn restaurador() -> ! {
    core::arch::naked_asm!("mov eax, 15", "syscall", "ud2")
}

/// Uma função que trata um sinal. Recebe o número do sinal.
pub type Tratador = extern "C" fn(i32);

fn sigaction(sinal: i32, tratador: u64, flags: u64) -> Resultado<()> {
    let acao = [tratador, flags | SA_RESTORER, restaurador as *const () as u64, 0];
    conferir(unsafe { syscall(13, sinal as u64, acao.as_ptr() as u64, 0, 8) }).map(|_| ())
}

/// Ignora (ou volta ao comportamento padrão de) um sinal.
pub fn ignorar_sinal(sinal: i32, ignorar: bool) {
    let _ = sigaction(sinal, ignorar as u64, 0);
}

/// Chama `tratador` quando o sinal chegar. Com `reiniciar`, chamadas de
/// sistema interrompidas por ele recomeçam sozinhas (SA_RESTART).
pub fn tratar_sinal(sinal: i32, tratador: Tratador, reiniciar: bool) -> Resultado<()> {
    sigaction(sinal, tratador as usize as u64, if reiniciar { SA_RESTART } else { 0 })
}

pub const SIG_BLOCK: u64 = 0;
pub const SIG_UNBLOCK: u64 = 1;
pub const SIG_SETMASK: u64 = 2;

/// Muda a máscara de sinais bloqueados. Devolve a anterior.
pub fn bloquear_sinais(como: u64, mascara: u64) -> Resultado<u64> {
    let mut antiga = 0u64;
    conferir(unsafe { syscall(14, como, &raw const mascara as u64, &raw mut antiga as u64, 8) })?;
    Ok(antiga)
}

/// SIGALRM daqui a `segundos` (0 cancela).
pub fn alarme(segundos: u64) -> u64 {
    unsafe { syscall(37, segundos, 0, 0, 0) as u64 }
}

/// Dorme até chegar um sinal.
pub fn pausar() {
    unsafe { syscall(34, 0, 0, 0, 0) };
}

pub fn pid() -> i32 {
    unsafe { syscall(39, 0, 0, 0, 0) as i32 }
}

pub fn pid_do_pai() -> i32 {
    unsafe { syscall(110, 0, 0, 0, 0) as i32 }
}

pub fn brk(endereco: usize) -> usize {
    unsafe { syscall(12, endereco as u64, 0, 0, 0) as usize }
}

pub fn dormir_ms(ms: u64) -> Resultado<()> {
    let tempo = [ms / 1000, (ms % 1000) * 1_000_000];
    conferir(unsafe { syscall(35, tempo.as_ptr() as u64, 0, 0, 0) }).map(|_| ())
}

/// Data e hora atuais (tempo Unix: segundos desde 1970-01-01 UTC).
pub fn hora_atual() -> u64 {
    let mut t = [0u64; 2];
    unsafe { syscall(228, 0, t.as_mut_ptr() as u64, 0, 0) };
    t[0]
}

/// Milissegundos desde que o sistema ligou.
pub fn tempo_ms() -> u64 {
    let mut t = [0u64; 2];
    unsafe { syscall(228, 1, t.as_mut_ptr() as u64, 0, 0) };
    t[0] * 1000 + t[1] / 1_000_000
}

pub struct Uname {
    pub sistema: String,
    pub maquina: String,
    pub versao: String,
    pub compilacao: String,
    pub arquitetura: String,
}

pub fn uname() -> Uname {
    let mut b = [0u8; 6 * 65];
    unsafe { syscall(63, b.as_mut_ptr() as u64, 0, 0, 0) };
    let campo = |i: usize| {
        let c = &b[i * 65..(i + 1) * 65];
        let fim = c.iter().position(|&x| x == 0).unwrap_or(65);
        String::from_utf8_lossy(&c[..fim]).into_owned()
    };
    Uname { sistema: campo(0), maquina: campo(1), versao: campo(2), compilacao: campo(3), arquitetura: campo(4) }
}

pub fn desligar() -> Erro {
    Erro(-unsafe { syscall(169, 0xfee1dead, 672274793, 0x4321FEDC, 0) })
}

pub fn reiniciar() -> Erro {
    Erro(-unsafe { syscall(169, 0xfee1dead, 672274793, 0x01234567, 0) })
}

// -----------------------------------------------------------------------------
// Extensões do FerrugemOS
// -----------------------------------------------------------------------------

pub struct InfoProcesso {
    pub pid: u32,
    pub pai: u32,
    pub pgid: u32,
    pub uid: u32,
    pub estado: u32,
    pub memoria_kib: u64,
    pub tiques: u64,
    pub nome: String,
}

pub fn processos() -> Vec<InfoProcesso> {
    let mut buf = alloc::vec![0u8; 64 * 128];
    let n = unsafe { syscall(1000, buf.as_mut_ptr() as u64, 128, 0, 0) }.max(0) as usize;
    (0..n)
        .map(|i| {
            let r = &buf[i * 64..(i + 1) * 64];
            let u32_em = |o: usize| u32::from_le_bytes(r[o..o + 4].try_into().unwrap());
            let u64_em = |o: usize| u64::from_le_bytes(r[o..o + 8].try_into().unwrap());
            let nome = &r[32..60];
            let fim = nome.iter().position(|&b| b == 0).unwrap_or(28);
            InfoProcesso {
                pid: u32_em(0),
                pai: u32_em(4),
                pgid: u32_em(60),
                estado: u32_em(8),
                uid: u32_em(12),
                memoria_kib: u64_em(16),
                tiques: u64_em(24),
                nome: String::from_utf8_lossy(&nome[..fim]).into_owned(),
            }
        })
        .collect()
}

pub struct InfoMemoria {
    pub total_kib: u64,
    pub livre_kib: u64,
    pub heap_kernel_kib: u64,
    pub heap_kernel_usado_kib: u64,
}

pub fn memoria() -> InfoMemoria {
    let mut b = [0u64; 4];
    unsafe { syscall(1001, b.as_mut_ptr() as u64, 0, 0, 0) };
    InfoMemoria { total_kib: b[0], livre_kib: b[1], heap_kernel_kib: b[2], heap_kernel_usado_kib: b[3] }
}

pub struct InfoDisco {
    pub total_blocos: u64,
    pub blocos_livres: u64,
    pub tamanho_bloco: u64,
}

pub fn disco() -> InfoDisco {
    let mut b = [0u64; 3];
    unsafe { syscall(1002, b.as_mut_ptr() as u64, 0, 0, 0) };
    InfoDisco { total_blocos: b[0], blocos_livres: b[1], tamanho_bloco: b[2] }
}

/// Converte tempo Unix em "AAAA-MM-DD HH:MM:SS" (UTC).
pub fn formatar_data(segundos: u64) -> String {
    // Algoritmo "civil_from_days" de Howard Hinnant.
    let dias = (segundos / 86400) as i64 + 719468;
    let era = dias.div_euclid(146097);
    let dia_da_era = dias - era * 146097;
    let ano_da_era = (dia_da_era - dia_da_era / 1460 + dia_da_era / 36524 - dia_da_era / 146096) / 365;
    let dia_do_ano = dia_da_era - (365 * ano_da_era + ano_da_era / 4 - ano_da_era / 100);
    let mp = (5 * dia_do_ano + 2) / 153;
    let dia = dia_do_ano - (153 * mp + 2) / 5 + 1;
    let mes = if mp < 10 { mp + 3 } else { mp - 9 };
    let ano = ano_da_era + era * 400 + (mes <= 2) as i64;
    let s = segundos % 86400;
    alloc::format!("{:04}-{:02}-{:02} {:02}:{:02}:{:02}", ano, mes, dia, s / 3600, s / 60 % 60, s % 60)
}

// -----------------------------------------------------------------------------
// Memória: mmap e companhia
// -----------------------------------------------------------------------------

pub const PROT_NONE: u64 = 0;
pub const PROT_READ: u64 = 1;
pub const PROT_WRITE: u64 = 2;
pub const PROT_EXEC: u64 = 4;
pub const MAP_SHARED: u64 = 0x01;
pub const MAP_PRIVATE: u64 = 0x02;
pub const MAP_ANONYMOUS: u64 = 0x20;

/// Mapeia `tamanho` bytes: de um arquivo (`fd`), ou memória zerada (MAP_ANONYMOUS, fd = -1).
pub fn mmap(tamanho: usize, prot: u64, flags: u64, fd: i32, deslocamento: u64) -> Resultado<usize> {
    let r = unsafe {
        let resposta: i64;
        asm!(
            "syscall",
            inlateout("rax") 9i64 => resposta,
            in("rdi") 0u64, in("rsi") tamanho as u64, in("rdx") prot,
            in("r10") flags, in("r8") fd as i64 as u64, in("r9") deslocamento,
            lateout("rcx") _, lateout("r11") _,
            options(nostack),
        );
        resposta
    };
    conferir(r).map(|e| e as usize)
}

pub fn munmap(endereco: usize, tamanho: usize) -> Resultado<()> {
    conferir(unsafe { syscall(11, endereco as u64, tamanho as u64, 0, 0) }).map(|_| ())
}

pub fn mprotect(endereco: usize, tamanho: usize, prot: u64) -> Resultado<()> {
    conferir(unsafe { syscall(10, endereco as u64, tamanho as u64, prot, 0) }).map(|_| ())
}

pub fn msync(endereco: usize, tamanho: usize) -> Resultado<()> {
    conferir(unsafe { syscall(26, endereco as u64, tamanho as u64, 4, 0) }).map(|_| ())
}
