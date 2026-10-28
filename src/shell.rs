//! Monitor do kernel: um shell embutido, com edição de linha e histórico
//! (setas ↑/↓). No boot ele inicia o /bin/sh; ao sair dele, volta para cá.

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicUsize, Ordering};

use crate::console;
use crate::cpu;
use crate::elf;
use crate::heap;
use crate::input;
use crate::memory;
use crate::multiboot::BootInfo;
use crate::power;
use crate::process;
use crate::signal::{self, SIGINT};
use crate::task::{self, KillError, Pid, State, WUNTRACED, WaitTarget};
use crate::timer;
use crate::vfs;
use crate::vga::{self, Color};
use crate::tty;

const MAX_LINE: usize = 200;
/// Diretório a partir do qual o monitor resolve caminhos relativos.
const MONITOR_CWD: &str = "/home";
const ENVIRONMENT: [&str; 4] = ["PATH=/bin", "HOME=/home", "TERM=ferrugem", "USER=root"];

pub fn run(boot: BootInfo) -> ! {
    if vfs::resolve("/", "/bin/sh").is_ok() {
        println!("Iniciando /bin/sh ('exit' volta ao monitor do kernel).\n");
        run_program("sh", &[]);
        println!("\nDe volta ao monitor do kernel: 'sh' reabre o shell, 'ajuda' lista os comandos.");
    }
    let mut history: Vec<String> = Vec::new();
    loop {
        prompt();
        let line = read_line(&history);
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        if history.last().map(String::as_str) != Some(line) {
            history.push(line.into());
        }
        execute(line, &boot);
    }
}

fn prompt() {
    console::set_color(Color::LightGreen);
    print!("ferrugem");
    console::reset_color();
    print!("> ");
}

fn read_line(history: &[String]) -> String {
    let mut line = String::new();
    let mut position = history.len();
    loop {
        match input::read_byte() {
            b'\n' => {
                println!();
                return line;
            }
            0x08 => {
                if line.pop().is_some() {
                    print!("\x08 \x08");
                }
            }
            0x03 => {
                println!("^C");
                return String::new();
            }
            0x0C => {
                console::clear();
                prompt();
                print!("{line}");
            }
            0x15 => replace_line(&mut line, ""),
            0x1B => {
                if input::read_byte() != b'[' {
                    continue;
                }
                match input::read_byte() {
                    b'A' if position > 0 => {
                        position -= 1;
                        replace_line(&mut line, &history[position]);
                    }
                    b'B' if position < history.len() => {
                        position += 1;
                        replace_line(&mut line, history.get(position).map_or("", String::as_str));
                    }
                    _ => {}
                }
            }
            byte @ 0x20..=0x7E if line.len() < MAX_LINE => {
                line.push(byte as char);
                print!("{}", byte as char);
            }
            _ => {}
        }
    }
}

fn replace_line(line: &mut String, new: &str) {
    for _ in 0..line.len() {
        print!("\x08 \x08");
    }
    line.clear();
    line.push_str(new);
    print!("{line}");
}

fn execute(line: &str, boot: &BootInfo) {
    let (command, rest) = line.split_once(' ').map_or((line, ""), |(c, r)| (c, r.trim()));
    let args: Vec<&str> = rest.split_whitespace().collect();

    match command {
        "ajuda" | "help" => help(),
        "limpar" | "clear" => console::clear(),
        "eco" | "echo" => println!("{rest}"),
        "sobre" | "about" => about(boot),
        "mem" => mem(),
        "tempo" | "uptime" => println!("ligado há {}", format_duration(timer::uptime_ms())),
        "ps" => ps(),
        "matar" | "kill" => kill(&args),
        "fg" | "continuar" => resume(&args),
        "girar" | "spin" => spin(),
        "primos" | "primes" => primes(&args),
        "ls" => ls(&args),
        "cat" => cat(&args),
        "escrever" | "write" => write_file(rest, false),
        "anexar" | "append" => write_file(rest, true),
        "rm" => rm(&args),
        "cpu" => cpu_info(),
        "cores" | "colors" => colors(),
        "dormir" | "sleep" => sleep(&args),
        "int3" => unsafe { core::arch::asm!("int3") },
        "panico" | "panic" => panic!("pânico pedido pelo usuário"),
        "reiniciar" | "reboot" => power::reboot(),
        "desligar" | "shutdown" => power::shutdown(),
        _ => run_program(command, &args),
    }
}

