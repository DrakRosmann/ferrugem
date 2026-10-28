//! O terminal (console VGA + serial) visto pelos processos.
//!
//! - Modo canônico (padrão): o kernel edita a linha (eco, backspace,
//!   Ctrl+U) e o `read` só devolve dados depois do Enter.
//! - Modo cru: cada byte é entregue assim que chega.
//! - Grupo em primeiro plano: só ele lê do terminal e recebe os sinais das
//!   teclas de controle. Um grupo em segundo plano que tenta ler recebe
//!   SIGTTIN (e para).
//!
//! O modo é controlado pela estrutura `termios`, via ioctl.

use alloc::collections::VecDeque;
use alloc::string::String;
use alloc::vec::Vec;
use core::mem::size_of;

use crate::input;
use crate::signal::{SIGINT, SIGQUIT, SIGTSTP, SIGTTIN, bit};
use crate::sync::SpinLock;
use crate::syscall::{self, EAGAIN, EINTR, EIO, ENOTTY, EPERM, Errno};
use crate::task::{self, Pid};

// Bits de c_lflag e c_iflag usados aqui.
const ISIG: u32 = 0x1;
const ICANON: u32 = 0x2;
const ECHO: u32 = 0x8;
const ICRNL: u32 = 0x100;

// Índices de c_cc.
const VINTR: usize = 0;
const VQUIT: usize = 1;
const VERASE: usize = 2;
const VKILL: usize = 3;
const VEOF: usize = 4;
const VMIN: usize = 6;
const VSUSP: usize = 10;

// ioctls de terminal do Linux.
const TCGETS: u64 = 0x5401;
const TCSETS: u64 = 0x5402;
const TCSETSW: u64 = 0x5403;
const TCSETSF: u64 = 0x5404;
const TIOCSCTTY: u64 = 0x540E;
const TIOCGPGRP: u64 = 0x540F;
const TIOCSPGRP: u64 = 0x5410;
const TIOCGWINSZ: u64 = 0x5413;
const TIOCGSID: u64 = 0x5429;

/// O `struct termios` do kernel Linux (36 bytes).
#[derive(Clone, Copy)]
#[repr(C)]
struct Termios {
    iflag: u32,
    oflag: u32,
    cflag: u32,
    lflag: u32,
    line: u8,
    cc: [u8; 19],
}

const DEFAULT_TERMIOS: Termios = Termios {
    iflag: ICRNL | 0x400,      // ICRNL | IXON
    oflag: 0x5,                // OPOST | ONLCR
    cflag: 0xBF,               // B38400 | CS8 | CREAD
    lflag: 0x8A3B,             // ISIG | ICANON | ECHO | ECHOE | ECHOK | ECHOCTL | ECHOKE | IEXTEN
    line: 0,
    cc: [3, 0x1C, 0x7F, 0x15, 4, 0, 1, 0, 0x11, 0x13, 0x1A, 0, 0x12, 0x0F, 0x17, 0x16, 0, 0, 0],
};

struct Tty {
    termios: Termios,
    /// Grupo em primeiro plano; 0 quando o monitor do kernel está no comando.
    foreground: Pid,
    session: Pid,
    /// Linha sendo editada (modo canônico).
    line: Vec<u8>,
    /// Bytes já liberados para leitura.
    ready: VecDeque<u8>,
    /// Um Ctrl+D numa linha vazia, ainda não entregue como fim de arquivo.
    eof: bool,
    /// Bytes de um caractere UTF-8 incompleto, esperando o resto para o eco.
    partial: Vec<u8>,
}

impl Tty {
    /// Ecoa um byte; um caractere de vários bytes (ç, ã...) só é mostrado
    /// quando chega inteiro.
    fn echo(&mut self, byte: u8, out: &mut String) {
        if byte < 0x80 && self.partial.is_empty() {
            out.push(byte as char);
            return;
        }
        self.partial.push(byte);
        match core::str::from_utf8(&self.partial) {
            Ok(text) => out.push_str(text),
            Err(error) if error.error_len().is_none() => return, // falta byte
            Err(_) => out.push_str(&String::from_utf8_lossy(&self.partial)),
        }
        self.partial.clear();
    }
}

static TTY: SpinLock<Tty> = SpinLock::new(Tty {
    termios: DEFAULT_TERMIOS,
    foreground: 0,
    session: 0,
    line: Vec::new(),
    ready: VecDeque::new(),
    eof: false,
    partial: Vec::new(),
});

