//! rastro — mostra cada chamada de sistema que um programa faz (como o `strace`).
//!
//! ```text
//!   rastro ls /tmp
//!   rastro -c cat /etc/motd       (só a contagem, no fim)
//! ```
//!
//! Funciona com o ptrace: o filho pede para ser rastreado (PTRACE_TRACEME) e
//! executa o programa; o kernel o para na entrada e na saída de cada syscall,
//! e nós lemos os registradores (número da chamada, argumentos, resultado) e a
//! memória dele (para mostrar caminhos e textos). Funciona com qualquer
//! programa: os nossos, os em C com a musl e o Lua.

#![no_std]
#![no_main]

use ulib::sys::{self, Registradores};
use ulib::*;

programa!(principal);

/// Como mostrar cada argumento: d = número, x = hexadecimal, s = texto (ponteiro),
/// b = buffer (ponteiro; o tamanho é o argumento seguinte), f = descritor do diretório.
const CHAMADAS: &[(u64, &str, &str)] = &[
    (0, "read", "dbd"), (1, "write", "dbd"), (2, "open", "sxo"), (3, "close", "d"),
    (4, "stat", "sx"), (5, "fstat", "dx"), (6, "lstat", "sx"), (7, "poll", "xdd"),
    (8, "lseek", "ddd"), (9, "mmap", "xdxxdx"), (10, "mprotect", "xdx"), (11, "munmap", "xd"),
    (12, "brk", "x"), (13, "rt_sigaction", "dxx"), (14, "rt_sigprocmask", "dxx"),
    (15, "rt_sigreturn", ""), (16, "ioctl", "dxx"), (19, "readv", "dxd"), (20, "writev", "dxd"),
    (21, "access", "so"), (22, "pipe", "x"), (23, "select", "dxxxx"), (24, "sched_yield", ""),
    (26, "msync", "xdx"), (28, "madvise", "xdd"), (32, "dup", "d"), (33, "dup2", "dd"), (34, "pause", ""),
    (35, "nanosleep", "xx"), (37, "alarm", "d"), (39, "getpid", ""), (56, "clone", "xxxxx"),
    (57, "fork", ""), (58, "vfork", ""), (59, "execve", "sxx"), (60, "exit", "d"),
    (61, "wait4", "dxdx"), (62, "kill", "dd"), (63, "uname", "x"), (72, "fcntl", "ddx"),
    (74, "fsync", "d"), (77, "ftruncate", "dd"), (79, "getcwd", "xd"), (80, "chdir", "s"),
    (82, "rename", "ss"), (83, "mkdir", "so"), (84, "rmdir", "s"), (86, "link", "ss"),
    (87, "unlink", "s"), (88, "symlink", "ss"), (89, "readlink", "sxd"), (90, "chmod", "so"),
    (92, "chown", "sdd"), (95, "umask", "o"), (96, "gettimeofday", "xx"), (101, "ptrace", "ddxx"),
    (102, "getuid", ""), (104, "getgid", ""), (105, "setuid", "d"), (106, "setgid", "d"),
    (107, "geteuid", ""), (108, "getegid", ""), (109, "setpgid", "dd"), (110, "getppid", ""),
    (111, "getpgrp", ""), (112, "setsid", ""), (121, "getpgid", "d"), (124, "getsid", "d"),
    (158, "arch_prctl", "xx"), (186, "gettid", ""), (200, "tkill", "dd"), (201, "time", "x"),
    (202, "futex", "xddxxd"), (217, "getdents64", "dxd"), (218, "set_tid_address", "x"),
    (228, "clock_gettime", "dx"), (230, "clock_nanosleep", "ddxx"), (231, "exit_group", "d"),
    (234, "tgkill", "ddd"), (257, "openat", "fsxo"), (258, "mkdirat", "fso"),
    (262, "newfstatat", "fsxx"), (263, "unlinkat", "fsx"), (264, "renameat", "fsfs"),
    (265, "linkat", "fsfsx"), (266, "symlinkat", "sfs"), (267, "readlinkat", "fsxd"),
    (269, "faccessat", "fso"), (270, "pselect6", "dxxxxx"), (271, "ppoll", "xdxx"),
    (292, "dup3", "ddx"), (293, "pipe2", "xx"), (302, "prlimit64", "ddxx"), (318, "getrandom", "xdx"),
    (1000, "ferrugem_processos", "xd"), (1001, "ferrugem_memoria", "x"), (1002, "ferrugem_disco", "x"),
];

