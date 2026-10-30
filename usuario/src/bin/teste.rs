//! teste — bateria de testes do kernel, rodando como um programa comum.
//!
//! Cada teste usa as chamadas de sistema de verdade. Os que provocam erros
//! fatais (page fault, divisão por zero...) rodam num processo filho: o
//! kernel precisa matar só o filho, com o sinal certo, e seguir em frente.

#![no_std]
#![no_main]

use core::arch::asm;
use core::hint::black_box;
use core::sync::atomic::{AtomicI32, AtomicU64, Ordering};
use ulib::sys;
use ulib::*;

programa!(principal);

type R = Result<(), String>;

/// Roda `f` num processo filho e devolve o status do `wait`.
fn em_filho(f: impl FnOnce() -> i32) -> Result<i32, String> {
    match sys::fork() {
        Ok(0) => sys::sair(f()),
        Ok(pid) => sys::esperar(pid).map(|(_, status)| status).map_err(|e| format!("wait: {}", e)),
        Err(e) => Err(format!("fork: {}", e)),
    }
}

fn morto_por(status: i32, sinal: i32) -> R {
    match sys::sinal_que_matou(status) {
        Some(s) if s == sinal => Ok(()),
        Some(s) => Err(format!("morreu com o sinal {}, esperava {}", s, sinal)),
        None => Err(format!("terminou normalmente (código {:?})", sys::codigo_de_saida(status))),
    }
}

fn ler_tudo(fd: i32) -> Vec<u8> {
    let mut dados = Vec::new();
    let mut buf = [0u8; 512];
    while let Ok(n) = sys::ler(fd, &mut buf) {
        if n == 0 {
            break;
        }
        dados.extend_from_slice(&buf[..n]);
    }
    dados
}

fn erro(e: sys::Erro) -> String {
    format!("{}", e)
}

// -----------------------------------------------------------------------------

fn fork_exit_wait() -> R {
    let status = em_filho(|| 42)?;
    match sys::codigo_de_saida(status) {
        Some(42) => Ok(()),
        outro => Err(format!("código {:?}, esperava 42", outro)),
    }
}

fn pipe_entre_processos() -> R {
    let (leitura, escrita) = sys::pipe().map_err(erro)?;
    em_filho(|| {
        let _ = sys::escrever_tudo(escrita, "olá pelo pipe".as_bytes());
        0
    })?;
    let _ = sys::fechar(escrita);
    let recebido = ler_tudo(leitura);
    let _ = sys::fechar(leitura);
    if recebido == "olá pelo pipe".as_bytes() { Ok(()) } else { Err(format!("recebi {:?}", recebido)) }
}

fn heap_grande() -> R {
    let mut v = vec![0u8; 4 * 1024 * 1024];
    for (i, b) in v.iter_mut().enumerate() {
        *b = (i % 251) as u8;
    }
    let soma: u64 = v.iter().map(|&b| b as u64).sum();
    let mut textos = Vec::new();
    for i in 0..5000 {
        textos.push(format!("item {}", i));
    }
    textos.retain(|t| t.ends_with('7'));
    let esperado: u64 = (0..4 * 1024 * 1024u64).map(|i| i % 251).sum();
    if soma == esperado && textos.len() == 500 { Ok(()) } else { Err(String::from("conteúdo errado")) }
}

fn page_fault() -> R {
    let status = em_filho(|| {
        let ptr = black_box(0x10usize) as *const u8;
        unsafe { core::ptr::read_volatile(ptr) as i32 }
    })?;
    morto_por(status, 11)
}

fn ler_o_kernel() -> R {
    let status = em_filho(|| {
        let ptr = black_box(0x10_0000usize) as *const u8; // onde o kernel está carregado
        unsafe { core::ptr::read_volatile(ptr) as i32 }
    })?;
    morto_por(status, 11)
}

fn escrever_no_codigo() -> R {
    let status = em_filho(|| {
        let ptr = principal as *const () as *mut u8; // o .text é só leitura
        unsafe { core::ptr::write_volatile(ptr, 0x90) };
        0
    })?;
    morto_por(status, 11)
}

fn divisao_por_zero() -> R {
    let status = em_filho(|| {
        unsafe { asm!("div {}", in(reg) black_box(0u64), inout("rax") 1u64 => _, inout("rdx") 0u64 => _) };
        0
    })?;
    morto_por(status, 8)
}

fn instrucao_privilegiada() -> R {
    let status = em_filho(|| {
        unsafe { asm!("hlt") }; // só o anel 0 pode
        0
    })?;
    morto_por(status, 11)
}

fn arquivos() -> R {
    let caminho = "/tmp/teste-arquivo.txt";
    let fd = sys::abrir(caminho, sys::O_RDWR | sys::O_CREAT | sys::O_TRUNC).map_err(erro)?;
    sys::escrever_tudo(fd, b"0123456789").map_err(erro)?;
    sys::posicionar(fd, 3, 0).map_err(erro)?;
    let mut buf = [0u8; 4];
    let n = sys::ler(fd, &mut buf).map_err(erro)?;
    sys::fechar(fd).map_err(erro)?;
    if &buf[..n] != b"3456" {
        return Err(format!("li {:?}", &buf[..n]));
    }
    let fd = sys::abrir(caminho, sys::O_WRONLY | sys::O_APPEND).map_err(erro)?;
    sys::escrever_tudo(fd, b"!").map_err(erro)?;
    let _ = sys::fechar(fd);
    if sys::stat(caminho).map_err(erro)?.tamanho != 11 {
        return Err(String::from("tamanho errado depois do append"));
    }
    sys::remover(caminho).map_err(erro)?;
    match sys::stat(caminho) {
        Err(e) if e == sys::ENOENT => Ok(()),
        _ => Err(String::from("o arquivo continuou existindo")),
    }
}

fn diretorios() -> R {
    sys::criar_diretorio("/tmp/teste-dir").map_err(erro)?;
    sys::criar_diretorio("/tmp/teste-dir/sub").map_err(erro)?;
    let fd = sys::abrir("/tmp/teste-dir/a", sys::O_WRONLY | sys::O_CREAT).map_err(erro)?;
    let _ = sys::fechar(fd);
    sys::renomear("/tmp/teste-dir/a", "/tmp/teste-dir/sub/b").map_err(erro)?;
    let nomes: Vec<String> = sys::listar_diretorio("/tmp/teste-dir/sub").map_err(erro)?.into_iter().map(|e| e.nome).collect();
    if nomes != ["b"] {
        return Err(format!("listagem errada: {:?}", nomes));
    }
    if sys::remover_diretorio("/tmp/teste-dir/sub").is_ok() {
        return Err(String::from("apagou um diretório que não estava vazio"));
    }
    sys::mudar_diretorio("/tmp/teste-dir/sub").map_err(erro)?;
    let atual = sys::diretorio_atual();
    sys::remover("b").map_err(erro)?; // caminho relativo
    sys::mudar_diretorio("/").map_err(erro)?;
    sys::remover_diretorio("/tmp/teste-dir/sub").map_err(erro)?;
    sys::remover_diretorio("/tmp/teste-dir").map_err(erro)?;
    if atual == "/tmp/teste-dir/sub" { Ok(()) } else { Err(format!("getcwd devolveu {}", atual)) }
}

fn preempcao() -> R {
    let pid = match sys::fork() {
        Ok(0) => loop {
            core::hint::spin_loop(); // nunca cede a CPU por conta própria
        },
        Ok(pid) => pid,
        Err(e) => return Err(erro(e)),
    };
    // Se o timer não tirasse a CPU do filho, o pai nunca voltaria a rodar aqui.
    sys::dormir_ms(50).map_err(erro)?;
    sys::matar(pid, sys::SIGTERM).map_err(erro)?;
    let (_, status) = sys::esperar(pid).map_err(erro)?;
    morto_por(status, sys::SIGTERM)
}

fn muitos_processos() -> R {
    let mut pids = Vec::new();
    for i in 0..40 {
        match sys::fork() {
            Ok(0) => {
                let _ = sys::dormir_ms(20);
                sys::sair(i);
            }
            Ok(pid) => pids.push(pid),
            Err(e) => return Err(format!("fork {}: {}", i, e)),
        }
    }
    let mut soma = 0;
    for _ in 0..40 {
        let (_, status) = sys::esperar(-1).map_err(erro)?;
        soma += sys::codigo_de_saida(status).unwrap_or(1000);
    }
    if sys::esperar(-1) != Err(sys::ECHILD) {
        return Err(String::from("sobrou filho"));
    }
    if soma == (0..40).sum::<i32>() { Ok(()) } else { Err(format!("soma dos códigos = {}", soma)) }
}

fn timer() -> R {
    let antes = sys::tempo_ms();
    sys::dormir_ms(200).map_err(erro)?;
    let passou = sys::tempo_ms() - antes;
    if (200..300).contains(&passou) { Ok(()) } else { Err(format!("dormiu {} ms", passou)) }
}

fn exec_com_pipe() -> R {
    let (leitura, escrita) = sys::pipe().map_err(erro)?;
    let status = em_filho(|| {
        let _ = sys::duplicar_para(escrita, 1);
        let _ = sys::fechar(escrita);
        let _ = sys::fechar(leitura);
        let e = sys::executar("/bin/echo", &["echo", "argumentos", "chegaram"]);
        eprintln!("exec: {}", e);
        1
    })?;
    let _ = sys::fechar(escrita);
    let saida = ler_tudo(leitura);
    let _ = sys::fechar(leitura);
    if sys::codigo_de_saida(status) != Some(0) {
        return Err(format!("status {}", status));
    }
    if saida == b"argumentos chegaram\n" { Ok(()) } else { Err(format!("saída: {:?}", String::from_utf8_lossy(&saida))) }
}

fn exec_inexistente() -> R {
    match sys::executar("/bin/nao-existe", &["x"]) {
        e if e == sys::ENOENT => Ok(()),
        e => Err(format!("erro {}", e)),
    }
}

fn sigpipe() -> R {
    let status = em_filho(|| {
        let (leitura, escrita) = sys::pipe().unwrap();
        let _ = sys::fechar(leitura);
        let _ = sys::escrever(escrita, b"ninguem vai ler");
        0
    })?;
    morto_por(status, 13)
}

fn orfaos() -> R {
    let (leitura, escrita) = sys::pipe().map_err(erro)?;
    em_filho(|| {
        if let Ok(0) = sys::fork() {
            let _ = sys::dormir_ms(100); // o pai já terá morrido
            let _ = sys::escrever_tudo(escrita, format!("{}", sys::pid_do_pai()).as_bytes());
            sys::sair(0);
        }
        0
    })?;
    let _ = sys::fechar(escrita);
    let pai = ler_tudo(leitura);
    let _ = sys::fechar(leitura);
    if pai == b"1" { Ok(()) } else { Err(format!("pai do órfão = {:?}", String::from_utf8_lossy(&pai))) }
}

fn memoria_devolvida() -> R {
    let antes = sys::memoria().livre_kib;
    for _ in 0..20 {
        em_filho(|| {
            let v = vec![1u8; 512 * 1024];
            v.len() as i32 & 0
        })?;
    }
    let depois = sys::memoria().livre_kib;
    if antes == depois { Ok(()) } else { Err(format!("antes {} KiB livres, depois {} KiB", antes, depois)) }
}

fn ponteiro_invalido_na_syscall() -> R {
    // O kernel tem que recusar (EFAULT), não travar.
    let r = sys::escrever(1, unsafe { core::slice::from_raw_parts(0x10_0000 as *const u8, 10) });
    match r {
        Err(sys::Erro(14)) => Ok(()),
        outro => Err(format!("{:?}", outro)),
    }
}

fn disco_ext2() -> R {
    if sys::stat("/disco").is_err() {
        return Err(String::from("sem disco em /disco (rode pelo `make teste`)"));
    }
    let livres_antes = sys::disco().blocos_livres;
    sys::criar_diretorio("/disco/teste").map_err(erro)?;
    // 300 KB: passa dos 12 blocos diretos e usa os blocos indiretos do ext2
    let dados: Vec<u8> = (0..300_000u32).map(|i| (i * 7 % 256) as u8).collect();
    let fd = sys::abrir("/disco/teste/grande", sys::O_WRONLY | sys::O_CREAT).map_err(erro)?;
    sys::escrever_tudo(fd, &dados).map_err(erro)?;
    let _ = sys::fechar(fd);
    sys::renomear("/disco/teste/grande", "/disco/teste/renomeado").map_err(erro)?;
    let lido = sys::ler_arquivo("/disco/teste/renomeado").map_err(erro)?;
    if lido != dados {
        return Err(format!("li {} bytes diferentes do que gravei", lido.len()));
    }
    sys::remover("/disco/teste/renomeado").map_err(erro)?;
    sys::remover_diretorio("/disco/teste").map_err(erro)?;
    let livres_depois = sys::disco().blocos_livres;
    if livres_antes == livres_depois {
        Ok(())
    } else {
        Err(format!("{} blocos livres antes, {} depois", livres_antes, livres_depois))
    }
}

// ----------------------------------------------------------------- sinais

static RECEBIDOS: AtomicI32 = AtomicI32::new(0);

extern "C" fn contar_sinal(_sinal: i32) {
    RECEBIDOS.fetch_add(1, Ordering::SeqCst);
}

extern "C" fn sair_com_42(_sinal: i32) {
    sys::sair(42);
}

fn tratador_de_sinal() -> R {
    RECEBIDOS.store(0, Ordering::SeqCst);
    sys::tratar_sinal(sys::SIGUSR1, contar_sinal, false).map_err(erro)?;
    // Variáveis vivas em registradores precisam sobreviver ao tratador.
    let a = black_box(1234u64);
    sys::matar(sys::pid(), sys::SIGUSR1).map_err(erro)?; // entregue na volta desta syscall
    let b = black_box(a * 2);
    sys::ignorar_sinal(sys::SIGUSR1, false);
    match (RECEBIDOS.load(Ordering::SeqCst), b) {
        (1, 2468) => Ok(()),
        (n, b) => Err(format!("tratador rodou {} vezes, b = {}", n, b)),
    }
}

fn mascara_de_sinais() -> R {
    RECEBIDOS.store(0, Ordering::SeqCst);
    sys::tratar_sinal(sys::SIGUSR2, contar_sinal, false).map_err(erro)?;
    sys::bloquear_sinais(sys::SIG_BLOCK, 1 << sys::SIGUSR2).map_err(erro)?;
    sys::matar(sys::pid(), sys::SIGUSR2).map_err(erro)?;
    let enquanto_bloqueado = RECEBIDOS.load(Ordering::SeqCst);
    sys::bloquear_sinais(sys::SIG_UNBLOCK, 1 << sys::SIGUSR2).map_err(erro)?;
    let depois = RECEBIDOS.load(Ordering::SeqCst);
    sys::ignorar_sinal(sys::SIGUSR2, false);
    if (enquanto_bloqueado, depois) == (0, 1) { Ok(()) } else { Err(format!("{} e {}", enquanto_bloqueado, depois)) }
}