/// Executa um programa (de /bin, ou um caminho) como processo de usuário.
/// Com `&` no fim, roda em segundo plano.
fn run_program(name: &str, args: &[&str]) {
    let path = if name.contains('/') { vfs::normalize(MONITOR_CWD, name) } else { format!("/bin/{name}") };
    if !vfs::read_file("/", &path).is_ok_and(|data| elf::is_elf(&data)) {
        return error(format_args!("comando desconhecido: {name} (digite 'ajuda')"));
    }
    let (args, background) = match args.split_last() {
        Some((&"&", rest)) => (rest, true),
        _ => (args, false),
    };
    let mut argv = Vec::with_capacity(args.len() + 1);
    argv.push(name);
    argv.extend_from_slice(args);

    let pid = match process::spawn(&path, &argv, &ENVIRONMENT) {
        Ok(pid) => pid,
        Err(message) => return error(format_args!("{name}: {message}")),
    };
    if background {
        println!("[{pid}] {name}");
    } else {
        foreground(pid);
    }
}

/// Dá o terminal ao processo `pid` (líder de grupo e sessão) e espera ele
/// terminar ou parar.
fn foreground(pid: Pid) {
    let Ok((pgid, session)) = task::ids_of(pid) else { return };
    tty::set_foreground(pgid, session);
    let result = task::wait_child(WaitTarget::Pid(pid), WUNTRACED);
    tty::set_foreground(0, 0);
    let Ok(Some((_, status))) = result else { return };
    console::set_color(Color::DarkGray);
    match status & 0x7F {
        0x7F => println!("[{pid}] parado por {}; 'fg {pid}' continua", signal::name(status >> 8 & 0xFF)),
        0 if status >> 8 != 0 => println!("[{pid}] terminou com código {}", status >> 8 & 0xFF),
        0 => {}
        signal if signal != SIGINT => println!("[{pid}] encerrado por {}", signal::name(signal)),
        _ => {}
    }
    console::reset_color();
}

fn resume(args: &[&str]) {
    let Some(pid) = args.first().and_then(|a| a.parse().ok()) else {
        return error(format_args!("uso: fg <pid>"));
    };
    match task::resume(pid) {
        Ok(()) => foreground(pid),
        Err(_) => error(format_args!("não existe tarefa {pid}")),
    }
}

fn error(message: core::fmt::Arguments) {
    console::set_color(Color::LightRed);
    println!("{message}");
    console::reset_color();
}

fn help() {
    const COMMANDS: &[(&str, &str)] = &[
        ("ajuda", "mostra esta lista"),
        ("limpar", "limpa a tela (ou Ctrl+L)"),
        ("eco <texto>", "imprime o texto"),
        ("sobre", "informações do sistema"),
        ("mem", "uso de memória física e do heap"),
        ("tempo", "tempo desde o boot"),
        ("ps", "lista as tarefas"),
        ("matar <pid>", "encerra uma tarefa (SIGKILL)"),
        ("fg <pid>", "continua um processo parado em primeiro plano"),
        ("girar", "cria uma tarefa animada na barra de status"),
        ("<programa> [args] [&]", "executa um programa de /bin no anel 3"),
        ("sh", "o shell Unix (pipes, redirecionamento, jobs)"),
        ("primos <n>", "conta primos até n em segundo plano"),
        ("ls [dir] / cat <arq>", "lista / mostra arquivos (base: /home)"),
        ("escrever <arq> <txt>", "cria ou substitui um arquivo"),
        ("anexar <arq> <txt>", "acrescenta uma linha a um arquivo"),
        ("rm <arq>", "apaga um arquivo"),
        ("cpu", "identifica o processador"),
        ("cores", "mostra a paleta VGA"),
        ("dormir <ms>", "espera (cedendo a CPU)"),
        ("int3 / panico", "testa exceções"),
        ("reiniciar / desligar", "energia"),
    ];
    for (name, description) in COMMANDS {
        console::set_color(Color::LightCyan);
        print!("  {name:<22}");
        console::reset_color();
        println!("{description}");
    }
    println!("Também aceita os nomes em inglês (help, clear, kill, write...).");
}