const ERROS: &[(i64, &str)] = &[
    (1, "EPERM"), (2, "ENOENT"), (3, "ESRCH"), (4, "EINTR"), (5, "EIO"), (8, "ENOEXEC"), (9, "EBADF"),
    (10, "ECHILD"), (11, "EAGAIN"), (12, "ENOMEM"), (13, "EACCES"), (14, "EFAULT"), (16, "EBUSY"),
    (17, "EEXIST"), (18, "EXDEV"), (19, "ENODEV"), (20, "ENOTDIR"), (21, "EISDIR"), (22, "EINVAL"),
    (24, "EMFILE"), (25, "ENOTTY"), (28, "ENOSPC"), (29, "ESPIPE"), (32, "EPIPE"), (34, "ERANGE"),
    (36, "ENAMETOOLONG"), (38, "ENOSYS"), (39, "ENOTEMPTY"), (40, "ELOOP"), (110, "ETIMEDOUT"),
];

/// Lê uma string (ou um buffer) da memória do rastreado, 8 bytes por vez.
fn ler_memoria(pid: i32, mut endereco: u64, maximo: usize, ate_zero: bool) -> Option<Vec<u8>> {
    let mut bytes = Vec::new();
    while bytes.len() < maximo {
        let palavra = sys::ptrace_ler(pid, endereco).ok()?;
        for b in palavra.to_le_bytes() {
            if ate_zero && b == 0 || bytes.len() >= maximo {
                return Some(bytes);
            }
            bytes.push(b);
        }
        endereco += 8;
    }
    Some(bytes)
}

fn texto_escapado(bytes: &[u8], cortado: bool) -> String {
    let mut s = String::from("\"");
    for c in String::from_utf8_lossy(bytes).chars() {
        match c {
            '\n' => s.push_str("\\n"),
            '\t' => s.push_str("\\t"),
            '"' => s.push_str("\\\""),
            c if c.is_control() => s.push_str(&format!("\\x{:02x}", c as u32)),
            c => s.push(c),
        }
    }
    s.push('"');
    if cortado {
        s.push_str("...");
    }
    s
}

fn formatar(pid: i32, numero: u64, args: &[u64; 6], resultado: Option<i64>) -> String {
    let (nome, tipos) = CHAMADAS
        .iter()
        .find(|(n, _, _)| *n == numero)
        .map(|(_, nome, tipos)| (String::from(*nome), *tipos))
        .unwrap_or_else(|| (format!("syscall_{}", numero), "xxx"));
    let mut partes = Vec::new();
    for (i, tipo) in tipos.bytes().enumerate() {
        let v = args[i];
        partes.push(match tipo {
            b'd' => format!("{}", v as i64),
            b'o' => format!("0{:o}", v),
            b'f' if v as i64 == -100 => String::from("AT_FDCWD"),
            b'f' => format!("{}", v as i64),
            b's' if v == 0 => String::from("NULL"),
            b's' => match ler_memoria(pid, v, 64, true) {
                Some(b) => texto_escapado(&b, b.len() == 64),
                None => format!("{:#x}", v),
            },
            b'b' => {
                // write: o conteúdo já existe na entrada; read: só depois de ler.
                let tamanho = args[i + 1] as usize;
                let disponivel = if numero == 0 { resultado.unwrap_or(0).max(0) as usize } else { tamanho };
                let n = disponivel.min(32);
                match ler_memoria(pid, v, n, false) {
                    Some(b) if n > 0 => texto_escapado(&b, disponivel > 32),
                    _ => format!("{:#x}", v),
                }
            }
            _ => format!("{:#x}", v),
        });
    }
    format!("{}({})", nome, partes.join(", "))
}

fn resultado(r: i64) -> String {
    if (-4095..0).contains(&r) {
        let nome = ERROS.iter().find(|(n, _)| *n == -r).map_or("?", |(_, s)| s);
        format!("-1 {} ({})", nome, sys::Erro(-r).descricao())
    } else if r > 0xFFFF {
        format!("{:#x}", r)
    } else {
        format!("{}", r)
    }
}