fn alarme_e_pause() -> R {
    RECEBIDOS.store(0, Ordering::SeqCst);
    sys::tratar_sinal(sys::SIGALRM, contar_sinal, false).map_err(erro)?;
    let antes = sys::tempo_ms();
    sys::alarme(1);
    sys::pausar();
    let passou = sys::tempo_ms() - antes;
    sys::ignorar_sinal(sys::SIGALRM, false);
    if RECEBIDOS.load(Ordering::SeqCst) == 1 && (900..1200).contains(&passou) {
        Ok(())
    } else {
        Err(format!("{} sinais em {} ms", RECEBIDOS.load(Ordering::SeqCst), passou))
    }
}

fn tratador_de_sigsegv() -> R {
    let status = em_filho(|| {
        let _ = sys::tratar_sinal(sys::SIGSEGV, sair_com_42, false);
        unsafe { core::ptr::read_volatile(black_box(0x10usize) as *const u8) as i32 }
    })?;
    match sys::codigo_de_saida(status) {
        Some(42) => Ok(()),
        _ => Err(format!("status {:#x}", status)),
    }
}

/// O filho fica bloqueado num `read`; o pai manda um sinal e depois os dados.
fn read_interrompido(reiniciar: bool) -> Result<i32, String> {
    let (leitura, escrita) = sys::pipe().map_err(erro)?;
    RECEBIDOS.store(0, Ordering::SeqCst); // antes do fork: o filho herda a memória
    let filho = match sys::fork() {
        Ok(0) => {
            let _ = sys::tratar_sinal(sys::SIGUSR1, contar_sinal, reiniciar);
            let mut buf = [0u8; 8];
            let codigo = match sys::ler(leitura, &mut buf) {
                Ok(2) if &buf[..2] == b"ok" => 0, // recomeçou e leu
                Err(e) if e == sys::EINTR => 4,    // devolveu EINTR
                _ => 1,
            };
            sys::sair(if RECEBIDOS.load(Ordering::SeqCst) == 1 { codigo } else { 9 });
        }
        Ok(pid) => pid,
        Err(e) => return Err(erro(e)),
    };
    sys::dormir_ms(50).map_err(erro)?;
    sys::matar(filho, sys::SIGUSR1).map_err(erro)?;
    sys::dormir_ms(50).map_err(erro)?;
    let _ = sys::escrever(escrita, b"ok");
    let _ = sys::fechar(escrita);
    let _ = sys::fechar(leitura);
    let (_, status) = sys::esperar(filho).map_err(erro)?;
    sys::codigo_de_saida(status).ok_or(format!("morreu: {:#x}", status))
}

fn sa_restart() -> R {
    match read_interrompido(true)? {
        0 => Ok(()),
        c => Err(format!("código {} (4 = devolveu EINTR, 9 = tratador não rodou)", c)),
    }
}

fn sem_sa_restart() -> R {
    match read_interrompido(false)? {
        4 => Ok(()),
        c => Err(format!("código {} (0 = recomeçou, 9 = tratador não rodou)", c)),
    }
}

fn copia_na_escrita() -> R {
    let mut v = vec![1u8; 4 * 1024 * 1024];
    let livre_antes = sys::memoria().livre_kib;
    let (leitura, escrita) = sys::pipe().map_err(erro)?;
    em_filho(|| {
        let depois_do_fork = sys::memoria().livre_kib;
        for b in v.iter_mut() {
            *b = 2; // agora sim cada página é copiada
        }
        let depois_de_escrever = sys::memoria().livre_kib;
        let texto = format!("{} {}", depois_do_fork, depois_de_escrever);
        let _ = sys::escrever_tudo(escrita, texto.as_bytes());
        0
    })?;
    let _ = sys::fechar(escrita);
    let resposta = String::from_utf8(ler_tudo(leitura)).unwrap_or_default();
    let _ = sys::fechar(leitura);
    let numeros: Vec<u64> = resposta.split(' ').filter_map(|n| n.parse().ok()).collect();
    let [depois_do_fork, depois_de_escrever] = numeros[..] else { return Err(resposta) };
    let gasto_no_fork = livre_antes - depois_do_fork;
    let gasto_na_escrita = depois_do_fork - depois_de_escrever;
    if v.iter().any(|&b| b != 1) {
        return Err(String::from("a escrita do filho apareceu no pai"));
    }
    if gasto_no_fork < 512 && gasto_na_escrita >= 4096 {
        Ok(())
    } else {
        Err(format!("fork gastou {} KiB, escrita {} KiB", gasto_no_fork, gasto_na_escrita))
    }
}

// ----------------------------------------------------------- permissões

/// Vira o usuário 1000 (só funciona sendo root).
fn virar_usuario_comum() -> bool {
    sys::mudar_gid(1000).is_ok() && sys::mudar_uid(1000).is_ok()
}

fn permissoes() -> R {
    if sys::uid() != 0 {
        return Err(String::from("precisa rodar como root (faça login como root)"));
    }
    let fd = sys::abrir("/tmp/do-root", sys::O_WRONLY | sys::O_CREAT | sys::O_TRUNC).map_err(erro)?;
    let _ = sys::fechar(fd);
    let status = em_filho(|| {
        if !virar_usuario_comum() {
            return 100;
        }
        let checagens: [(bool, i32); 10] = [
            (sys::abrir("/etc/shadow", sys::O_RDONLY) == Err(sys::EACCES), 1),
            (sys::abrir("/bin/novo", sys::O_WRONLY | sys::O_CREAT) == Err(sys::EACCES), 2),
            (sys::matar(1, sys::SIGTERM) == Err(sys::EPERM), 3),
            (sys::mudar_uid(0) == Err(sys::EPERM), 4),
            (sys::remover("/tmp/do-root") == Err(sys::EPERM), 5), // sticky bit do /tmp
            (sys::mudar_permissoes("/tmp/do-root", 0o777) == Err(sys::EPERM), 6),
            (sys::listar_diretorio("/root").err() == Some(sys::EACCES), 7),
            (sys::mudar_dono("/tmp/do-root", Some(1000), None) == Err(sys::EPERM), 8),
            ({
                // um arquivo meu, sem permissão nem para mim: nem eu leio
                let _ = sys::abrir_com_permissoes("/tmp/meu", sys::O_WRONLY | sys::O_CREAT, 0).map(sys::fechar);
                sys::abrir("/tmp/meu", sys::O_RDONLY) == Err(sys::EACCES)
            }, 9),
            (sys::remover("/tmp/meu").is_ok(), 10), // mas apagar o que é meu, posso
        ];
        checagens.iter().find(|(ok, _)| !ok).map_or(0, |(_, n)| *n)
    })?;
    let _ = sys::remover("/tmp/do-root");
    match sys::codigo_de_saida(status) {
        Some(0) => Ok(()),
        Some(n) => Err(format!("a checagem {} falhou", n)),
        None => Err(format!("status {:#x}", status)),
    }
}

fn programa_setuid() -> R {
    // Uma cópia deste programa, do root, com o bit setuid.
    let dados = sys::ler_arquivo("/bin/teste").map_err(erro)?;
    let fd = sys::abrir_com_permissoes("/tmp/teste-suid", sys::O_WRONLY | sys::O_CREAT | sys::O_TRUNC, 0o755)
        .map_err(erro)?;
    sys::escrever_tudo(fd, &dados).map_err(erro)?;
    let _ = sys::fechar(fd);
    sys::mudar_permissoes("/tmp/teste-suid", 0o4755).map_err(erro)?;
    let (leitura, escrita) = sys::pipe().map_err(erro)?;
    em_filho(|| {
        if !virar_usuario_comum() {
            return 1;
        }
        let _ = sys::duplicar_para(escrita, 1);
        sys::executar("/tmp/teste-suid", &["teste-suid", "--identidade"]);
        1
    })?;
    let _ = sys::fechar(escrita);
    let saida = String::from_utf8(ler_tudo(leitura)).unwrap_or_default();
    let _ = sys::fechar(leitura);
    let _ = sys::remover("/tmp/teste-suid");
    if saida == "euid=0 uid=1000" { Ok(()) } else { Err(format!("o programa disse {:?}", saida)) }
}

// ------------------------------------------------------- controle de jobs

fn parar_e_continuar() -> R {
    let pid = match sys::fork() {
        Ok(0) => loop {
            core::hint::spin_loop();
        },
        Ok(pid) => pid,
        Err(e) => return Err(erro(e)),
    };
    sys::matar(pid, sys::SIGSTOP).map_err(erro)?;
    let parou = sys::esperar_com(pid, sys::WUNTRACED).map_err(erro)?;
    let parado_certo = matches!(parou, Some((p, s)) if p == pid && sys::sinal_que_parou(s) == Some(sys::SIGSTOP));
    let estado = sys::processos().into_iter().find(|p| p.pid == pid as u32).map(|p| p.estado);
    sys::matar(pid, sys::SIGCONT).map_err(erro)?;
    sys::matar(pid, sys::SIGTERM).map_err(erro)?;
    let (_, status) = sys::esperar(pid).map_err(erro)?;
    if !parado_certo || estado != Some(4) {
        return Err(format!("wait devolveu {:?}, estado {:?}", parou, estado));
    }
    morto_por(status, sys::SIGTERM)
}

/// Um processo parado no meio de um sleep continua o sleep depois do SIGCONT
/// (não acorda antes da hora com EINTR).
fn sleep_sobrevive_a_parada() -> R {
    let antes = sys::tempo_ms();
    let pid = match sys::fork() {
        Ok(0) => sys::sair(if sys::dormir_ms(300).is_ok() { 7 } else { 1 }),
        Ok(pid) => pid,
        Err(e) => return Err(erro(e)),
    };
    sys::dormir_ms(50).map_err(erro)?;
    sys::matar(pid, sys::SIGTSTP).map_err(erro)?;
    match sys::esperar_com(pid, sys::WUNTRACED).map_err(erro)? {
        Some((_, s)) if sys::sinal_que_parou(s) == Some(sys::SIGTSTP) => {}
        outro => return Err(format!("não parou: {:?}", outro)),
    }
    sys::dormir_ms(50).map_err(erro)?;
    sys::matar(pid, sys::SIGCONT).map_err(erro)?;
    let (_, status) = sys::esperar(pid).map_err(erro)?;
    let passou = sys::tempo_ms() - antes;
    match sys::codigo_de_saida(status) {
        Some(7) if passou >= 300 => Ok(()),
        c => Err(format!("código {:?} depois de {} ms", c, passou)),
    }
}

/// `kill(-grupo)` atinge todos do grupo e só eles.
fn sinal_para_grupo() -> R {
    let mut filhos = Vec::new();
    for i in 0..3 {
        match sys::fork() {
            Ok(0) => {
                // Os dois primeiros num grupo novo: o do primeiro filho.
                match i {
                    0 => drop(sys::mudar_grupo(0, 0)),
                    1 => drop(sys::mudar_grupo(0, filhos[0])),
                    _ => {}
                }
                let _ = sys::dormir_ms(2000);
                sys::sair(0);
            }
            Ok(pid) => filhos.push(pid),
            Err(e) => return Err(erro(e)),
        }
    }
    // Ajusta também daqui (o filho pode não ter rodado ainda), como faz um shell.
    let _ = sys::mudar_grupo(filhos[0], filhos[0]);
    let _ = sys::mudar_grupo(filhos[1], filhos[0]);
    sys::dormir_ms(20).map_err(erro)?;
    sys::matar(-filhos[0], sys::SIGTERM).map_err(erro)?;
    let mut mortos = 0;
    for &pid in &filhos[..2] {
        let (_, s) = sys::esperar(pid).map_err(erro)?;
        mortos += (sys::sinal_que_matou(s) == Some(sys::SIGTERM)) as i32;
    }
    let (_, terceiro) = sys::esperar(filhos[2]).map_err(erro)?; // este dorme até o fim
    if mortos == 2 && sys::codigo_de_saida(terceiro) == Some(0) {
        Ok(())
    } else {
        Err(format!("{} do grupo morreram; o de fora: {:#x}", mortos, terceiro))
    }
}

// ----------------------------------------------------------------- links

/// Links simbólicos e físicos, num diretório (`/tmp` na memória ou `/disco`).
fn links_em(base: &str) -> R {
    let d = format!("{}/teste-links", base);
    let _ = sys::criar_diretorio(&d);
    let arquivo = format!("{}/original", d);
    let fd = sys::abrir(&arquivo, sys::O_WRONLY | sys::O_CREAT | sys::O_TRUNC).map_err(erro)?;
    sys::escrever_tudo(fd, b"conteudo").map_err(erro)?;
    let _ = sys::fechar(fd);

    // simbólico, relativo e com "..": sub/atalho -> ../original
    sys::criar_diretorio(&format!("{}/sub", d)).map_err(erro)?;
    let atalho = format!("{}/sub/atalho", d);
    sys::criar_link_simbolico("../original", &atalho).map_err(erro)?;
    if sys::ler_arquivo(&atalho).map_err(erro)? != b"conteudo" {
        return Err(String::from("ler pelo link simbólico"));
    }
    if sys::ler_link(&atalho).map_err(erro)? != "../original" || !sys::lstat(&atalho).map_err(erro)?.eh_link() {
        return Err(String::from("readlink/lstat"));
    }
    // um link longo (mais de 60 bytes: no ext2 vai para um bloco de dados)
    let longo = format!("{}/longo", d);
    let alvo_longo = format!("{}/{}", d, "././././././././././././././././././././././././././original");
    sys::criar_link_simbolico(&alvo_longo, &longo).map_err(erro)?;
    if sys::ler_arquivo(&longo).map_err(erro)? != b"conteudo" {
        return Err(String::from("link longo"));
    }
    // laço e link quebrado
    let laco = format!("{}/laco", d);
    sys::criar_link_simbolico(&laco, &laco).map_err(erro)?;
    if sys::abrir(&laco, sys::O_RDONLY) != Err(sys::ELOOP) {
        return Err(String::from("laço de links não deu ELOOP"));
    }
    let quebrado = format!("{}/quebrado", d);
    sys::criar_link_simbolico("nao-existe", &quebrado).map_err(erro)?;
    if sys::abrir(&quebrado, sys::O_RDONLY) != Err(sys::ENOENT) {
        return Err(String::from("link quebrado não deu ENOENT"));
    }

    // físico: dois nomes, um arquivo
    let outro = format!("{}/outro-nome", d);
    sys::criar_link(&arquivo, &outro).map_err(erro)?;
    let fd = sys::abrir(&outro, sys::O_WRONLY | sys::O_APPEND).map_err(erro)?;
    sys::escrever_tudo(fd, b"!").map_err(erro)?;
    let _ = sys::fechar(fd);
    if sys::stat(&arquivo).map_err(erro)?.inode != sys::stat(&outro).map_err(erro)?.inode {
        return Err(String::from("os dois nomes têm inodes diferentes"));
    }
    sys::remover(&arquivo).map_err(erro)?;
    if sys::ler_arquivo(&outro).map_err(erro)? != b"conteudo!" {
        return Err(String::from("apagar um nome levou o arquivo junto"));
    }

    // apagar um link não apaga o alvo; e limpa tudo
    for nome in ["sub/atalho", "longo", "laco", "quebrado", "outro-nome"] {
        sys::remover(&format!("{}/{}", d, nome)).map_err(|e| format!("rm {}: {}", nome, e))?;
    }
    sys::remover_diretorio(&format!("{}/sub", d)).map_err(erro)?;
    sys::remover_diretorio(&d).map_err(erro)
}