/// Entrega o terminal a uma sessão e grupo (0, 0 = de volta ao monitor).
/// Uma sessão nova começa com o termios padrão.
pub fn set_foreground(pgid: Pid, session: Pid) {
    let mut tty = TTY.lock();
    if tty.session != session {
        tty.termios = DEFAULT_TERMIOS;
        tty.line.clear();
        tty.ready.clear();
        tty.eof = false;
    }
    tty.foreground = pgid;
    tty.session = session;
}

/// Chamado na chegada de cada byte (em contexto de interrupção). Devolve
/// `true` se o byte era uma tecla de controle que virou sinal.
pub fn intercept(byte: u8) -> bool {
    let (group, signal, echo) = {
        let mut tty = TTY.lock();
        if tty.foreground == 0 || tty.termios.lflag & ISIG == 0 {
            return false;
        }
        let cc = tty.termios.cc;
        let (signal, echo) = match byte {
            b if b == cc[VINTR] => (SIGINT, "^C\n"),
            b if b == cc[VSUSP] => (SIGTSTP, "^Z\n"),
            b if b == cc[VQUIT] => (SIGQUIT, "^\\\n"),
            _ => return false,
        };
        tty.line.clear();
        tty.ready.clear();
        (tty.foreground, signal, echo)
    };
    input::flush();
    crate::print!("{echo}");
    task::signal_group(group, signal);
    true
}

pub fn write(bytes: &[u8]) -> usize {
    crate::print!("{}", String::from_utf8_lossy(bytes));
    bytes.len()
}

/// Um grupo em segundo plano tentando ler: recebe SIGTTIN, ou EIO se o
/// sinal estiver bloqueado ou ignorado.
fn check_foreground() -> Result<(), Errno> {
    let (_, _, pgid, _) = task::current_ids();
    let foreground = TTY.lock().foreground;
    if foreground == pgid {
        return Ok(());
    }
    let refuses =
        task::with_current_process(|p| p.signals.ignores(SIGTTIN) || p.signals.mask & bit(SIGTTIN) != 0);
    if refuses.unwrap_or(true) {
        return Err(EIO);
    }
    task::signal_group(pgid, SIGTTIN);
    Err(EINTR) // ao ser continuado, o read recomeça
}

pub fn read(max: usize, nonblocking: bool) -> Result<Vec<u8>, Errno> {
    check_foreground()?;
    loop {
        let seen = input::QUEUE.generation();
        {
            let mut tty = TTY.lock();
            if tty.ready.is_empty() && core::mem::take(&mut tty.eof) {
                return Ok(Vec::new());
            }
            if !tty.ready.is_empty() {
                let count = max.min(tty.ready.len());
                return Ok(tty.ready.drain(..count).collect());
            }
        }
        match input::pop() {
            Some(byte) => {
                if process_byte(byte) {
                    return Ok(Vec::new()); // fim de arquivo (Ctrl+D numa linha vazia)
                }
            }
            None => {
                let vmin = TTY.lock().termios.cc[VMIN];
                if nonblocking {
                    return Err(EAGAIN);
                }
                if vmin == 0 && TTY.lock().termios.lflag & ICANON == 0 {
                    return Ok(Vec::new());
                }
                if task::signal_pending() {
                    return Err(EINTR);
                }
                task::block_on(&input::QUEUE, seen);
            }
        }
    }
}

/// Para `poll`: há algo para ler? Passa pela disciplina de linha o que já
/// foi digitado (em modo canônico, só uma linha completa conta).
pub fn readable() -> bool {
    while let Some(byte) = input::pop() {
        if process_byte(byte) {
            TTY.lock().eof = true;
        }
    }
    let tty = TTY.lock();
    !tty.ready.is_empty() || tty.eof
}

/// Aplica a disciplina de linha a um byte recebido. Devolve `true` para
/// fim de arquivo.
fn process_byte(mut byte: u8) -> bool {
    let mut out = String::new();
    let mut eof = false;
    {
        let mut tty = TTY.lock();
        let termios = tty.termios;
        let echo = termios.lflag & ECHO != 0;
        if byte == b'\r' && termios.iflag & ICRNL != 0 {
            byte = b'\n';
        }
        if termios.lflag & ICANON == 0 {
            tty.ready.push_back(byte);
            if echo {
                tty.echo(byte, &mut out);
            }
        } else {
            eof = edit_line(&mut tty, byte, echo, &mut out);
        }
    }
    if !out.is_empty() {
        crate::print!("{out}");
    }
    eof
}

