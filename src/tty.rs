//! O terminal (`/dev/tty`): liga o teclado e a serial aos programas.
//!
//! No modo "canônico" (o padrão do Unix), quem edita a linha é o kernel: ele
//! ecoa o que você digita, trata o Backspace e só entrega a linha ao programa
//! quando você aperta Enter. Ele também transforma teclas especiais:
//!
//! ```text
//!   Ctrl+C  → sinal SIGINT para os programas do terminal
//!   Ctrl+D  → fim de arquivo (o `read` devolve 0) se a linha estiver vazia
//! ```

use crate::erro::{EFAULT, EINTR, ENOTTY};
use crate::processo;
use crate::spin::Mutex;
use crate::{print, println};
use alloc::collections::VecDeque;
use alloc::string::String;

struct Tty {
    linha: String,          // o que está sendo digitado
    prontos: VecDeque<u8>,  // linhas completas, esperando um `read`
    fins: usize,            // Ctrl+D pendentes
    interrupcoes: u64,      // quantos Ctrl+C já houve
    /// A `struct termios` do Linux (36 bytes): os programas ligam e desligam
    /// o eco e o modo canônico com ioctl(TCSETS). O que importa aqui é c_lflag.
    termios: [u8; 36],
}

const ISIG: u32 = 0o1;
const ICANON: u32 = 0o2;
const ECHO: u32 = 0o10;

const TERMIOS_PADRAO: [u8; 36] = {
    let mut t = [0u8; 36];
    let campos: [u32; 4] = [
        0o400,                         // c_iflag: ICRNL
        0o5,                           // c_oflag: OPOST | ONLCR
        0o17 | 0o60 | 0o200,           // c_cflag: 38400 baud, 8 bits, CREAD
        ISIG | ICANON | ECHO | 0o60 | 0o100000, // c_lflag: + ECHOE, ECHOK, IEXTEN
    ];
    let mut i = 0;
    while i < 4 {
        let b = campos[i].to_le_bytes();
        t[i * 4] = b[0];
        t[i * 4 + 1] = b[1];
        t[i * 4 + 2] = b[2];
        t[i * 4 + 3] = b[3];
        i += 1;
    }
    // c_cc (começa no byte 17): INTR, QUIT, ERASE, KILL, EOF, TIME, MIN
    t[17] = 3;
    t[18] = 0x1C;
    t[19] = 0x7F;
    t[20] = 0x15;
    t[21] = 4;
    t[22] = 0;
    t[23] = 1;
    t
};

static TTY: Mutex<Tty> = Mutex::new(Tty {
    linha: String::new(),
    prontos: VecDeque::new(),
    fins: 0,
    interrupcoes: 0,
    termios: TERMIOS_PADRAO,
});

impl Tty {
    fn lflag(&self) -> u32 {
        u32::from_le_bytes(self.termios[12..16].try_into().unwrap())
    }
}

pub const TCGETS: u64 = 0x5401;
pub const TCSETS: u64 = 0x5402;
const TCSETSW: u64 = 0x5403;
const TCSETSF: u64 = 0x5404;
pub const TIOCGWINSZ: u64 = 0x5413;
const TIOCGPGRP: u64 = 0x540F;
const TIOCSPGRP: u64 = 0x5410;

/// Os `ioctl` de terminal. `dados` é o que o programa mandou (para os SETs);
/// a resposta é o que deve ser copiado de volta para ele.
pub fn ioctl(pedido: u64, dados: Option<&[u8]>, pid: u32) -> Result<alloc::vec::Vec<u8>, i64> {
    let mut tty = TTY.lock();
    match pedido {
        TCGETS => Ok(tty.termios.to_vec()),
        TCSETS | TCSETSW | TCSETSF => {
            let novo = dados.filter(|d| d.len() >= 36).ok_or(EFAULT)?;
            tty.termios.copy_from_slice(&novo[..36]);
            if pedido == TCSETSF {
                tty.prontos.clear();
            }
            Ok(alloc::vec::Vec::new())
        }
        TIOCGWINSZ => Ok(alloc::vec![25, 0, 80, 0, 0, 0, 0, 0]), // 25 linhas, 80 colunas
        TIOCGPGRP => Ok(pid.to_le_bytes().to_vec()),
        TIOCSPGRP => Ok(alloc::vec::Vec::new()),
        _ => Err(ENOTTY),
    }
}