fn links_na_memoria() -> R {
    links_em("/tmp")
}

fn links_no_disco() -> R {
    if sys::stat("/disco").is_err() {
        return Err(String::from("sem disco em /disco"));
    }
    let livres = sys::disco().blocos_livres;
    links_em("/disco")?;
    let depois = sys::disco().blocos_livres;
    if livres == depois { Ok(()) } else { Err(format!("blocos livres: {} antes, {} depois", livres, depois)) }
}

// ------------------------------------------------------------------ memória

fn livre_kib() -> u64 {
    sys::memoria().livre_kib
}

fn paginas_sob_demanda() -> R {
    let antes = livre_kib();
    let tamanho = 64 * 1024 * 1024;
    let m = sys::mmap(tamanho, sys::PROT_READ | sys::PROT_WRITE, sys::MAP_PRIVATE | sys::MAP_ANONYMOUS, -1, 0)
        .map_err(erro)?;
    let depois_do_mmap = livre_kib();
    for i in (0..tamanho).step_by(1024 * 1024) {
        unsafe { core::ptr::write_volatile((m + i) as *mut u8, 7) }; // uma página por MiB
    }
    let depois_de_tocar = livre_kib();
    sys::munmap(m, tamanho).map_err(erro)?;
    let depois_do_munmap = livre_kib();
    let custo_mmap = antes - depois_do_mmap;
    let custo_toque = depois_do_mmap - depois_de_tocar;
    if custo_mmap <= 8 && (256..512).contains(&custo_toque) && depois_do_munmap + 8 >= antes {
        Ok(())
    } else {
        Err(format!("mmap de 64 MiB gastou {} KiB; tocar 64 páginas, {} KiB", custo_mmap, custo_toque))
    }
}

fn pilha_cresce() -> R {
    fn recursiva(n: u32) -> u64 {
        let local = black_box([n as u8; 16 * 1024]); // 16 KiB por chamada
        if n == 0 { local[0] as u64 } else { recursiva(n - 1) + local[100] as u64 }
    }
    // 200 chamadas ≈ 3 MiB de pilha, bem mais que o que existe no começo
    let status = em_filho(|| (recursiva(200) % 256) as i32)?;
    match sys::codigo_de_saida(status) {
        Some(_) => Ok(()),
        None => Err(format!("morreu: {:#x}", status)),
    }
}

fn pilha_estoura() -> R {
    #[allow(unconditional_recursion)] // é de propósito: até estourar a pilha
    fn sem_fim(n: u64) -> u64 {
        let local = black_box([n as u8; 64 * 1024]);
        sem_fim(n + 1) + local[1] as u64
    }
    let status = em_filho(|| sem_fim(0) as i32)?; // passa dos 8 MiB
    morto_por(status, 11)
}

fn nx_na_pilha() -> R {
    let status = em_filho(|| {
        let codigo = black_box([0xC3u8; 16]); // `ret`, na pilha
        let funcao: extern "C" fn() = unsafe { core::mem::transmute(codigo.as_ptr()) };
        funcao();
        0
    })?;
    morto_por(status, 11)
}

fn mprotect_funciona() -> R {
    let m = sys::mmap(4096, sys::PROT_READ | sys::PROT_WRITE, sys::MAP_PRIVATE | sys::MAP_ANONYMOUS, -1, 0)
        .map_err(erro)?;
    unsafe { core::ptr::write_volatile(m as *mut u8, 1) };
    sys::mprotect(m, 4096, sys::PROT_READ).map_err(erro)?;
    let status = em_filho(|| {
        unsafe { core::ptr::write_volatile(m as *mut u8, 2) };
        0
    })?;
    morto_por(status, 11)?;
    sys::mprotect(m, 4096, sys::PROT_READ | sys::PROT_WRITE).map_err(erro)?;
    unsafe { core::ptr::write_volatile(m as *mut u8, 3) };
    let lido = unsafe { core::ptr::read_volatile(m as *const u8) };
    sys::munmap(m, 4096).map_err(erro)?;
    if lido == 3 { Ok(()) } else { Err(String::from("leitura errada")) }
}

fn mmap_de_arquivo_em(base: &str) -> R {
    let caminho = format!("{}/teste-mmap", base);
    let conteudo: Vec<u8> = (0..10_000u32).map(|i| b'a' + (i % 26) as u8).collect();
    let fd = sys::abrir(&caminho, sys::O_RDWR | sys::O_CREAT | sys::O_TRUNC).map_err(erro)?;
    sys::escrever_tudo(fd, &conteudo).map_err(erro)?;

    // privado: lê o arquivo; escrever não muda o arquivo
    let m = sys::mmap(10_000, sys::PROT_READ | sys::PROT_WRITE, sys::MAP_PRIVATE, fd, 0).map_err(erro)?;
    let visto = unsafe { core::slice::from_raw_parts(m as *const u8, 10_000) };
    if visto != &conteudo[..] {
        return Err(String::from("mapeamento privado com conteúdo errado"));
    }
    unsafe { core::ptr::write_volatile((m + 5000) as *mut u8, b'#') };
    sys::munmap(m, 10_000).map_err(erro)?;
    if sys::ler_arquivo(&caminho).map_err(erro)?[5000] != conteudo[5000] {
        return Err(String::from("escrita num MAP_PRIVATE chegou ao arquivo"));
    }

    // compartilhado, a partir da segunda página: escrever muda o arquivo
    let m = sys::mmap(4096, sys::PROT_READ | sys::PROT_WRITE, sys::MAP_SHARED, fd, 4096).map_err(erro)?;
    unsafe { core::ptr::write_volatile((m + 10) as *mut u8, b'!') };
    sys::msync(m, 4096).map_err(erro)?;
    let depois_do_msync = sys::ler_arquivo(&caminho).map_err(erro)?[4096 + 10];
    sys::munmap(m, 4096).map_err(erro)?;
    let _ = sys::fechar(fd);
    let final_ = sys::ler_arquivo(&caminho).map_err(erro)?;
    sys::remover(&caminho).map_err(erro)?;
    if depois_do_msync == b'!' && final_.len() == 10_000 && final_[4096 + 10] == b'!' {
        Ok(())
    } else {
        Err(String::from("escrita num MAP_SHARED não voltou para o arquivo"))
    }
}

fn mmap_de_arquivo() -> R {
    mmap_de_arquivo_em("/tmp")?;
    if sys::stat("/disco").is_ok() { mmap_de_arquivo_em("/disco") } else { Ok(()) }
}

fn compartilhada_entre_processos() -> R {
    let m = sys::mmap(4096, sys::PROT_READ | sys::PROT_WRITE, sys::MAP_SHARED | sys::MAP_ANONYMOUS, -1, 0)
        .map_err(erro)?;
    em_filho(|| {
        unsafe { core::ptr::write_volatile(m as *mut u32, 0xF0F0) };
        0
    })?;
    let visto = unsafe { core::ptr::read_volatile(m as *const u32) };
    sys::munmap(m, 4096).map_err(erro)?;
    if visto == 0xF0F0 { Ok(()) } else { Err(format!("o pai viu {:#x}", visto)) }
}

fn aslr() -> R {
    let mut enderecos = Vec::new();
    for _ in 0..2 {
        let (leitura, escrita) = sys::pipe().map_err(erro)?;
        em_filho(|| {
            let _ = sys::duplicar_para(escrita, 1);
            sys::executar("/bin/teste", &["teste", "--endereco-da-pilha"]);
            1
        })?;
        let _ = sys::fechar(escrita);
        enderecos.push(String::from_utf8(ler_tudo(leitura)).unwrap_or_default());
        let _ = sys::fechar(leitura);
    }
    if !enderecos[0].is_empty() && enderecos[0] != enderecos[1] {
        Ok(())
    } else {
        Err(format!("a pilha ficou no mesmo lugar: {:?}", enderecos))
    }
}

fn sem_memoria() -> R {
    // Pede 2 GiB (o QEMU tem 256 MiB) e toca tudo: o kernel tem que matar só este.
    let status = em_filho(|| {
        let tamanho = 2usize << 30;
        let Ok(m) = sys::mmap(tamanho, sys::PROT_READ | sys::PROT_WRITE, sys::MAP_PRIVATE | sys::MAP_ANONYMOUS, -1, 0)
        else {
            return 1;
        };
        for i in (0..tamanho).step_by(4096) {
            unsafe { core::ptr::write_volatile((m + i) as *mut u8, 1) };
        }
        0
    })?;
    morto_por(status, sys::SIGKILL)?;
    if livre_kib() > 100_000 { Ok(()) } else { Err(String::from("a memória não voltou")) }
}

// ------------------------------------------------------------------ ptrace

/// Começa um filho rastreado, que para (SIGSTOP) antes de rodar `f`.
fn filho_rastreado(f: fn() -> i32) -> Result<i32, String> {
    match sys::fork() {
        Ok(0) => {
            let _ = sys::ptrace(sys::PTRACE_TRACEME, 0, 0, 0);
            let _ = sys::matar(sys::pid(), sys::SIGSTOP);
            sys::sair(f());
        }
        Ok(pid) => match sys::esperar_com(pid, 0).map_err(erro)? {
            Some((_, s)) if sys::sinal_que_parou(s) == Some(sys::SIGSTOP) => Ok(pid),
            outro => Err(format!("o filho não parou: {:?}", outro)),
        },
        Err(e) => Err(erro(e)),
    }
}

/// Espera o rastreado parar com `sinal` (ou terminar: devolve None).
fn esperar_parada(pid: i32) -> Result<Option<i32>, String> {
    match sys::esperar_com(pid, 0).map_err(erro)? {
        Some((_, s)) => Ok(sys::sinal_que_parou(s)),
        None => Err(String::from("sem status")),
    }
}

fn rastrear_syscalls() -> R {
    let pid = filho_rastreado(|| {
        let _ = sys::pid(); // getpid (39)
        3
    })?;
    sys::ptrace(sys::PTRACE_SETOPTIONS, pid, 0, sys::PTRACE_O_TRACESYSGOOD).map_err(erro)?;
    let mut vistas = Vec::new();
    let mut entrando = true;
    loop {
        sys::ptrace(sys::PTRACE_SYSCALL, pid, 0, 0).map_err(erro)?;
        match sys::esperar_com(pid, 0).map_err(erro)? {
            Some((_, s)) if sys::sinal_que_parou(s) == Some(sys::SIGTRAP | 0x80) => {
                if entrando {
                    vistas.push(sys::ptrace_registradores(pid).map_err(erro)?.orig_rax());
                }
                entrando = !entrando;
            }
            Some((_, s)) if sys::codigo_de_saida(s) == Some(3) => break,
            outro => return Err(format!("status inesperado: {:?}", outro)),
        }
    }
    if vistas.contains(&39) && vistas.last() == Some(&60) {
        Ok(())
    } else {
        Err(format!("syscalls vistas: {:?}", vistas))
    }
}

static ALVO: AtomicU64 = AtomicU64::new(0x1234_5678);

fn ler_e_escrever_memoria() -> R {
    let pid = filho_rastreado(|| (ALVO.load(Ordering::SeqCst) & 0xFF) as i32)?;
    let endereco = &raw const ALVO as u64;
    let lido = sys::ptrace_ler(pid, endereco).map_err(erro)?;
    sys::ptrace(sys::PTRACE_POKEDATA, pid, endereco, 0xABCD).map_err(erro)?;
    sys::ptrace(sys::PTRACE_CONT, pid, 0, 0).map_err(erro)?;
    let (_, status) = sys::esperar(pid).map_err(erro)?;
    // O filho viu o valor novo; o pai (que divide a página por copy-on-write) não.
    match (lido, sys::codigo_de_saida(status), ALVO.load(Ordering::SeqCst)) {
        (0x1234_5678, Some(0xCD), 0x1234_5678) => Ok(()),
        outro => Err(format!("{:x?}", outro)),
    }
}

#[inline(never)]
extern "C" fn ponto_de_parada() -> i32 {
    black_box(21) * 2
}

fn breakpoint() -> R {
    let pid = filho_rastreado(|| ponto_de_parada())?;
    let endereco = ponto_de_parada as *const () as u64;
    // Troca o primeiro byte da função por `int3` (0xCC), como faz um depurador.
    let original = sys::ptrace_ler(pid, endereco).map_err(erro)?;
    sys::ptrace(sys::PTRACE_POKEDATA, pid, endereco, (original & !0xFF) | 0xCC).map_err(erro)?;
    sys::ptrace(sys::PTRACE_CONT, pid, 0, 0).map_err(erro)?;
    if esperar_parada(pid)? != Some(sys::SIGTRAP) {
        return Err(String::from("não parou no breakpoint"));
    }
    let mut regs = sys::ptrace_registradores(pid).map_err(erro)?;
    if regs.rip() != endereco + 1 {
        return Err(format!("parou em {:#x}, esperava {:#x}", regs.rip(), endereco + 1));
    }
    // Desfaz: o byte original volta e o RIP recua para refazer a instrução.
    sys::ptrace(sys::PTRACE_POKEDATA, pid, endereco, original).map_err(erro)?;
    regs.definir_rip(endereco);
    sys::ptrace_definir_registradores(pid, &regs).map_err(erro)?;
    sys::ptrace(sys::PTRACE_CONT, pid, 0, 0).map_err(erro)?;
    let (_, status) = sys::esperar(pid).map_err(erro)?;
    // O breakpoint foi só no filho: o código do pai (mesmas páginas, antes) está intacto.
    if ponto_de_parada() != 42 {
        return Err(String::from("o breakpoint apareceu no processo pai"));
    }
    if sys::codigo_de_saida(status) == Some(42) { Ok(()) } else { Err(format!("status {:#x}", status)) }
}

