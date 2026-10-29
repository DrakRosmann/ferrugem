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
    let testes: [(&str, fn() -> R); 55] = [
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
    ];

    println!("Rodando {} testes do kernel...", testes.len());
    let mut falhas = 0;
    for (nome, teste) in testes {
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