/// Chamada pelos tratadores do teclado e da serial a cada caractere.
pub fn receber(c: char) {
    let mut tty = TTY.lock();
    let lflag = tty.lflag();
    if c == '\u{3}' && lflag & ISIG == 0 || lflag & ICANON == 0 && c != '\u{3}' {
        // Modo "cru" (editores, jogos): cada tecla vai direto para o programa.
        let c = if c == '\u{8}' { '\u{7f}' } else { c };
        let mut utf8 = [0u8; 4];
        tty.prontos.extend(c.encode_utf8(&mut utf8).as_bytes());
        if lflag & ECHO != 0 && !c.is_control() {
            print!("{}", c);
        }
        processo::acordar(processo::CANAL_TERMINAL);
        return;
    }
    let eco = lflag & ECHO != 0;
    match c {
        '\u{3}' => {
            tty.linha.clear();
            tty.interrupcoes += 1;
            drop(tty);
            println!("^C");
            processo::sinal_para_o_terminal(processo::SIGINT);
            processo::acordar(processo::CANAL_TERMINAL);
        }
        '\u{4}' => {
            if tty.linha.is_empty() {
                tty.fins += 1;
            } else {
                let linha = core::mem::take(&mut tty.linha);
                tty.prontos.extend(linha.as_bytes());
            }
            processo::acordar(processo::CANAL_TERMINAL);
        }
        '\u{8}' => {
            if tty.linha.pop().is_some() && eco {
                print!("\u{8} \u{8}");
            }
        }
        '\n' => {
            let mut linha = core::mem::take(&mut tty.linha);
            linha.push('\n');
            tty.prontos.extend(linha.as_bytes());
            if eco {
                println!();
            }
            processo::acordar(processo::CANAL_TERMINAL);
        }
        '\t' => receber_normal(&mut tty, ' ', eco),
        c if c.is_control() => {}
        c => receber_normal(&mut tty, c, eco),
    }
}

fn receber_normal(tty: &mut Tty, c: char, eco: bool) {
    if tty.linha.len() < 1024 {
        tty.linha.push(c);
        if eco {
            print!("{}", c);
        }
    }
}

/// Entrega no máximo uma linha. Dorme enquanto não houver nenhuma pronta.
pub fn ler(buf: &mut [u8]) -> Result<usize, i64> {
    let interrupcoes_antes = TTY.lock().interrupcoes;
    loop {
        {
            let mut tty = TTY.lock();
            if !tty.prontos.is_empty() {
                let canonico = tty.lflag() & ICANON != 0;
                let mut n = 0;
                while n < buf.len() {
                    let Some(byte) = tty.prontos.pop_front() else { break };
                    buf[n] = byte;
                    n += 1;
                    if byte == b'\n' && canonico {
                        break;
                    }
                }
                return Ok(n);
            }
            if tty.fins > 0 {
                tty.fins -= 1;
                return Ok(0);
            }
            // Um Ctrl+C interrompe quem estava esperando (o shell, que ignora
            // o SIGINT, usa isso para mostrar um prompt novo).
            if tty.interrupcoes != interrupcoes_antes {
                return Err(EINTR);
            }
        }
        if processo::tem_sinal_pendente() {
            return Err(EINTR);
        }
        processo::dormir(processo::CANAL_TERMINAL);
    }
}

pub fn escrever(dados: &[u8]) {
    print!("{}", String::from_utf8_lossy(dados));
}