fn passo_a_passo() -> R {
    let pid = filho_rastreado(|| {
        let mut x = 0u64;
        for i in 0..100 {
            x = black_box(x + i);
        }
        (x % 7) as i32
    })?;
    let mut ultimo_rip = 0;
    let mut mudou = 0;
    for _ in 0..200 {
        sys::ptrace(sys::PTRACE_SINGLESTEP, pid, 0, 0).map_err(erro)?;
        if esperar_parada(pid)? != Some(sys::SIGTRAP) {
            return Err(String::from("um passo não parou com SIGTRAP"));
        }
        let rip = sys::ptrace_registradores(pid).map_err(erro)?.rip();
        mudou += (rip != ultimo_rip) as u32;
        ultimo_rip = rip;
    }
    sys::ptrace(sys::PTRACE_CONT, pid, 0, 0).map_err(erro)?;
    let (_, status) = sys::esperar(pid).map_err(erro)?;
    if mudou >= 150 && sys::codigo_de_saida(status).is_some() {
        Ok(())
    } else {
        Err(format!("o RIP mudou {} vezes em 200 passos; status {:#x}", mudou, status))
    }
}

fn ptrace_permissao() -> R {
    let status = em_filho(|| {
        if !virar_usuario_comum() {
            return 100;
        }
        match sys::ptrace(sys::PTRACE_ATTACH, 1, 0, 0) {
            Err(e) if e == sys::EPERM => 0,
            _ => 1,
        }
    })?;
    if sys::codigo_de_saida(status) == Some(0) { Ok(()) } else { Err(format!("status {:#x}", status)) }
}

// ---------------------------------------------------------- disco e montagens

/// O `make teste` liga um segundo disco (hdb) com uma tabela de partições e
/// um ext2 na partição 1.
fn montar_particao() -> R {
    if sys::stat("/dev/hdb1").is_err() {
        return Err(String::from("sem /dev/hdb1 (rode pelo `make teste`)"));
    }
    let _ = sys::criar_diretorio("/mnt/teste");
    sys::montar("/dev/hdb1", "/mnt/teste", "ext2").map_err(|e| format!("mount: {}", e))?;
    if sys::montar("/dev/hdb1", "/mnt", "ext2").is_ok() {
        return Err(String::from("montou o mesmo disco duas vezes"));
    }
    let fd = sys::abrir("/mnt/teste/nota", sys::O_WRONLY | sys::O_CREAT | sys::O_TRUNC).map_err(erro)?;
    sys::escrever_tudo(fd, b"na particao").map_err(erro)?;
    let _ = sys::fechar(fd);
    let livres = sys::statfs("/mnt/teste").map_err(erro)?.livres;
    // Ocupado: não dá para apagar o ponto de montagem.
    if sys::remover_diretorio("/mnt/teste").is_ok() {
        return Err(String::from("apagou um ponto de montagem"));
    }
    sys::desmontar("/mnt/teste").map_err(|e| format!("umount: {}", e))?;
    if sys::stat("/mnt/teste/nota").is_ok() {
        return Err(String::from("o arquivo continuou visível depois do umount"));
    }
    sys::montar("/dev/hdb1", "/mnt/teste", "ext2").map_err(|e| format!("mount de novo: {}", e))?;
    let lido = sys::ler_arquivo("/mnt/teste/nota").map_err(erro)?;
    let livres_de_novo = sys::statfs("/mnt/teste").map_err(erro)?.livres;
    sys::desmontar("/mnt/teste").map_err(erro)?;
    sys::remover_diretorio("/mnt/teste").map_err(erro)?;
    if lido == b"na particao" && livres == livres_de_novo { Ok(()) } else { Err(String::from("conteúdo errado")) }
}

fn tmpfs_e_proc() -> R {
    let _ = sys::criar_diretorio("/mnt/t");
    sys::montar("tmpfs", "/mnt/t", "tmpfs").map_err(erro)?;
    let fd = sys::abrir("/mnt/t/a", sys::O_WRONLY | sys::O_CREAT).map_err(erro)?;
    let _ = sys::fechar(fd);
    let montagens = String::from_utf8(sys::ler_arquivo("/proc/mounts").map_err(erro)?).unwrap_or_default();
    sys::desmontar("/mnt/t").map_err(erro)?;
    let sumiu = sys::stat("/mnt/t/a").is_err();
    sys::remover_diretorio("/mnt/t").map_err(erro)?;
    if montagens.contains("tmpfs /mnt/t tmpfs") && sumiu { Ok(()) } else { Err(montagens) }
}

fn disco_cru() -> R {
    // /dev/hdb é o disco inteiro: o setor 0 tem a tabela de partições (0x55 0xAA no fim).
    let fd = sys::abrir("/dev/hdb", sys::O_RDONLY).map_err(erro)?;
    let mut mbr = [0u8; 512];
    let n = sys::ler(fd, &mut mbr).map_err(erro)?;
    let _ = sys::fechar(fd);
    let usuario_nao_le = em_filho(|| {
        if !virar_usuario_comum() {
            return 100;
        }
        (sys::abrir("/dev/hdb", sys::O_RDONLY) == Err(sys::EACCES)) as i32
    })?;
    if n == 512 && mbr[510] == 0x55 && mbr[511] == 0xAA && sys::codigo_de_saida(usuario_nao_le) == Some(1) {
        Ok(())
    } else {
        Err(format!("lidos {}, assinatura {:x} {:x}", n, mbr[510], mbr[511]))
    }
}

fn truncar_em(base: &str) -> R {
    let caminho = format!("{}/teste-truncar", base);
    let livres_antes = sys::statfs(base).map_err(erro)?.livres;
    // 400 KB: no ext2, passa pelos blocos diretos, indiretos e duplo-indiretos
    let dados: Vec<u8> = (0..400_000u32).map(|i| (i % 253) as u8 + 1).collect();
    let fd = sys::abrir(&caminho, sys::O_RDWR | sys::O_CREAT | sys::O_TRUNC).map_err(erro)?;
    sys::escrever_tudo(fd, &dados).map_err(erro)?;
    sys::truncar_fd(fd, 5000).map_err(erro)?;
    let _ = sys::fechar(fd);
    let encolhido = sys::ler_arquivo(&caminho).map_err(erro)?;
    if encolhido != dados[..5000] {
        return Err(format!("depois de encolher: {} bytes", encolhido.len()));
    }
    // Crescer: o que vem depois tem que ser zero (não o conteúdo antigo).
    sys::truncar(&caminho, 20_000).map_err(erro)?;
    let crescido = sys::ler_arquivo(&caminho).map_err(erro)?;
    if crescido.len() != 20_000 || crescido[..5000] != dados[..5000] || crescido[5000..].iter().any(|&b| b != 0) {
        return Err(String::from("depois de crescer, o fim não é só zeros"));
    }
    sys::remover(&caminho).map_err(erro)?;
    let livres_depois = sys::statfs(base).map_err(erro)?.livres;
    if base == "/disco" && livres_antes != livres_depois {
        return Err(format!("blocos livres: {} antes, {} depois", livres_antes, livres_depois));
    }
    Ok(())
}

fn truncar() -> R {
    truncar_em("/tmp")?;
    truncar_em("/disco")
}

fn fifo_em(base: &str) -> R {
    let caminho = format!("{}/teste-fifo", base);
    sys::criar_fifo(&caminho, 0o600).map_err(erro)?;
    if !sys::stat(&caminho).map_err(erro)?.eh_fifo() {
        return Err(String::from("stat não diz que é FIFO"));
    }
    if sys::abrir(&caminho, sys::O_WRONLY | sys::O_NONBLOCK) != Err(sys::Erro(6)) {
        return Err(String::from("abrir para escrever sem leitor deveria dar ENXIO"));
    }
    let filho = match sys::fork() {
        Ok(0) => {
            let _ = sys::dormir_ms(30); // o pai já vai estar esperando no open
            let Ok(fd) = sys::abrir(&caminho, sys::O_WRONLY) else { sys::sair(1) };
            let _ = sys::escrever_tudo(fd, b"pelo fifo");
            sys::sair(0);
        }
        Ok(pid) => pid,
        Err(e) => return Err(erro(e)),
    };
    // Este open espera o filho abrir a outra ponta.
    let fd = sys::abrir(&caminho, sys::O_RDONLY).map_err(erro)?;
    let recebido = ler_tudo(fd);
    let _ = sys::fechar(fd);
    let _ = sys::esperar(filho);
    sys::remover(&caminho).map_err(erro)?;
    if recebido == b"pelo fifo" { Ok(()) } else { Err(format!("recebi {:?}", String::from_utf8_lossy(&recebido))) }
}

fn fifos() -> R {
    fifo_em("/tmp")?;
    fifo_em("/disco")
}

/// Lê o que houver no descritor (uma chamada só) como texto.
fn ler_texto(fd: i32) -> Result<String, String> {
    let mut buf = [0u8; 128];
    let n = sys::ler(fd, &mut buf).map_err(erro)?;
    Ok(String::from_utf8_lossy(&buf[..n]).into_owned())
}

/// Num processo filho: vira líder de uma sessão nova cujo terminal de
/// controle é o pseudo-terminal `caminho`.
fn sessao_no_pty(caminho: &str, fechar: &[i32]) -> i32 {
    for &fd in fechar {
        let _ = sys::fechar(fd);
    }
    let _ = sys::nova_sessao();
    sys::abrir(caminho, sys::O_RDWR).unwrap_or(-1)
}

fn pseudo_terminais() -> R {
    let (m, caminho) = sys::abrir_pty().map_err(erro)?;
    let s = sys::abrir(&caminho, sys::O_RDWR | sys::O_NOCTTY).map_err(erro)?;
    if sys::nome_do_terminal(s).as_deref() != Some(caminho.as_str()) {
        return Err(format!("o escravo diz ser {:?}, não {}", sys::nome_do_terminal(s), caminho));
    }
    // Escrever no mestre é digitar: a linha chega ao escravo e o eco volta ao mestre.
    sys::escrever_tudo(m, b"oi\n").map_err(erro)?;
    if ler_texto(s)? != "oi\n" {
        return Err(String::from("o escravo não recebeu a linha"));
    }
    if ler_texto(m)? != "oi\r\n" {
        return Err(String::from("o eco não voltou ao mestre (com CR LF)"));
    }
    sys::escrever_tudo(s, b"tela\n").map_err(erro)?;
    if ler_texto(m)? != "tela\r\n" {
        return Err(String::from("o que o escravo escreveu não chegou ao mestre"));
    }

    // Ctrl+C digitado no mestre: SIGINT para quem está lendo no escravo.
    let filho = match sys::fork().map_err(erro)? {
        0 => {
            let fd = sessao_no_pty(&caminho, &[m, s]);
            let mut buf = [0u8; 8];
            let _ = sys::ler(fd, &mut buf);
            sys::sair(0);
        }
        pid => pid,
    };
    let _ = sys::dormir_ms(100);
    sys::escrever_tudo(m, b"\x03").map_err(erro)?;
    let (_, status) = sys::esperar(filho).map_err(erro)?;
    morto_por(status, sys::SIGINT).map_err(|e| format!("Ctrl+C: {}", e))?;
    let _ = ler_texto(m); // o eco "^C"

    // O líder da sessão sai: o grupo em primeiro plano recebe SIGHUP.
    let (leitura, escrita) = sys::pipe().map_err(erro)?;
    let inicio = sys::tempo_ms();
    match sys::fork().map_err(erro)? {
        0 => {
            let fd = sessao_no_pty(&caminho, &[m, s, leitura]);
            match sys::fork() {
                Ok(0) => {
                    // O neto, num grupo próprio, em primeiro plano; segura a
                    // ponta de escrita do pipe até morrer.
                    let _ = sys::mudar_grupo(0, 0);
                    let _ = sys::dormir_ms(3000);
                    sys::sair(0);
                }
                Ok(neto) => {
                    let _ = sys::mudar_grupo(neto, neto);
                    let _ = sys::dar_terminal(fd, neto);
                    sys::sair(0);
                }
                Err(_) => sys::sair(1),
            }
        }
        pid => {
            let _ = sys::esperar(pid);
        }
    }
    let _ = sys::fechar(escrita);
    let _ = ler_tudo(leitura); // fim de arquivo quando o neto morrer
    let _ = sys::fechar(leitura);
    let demorou = sys::tempo_ms() - inicio;
    if demorou > 2000 {
        return Err(format!("o grupo em primeiro plano não recebeu SIGHUP ({} ms)", demorou));
    }

    // Fechar o mestre "derruba a linha": SIGHUP para o primeiro plano.
    let filho = match sys::fork().map_err(erro)? {
        0 => {
            let fd = sessao_no_pty(&caminho, &[m, s]);
            let mut buf = [0u8; 8];
            let _ = sys::ler(fd, &mut buf);
            sys::sair(0);
        }
        pid => pid,
    };
    let _ = sys::dormir_ms(100);
    let _ = sys::fechar(m);
    let (_, status) = sys::esperar(filho).map_err(erro)?;
    morto_por(status, sys::SIGHUP).map_err(|e| format!("mestre fechado: {}", e))?;
    let mut buf = [0u8; 8];
    if sys::ler(s, &mut buf) != Err(sys::EIO) {
        return Err(String::from("ler o escravo sem mestre deveria dar EIO"));
    }
    let _ = sys::fechar(s);
    if sys::abrir(&caminho, sys::O_RDWR).is_ok() {
        return Err(format!("{} deveria ter sumido", caminho));
    }
    Ok(())
}

