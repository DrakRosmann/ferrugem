//! Terminais: os consoles (`tty1` a `tty4`) e os pseudo-terminais (`/dev/pts/N`).
//!
//! Um terminal liga uma *entrada* (teclado, serial, ou o lado mestre de um
//! pseudo-terminal) a uma *saída* (uma tela, ou de novo o mestre). No meio
//! fica a "disciplina de linha", feita pelo kernel no modo canônico (o
//! padrão do Unix): ecoa o que você digita, trata o Backspace e só entrega a
//! linha ao programa quando você aperta Enter. Ela também transforma teclas:
//!
//! ```text
//!   Ctrl+C  → SIGINT para o grupo em primeiro plano (interrompe)
//!   Ctrl+Z  → SIGTSTP para o grupo em primeiro plano (para; o shell retoma com fg/bg)
//!   Ctrl+\  → SIGQUIT
//!   Ctrl+D  → fim de arquivo (o `read` devolve 0) se a linha estiver vazia
//! ```
//!
//! Cada terminal pertence a no máximo uma *sessão* (um login): é o "terminal
//! de controle" dela, o que `/dev/tty` abre. Só o grupo em primeiro plano
//! recebe os sinais do teclado e pode ler; quando o líder da sessão termina,
//! o grupo em primeiro plano recebe SIGHUP ("desligaram o telefone").
//!
//! ## Pseudo-terminais
//!
//! Abrir `/dev/ptmx` cria um par: o *mestre* (o descritor devolvido) e o
//! *escravo* (`/dev/pts/N`). Para os programas no escravo, ele é um terminal
//! como outro qualquer; quem segura o mestre faz o papel do teclado e da
//! tela — é assim que funcionam o `ssh`, o `script` e os emuladores de terminal.

use crate::erro::*;
use crate::processo;
use crate::sinal;
use crate::spin::Mutex;
use crate::vga;
use alloc::collections::VecDeque;
use alloc::format;
use alloc::string::String;
use alloc::sync::Arc;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

/// Os primeiros números são os consoles (0 = tty1, espelhado na serial);
/// daí em diante, os pseudo-terminais.
pub type IdTerminal = u32;
pub const CONSOLES: u32 = vga::CONSOLES as u32;

const ISIG: u32 = 0o1;
const ICANON: u32 = 0o2;
const ECHO: u32 = 0o10;
const OPOST: u32 = 0o1;
const ONLCR: u32 = 0o4;