fn principal(args: &[&str]) -> i32 {
    let so_contar = args.get(1) == Some(&"-c");
    let comando = &args[if so_contar { 2 } else { 1 }..];
    let Some(&programa) = comando.first() else {
        eprintln!("uso: rastro [-c] <programa> [argumentos...]");
        return 2;
    };
    let caminho = if programa.contains('/') { String::from(programa) } else { format!("/bin/{}", programa) };

    let pid = match sys::fork() {
        Ok(0) => {
            let _ = sys::ptrace(sys::PTRACE_TRACEME, 0, 0, 0);
            let _ = sys::matar(sys::pid(), sys::SIGSTOP); // espera o rastro se preparar
            let e = sys::executar(&caminho, comando);
            eprintln!("rastro: {}: {}", caminho, e);
            sys::sair(127);
        }
        Ok(pid) => pid,
        Err(e) => {
            eprintln!("rastro: fork: {}", e);
            return 1;
        }
    };
    let _ = sys::esperar_com(pid, 0); // a parada do SIGSTOP
    let _ = sys::ptrace(sys::PTRACE_SETOPTIONS, pid, 0, sys::PTRACE_O_TRACESYSGOOD);

    let mut contagem: Vec<(u64, u32, u32)> = Vec::new(); // (número, chamadas, erros)
    let mut entrada: Option<Registradores> = None;
    let mut sinal = 0;
    let codigo = loop {
        if sys::ptrace(sys::PTRACE_SYSCALL, pid, 0, sinal).is_err() {
            break 1;
        }
        sinal = 0;
        let status = match sys::esperar_com(pid, 0) {
            Ok(Some((_, s))) => s,
            _ => break 1,
        };
        if let Some(c) = sys::codigo_de_saida(status) {
            eprintln!("+++ saiu com {} +++", c);
            break c;
        }
        if let Some(s) = sys::sinal_que_matou(status) {
            eprintln!("+++ morto por {} ({}) +++", s, sys::nome_do_sinal(s));
            break 128 + s;
        }
        let Some(parou) = sys::sinal_que_parou(status) else { continue };
        if parou != sys::SIGTRAP | 0x80 {
            if parou != sys::SIGTRAP {
                // Um sinal de verdade: mostra e entrega ao programa.
                eprintln!("--- sinal {} ({}) ---", parou, sys::nome_do_sinal(parou));
                sinal = parou as u64;
            }
            continue;
        }
        let Ok(regs) = sys::ptrace_registradores(pid) else { continue };
        match entrada.take() {
            None => {
                entrada = Some(regs);
                // exit e exit_group não voltam: mostra já na entrada.
                if matches!(regs.orig_rax(), 60 | 231) && !so_contar {
                    eprintln!("{} = ?", formatar(pid, regs.orig_rax(), &regs.argumentos(), None));
                }
            }
            Some(e) => {
                let r = regs.rax() as i64;
                let numero = e.orig_rax();
                match contagem.iter_mut().find(|(n, _, _)| *n == numero) {
                    Some(c) => {
                        c.1 += 1;
                        c.2 += (-4095..0).contains(&r) as u32;
                    }
                    None => contagem.push((numero, 1, (-4095..0).contains(&r) as u32)),
                }
                if !so_contar {
                    eprintln!("{} = {}", formatar(pid, numero, &e.argumentos(), Some(r)), resultado(r));
                }
            }
        }
    };

    if so_contar {
        contagem.sort_by(|a, b| b.1.cmp(&a.1));
        eprintln!("  chamadas   erros  syscall");
        eprintln!("  --------  ------  ----------------");
        let mut total = 0;
        for (numero, chamadas, erros) in &contagem {
            let nome = CHAMADAS.iter().find(|(n, _, _)| n == numero).map_or("?", |(_, s, _)| s);
            eprintln!("  {:>8}  {:>6}  {}", chamadas, erros, nome);
            total += chamadas;
        }
        eprintln!("  {:>8}          total", total);
    }
    codigo
}