fn interface_grafica() -> R {
    // O framebuffer: tamanho pelo ioctl, pixels pelo mmap, um dono por vez.
    let fb = sys::abrir("/dev/fb0", sys::O_RDWR).map_err(|e| format!("/dev/fb0: {}", e))?;
    let mut var = [0u8; 160];
    sys::ioctl(fb, 0x4600, &mut var).map_err(erro)?;
    let (largura, altura) = (u32::from_le_bytes(var[..4].try_into().unwrap()), u32::from_le_bytes(var[4..8].try_into().unwrap()));
    if (largura, altura) != (1024, 768) {
        return Err(format!("tela de {}x{}", largura, altura));
    }
    let tamanho = 1024 * 768 * 4;
    let p = sys::mmap(tamanho, sys::PROT_READ | sys::PROT_WRITE, sys::MAP_SHARED, fb, 0).map_err(erro)?;
    let pixels = unsafe { core::slice::from_raw_parts_mut(p as *mut u32, 1024 * 768) };
    pixels[1024 * 700 + 1000] = 0x00C0FFEE;
    let lido = unsafe { core::ptr::read_volatile(&pixels[1024 * 700 + 1000]) };
    let segundo = sys::abrir("/dev/fb0", sys::O_RDWR);
    let _ = sys::munmap(p, tamanho);
    let _ = sys::fechar(fb);
    if lido != 0x00C0FFEE {
        return Err(format!("a memória de vídeo devolveu {:#x}", lido));
    }
    if segundo != Err(sys::Erro(16)) {
        return Err(String::from("um segundo open do /dev/fb0 deveria dar EBUSY"));
    }
    // O servidor de janelas de verdade e um cliente (socket + memória compartilhada).
    let servidor = match sys::fork().map_err(erro)? {
        0 => {
            let _ = sys::executar("/bin/janelas", &["janelas"]);
            sys::sair(127);
        }
        pid => pid,
    };
    let mut janela = None;
    for _ in 0..60 {
        let _ = sys::dormir_ms(50);
        if let Ok(j) = ulib::janela::Janela::abrir(64, 32, "teste") {
            janela = Some(j);
            break;
        }
    }
    let resultado = match janela.as_mut() {
        Some(j) => {
            j.quadro().retangulo(0, 0, 64, 32, 0x00FF00);
            j.mostrar();
            if j.quadro().pixels[64 * 31 + 63] == 0x00FF00 { Ok(()) } else { Err(String::from("pixels da janela")) }
        }
        None => Err(String::from("o janelas não aceitou a conexão")),
    };
    drop(janela);
    let _ = sys::matar(servidor, sys::SIGTERM);
    let _ = sys::esperar(servidor);
    resultado?;
    // Com o servidor morto, a tela voltou ao modo texto e o /dev/fb0 está livre.
    let fb = sys::abrir("/dev/fb0", sys::O_RDWR).map_err(|e| format!("depois do servidor: {}", e))?;
    let _ = sys::fechar(fb);
    Ok(())
}

/// Roda um programa e devolve (status, o que ele escreveu na saída).
fn saida_de(caminho: &str, args: &[&str]) -> Result<(i32, String), String> {
    let (leitura, escrita) = sys::pipe().map_err(erro)?;
    let status = em_filho(|| {
        let _ = sys::duplicar_para(escrita, 1);
        let _ = sys::fechar(escrita);
        let _ = sys::fechar(leitura);
        let e = sys::executar(caminho, args);
        eprintln!("exec {}: {}", caminho, e);
        127
    })?;
    let _ = sys::fechar(escrita);
    let saida = String::from_utf8_lossy(&ler_tudo(leitura)).into_owned();
    let _ = sys::fechar(leitura);
    Ok((status, saida))
}

fn bibliotecas_dinamicas() -> R {
    // Um PIE com PT_INTERP (/lib/ld-ferrugem.so) e uma biblioteca (libsaudacao.so).
    let (status, saida) = saida_de("/bin/dinamico", &["dinamico", "teste"])?;
    if sys::codigo_de_saida(status) != Some(0) {
        return Err(format!("status {:#x}: {}", status, saida));
    }
    for esperado in ["olá, teste, da libsaudacao.so", "soma(2, 3) = 5", "cor(1) = verde", "contador = 41 -> 42"] {
        if !saida.contains(esperado) {
            return Err(format!("faltou {:?} em {:?}", esperado, saida));
        }
    }
    // ASLR: o programa e a biblioteca mudam de lugar a cada execução.
    let (_, outra) = saida_de("/bin/dinamico", &["dinamico", "teste"])?;
    let endereco = |s: &str, chave: &str| s.lines().find(|l| l.contains(chave)).map(String::from);
    if endereco(&saida, "programa em") == endereco(&outra, "programa em") || endereco(&saida, "carregada") == endereco(&outra, "carregada") {
        return Err(String::from("o programa ou a biblioteca não mudaram de endereço"));
    }
    let (_, lista) = saida_de("/bin/ldd", &["ldd", "/bin/dinamico"])?;
    if !lista.contains("libsaudacao.so => /lib/libsaudacao.so") {
        return Err(format!("ldd: {:?}", lista));
    }
    // Com a musl compartilhada instalada: um programa C comum, ligado à
    // libc.so, pelo carregador da própria musl.
    if sys::stat("/bin/ola-dinamico").is_ok() {
        let (status, saida) = saida_de("/bin/ola-dinamico", &["ola-dinamico", "x"])?;
        if sys::codigo_de_saida(status) != Some(0) || !saida.contains("malloc e snprintf funcionam: 3.142") {
            return Err(format!("ola-dinamico (musl): status {:#x}, {:?}", status, saida));
        }
        let (_, lista) = saida_de("/bin/ldd", &["ldd", "/bin/ola-dinamico"])?;
        if !lista.contains("ld-musl-x86_64.so.1") || lista.contains("Olá do C") {
            return Err(format!("ldd da musl: {:?}", lista));
        }
    }
    Ok(())
}

/// Uma conta pesada; anota em `cpus` (um bit por CPU) onde ela rodou.
fn conta_pesada(iteracoes: u64, cpus: &AtomicU64) -> u64 {
    let mut x = 1u64;
    for i in 0..iteracoes {
        x = black_box(x.wrapping_mul(6364136223846793005).wrapping_add(i));
        if i % 65536 == 0 {
            cpus.fetch_or(1 << sys::cpu_atual(), Ordering::Relaxed);
        }
    }
    x
}

fn varias_cpus() -> R {
    let info = sys::ler_arquivo("/proc/cpuinfo").map_err(erro)?;
    let n = String::from_utf8_lossy(&info).matches("processor").count();
    if n < 2 {
        return Ok(()); // com uma CPU só, não há o que provar
    }
    // Memória dividida com os filhos: a máscara das CPUs por onde passaram.
    let pagina = sys::mmap(4096, sys::PROT_READ | sys::PROT_WRITE, sys::MAP_SHARED | sys::MAP_ANONYMOUS, -1, 0)
        .map_err(erro)?;
    let cpus = unsafe { &*(pagina as *const AtomicU64) };
    // Quanto trabalho dá uns 300 ms aqui.
    let inicio = sys::tempo_ms();
    conta_pesada(2_000_000, cpus);
    let iteracoes = 2_000_000 * 300 / (sys::tempo_ms() - inicio).max(1);
    let rodar = |quantos: usize| -> Result<u64, String> {
        let inicio = sys::tempo_ms();
        let mut filhos = Vec::new();
        for _ in 0..quantos {
            match sys::fork().map_err(erro)? {
                0 => {
                    conta_pesada(iteracoes, cpus);
                    sys::sair(0);
                }
                pid => filhos.push(pid),
            }
        }
        for pid in filhos {
            let _ = sys::esperar(pid);
        }
        Ok(sys::tempo_ms() - inicio)
    };
    let sozinho = rodar(1)?;
    cpus.store(0, Ordering::Relaxed);
    let juntos = rodar(4)?;
    let usadas = cpus.load(Ordering::Relaxed).count_ones();
    let _ = sys::munmap(pagina, 4096);
    if usadas < 2 {
        return Err(format!("os 4 processos rodaram em {} CPU", usadas));
    }
    // Em série, 4 contas levariam 4x mais; em paralelo, bem menos.
    if juntos * 10 > sozinho * 30 {
        return Err(format!("sem paralelismo: 1 conta em {} ms, 4 em {} ms", sozinho, juntos));
    }
    Ok(())
}

fn fontes_truetype() -> R {
    use ulib::truetype::FonteTT;
    let Some(mut f) = FonteTT::carregar("/usr/share/fonts/DejaVuSans.ttf") else {
        return if sys::stat("/usr/share/fonts/DejaVuSans.ttf").is_ok() {
            Err(String::from("não consegui ler a DejaVuSans.ttf"))
        } else {
            Ok(()) // o sistema de quem compilou não tinha a fonte
        };
    };
    // "ã" é um glifo próprio (ou composto de "a" + "~"); um caractere que não existe vira o 0.
    if f.indice('a') == 0 || f.indice('ã') == 0 || f.indice('ã') == f.indice('a') || f.indice('\u{10FFF0}') != 0 {
        return Err(String::from("cmap: índices errados"));
    }
    let g = f.glifo('A', 32);
    let (cheios, meios) = g.alfa.iter().fold((0, 0), |(c, m), &a| (c + (a == 255) as u32, m + (a > 0 && a < 255) as u32));
    if !(18..=26).contains(&g.largura) || !(22..=28).contains(&g.altura) || cheios < 60 || meios < 20 {
        return Err(format!("'A' em 32 px: {}x{}, {} pixels cheios, {} na borda", g.largura, g.altura, cheios, meios));
    }
    // O "A" é simétrico: a cobertura de cada linha também.
    let w = g.largura as usize;
    let linha = g.altura as usize / 2;
    let soma = |de: usize, ate: usize| g.alfa[linha * w + de..linha * w + ate].iter().map(|&a| a as u32).sum::<u32>();
    let (esquerda, direita) = (soma(0, w / 2), soma(w - w / 2, w));
    if esquerda.abs_diff(direita) > 255 {
        return Err(format!("o 'A' saiu torto: {} × {}", esquerda, direita));
    }
    // Um glifo composto ("Ã" = "A" + "~") cobre mais que o "A" sozinho.
    let a: u32 = f.glifo('A', 20).alfa.iter().map(|&v| v as u32).sum();
    let a_til: u32 = f.glifo('Ã', 20).alfa.iter().map(|&v| v as u32).sum();
    if a_til <= a {
        return Err(String::from("o 'Ã' não ganhou o til"));
    }
    if f.largura("FerrugemOS", 13) < 60 {
        return Err(String::from("largura do texto"));
    }
    Ok(())
}

/// O estado de um programa de teste para a ulib::ui (desenhado numa imagem na memória).
struct ProgramaDeTeste {
    cliques: u32,
    marcada: bool,
    campo: ulib::ui::CampoTexto,
    enter: bool,
    lista: ulib::ui::Lista,
    escolhido: Option<usize>,
    dialogo: Option<ulib::ui::Dialogo>,
    resposta: Option<usize>,
}

impl ProgramaDeTeste {
    /// Um evento: o Ui o anota e o programa é desenhado (de novo, se um clique mudou algo).
    fn evento(&mut self, ui: &mut ulib::ui::Ui, pixels: &mut [u32], e: Option<ulib::janela::Evento>) {
        if let Some(e) = e {
            ui.receber(&e);
        }
        loop {
            {
                let mut t = ui.comecar(ulib::grafico::Quadro::novo(pixels, 400, 300));
                if t.botao("Contar") {
                    self.cliques += 1;
                }
                t.caixa("Marcar", &mut self.marcada);
                self.enter |= t.campo(&mut self.campo, 200);
                let itens: Vec<String> = (0..30).map(|i| format!("item {}", i)).collect();
                if let Some(i) = t.lista(&mut self.lista, &itens, 200, 100) {
                    self.escolhido = Some(i);
                }
                if let Some(b) = t.dialogo(&mut self.dialogo) {
                    self.resposta = Some(b);
                }
            }
            if !ui.precisa_redesenhar() {
                break;
            }
        }
    }
}

fn componentes_da_interface() -> R {
    use ulib::janela::{Evento, Seta};
    let mut ui = ulib::ui::Ui::novo();
    let mut pixels = vec![0u32; 400 * 300];
    let mut p = ProgramaDeTeste {
        cliques: 0,
        marcada: false,
        campo: ulib::ui::CampoTexto::novo(""),
        enter: false,
        lista: ulib::ui::Lista::default(),
        escolhido: None,
        dialogo: None,
        resposta: None,
    };
    let botao = |x, y, apertado| Some(Evento::Botao { x, y, botao: 1, apertado });
    let clicar = |p: &mut ProgramaDeTeste, ui: &mut ulib::ui::Ui, pixels: &mut [u32], x, y| {
        p.evento(ui, pixels, botao(x, y, true));
        p.evento(ui, pixels, botao(x, y, false));
    };
    // Disposição: botão em y=12, caixa em y=46, campo em y=80, lista em y=114 (100 de altura).
    p.evento(&mut ui, &mut pixels, None);
    clicar(&mut p, &mut ui, &mut pixels, 20, 20);
    if p.cliques != 1 {
        return Err(format!("o botão contou {} cliques", p.cliques));
    }
    // Apertar no botão e soltar fora dele não é um clique.
    p.evento(&mut ui, &mut pixels, botao(20, 20, true));
    p.evento(&mut ui, &mut pixels, Some(Evento::Mover { x: 300, y: 250 }));
    p.evento(&mut ui, &mut pixels, botao(300, 250, false));
    if p.cliques != 1 {
        return Err(String::from("soltar fora do botão contou um clique"));
    }
    clicar(&mut p, &mut ui, &mut pixels, 16, 59);
    if !p.marcada {
        return Err(String::from("a caixa não foi marcada"));
    }
    // O campo: clicar dá o foco; o teclado edita.
    clicar(&mut p, &mut ui, &mut pixels, 50, 93);
    for e in [
        Evento::Tecla('o'),
        Evento::Tecla('l'),
        Evento::Tecla('á'),
        Evento::Tecla('\u{8}'),
        Evento::Seta(Seta::Esquerda),
        Evento::Tecla('X'),
        Evento::Tecla('\n'),
    ] {
        p.evento(&mut ui, &mut pixels, Some(e));
    }
    if p.campo.texto != "oXl" || !p.enter {
        return Err(format!("campo: {:?} (Enter: {})", p.campo.texto, p.enter));
    }
    // Área de transferência: Ctrl+C pede para copiar o campo; Ctrl+V pede o
    // texto, que chega (Evento::Colar) como se fosse digitado.
    p.evento(&mut ui, &mut pixels, Some(Evento::Tecla('\u{3}')));
    p.evento(&mut ui, &mut pixels, Some(Evento::Tecla('\u{16}')));
    let pedidos = core::mem::take(&mut ui.pedidos);
    if !matches!(&pedidos[..], [ulib::ui::Pedido::Copiar(t), ulib::ui::Pedido::Colar] if t == "oXl") {
        return Err(String::from("Ctrl+C / Ctrl+V não pediram copiar e colar"));
    }
    p.evento(&mut ui, &mut pixels, Some(Evento::Colar(String::from("ão"))));
    if p.campo.texto != "oXãol" {
        return Err(format!("colar: {:?}", p.campo.texto));
    }
    // A lista: a roda rola, o clique escolhe, a seta desce.
    p.evento(&mut ui, &mut pixels, Some(Evento::Roda { x: 50, y: 150, giro: 2 }));
    if p.lista.rolagem <= 0 {
        return Err(String::from("a roda não rolou a lista"));
    }
    clicar(&mut p, &mut ui, &mut pixels, 50, 120);
    let Some(i) = p.escolhido.filter(|&i| i > 0 && p.lista.selecionado == Some(i)) else {
        return Err(format!("lista: escolhido {:?}, selecionado {:?}", p.escolhido, p.lista.selecionado));
    };
    p.evento(&mut ui, &mut pixels, Some(Evento::Seta(Seta::Baixo)));
    if p.lista.selecionado != Some(i + 1) {
        return Err(String::from("a seta não desceu na lista"));
    }
    // Com uma caixa de diálogo aberta, o resto da janela não recebe nada.
    p.dialogo = Some(ulib::ui::Dialogo::novo("Pergunta", "Tudo certo?", &["Sim", "Não"]));
    p.evento(&mut ui, &mut pixels, None);
    clicar(&mut p, &mut ui, &mut pixels, 20, 20);
    if p.cliques != 1 {
        return Err(String::from("o botão atrás da caixa de diálogo recebeu o clique"));
    }
    p.evento(&mut ui, &mut pixels, Some(Evento::Tecla('\n')));
    if p.resposta != Some(0) || p.dialogo.is_some() {
        return Err(format!("diálogo: resposta {:?}", p.resposta));
    }
    clicar(&mut p, &mut ui, &mut pixels, 20, 20);
    if p.cliques != 2 {
        return Err(String::from("depois do diálogo, o botão não voltou a funcionar"));
    }
    Ok(())
}