fn about(boot: &BootInfo) {
    let (start, end) = memory::kernel_range();
    println!("FerrugemOS {} - kernel x86_64 escrito do zero em Rust", env!("CARGO_PKG_VERSION"));
    println!("  carregador: {}", boot.loader_name());
    if !boot.cmdline().is_empty() {
        println!("  linha de comando: {}", boot.cmdline());
    }
    println!("  kernel: {:#x}..{:#x} ({})", start, end, format_size(end - start));
    println!("  timer: {} Hz, tarefa atual: pid {}", timer::HZ, task::current_pid());
}

fn mem() {
    let (total, free) = memory::stats();
    let (heap_size, heap_used) = heap::stats();
    println!("memória física: {} livres de {}", format_size(free), format_size(total));
    println!("heap do kernel: {} usados de {}", format_size(heap_used as u64), format_size(heap_size as u64));
}

fn ps() {
    println!(
        "{:>5} {:>5} {:>5}  {:>4}  {:<10} {:>8}  {:>8}  NOME",
        "PID", "PAI", "GRUPO", "ANEL", "ESTADO", "CPU", "MEMÓRIA"
    );
    for t in task::list() {
        let state = match t.state {
            State::Ready => "pronta",
            State::Sleeping { .. } => "dormindo",
            State::Blocked { .. } => "esperando",
            State::Stopped => "parada",
            State::Dead => "morta",
        };
        let (ring, memory) = if t.user { ("3", format_size(t.memory)) } else { ("0", String::from("-")) };
        let cpu = format!("{}ms", t.cpu_ms);
        println!(
            "{:>5} {:>5} {:>5}  {ring:>4}  {state:<10} {cpu:>8}  {memory:>8}  {}",
            t.pid, t.parent, t.pgid, t.name
        );
    }
}

fn kill(args: &[&str]) {
    let Some(pid) = args.first().and_then(|a| a.parse().ok()) else {
        return error(format_args!("uso: matar <pid>"));
    };
    match task::terminate(pid) {
        Ok(()) => println!("tarefa {pid} encerrada"),
        Err(KillError::NotFound) => error(format_args!("não existe tarefa {pid}")),
        Err(KillError::Protected) => error(format_args!("a tarefa {pid} não pode ser encerrada")),
    }
}

static SPINNERS: AtomicUsize = AtomicUsize::new(0);

fn spin() {
    let slot = SPINNERS.fetch_add(1, Ordering::Relaxed) % 8;
    let pid = task::spawn("girador", move || {
        const FRAMES: &[u8] = b"|/-\\";
        let col = vga::WIDTH - 2 - slot * 2;
        let color = vga::attr(Color::ALL[9 + slot % 7], Color::Blue);
        for i in 0.. {
            vga::put(0, col, FRAMES[i % FRAMES.len()], color);
            task::sleep_ms(100 + 40 * slot as u64);
        }
    });
    println!("girador criado (pid {pid}); use 'ps' e 'matar {pid}'");
}

fn primes(args: &[&str]) {
    let Some(limit) = args.first().and_then(|a| a.parse::<u64>().ok()) else {
        return error(format_args!("uso: primos <n>"));
    };
    let pid = task::spawn("primos", move || {
        let start = timer::uptime_ms();
        let mut count = 0u64;
        let mut last = 0;
        for n in 2..=limit {
            if (2..).take_while(|d| d * d <= n).all(|d| n % d != 0) {
                count += 1;
                last = n;
            }
        }
        let elapsed = timer::uptime_ms() - start;
        let report = format!("{count} primos até {limit} (maior: {last}), em {elapsed} ms\n");
        let _ = vfs::write_file("/", "/tmp/primos.txt", report.as_bytes(), false);
        console::set_color(Color::Yellow);
        print!("\n[primos] {report}");
        console::reset_color();
    });
    println!("calculando em segundo plano (pid {pid}); resultado em /tmp/primos.txt");
}