const TERMIOS_PADRAO: [u8; 36] = {
    let mut t = [0u8; 36];
    let campos: [u32; 4] = [
        0o400,                                  // c_iflag: ICRNL
        OPOST | ONLCR,                          // c_oflag
        0o17 | 0o60 | 0o200,                    // c_cflag: 38400 baud, 8 bits, CREAD
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

struct Disciplina {
    linha: String,         // o que está sendo digitado
    prontos: VecDeque<u8>, // linhas completas, esperando um `read`
    fins: usize,           // Ctrl+D pendentes
    interrupcoes: u64,     // quantos Ctrl+C já houve
    /// A `struct termios` do Linux (36 bytes): os programas ligam e desligam
    /// o eco e o modo canônico com ioctl(TCSETS).
    termios: [u8; 36],
    primeiro_plano: u32,
    /// A sessão de que este é o terminal de controle (0 = nenhuma).
    sessao: u32,
    /// Linhas, colunas e tamanho em pixels (TIOCGWINSZ).
    janela: [u16; 4],
}

impl Disciplina {
    fn flag(&self, i: usize) -> u32 {
        u32::from_le_bytes(self.termios[i * 4..i * 4 + 4].try_into().unwrap())
    }
    fn lflag(&self) -> u32 {
        self.flag(3)
    }
}

enum Saida {
    Console(usize),
    Pty,
}

pub struct Terminal {
    id: IdTerminal,
    estado: Mutex<Disciplina>,
    saida: Saida,
    /// Pseudo-terminal: o que o lado escravo escreveu, esperando o mestre ler.
    para_o_mestre: Mutex<VecDeque<u8>>,
    mestre_aberto: AtomicBool,
    escravos_abertos: AtomicUsize,
}

static TERMINAIS: Mutex<Vec<Option<Arc<Terminal>>>> = Mutex::new(Vec::new());

fn novo(id: IdTerminal, saida: Saida) -> Arc<Terminal> {
    Arc::new(Terminal {
        id,
        estado: Mutex::new(Disciplina {
            linha: String::new(),
            prontos: VecDeque::new(),
            fins: 0,
            interrupcoes: 0,
            termios: TERMIOS_PADRAO,
            primeiro_plano: 0,
            sessao: 0,
            janela: [25, 80, 0, 0],
        }),
        saida,
        para_o_mestre: Mutex::new(VecDeque::new()),
        mestre_aberto: AtomicBool::new(true),
        escravos_abertos: AtomicUsize::new(0),
    })
}

pub fn init() {
    let mut t = TERMINAIS.lock();
    for i in 0..CONSOLES {
        t.push(Some(novo(i, Saida::Console(i as usize))));
    }
}

fn terminal(id: IdTerminal) -> Result<Arc<Terminal>, i64> {
    TERMINAIS.lock().get(id as usize).cloned().flatten().ok_or(ENXIO)
}

/// Onde dormem os que leem este terminal.
fn canal(id: IdTerminal) -> usize {
    0x1000 + id as usize
}

/// Onde dorme quem lê o lado mestre deste pseudo-terminal.
fn canal_mestre(id: IdTerminal) -> usize {
    0x8000 + id as usize
}

/// O número do pseudo-terminal (o N de /dev/pts/N).
pub fn numero_pts(id: IdTerminal) -> u32 {
    id - CONSOLES
}

/// O "número de dispositivo" que o `stat` mostra, como no Linux:
/// tty1 = (4, 1); /dev/pts/N = (136, N). Codificado como (major << 8) | minor.
pub fn dispositivo(id: IdTerminal) -> u64 {
    let (major, minor) = if id < CONSOLES { (4, id + 1) } else { (136, numero_pts(id)) };
    (major << 8 | minor) as u64
}

impl Terminal {
    /// Põe texto na saída deste terminal (eco ou o que um programa escreveu).
    fn emitir(&self, texto: &str) {
        match self.saida {
            // O tty1 vai para a tela e para a serial (o terminal do QEMU).
            Saida::Console(0) => crate::escrever_no_console(format_args!("{}", texto)),
            Saida::Console(n) => vga::escrever(n, texto),
            Saida::Pty => {
                let oflag = self.estado.lock().flag(1);
                let crlf = oflag & OPOST != 0 && oflag & ONLCR != 0;
                let mut fila = self.para_o_mestre.lock();
                for b in texto.bytes() {
                    if b == b'\n' && crlf {
                        fila.push_back(b'\r'); // terminais "de verdade" querem CR LF
                    }
                    fila.push_back(b);
                }
                drop(fila);
                processo::acordar(canal_mestre(self.id));
            }
        }
    }

    fn sinal_do_teclado(&self, grupo: u32, numero: u32) {
        if grupo != 0 {
            processo::sinal_para_grupo(grupo, numero);
        }
    }

    fn receber(&self, c: char) {
        let mut tty = self.estado.lock();
        let lflag = tty.lflag();
        let grupo = tty.primeiro_plano;
        if lflag & ISIG != 0 && (c == '\u{1a}' || c == '\u{1c}') {
            let (eco, numero) = if c == '\u{1a}' { ("^Z\n", sinal::SIGTSTP) } else { ("^\\\n", sinal::SIGQUIT) };
            tty.linha.clear();
            drop(tty);
            self.emitir(eco);
            self.sinal_do_teclado(grupo, numero);
            return;
        }
        let especial = c == '\u{3}' && lflag & ISIG != 0;
        let mut utf8 = [0u8; 4];
        if lflag & ICANON == 0 && !especial {
            // Modo "cru" (editores, jogos): cada tecla vai direto para o programa.
            let c = if c == '\u{8}' { '\u{7f}' } else { c };
            tty.prontos.extend(c.encode_utf8(&mut utf8).as_bytes());
            drop(tty);
            if lflag & ECHO != 0 && !c.is_control() {
                self.emitir(c.encode_utf8(&mut utf8));
            }
            processo::acordar(canal(self.id));
            return;
        }
        let eco = lflag & ECHO != 0;
        match c {
            '\u{3}' => {
                tty.linha.clear();
                tty.interrupcoes += 1;
                drop(tty);
                self.emitir("^C\n");
                self.sinal_do_teclado(grupo, sinal::SIGINT);
                processo::acordar(canal(self.id));
            }
            '\u{4}' => {
                if tty.linha.is_empty() {
                    tty.fins += 1;
                } else {
                    let linha = core::mem::take(&mut tty.linha);
                    tty.prontos.extend(linha.as_bytes());
                }
                drop(tty);
                processo::acordar(canal(self.id));
            }
            '\u{8}' | '\u{7f}' => {
                let apagou = tty.linha.pop().is_some();
                drop(tty);
                if apagou && eco {
                    self.emitir("\u{8} \u{8}");
                }
            }
            '\n' | '\r' => {
                let mut linha = core::mem::take(&mut tty.linha);
                linha.push('\n');
                tty.prontos.extend(linha.as_bytes());
                drop(tty);
                if eco {
                    self.emitir("\n");
                }
                processo::acordar(canal(self.id));
            }
            c if c.is_control() && c != '\t' => {}
            c => {
                let c = if c == '\t' { ' ' } else { c };
                if tty.linha.len() < 1024 {
                    tty.linha.push(c);
                    drop(tty);
                    if eco {
                        self.emitir(c.encode_utf8(&mut utf8));
                    }
                }
            }
        }
    }
}

/// Chamada pelo teclado (para o console ativo) e pela serial (para o tty1).
pub fn receber(id: IdTerminal, c: char) {
    if let Ok(t) = terminal(id) {
        t.receber(c);
    }
}

/// Entrega no máximo uma linha (no modo canônico). Dorme enquanto não houver nada.
pub fn ler(id: IdTerminal, buf: &mut [u8], bloquear: bool) -> Result<usize, i64> {
    let t = terminal(id)?;
    // Quem não está em primeiro plano no seu terminal de controle não lê o
    // teclado: o grupo dele é parado com SIGTTIN, e esta chamada é refeita
    // quando ele voltar (com `fg`).
    let primeiro_plano = t.estado.lock().primeiro_plano;
    let (meu_grupo, _) = processo::grupo_e_sessao(0)?;
    if processo::atual().terminal == Some(id) && primeiro_plano != 0 && meu_grupo != primeiro_plano {
        if processo::atual().sinais.ignora(sinal::SIGTTIN) {
            return Err(EIO);
        }
        processo::sinal_para_grupo(meu_grupo, sinal::SIGTTIN);
        return Err(ERESTARTSYS);
    }
    let interrupcoes_antes = t.estado.lock().interrupcoes;
    loop {
        {
            let mut tty = t.estado.lock();
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
        // Pseudo-terminal cujo mestre fechou: ninguém mais vai digitar.
        if !t.mestre_aberto.load(Ordering::Relaxed) {
            return Err(EIO);
        }
        if !bloquear {
            return Err(EAGAIN);
        }
        if processo::tem_sinal_pendente() {
            return Err(EINTR);
        }
        processo::dormir(canal(id));
    }
}

/// Há algo para ler (uma linha pronta, um Ctrl+D, ou a linha caiu)?
pub fn pode_ler(id: IdTerminal) -> bool {
    terminal(id).is_ok_and(|t| {
        let tty = t.estado.lock();
        !tty.prontos.is_empty() || tty.fins > 0 || !t.mestre_aberto.load(Ordering::Relaxed)
    })
}

pub fn escrever(id: IdTerminal, dados: &[u8]) -> Result<usize, i64> {
    let t = terminal(id)?;
    if !t.mestre_aberto.load(Ordering::Relaxed) {
        return Err(EIO);
    }
    t.emitir(&String::from_utf8_lossy(dados));
    Ok(dados.len())
}

/// Um processo abriu o terminal. Se for líder de uma sessão sem terminal de
/// controle, e o terminal estiver livre, ele vira o terminal da sessão
/// (a não ser com O_NOCTTY).
pub fn abrir(id: IdTerminal, sem_controle: bool) -> Result<(), i64> {
    let t = terminal(id)?;
    if !t.mestre_aberto.load(Ordering::Relaxed) {
        return Err(EIO);
    }
    t.escravos_abertos.fetch_add(1, Ordering::Relaxed);
    let p = processo::atual();
    let mut tty = t.estado.lock();
    if !sem_controle && p.sid == p.pid && p.terminal.is_none() && tty.sessao == 0 {
        p.terminal = Some(id);
        tty.sessao = p.sid;
        tty.primeiro_plano = p.pgid;
    }
    Ok(())
}

/// Um descritor do terminal (escravo) foi fechado.
pub fn fechou(id: IdTerminal) {
    if let Ok(t) = terminal(id) {
        t.escravos_abertos.fetch_sub(1, Ordering::Relaxed);
        talvez_liberar(&t);
    }
}

/// A sessão que tinha este terminal acabou: o grupo em primeiro plano
/// recebe SIGHUP (e SIGCONT, se estava parado), e o terminal fica livre.
pub fn sessao_terminou(id: IdTerminal) {
    let Ok(t) = terminal(id) else { return };
    let grupo = {
        let mut tty = t.estado.lock();
        tty.sessao = 0;
        core::mem::take(&mut tty.primeiro_plano)
    };
    if grupo != 0 {
        processo::sinal_para_grupo(grupo, sinal::SIGHUP);
        processo::sinal_para_grupo(grupo, sinal::SIGCONT);
    }
}

pub const TCGETS: u64 = 0x5401;
const TCSETS: u64 = 0x5402;
const TCSETSW: u64 = 0x5403;
const TCSETSF: u64 = 0x5404;
const TIOCSCTTY: u64 = 0x540E;
const TIOCGPGRP: u64 = 0x540F;
const TIOCSPGRP: u64 = 0x5410;
const TIOCGWINSZ: u64 = 0x5413;
const TIOCSWINSZ: u64 = 0x5414;
const TIOCNOTTY: u64 = 0x5422;
const TIOCGSID: u64 = 0x5429;
const TIOCGPTN: u64 = 0x8004_5430;
const TIOCSPTLCK: u64 = 0x4004_5431;

/// Quantos bytes do programa cada ioctl de terminal lê (para os SETs).
pub fn bytes_de_entrada(pedido: u64) -> usize {
    match pedido {
        TCSETS | TCSETSW | TCSETSF => 36,
        TIOCSPGRP | TIOCSPTLCK => 4,
        TIOCSWINSZ => 8,
        _ => 0,
    }
}

/// Os `ioctl` de terminal. `dados` é o que o programa mandou (para os SETs);
/// a resposta é o que deve ser copiado de volta para ele.
pub fn ioctl(id: IdTerminal, pedido: u64, dados: Option<&[u8]>, mestre: bool) -> Result<Vec<u8>, i64> {
    let t = terminal(id)?;
    let p = processo::atual();
    let mut tty = t.estado.lock();
    let entrada = |n: usize| dados.filter(|d| d.len() >= n).ok_or(EFAULT);
    match pedido {
        TCGETS => Ok(tty.termios.to_vec()),
        TCSETS | TCSETSW | TCSETSF => {
            tty.termios.copy_from_slice(&entrada(36)?[..36]);
            if pedido == TCSETSF {
                tty.prontos.clear();
            }
            Ok(Vec::new())
        }
        TIOCGWINSZ => Ok(tty.janela.iter().flat_map(|v| v.to_le_bytes()).collect()),
        TIOCSWINSZ => {
            let d = entrada(8)?;
            for i in 0..4 {
                tty.janela[i] = u16::from_le_bytes([d[i * 2], d[i * 2 + 1]]);
            }
            let grupo = tty.primeiro_plano;
            drop(tty);
            if grupo != 0 {
                processo::sinal_para_grupo(grupo, sinal::SIGWINCH); // o tamanho mudou
            }
            Ok(Vec::new())
        }
        TIOCGPGRP => Ok((if tty.primeiro_plano != 0 { tty.primeiro_plano } else { p.pgid }).to_le_bytes().to_vec()),
        TIOCSPGRP => {
            tty.primeiro_plano = u32::from_le_bytes(entrada(4)?[..4].try_into().unwrap());
            Ok(Vec::new())
        }
        TIOCGSID => Ok(tty.sessao.to_le_bytes().to_vec()),
        TIOCSCTTY => {
            // O líder de uma sessão adota este terminal (o root pode tomá-lo de outra sessão).
            if p.sid != p.pid || p.terminal.is_some() || tty.sessao != 0 && p.euid != 0 {
                return Err(EPERM);
            }
            p.terminal = Some(id);
            tty.sessao = p.sid;
            tty.primeiro_plano = p.pgid;
            Ok(Vec::new())
        }
        TIOCNOTTY => {
            if p.terminal == Some(id) {
                p.terminal = None;
            }
            Ok(Vec::new())
        }
        TIOCGPTN if mestre => Ok(numero_pts(id).to_le_bytes().to_vec()),
        TIOCSPTLCK if mestre => Ok(Vec::new()), // os nossos já nascem destravados
        _ => Err(ENOTTY),
    }
}

// -----------------------------------------------------------------------------
// O lado mestre dos pseudo-terminais
// -----------------------------------------------------------------------------

/// Abrir /dev/ptmx: cria um pseudo-terminal e o nó /dev/pts/N do escravo.
pub fn abrir_mestre(uid: u32, gid: u32) -> Result<IdTerminal, i64> {
    let id = {
        let mut lista = TERMINAIS.lock();
        let livre = (CONSOLES as usize..lista.len()).find(|&i| lista[i].is_none());
        let id = livre.unwrap_or(lista.len()) as IdTerminal;
        if id - CONSOLES >= 256 {
            return Err(ENOSPC);
        }
        let t = novo(id, Saida::Pty);
        if (id as usize) < lista.len() {
            lista[id as usize] = Some(t);
        } else {
            lista.push(Some(t));
        }
        id
    };
    let caminho = format!("/dev/pts/{}", numero_pts(id));
    let meta = crate::vfs::Meta { permissoes: 0o620, uid, gid, modificado: crate::rtc::agora() };
    let no = crate::vfs::Conteudo::Dispositivo(crate::vfs::Dispositivo::PtyEscravo(id));
    let _ = crate::vfs::criar_na_memoria(&caminho, no, Some(meta));
    Ok(id)
}

/// Lê o que os programas do escravo escreveram (a "tela" do pseudo-terminal).
pub fn ler_mestre(id: IdTerminal, buf: &mut [u8], bloquear: bool) -> Result<usize, i64> {
    let t = terminal(id)?;
    loop {
        {
            let mut fila = t.para_o_mestre.lock();
            if !fila.is_empty() {
                let n = buf.len().min(fila.len());
                for (destino, b) in buf.iter_mut().zip(fila.drain(..n)) {
                    *destino = b;
                }
                return Ok(n);
            }
        }
        if !bloquear {
            return Err(EAGAIN);
        }
        if processo::tem_sinal_pendente() {
            return Err(EINTR);
        }
        processo::dormir(canal_mestre(id));
    }
}

/// Escrever no mestre é como digitar no terminal escravo.
pub fn escrever_mestre(id: IdTerminal, dados: &[u8]) -> Result<usize, i64> {
    let t = terminal(id)?;
    for c in String::from_utf8_lossy(dados).chars() {
        t.receber(c);
    }
    Ok(dados.len())
}

pub fn pode_ler_mestre(id: IdTerminal) -> bool {
    terminal(id).is_ok_and(|t| !t.para_o_mestre.lock().is_empty())
}

/// O mestre fechou: o escravo "perdeu a linha" (SIGHUP; ler e escrever dão EIO).
pub fn fechar_mestre(id: IdTerminal) {
    let Ok(t) = terminal(id) else { return };
    t.mestre_aberto.store(false, Ordering::Relaxed);
    let grupo = core::mem::take(&mut t.estado.lock().primeiro_plano);
    if grupo != 0 {
        processo::sinal_para_grupo(grupo, sinal::SIGHUP);
        processo::sinal_para_grupo(grupo, sinal::SIGCONT);
    }
    processo::acordar(canal(id));
    let _ = crate::vfs::remover(&format!("/dev/pts/{}", numero_pts(id)), false);
    talvez_liberar(&t);
}

/// Um pseudo-terminal sem mestre e sem ninguém no escravo pode ser reaproveitado.
fn talvez_liberar(t: &Terminal) {
    let livre = !t.mestre_aberto.load(Ordering::Relaxed) && t.escravos_abertos.load(Ordering::Relaxed) == 0;
    if t.id >= CONSOLES && livre {
        if let Some(lugar) = TERMINAIS.lock().get_mut(t.id as usize) {
            *lugar = None;
        }
    }
}