fn area_de_texto() -> R {
    use ulib::janela::{Evento, Seta};
    use ulib::ui::{AreaTexto, Pedido, Ui};
    let mut ui = Ui::novo();
    let mut pixels = vec![0u32; 400 * 300];
    let mut area = AreaTexto::novo("");
    let mut evento = |ui: &mut Ui, area: &mut AreaTexto, e: Evento| {
        ui.receber(&e);
        let mut t = ui.comecar(ulib::grafico::Quadro::novo(&mut pixels, 400, 300));
        t.area_texto(area, 376, 276);
    };
    for e in [
        Evento::Tecla('a'),
        Evento::Tecla('b'),
        Evento::Tecla('\n'),
        Evento::Tecla('c'),
        Evento::Tecla('\u{8}'),
        Evento::Colar(String::from("xy\nz")),
        Evento::Seta(Seta::Cima),
        Evento::Seta(Seta::Fim),
        Evento::Tecla('!'),
        Evento::Seta(Seta::Inicio),
        Evento::Seta(Seta::Delete),
    ] {
        evento(&mut ui, &mut area, e);
    }
    if area.texto() != "ab\ny!\nz" || !area.modificado {
        return Err(format!("texto: {:?}", area.texto()));
    }
    // Selecionar com o mouse (arrastar pela 2ª linha, "y!") e copiar.
    let (cw, lh) = ui.medidas_mono(13);
    let (x0, y0) = (12 + cw * 4 + 12, 16 + lh + lh / 2); // a 2ª linha, 1ª coluna
    evento(&mut ui, &mut area, Evento::Botao { x: x0 + 1, y: y0, botao: 1, apertado: true });
    evento(&mut ui, &mut area, Evento::Mover { x: x0 + 2 * cw, y: y0 });
    evento(&mut ui, &mut area, Evento::Botao { x: x0 + 2 * cw, y: y0, botao: 1, apertado: false });
    if area.texto_selecionado() != "y!" {
        return Err(format!("seleção: {:?}", area.texto_selecionado()));
    }
    evento(&mut ui, &mut area, Evento::Tecla('\u{3}'));
    if !matches!(ui.pedidos.first(), Some(Pedido::Copiar(t)) if t == "y!") {
        return Err(String::from("Ctrl+C não copiou a seleção"));
    }
    // Digitar com uma seleção troca o que estava selecionado.
    evento(&mut ui, &mut area, Evento::Tecla('Ç'));
    if area.texto() != "ab\nÇ\nz" {
        return Err(format!("trocar a seleção: {:?}", area.texto()));
    }
    Ok(())
}

/// O byte `i` dos dados de teste da rede (o rodar.sh gera o mesmo arquivo).
fn byte_de_teste(i: usize) -> u8 {
    (i * 7 % 251) as u8
}

fn rede_tcp_local() -> R {
    const PORTA: u16 = 7777;
    const TAMANHO: usize = 200_000;
    let ouvinte = sys::socket_inet(sys::SOCK_STREAM, 0).map_err(erro)?;
    sys::ligar_inet(ouvinte, 0x7F00_0001, PORTA).map_err(erro)?;
    sys::ouvir(ouvinte, 4).map_err(erro)?;
    // Uma porta sem ninguém: o kernel responde com RST.
    let s = sys::socket_inet(sys::SOCK_STREAM, 0).map_err(erro)?;
    match sys::conectar_inet(s, 0x7F00_0001, PORTA + 1) {
        Err(e) if e.0 == 111 => {}
        r => return Err(format!("porta fechada: esperava ECONNREFUSED, veio {:?}", r.map_err(|e| e.0))),
    }
    let _ = sys::fechar(s);
    // O cliente (filho): manda 200 KB, fecha a escrita e lê a resposta.
    let filho = match sys::fork().map_err(erro)? {
        0 => {
            let ok = (|| -> Option<()> {
                let s = sys::socket_inet(sys::SOCK_STREAM, 0).ok()?;
                sys::conectar_inet(s, 0x7F00_0001, PORTA).ok()?;
                let dados: Vec<u8> = (0..TAMANHO).map(byte_de_teste).collect();
                sys::escrever_tudo(s, &dados).ok()?;
                sys::desligar_socket(s, 1).ok()?;
                let mut volta = Vec::new();
                let mut buf = [0u8; 3000];
                loop {
                    match sys::ler(s, &mut buf).ok()? {
                        0 => break,
                        n => volta.extend_from_slice(&buf[..n]),
                    }
                }
                (volta.len() == TAMANHO && volta.iter().enumerate().all(|(i, &b)| b == byte_de_teste(i) ^ 0xFF)).then_some(())
            })();
            sys::sair(if ok.is_some() { 0 } else { 1 });
        }
        pid => pid,
    };
    // O servidor: o poll avisa a conexão chegando; lê tudo e devolve invertido.
    let pronto = sys::poll(&[(ouvinte, sys::POLLIN)], 3000).map_err(erro)?;
    if pronto[0] & sys::POLLIN == 0 {
        return Err("o poll no socket ouvindo não avisou a conexão".into());
    }
    let (c, ip, _) = sys::aceitar_inet(ouvinte).map_err(erro)?;
    if ip != 0x7F00_0001 {
        return Err(format!("accept: endereço do cliente {:#x}", ip));
    }
    let mut recebido = Vec::new();
    let mut buf = [0u8; 5000];
    loop {
        match sys::ler(c, &mut buf).map_err(erro)? {
            0 => break,
            n => recebido.extend_from_slice(&buf[..n]),
        }
    }
    if recebido.len() != TAMANHO || recebido.iter().enumerate().any(|(i, &b)| b != byte_de_teste(i)) {
        return Err(format!("o servidor recebeu {} bytes (ou bytes errados)", recebido.len()));
    }
    let volta: Vec<u8> = recebido.iter().map(|b| b ^ 0xFF).collect();
    sys::escrever_tudo(c, &volta).map_err(erro)?;
    let _ = sys::fechar(c);
    let _ = sys::fechar(ouvinte);
    match sys::esperar(filho).map_err(erro)?.1 {
        0 => Ok(()),
        s => Err(format!("o cliente não recebeu a resposta certa (status {})", s)),
    }
}

fn rede_udp_local() -> R {
    let a = sys::socket_inet(sys::SOCK_DGRAM, 0).map_err(erro)?;
    let b = sys::socket_inet(sys::SOCK_DGRAM, 0).map_err(erro)?;
    let c = sys::socket_inet(sys::SOCK_DGRAM, 0).map_err(erro)?;
    sys::ligar_inet(b, 0, 0).map_err(erro)?; // porta efêmera
    let (_, porta_b) = sys::endereco_local_inet(b).map_err(erro)?;
    sys::enviar_para_inet(a, b"primeiro", 0x7F00_0001, porta_b).map_err(erro)?;
    let (_, porta_a) = sys::endereco_local_inet(a).map_err(erro)?;
    let mut buf = [0u8; 100];
    let (n, ip, porta) = sys::receber_de_inet(b, &mut buf).map_err(erro)?;
    if &buf[..n] != b"primeiro" || ip != 0x7F00_0001 || porta != porta_a {
        return Err(format!("recebeu {:?} de {:#x}:{}", core::str::from_utf8(&buf[..n]), ip, porta));
    }
    // "Conectado" a `a`: o que vier de `c` é descartado.
    sys::conectar_inet(b, 0x7F00_0001, porta_a).map_err(erro)?;
    sys::enviar_para_inet(c, b"intruso", 0x7F00_0001, porta_b).map_err(erro)?;
    sys::enviar_para_inet(a, b"segundo", 0x7F00_0001, porta_b).map_err(erro)?;
    let (n, _, _) = sys::receber_de_inet(b, &mut buf).map_err(erro)?;
    if &buf[..n] != b"segundo" {
        return Err(format!("o socket conectado recebeu {:?}", core::str::from_utf8(&buf[..n])));
    }
    sys::escrever(b, b"resposta").map_err(erro)?; // write num UDP conectado
    let (n, _, _) = sys::receber_de_inet(a, &mut buf).map_err(erro)?;
    for fd in [a, b, c] {
        let _ = sys::fechar(fd);
    }
    if &buf[..n] != b"resposta" {
        return Err("a resposta pelo write não chegou".into());
    }
    Ok(())
}

/// Com a placa de rede (a rede "user" do QEMU): ping no roteador e, se o
/// rodar.sh tiver subido o servidor HTTP do lado de fora, um download.
fn rede_placa() -> R {
    let estado = String::from_utf8_lossy(&sys::ler_arquivo("/proc/rede").map_err(erro)?).into_owned();
    if !estado.starts_with("placa RTL8139") {
        return Ok(()); // sem placa: só o loopback
    }
    let fd = sys::socket_inet(sys::SOCK_DGRAM, sys::IPPROTO_ICMP).map_err(erro)?;
    let mut m = vec![8u8, 0, 0, 0, 0, 0, 0, 1];
    m.extend_from_slice(b"ferrugem");
    sys::enviar_para_inet(fd, &m, 0x0A00_0202, 0).map_err(erro)?;
    if sys::poll(&[(fd, sys::POLLIN)], 3000).map_err(erro)?[0] & sys::POLLIN == 0 {
        return Err("o roteador (10.0.2.2) não respondeu ao ping".into());
    }
    let mut buf = [0u8; 200];
    let (n, de, _) = sys::receber_de_inet(fd, &mut buf).map_err(erro)?;
    let _ = sys::fechar(fd);
    if de != 0x0A00_0202 || n != m.len() || buf[0] != 0 || &buf[8..n] != b"ferrugem" {
        return Err(format!("resposta de eco estranha: {} bytes de {:#x}", n, de));
    }
    if !String::from_utf8_lossy(&sys::ler_arquivo("/proc/rede").map_err(erro)?).contains("arp 10.0.2.2 ") {
        return Err("o roteador não entrou na tabela ARP".into());
    }
    // HTTP e HTTPS dos servidores que o rodar.sh sobe. Se o de HTTP não
    // estiver lá (rodando fora do `make teste`), pula.
    use ulib::http::{self, Url};
    use ulib::x509::{self, Raizes};
    let pegar = |endereco: &str, abrir: http::Abrir<'_>| -> Result<http::Resposta, String> {
        let r = http::obter(&Url::ler(endereco).unwrap(), abrir, &mut |_| {})?;
        if r.status != 200 {
            return Err(format!("{}: estado {}", endereco, r.status));
        }
        Ok(r)
    };
    // A raiz da autoridade de teste que o rodar.sh criou.
    let ca = match pegar("http://10.0.2.2:18080/ca.pem", &http::abrir_simples) {
        Ok(r) => r.corpo,
        Err(e) if e.contains("recusada") => return Ok(()), // não há servidores aqui fora
        Err(e) => return Err(e),
    };
    let mut raizes = Raizes::vazia();
    raizes.acrescentar_pem(&ca);
    if raizes.quantas() != 1 {
        return Err("não consegui ler a raiz de teste (ca.pem)".into());
    }
    let abrir = |u: &Url| http::abrir(u, Some(&raizes));
    for endereco in ["http://10.0.2.2:18080", "https://10.0.2.2:18443", "https://10.0.2.2:18444", "https://10.0.2.2:18445"] {
        let r = pegar(&format!("{}/padrao.bin", endereco), &abrir).map_err(|e| format!("{}: {}", endereco, e))?;
        if r.corpo.len() != 100_000 || !r.corpo.iter().enumerate().all(|(i, &b)| b == byte_de_teste(i)) {
            return Err(format!("{}: {} bytes (ou bytes errados)", endereco, r.corpo.len()));
        }
        let garantido = r.seguranca.as_deref().is_some_and(|s| s.contains("garantido por Ferrugem Teste CA"));
        if endereco.starts_with("https") && !garantido {
            return Err(format!("{}: {:?}", endereco, r.seguranca));
        }
    }
    // Com as autoridades do sistema, a de teste é uma desconhecida.
    match pegar("https://10.0.2.2:18443/padrao.bin", &http::abrir_simples) {
        Err(e) if e.starts_with("certificado: ") && (e.contains("não conheço") || e.contains("não há autoridades")) => {}
        Err(e) => return Err(format!("com as raízes do sistema: {}", e)),
        Ok(_) => return Err("aceitou um certificado de uma autoridade desconhecida".into()),
    }
    // A cadeia do 18445 (site ← intermediária ← raiz), conferida aqui com
    // outros nomes e outras datas.
    let cadeia = x509::ler_pem(&pegar("http://10.0.2.2:18080/cadeia.pem", &abrir)?.corpo);
    let agora = sys::hora_atual();
    for (nome, quando, problema) in [
        ("10.0.2.2", agora, ""),
        ("www.ferrugem.teste", agora, ""),
        ("ferrugem.teste", agora, "outro site"),
        ("a.b.ferrugem.teste", agora, "outro site"),
        ("10.0.2.3", agora, "outro site"),
        ("10.0.2.2", agora + 60 * 86400, "venceu"),
        ("10.0.2.2", agora - 86400, "só vale a partir"),
    ] {
        match (x509::conferir_cadeia(&cadeia, nome, quando, &raizes), problema) {
            (Ok(_), "") => {}
            (Err(e), p) if !p.is_empty() && e.contains(p) => {}
            (r, _) => return Err(format!("cadeia para {}: {:?}", nome, r.err())),
        }
    }
    match x509::conferir_cadeia(&cadeia[..1], "10.0.2.2", agora, &raizes) {
        Err(e) if e.contains("Ferrugem Teste") || e.contains("intermediaria") => Ok(()),
        r => Err(format!("sem a intermediária: {:?}", r.err())),
    }
}

