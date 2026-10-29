//! teste — bateria de testes do kernel, rodando como um programa comum.
//!
//! Cada teste usa as chamadas de sistema de verdade. Os que provocam erros
//! fatais (page fault, divisão por zero...) rodam num processo filho: o
//! kernel precisa matar só o filho, com o sinal certo, e seguir em frente.

#![no_std]
#![no_main]

use core::arch::asm;
use core::hint::black_box;
use core::sync::atomic::{AtomicI32, Ordering};
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

fn principal(args: &[&str]) -> i32 {
    if args.get(1) == Some(&"--identidade") {
        print!("euid={} uid={}", sys::euid(), sys::uid());
        return 0;
    }
    let testes: [(&str, fn() -> R); 29] = [
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