fn ls(args: &[&str]) {
    let path = args.first().copied().unwrap_or(".");
    let entries = match vfs::resolve(MONITOR_CWD, path).and_then(|dir| dir.list()) {
        Ok(entries) => entries,
        Err(_) => return error(format_args!("não é um diretório: {path}")),
    };
    for (name, node) in entries {
        if node.is_dir() {
            console::set_color(Color::LightBlue);
            println!("  {:>8}  {name}/", "-");
        } else {
            print!("  {:>8}  ", format_size(node.size()));
            if node.mode & 0o111 != 0 {
                console::set_color(Color::LightGreen);
            }
            println!("{name}");
        }
        console::reset_color();
    }
}

fn cat(args: &[&str]) {
    let Some(path) = args.first() else {
        return error(format_args!("uso: cat <arquivo>"));
    };
    match vfs::read_file(MONITOR_CWD, path) {
        Ok(data) => {
            let text = String::from_utf8_lossy(&data);
            print!("{text}");
            if !text.ends_with('\n') && !text.is_empty() {
                println!();
            }
        }
        Err(_) => error(format_args!("não foi possível ler {path}")),
    }
}

fn write_file(rest: &str, append: bool) {
    let Some((path, text)) = rest.split_once(' ') else {
        return error(format_args!("uso: {} <arquivo> <texto>", if append { "anexar" } else { "escrever" }));
    };
    let content = format!("{}\n", text.trim());
    if vfs::write_file(MONITOR_CWD, path, content.as_bytes(), append).is_err() {
        error(format_args!("não foi possível escrever em {path}"));
    }
}

fn rm(args: &[&str]) {
    for path in args {
        if vfs::remove(MONITOR_CWD, path, false).is_err() {
            error(format_args!("não foi possível apagar {path}"));
        }
    }
}

fn cpu_info() {
    let vendor = cpu::cpuid(0);
    let mut name = [0u8; 12];
    name[0..4].copy_from_slice(&vendor.ebx.to_le_bytes());
    name[4..8].copy_from_slice(&vendor.edx.to_le_bytes());
    name[8..12].copy_from_slice(&vendor.ecx.to_le_bytes());
    println!("fabricante: {}", core::str::from_utf8(&name).unwrap_or("?"));

    if cpu::cpuid(0x8000_0000).eax >= 0x8000_0004 {
        let mut brand = [0u8; 48];
        for (i, leaf) in (0x8000_0002..=0x8000_0004).enumerate() {
            let r = cpu::cpuid(leaf);
            for (j, reg) in [r.eax, r.ebx, r.ecx, r.edx].iter().enumerate() {
                brand[i * 16 + j * 4..][..4].copy_from_slice(&reg.to_le_bytes());
            }
        }
        let brand = core::str::from_utf8(&brand).unwrap_or("?");
        println!("modelo: {}", brand.trim_matches(|c: char| c == '\0' || c == ' '));
    }

    let features = cpu::cpuid(1);
    let flags = [
        (features.edx & (1 << 25) != 0, "sse"),
        (features.edx & (1 << 26) != 0, "sse2"),
        (features.ecx & (1 << 0) != 0, "sse3"),
        (features.ecx & (1 << 19) != 0, "sse4.1"),
        (features.ecx & (1 << 20) != 0, "sse4.2"),
        (features.ecx & (1 << 28) != 0, "avx"),
        (features.ecx & (1 << 31) != 0, "hipervisor"),
    ];
    print!("recursos:");
    for (_, flag) in flags.iter().filter(|(present, _)| *present) {
        print!(" {flag}");
    }
    println!();
}

fn colors() {
    for color in Color::ALL {
        console::set_color(color);
        print!("██ ");
        console::reset_color();
        println!("{:>2} {}", color as u8, color.name());
    }
}

fn sleep(args: &[&str]) {
    let Some(ms) = args.first().and_then(|a| a.parse().ok()) else {
        return error(format_args!("uso: dormir <ms>"));
    };
    task::sleep_ms(ms);
}

pub fn format_size(bytes: u64) -> String {
    match bytes {
        b if b >= 1 << 30 => format!("{} GiB", b >> 30),
        b if b >= 1 << 20 => format!("{} MiB", b >> 20),
        b if b >= 1 << 10 => format!("{} KiB", b >> 10),
        b => format!("{b} B"),
    }
}

pub fn format_duration(ms: u64) -> String {
    let seconds = ms / 1000;
    format!("{:02}:{:02}:{:02}", seconds / 3600, seconds / 60 % 60, seconds % 60)
}