/// Formulários: a codificação, e um POST de verdade para um servidor no
/// loopback (que redireciona com 303: o destino tem que vir por GET).
fn formularios() -> R {
    use ulib::http::{self, Url};
    let pares = vec![(String::from("q"), String::from("rust e c++")), (String::from("nome"), String::from("José/Ana&Cia"))];
    let dados = http::codificar_formulario(&pares);
    if dados != "q=rust+e+c%2B%2B&nome=Jos%C3%A9%2FAna%26Cia" {
        return Err(format!("codificação: {}", dados));
    }
    const PORTA: u16 = 7790;
    let ouvinte = sys::socket_inet(sys::SOCK_STREAM, 0).map_err(erro)?;
    sys::ligar_inet(ouvinte, 0x7F00_0001, PORTA).map_err(erro)?;
    sys::ouvir(ouvinte, 4).map_err(erro)?;
    let filho = match sys::fork().map_err(erro)? {
        0 => {
            let url = Url::ler(&format!("http://127.0.0.1:{}/enviar?x=1", PORTA)).unwrap();
            let r = http::obter_com(&url, &http::abrir_simples, Some(dados.as_bytes()), &mut |_| {});
            let certo = r.is_ok_and(|r| r.status == 200 && r.corpo == b"recebido" && r.url.caminho == "/pronto");
            sys::sair(if certo { 0 } else { 1 });
        }
        pid => pid,
    };
    // Lê um pedido inteiro (cabeçalho + Content-Length bytes) de uma conexão.
    let ler_pedido = |c: i32| -> Result<String, String> {
        let mut pedido = Vec::new();
        let mut buf = [0u8; 2000];
        loop {
            if let Some(fim) = pedido.windows(4).position(|j| j == b"\r\n\r\n") {
                let cabeca = String::from_utf8_lossy(&pedido[..fim]).to_ascii_lowercase();
                let n = cabeca.split("content-length:").nth(1).and_then(|r| r.lines().next()?.trim().parse().ok()).unwrap_or(0);
                if pedido.len() >= fim + 4 + n {
                    return Ok(String::from_utf8_lossy(&pedido).into_owned());
                }
            }
            match sys::ler(c, &mut buf).map_err(erro)? {
                0 => return Err("a conexão fechou no meio do pedido".into()),
                n => pedido.extend_from_slice(&buf[..n]),
            }
        }
    };
    let mut pedidos = Vec::new();
    for resposta in ["HTTP/1.1 303 See Other\r\nLocation: /pronto\r\nContent-Length: 0\r\n\r\n", "HTTP/1.1 200 OK\r\nContent-Length: 8\r\n\r\nrecebido"] {
        if sys::poll(&[(ouvinte, sys::POLLIN)], 3000).map_err(erro)?[0] & sys::POLLIN == 0 {
            return Err(format!("o cliente não conectou (pedidos até aqui: {:?})", pedidos));
        }
        let (c, _, _) = sys::aceitar_inet(ouvinte).map_err(erro)?;
        pedidos.push(ler_pedido(c)?);
        sys::escrever_tudo(c, resposta.as_bytes()).map_err(erro)?;
        let _ = sys::fechar(c);
    }
    let _ = sys::fechar(ouvinte);
    let (_, estado) = sys::esperar(filho).map_err(erro)?;
    let post = &pedidos[0];
    if !post.starts_with("POST /enviar?x=1 HTTP/1.1\r\n")
        || !post.contains("Content-Type: application/x-www-form-urlencoded\r\n")
        || !post.ends_with(&format!("\r\n\r\n{}", dados))
    {
        return Err(format!("o POST chegou assim: {:?}", post));
    }
    if !pedidos[1].starts_with("GET /pronto HTTP/1.1\r\n") || pedidos[1].contains("Content-Length") {
        return Err(format!("depois do 303: {:?}", pedidos[1]));
    }
    if estado != 0 {
        return Err("o cliente não recebeu a resposta certa".into());
    }
    Ok(())
}

fn html_e_urls() -> R {
    use ulib::html::{self, No};
    use ulib::http::Url;
    let doc = html::analisar(
        "<!DOCTYPE html><title>T&iacute;tulo</title><p>um<p>dois &amp; tr&#234;s<ul><li>a<li>b</ul>\
         <script>if (a < b) document.write('<p>')</script><a href=x.html class=c>link</a><br><img src='i.png'>",
    );
    let titulo = doc.achar("title").map(|t| t.texto()).unwrap_or_default();
    if titulo != "Título" {
        return Err(format!("título: {:?}", titulo));
    }
    let ps: Vec<String> = doc
        .filhos
        .iter()
        .filter_map(|f| match f {
            No::Elemento(e) if e.nome == "p" => Some(e.texto()),
            _ => None,
        })
        .collect();
    // o segundo <p> fecha o primeiro; a <ul> fecha o segundo; o <p> do script é texto
    if ps.len() != 2 || ps[0] != "um" || ps[1] != "dois & três" {
        return Err(format!("parágrafos: {:?}", ps));
    }
    let ul = doc.achar("ul").ok_or("sem <ul>")?;
    let itens: Vec<String> = ul.filhos.iter().filter_map(|f| if let No::Elemento(e) = f { Some(e.texto()) } else { None }).collect();
    if itens != ["a", "b"] {
        return Err(format!("itens: {:?}", itens));
    }
    if doc.achar("script").map(|s| s.texto()) != Some("if (a < b) document.write('<p>')".into()) {
        return Err("o script não ficou cru".into());
    }
    let a = doc.achar("a").ok_or("sem <a>")?;
    if a.atributo("href") != Some("x.html") || a.atributo("class") != Some("c") || doc.achar("img").and_then(|i| i.atributo("src")) != Some("i.png") {
        return Err("atributos".into());
    }
    let base = Url::ler("http://exemplo.com:8080/a/b/c.html?x=1#topo").ok_or("url base")?;
    if base.host != "exemplo.com" || base.porta != 8080 || base.caminho != "/a/b/c.html?x=1" || base.fragmento != "topo" {
        return Err(format!("url: {:?}", base));
    }
    for (relativo, esperado) in [
        ("d.html", "http://exemplo.com:8080/a/b/d.html"),
        ("../d.html", "http://exemplo.com:8080/a/d.html"),
        ("../../../d.html", "http://exemplo.com:8080/d.html"),
        ("/raiz", "http://exemplo.com:8080/raiz"),
        ("?y=2", "http://exemplo.com:8080/a/b/c.html?y=2"),
        ("#fim", "http://exemplo.com:8080/a/b/c.html?x=1#fim"),
        ("//outro.org/p", "http://outro.org/p"),
        ("https://seguro.org", "https://seguro.org/"),
        ("./", "http://exemplo.com:8080/a/b/"),
    ] {
        let u = base.juntar(relativo).ok_or_else(|| format!("juntar {:?}", relativo))?;
        if u.texto() != esperado {
            return Err(format!("{:?} → {} (esperava {})", relativo, u.texto(), esperado));
        }
    }
    if base.juntar("mailto:alguem@exemplo.com").is_some() || base.juntar("javascript:void(0)").is_some() {
        return Err("mailto:/javascript: viraram endereço".into());
    }
    Ok(())
}

fn de_hex(h: &str) -> Vec<u8> {
    (0..h.len() / 2).map(|i| u8::from_str_radix(&h[i * 2..i * 2 + 2], 16).unwrap()).collect()
}

/// SHA-512/384, RSA (PKCS#1 v1.5 e PSS) e ECDSA (P-256, P-384), com chaves e
/// assinaturas feitas pela biblioteca "cryptography" do Python.
fn assinaturas() -> R {
    use ulib::assinatura::{self, NomeCurva, Resumo};
    use ulib::sha256::hex;
    use ulib::sha512;
    if hex(&sha512::sha512(b"abc")) != "ddaf35a193617abacc417349ae20413112e6fa4e89a97ea20a9eeee64b55d39a2192992a274fc1a836ba3c23a3feebbd454d4423643ce80e2a9ac94fa54ca49f" {
        return Err(format!("SHA-512(\"abc\") = {}", hex(&sha512::sha512(b"abc"))));
    }
    if hex(&sha512::sha384(b"abc")) != "cb00753f45a35e8bb5a03d699ac65007272c32ab0eded1631a8b605a43ff5bed8086072ba1e7cc2358baeca134c825a7" {
        return Err(format!("SHA-384(\"abc\") = {}", hex(&sha512::sha384(b"abc"))));
    }
    let longo: Vec<u8> = (0..1000).map(|i| (i % 251) as u8).collect();
    if hex(&sha512::sha512(&longo)) != "5096498d96f50f9a137c4db5b8b0cd38383ad55350fb5a98805fedc31fa1262f1f0cf4d6f12d7ecd8dedd933a4c9126344fe22e937a8ad35fdeae1e876ae698b" {
        return Err("SHA-512 de 1000 bytes".into());
    }
    let mensagem = b"FerrugemOS: assinado de verdade";
    let adulterada = b"FerrugemOS: assinado de mentira";
    let n = de_hex(concat!(
        "a9fdd4b67da7e2520e4a5e95244d81c625fbe4aee5f5d952b60bb1697a7da4c1bdc680da0368145e51396ccc75a764a0",
        "580c5df18b14491e0749049ca3b85ff880e97a0b248f5a32990ea5eb97a1b30747def1d8072681b98d52c81c0598ed4e",
        "34425b0af37d2bac05c7c09d33f595ba3b4da486ea2072a66030f2108d313f3e4a31eb556a00495392e333764dbd6072",
        "0ab94cdc55983de78899e8f8a43ffe8d534f50d4ec60a3f0bac244bbe58f69b5d43c789241b827d01f737c4dbac8a849",
        "ed5adc3bf09902b33098d231f969a8eed27f5535e845edd647503b0b1f529e92d03322aa0cd8b8c2e0b37df8b9bde147",
        "851626e643c8f99088efa2f9a56ebcf7",
    ));
    let e = [1, 0, 1];
    let pkcs1 = de_hex(concat!(
        "569adaee183a0a12add1a174099c48dc353403aa0553b2451b5cb72559860bf9a1d8a490b5eb1ddd3df88f1e35078902",
        "149591724891437ffa3cbc66e9b0f5c5c0f796c38686d06c61f9c0f3c90e995cd426e2bff206155762ee900f17b2f59e",
        "baa9dae321c39bd2170e6378ba5f9dd579a5ffa3b6cc3dd92c6688aa10464e82caa715955010f36daa8e29d917fdef23",
        "332b911ae084955c493a505faa0271c61993dc91280f397f6c634f891279db6c1491198a40bb4d0ee9822dfb8d041fd6",
        "c81ea13f1586d4698e65b24fc0752095a823d0658e177d126c8092d6fb1ba9874962330fb2efbcf605de05ba319d7364",
        "5824030a88c9ea4948745e6ff2398bb2",
    ));
    let pss = de_hex(concat!(
        "4deee2fc27c1dd11d650f39ccb2241cfacbe4e8b5928f54e645a833e86e190d15bb0920a82394c6101d279ce872b005b",
        "cb4fda29ec55813ea5e62447102a760107bfe43b314e2dd3cae2c83c7b1f88b025bdc81a892aa52cb7f51de84d8b695d",
        "bec76c5c634c5678c8898f25e75b1326becb2f5a1b56a3c8ca17cf07f427335e9a4c19b917a1b3d41f4e61083d991e3c",
        "facd9b0bd485d2f370b7fb925f5f2713dce4f516b8cbe3cef90f5740c27a911b0338b94d8298c4c11d37d1fae370fed4",
        "dfda0bb02cbc487949174555b446182ad36beb1a5ed5003ed9b437964bf2e9934a066746ed708c8e44b4fcbf84b56155",
        "c6240fe7c136440807ffa2076cb8b92b",
    ));
    let p256 = de_hex(concat!(
        "0469019b2bb7dba81a7ad925c55b93234511dd84358ee2dba851deb104025482fb6f46ba67296010cf5b1782b37c7175",
        "816d9b45113482ba1f5e800b76e81128c4",
    ));
    let p256_sig = de_hex(concat!(
        "304402203f362e4d2676bb76b14a39f5483d439c3186db82e261ffe91aa822a5e19cc1d902202633848726eb22ea013f",
        "b0b3dc7ae829233b35b6b25988af9a54f841d8240987",
    ));
    let p384 = de_hex(concat!(
        "04bdb18580f63947bed5752735cb3cfed1eb29cfc8ee91451732ee034ec889c31898385825299bd133504eaa1cc45221",
        "6fc690aaf8eea3656a8980bc340910a1c46de773acb4a84e6b1ea9829851e7fc875a9c1bb2e475b92d823150a545df8a",
        "67",
    ));
    let p384_sig = de_hex(concat!(
        "3066023100fa2e1366d06a00e5ff0e530871f101b9eb2af596251fa1a1d033dbd65c448d2b4971073528bb13ad610ea9",
        "f5da6bb535023100cbb836844c73b5571163823a7591c938a226398d2d77e72a6d31f1652e8d5cfaa2122e50c4e1d8c1",
        "dd1ce937d7d7599f",
    ));
    let casos: [(&str, &dyn Fn(&[u8]) -> bool); 4] = [
        ("RSA PKCS#1 v1.5 (SHA-256)", &|m| assinatura::rsa_pkcs1(&n, &e, Resumo::Sha256, &Resumo::Sha256.calcular(m), &pkcs1)),
        ("RSA-PSS (SHA-384)", &|m| assinatura::rsa_pss(&n, &e, Resumo::Sha384, &Resumo::Sha384.calcular(m), &pss)),
        ("ECDSA P-256", &|m| assinatura::ecdsa(NomeCurva::P256, &p256, &Resumo::Sha256.calcular(m), &p256_sig)),
        ("ECDSA P-384", &|m| assinatura::ecdsa(NomeCurva::P384, &p384, &Resumo::Sha384.calcular(m), &p384_sig)),
    ];
    for (nome, conferir) in casos {
        if !conferir(mensagem) {
            return Err(format!("{}: a assinatura certa não conferiu", nome));
        }
        if conferir(adulterada) {
            return Err(format!("{}: conferiu a assinatura de outra mensagem", nome));
        }
    }
    Ok(())
}