/// Modo canônico: edição da linha (apagar, matar a linha, fim de arquivo).
/// Devolve `true` para fim de arquivo.
fn edit_line(tty: &mut Tty, byte: u8, echo: bool, out: &mut String) -> bool {
    let cc = tty.termios.cc;
    let mut eof = false;
    {
        match byte {
            b'\n' => {
                tty.line.push(b'\n');
                let line = core::mem::take(&mut tty.line);
                tty.ready.extend(line);
                if echo {
                    out.push('\n');
                }
            }
            b if b == cc[VEOF] => {
                if tty.line.is_empty() {
                    eof = true;
                } else {
                    let line = core::mem::take(&mut tty.line);
                    tty.ready.extend(line);
                }
            }
            b if b == cc[VERASE] || b == 0x08 => {
                // Apaga um caractere inteiro, mesmo que tenha vários bytes.
                let mut erased = false;
                while let Some(last) = tty.line.pop() {
                    erased = true;
                    if last & 0xC0 != 0x80 {
                        break; // chegou ao primeiro byte do caractere
                    }
                }
                if erased && echo {
                    out.push_str("\x08 \x08");
                }
            }
            b if b == cc[VKILL] => {
                if echo {
                    let width = String::from_utf8_lossy(&tty.line).chars().count();
                    (0..width).for_each(|_| out.push_str("\x08 \x08"));
                }
                tty.line.clear();
            }
            // Texto, inclusive os bytes de caracteres UTF-8 (≥ 0x80).
            b'\t' | 0x20..=0x7E | 0x80..=0xFF => {
                tty.line.push(byte);
                if echo {
                    tty.echo(byte, out);
                }
            }
            _ => {} // outras teclas de controle e sequências de escape
        }
    }
    eof
}

pub fn ioctl(request: u64, arg: u64) -> Result<i64, Errno> {
    let (_, _, _, my_session) = task::current_ids();
    match request {
        TCGETS => {
            let termios = TTY.lock().termios;
            let bytes = unsafe {
                core::slice::from_raw_parts(&termios as *const Termios as *const u8, size_of::<Termios>())
            };
            syscall::user_slice(arg, bytes.len() as u64, true)?.copy_from_slice(bytes);
            Ok(0)
        }
        TCSETS | TCSETSW | TCSETSF => {
            let bytes = syscall::user_slice(arg, size_of::<Termios>() as u64, false)?;
            let termios = unsafe { (bytes.as_ptr() as *const Termios).read_unaligned() };
            let mut tty = TTY.lock();
            if request == TCSETSF {
                tty.line.clear();
                tty.ready.clear();
            }
            tty.termios = termios;
            Ok(0)
        }
        TIOCGPGRP => {
            let foreground = TTY.lock().foreground as u32;
            syscall::user_slice(arg, 4, true)?.copy_from_slice(&foreground.to_le_bytes());
            Ok(0)
        }
        TIOCSPGRP => {
            let bytes = syscall::user_slice(arg, 4, false)?;
            let pgid = u32::from_le_bytes(bytes.try_into().unwrap()) as Pid;
            let mut tty = TTY.lock();
            if tty.session != my_session {
                return Err(ENOTTY);
            }
            if !task::group_in_session(pgid, my_session) {
                return Err(EPERM);
            }
            tty.foreground = pgid;
            Ok(0)
        }
        TIOCGSID => {
            let session = TTY.lock().session as u32;
            syscall::user_slice(arg, 4, true)?.copy_from_slice(&session.to_le_bytes());
            Ok(0)
        }
        TIOCSCTTY => Ok(0),
        TIOCGWINSZ => {
            // 24 linhas x 80 colunas (a linha 0 da tela é a barra de status).
            let size: [u16; 4] = [24, 80, 0, 0];
            let out = syscall::user_slice(arg, 8, true)?;
            for (i, value) in size.iter().enumerate() {
                out[i * 2..i * 2 + 2].copy_from_slice(&value.to_le_bytes());
            }
            Ok(0)
        }
        _ => Err(ENOTTY),
    }
}