/// Os vetores de teste das RFCs, para cada peça do TLS.
fn criptografia() -> R {
    use ulib::cripto;
    use ulib::sha256::hex;
    let confere = |nome: &str, obtido: &[u8], esperado: &str| -> R {
        if hex(obtido) == esperado { Ok(()) } else { Err(format!("{}: {} (esperava {})", nome, hex(obtido), esperado)) }
    };
    // RFC 7748, 5.2 e 6.1
    let k: [u8; 32] = de_hex("a546e36bf0527c9d3b16154b82465edd62144c0ac1fc5a18506a2244ba449ac4").try_into().unwrap();
    let u: [u8; 32] = de_hex("e6db6867583030db3594c1a424b15f7c726624ec26b3353b10a903a6d0ab1c4c").try_into().unwrap();
    confere("X25519", &cripto::x25519(&k, &u), "c3da55379de9c6908e94ea4df28d084f32eccf03491c71f754b4075577a28552")?;
    let alice: [u8; 32] = de_hex("77076d0a7318a57d3c16c17251b26645df4c2f87ebc0992ab177fba51db92c2a").try_into().unwrap();
    let bob: [u8; 32] = de_hex("5dab087e624a8a4b79e17f8b83800ee66f3bb1292618b6fd1c2f8b27ff88e0eb").try_into().unwrap();
    let (pa, pb) = (cripto::x25519_publica(&alice), cripto::x25519_publica(&bob));
    confere("X25519 (pública de Alice)", &pa, "8520f0098930a754748b7ddcb43ef75a0dbf3a0d26381af4eba4a98eaa9b4e6a")?;
    confere("X25519 (pública de Bob)", &pb, "de9edb7d7b7dc1b4d35b61c2ece435373f8343c85b78674dadfc7e146f882b4f")?;
    let segredo = cripto::x25519(&alice, &pb);
    confere("X25519 (segredo)", &segredo, "4a5d9d5ba4ce2de1728e3bf480350f25e07e21c947d19e3376f09b3c1e161742")?;
    if cripto::x25519(&bob, &pa) != segredo {
        return Err("X25519: Alice e Bob chegaram a segredos diferentes".into());
    }
    // RFC 8439, 2.8.2
    let chave: [u8; 32] = (0x80..0xa0).collect::<Vec<u8>>().try_into().unwrap();
    let nonce: [u8; 12] = de_hex("070000004041424344454647").try_into().unwrap();
    let extra = de_hex("50515253c0c1c2c3c4c5c6c7");
    let texto = b"Ladies and Gentlemen of the class of '99: If I could offer you only one tip for the future, sunscreen would be it.";
    let cifrado = cripto::chacha20_poly1305_cifrar(&chave, &nonce, &extra, texto);
    confere("ChaCha20 (começo)", &cifrado[..16], "d31a8d34648e60db7b86afbc53ef7ec2")?;
    confere("Poly1305 (etiqueta)", &cifrado[cifrado.len() - 16..], "1ae10b594f09e26a7e902ecbd0600691")?;
    if cripto::chacha20_poly1305_decifrar(&chave, &nonce, &extra, &cifrado).as_deref() != Some(&texto[..]) {
        return Err("ChaCha20-Poly1305: não decifrou de volta".into());
    }
    let mut adulterado = cifrado.clone();
    adulterado[5] ^= 1;
    if cripto::chacha20_poly1305_decifrar(&chave, &nonce, &extra, &adulterado).is_some() {
        return Err("ChaCha20-Poly1305: aceitou um texto adulterado".into());
    }
    // A especificação do GCM (McGrew e Viega), casos 1, 2 e 3
    let zero = cripto::Aes128::novo(&[0; 16]);
    confere("AES-GCM (caso 1)", &zero.gcm_cifrar(&[0; 12], &[], &[]), "58e2fccefa7e3061367f1d57a4e7455a")?;
    confere("AES-GCM (caso 2)", &zero.gcm_cifrar(&[0; 12], &[], &[0; 16]), "0388dace60b6a392f328c2b971b2fe78ab6e47d42cec13bdf53a67b21257bddf")?;
    let aes = cripto::Aes128::novo(&de_hex("feffe9928665731c6d6a8f9467308308").try_into().unwrap());
    let iv: [u8; 12] = de_hex("cafebabefacedbaddecaf888").try_into().unwrap();
    let p = de_hex("d9313225f88406e5a55909c5aff5269a86a7a9531534f7da2e4c303d8a318a721c3c0c95956809532fcf0e2449a6b525b16aedf5aa0de657ba637b391aafd255");
    let c = aes.gcm_cifrar(&iv, &[], &p);
    confere("AES-GCM (caso 3, etiqueta)", &c[c.len() - 16..], "4d5c2af327cd64a62cf35abd2ba6fab4")?;
    if aes.gcm_decifrar(&iv, &[], &c).as_deref() != Some(&p[..]) {
        return Err("AES-GCM: não decifrou de volta".into());
    }
    // RFC 5869, caso 1
    let prk = cripto::hkdf_extrair(&de_hex("000102030405060708090a0b0c"), &[0x0b; 22]);
    confere("HKDF (extrair)", &prk, "077709362c2e32df0ddc3f0dc47bba6390b6c73bb50f9c3122ec844ad7c2b3e5")?;
    let okm = cripto::hkdf_expandir(&prk, &de_hex("f0f1f2f3f4f5f6f7f8f9"), 42);
    confere("HKDF (expandir)", &okm, "3cb25f25faacd57a90434f64d0362f2a2d2d0a90cf1a5a4c5db02d56ecc4c5bf34007208d5b887185865")?;
    // SHA-256 aos pedaços = de uma vez
    let mut h = ulib::sha256::Sha256::novo();
    for parte in texto.chunks(7) {
        h.atualizar(parte);
    }
    if h.terminar() != ulib::sha256::sha256(texto) {
        return Err("SHA-256 aos pedaços deu diferente".into());
    }
    Ok(())
}

fn png_ler_e_gravar() -> R {
    use ulib::png;
    let abrir = |nome: &str| -> Result<png::Imagem, String> {
        let dados = sys::ler_arquivo(&format!("/usr/share/imagens/{}", nome)).map_err(|e| format!("{}: {}", nome, e))?;
        png::decodificar(&dados).ok_or_else(|| format!("{}: não decodificou", nome))
    };
    // Cada pixel das imagens de teste é uma fórmula (veja ferramentas/imagens.py).
    let rgba = abrir("testes/rgba-entrelacado.png")?; // Adam7, os 5 filtros
    for y in 0..23u32 {
        for x in 0..37u32 {
            let esperado = (255 - x) << 24 | (x * 7 % 256) << 16 | (y * 11 % 256) << 8 | ((x + y) * 3 % 256);
            if rgba.pixels[(y * 37 + x) as usize] != esperado {
                return Err(format!("rgba-entrelacado ({}, {}): {:#x}", x, y, rgba.pixels[(y * 37 + x) as usize]));
            }
        }
    }
    let paleta = abrir("testes/paleta.png")?; // 4 bits por pixel, com transparência
    for y in 0..10u32 {
        for x in 0..20u32 {
            let i = (x + 2 * y) % 16;
            let alfa = if i == 0 { 0 } else { 0xFF };
            let esperado = alfa << 24 | (i * 16) << 16 | (255 - i * 16) << 8 | (i * 37 % 256);
            if paleta.pixels[(y * 20 + x) as usize] != esperado {
                return Err(format!("paleta ({}, {})", x, y));
            }
        }
    }
    let cinza = abrir("testes/cinza16.png")?; // 16 bits por canal
    for y in 0..8u32 {
        for x in 0..16u32 {
            let g = (x * 4000 + y * 100) >> 8;
            if cinza.pixels[(y * 16 + x) as usize] != 0xFF00_0000 | g << 16 | g << 8 | g {
                return Err(format!("cinza16 ({}, {})", x, y));
            }
        }
    }
    // Uma imagem de verdade, com Huffman dinâmico (gravada pelo Pillow).
    let m = abrir("mandelbrot.png")?;
    if (m.largura, m.altura) != (640, 480) {
        return Err(String::from("mandelbrot: tamanho"));
    }
    // Ida e volta pelo nosso codificador.
    let original: Vec<u32> = (0..50 * 30).map(|i| (i * 2654435761u32) & 0xFFFFFF).collect();
    let volta = png::decodificar(&png::codificar(&original, 50, 30)).ok_or("não leu o próprio PNG")?;
    if volta.pixels.iter().zip(&original).any(|(a, b)| *a != 0xFF00_0000 | b) {
        return Err(String::from("o PNG gravado não voltou igual"));
    }
    Ok(())
}

fn principal(args: &[&str]) -> i32 {
    if args.get(1) == Some(&"--endereco-da-pilha") {
        let local = 0u8;
        print!("{:p}", &raw const local);
        return 0;
    }
    if args.get(1) == Some(&"--identidade") {
        print!("euid={} uid={}", sys::euid(), sys::uid());
        return 0;
    }
    let testes: [(&str, fn() -> R); 68] = [
        ("fork, exit e wait", fork_exit_wait),
        ("pipe entre dois processos", pipe_entre_processos),
        ("heap do programa (4 MiB + 5000 Strings)", heap_grande),
        ("page fault mata só o processo (SIGSEGV)", page_fault),
        ("programa não lê a memória do kernel", ler_o_kernel),
        ("programa não escreve no próprio código", escrever_no_codigo),
        ("divisão por zero (SIGFPE)", divisao_por_zero),
        ("instrução privilegiada no anel 3", instrucao_privilegiada),
        ("ponteiro inválido numa syscall (EFAULT)", ponteiro_invalido_na_syscall),
        ("arquivos: criar, posicionar, ler, append, apagar", arquivos),
        ("diretórios: mkdir, rename, cd, rmdir", diretorios),
        ("preempção: matar um laço infinito", preempcao),
        ("40 processos ao mesmo tempo", muitos_processos),
        ("timer: sleep de 200 ms", timer),
        ("exec com argumentos e saída num pipe", exec_com_pipe),
        ("exec de arquivo inexistente (ENOENT)", exec_inexistente),
        ("escrever em pipe sem leitor (SIGPIPE)", sigpipe),
        ("órfãos são adotados pelo init", orfaos),
        ("memória volta depois de 20 processos", memoria_devolvida),
        ("disco ext2: 300 KB, renomear, apagar devolve os blocos", disco_ext2),
        ("sinal com tratador próprio (registradores preservados)", tratador_de_sinal),
        ("sinal bloqueado espera ser desbloqueado", mascara_de_sinais),
        ("alarm(1) + pause", alarme_e_pause),
        ("tratador de SIGSEGV", tratador_de_sigsegv),
        ("SA_RESTART recomeça um read interrompido", sa_restart),
        ("sem SA_RESTART o read devolve EINTR", sem_sa_restart),
        ("fork com copy-on-write (4 MiB só copiados ao escrever)", copia_na_escrita),
        ("permissões: usuário comum barrado onde deve", permissoes),
        ("programa setuid roda com o dono do arquivo", programa_setuid),
        ("SIGSTOP para, wait(WUNTRACED) vê, SIGCONT continua", parar_e_continuar),
        ("parado no meio de um sleep, continua o sleep", sleep_sobrevive_a_parada),
        ("kill(-grupo) atinge só o grupo", sinal_para_grupo),
        ("links simbólicos e físicos na memória", links_na_memoria),
        ("links simbólicos e físicos no ext2", links_no_disco),
        ("páginas sob demanda: mmap de 64 MiB só gasta o que é tocado", paginas_sob_demanda),
        ("a pilha cresce sozinha (3 MiB)", pilha_cresce),
        ("pilha estourada (> 8 MiB) dá SIGSEGV", pilha_estoura),
        ("NX: código na pilha não executa", nx_na_pilha),
        ("mprotect tira e devolve a escrita", mprotect_funciona),
        ("mmap de arquivo: privado e compartilhado (memória e ext2)", mmap_de_arquivo),
        ("MAP_SHARED anônimo é o mesmo no pai e no filho", compartilhada_entre_processos),
        ("ASLR: a pilha muda de lugar a cada execução", aslr),
        ("sem memória: só o culpado morre", sem_memoria),
        ("ptrace: parar em cada syscall", rastrear_syscalls),
        ("ptrace: ler e escrever a memória do rastreado", ler_e_escrever_memoria),
        ("ptrace: breakpoint com int3", breakpoint),
        ("ptrace: passo a passo (200 instruções)", passo_a_passo),
        ("ptrace: usuário comum não rastreia o init", ptrace_permissao),
        ("mount/umount de uma partição (hdb1) e statfs", montar_particao),
        ("tmpfs e /proc/mounts", tmpfs_e_proc),
        ("disco cru (/dev/hdb): tabela de partições; só o root lê", disco_cru),
        ("truncate: encolher (400 KB → 5 KB) e crescer com zeros", truncar),
        ("FIFOs: open espera a outra ponta (memória e ext2)", fifos),
        ("pseudo-terminais: eco, Ctrl+C e SIGHUP", pseudo_terminais),
        ("interface gráfica: framebuffer, servidor de janelas e um cliente", interface_grafica),
        ("bibliotecas dinâmicas: ld-ferrugem.so, ld-musl, PIE e ASLR", bibliotecas_dinamicas),
        ("várias CPUs: 4 processos em CPUs diferentes, ao mesmo tempo", varias_cpus),
        ("fontes TrueType: cmap, glifos compostos, suavização", fontes_truetype),
        ("componentes da interface: botão, caixa, campo, lista, diálogo", componentes_da_interface),
        ("área de texto do editor: teclado, colar, seleção pelo mouse", area_de_texto),
        ("PNG: inflate, filtros, Adam7, paleta, 16 bits; ida e volta", png_ler_e_gravar),
        ("TCP pelo loopback: 200 KB e de volta, recusa, poll no accept", rede_tcp_local),
        ("UDP pelo loopback: endereços, porta efêmera, socket conectado", rede_udp_local),
        ("placa de rede: ARP, ping; HTTP e HTTPS (cifras, certificados e cadeias)", rede_placa),
        ("HTML tolerante (fechamentos implícitos, entidades) e URLs relativas", html_e_urls),
        ("formulários: codificação, POST e o GET depois do 303", formularios),
        ("criptografia: X25519, ChaCha20-Poly1305, AES-GCM e HKDF (vetores das RFCs)", criptografia),
        ("assinaturas: SHA-512/384, RSA (PKCS#1 e PSS), ECDSA P-256 e P-384", assinaturas),
    ];

    // `teste trecho`: só os testes cujo nome contém o trecho.
    let filtro = args.get(1).copied().unwrap_or("");
    let testes: Vec<_> = testes.into_iter().filter(|(nome, _)| nome.contains(filtro)).collect();
    println!("Rodando {} testes do kernel...", testes.len());
    let mut falhas = 0;
    for (nome, teste) in testes.iter().copied() {
        match teste() {
            Ok(()) => println!("  \x1b[92m[ OK ]\x1b[0m {}", nome),
            Err(motivo) => {
                falhas += 1;
                println!("  \x1b[91m[FALHOU]\x1b[0m {}: {}", nome, motivo);
            }
        }
    }
    if falhas == 0 {
        println!("\x1b[92mTodos os {} testes passaram.\x1b[0m", testes.len());
    } else {
        println!("\x1b[91m{} de {} testes falharam.\x1b[0m", falhas, testes.len());
    }
    falhas
}
