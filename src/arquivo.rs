//! Arquivos abertos e pipes.
//!
//! No Unix, `open` devolve um número (o descritor, `fd`) que indexa a tabela
//! de arquivos do processo. Cada entrada aponta para um `Arquivo` aberto,
//! que guarda a posição de leitura/escrita. Depois de um `fork`, pai e filho
//! *compartilham* o mesmo `Arquivo` (por isso é um `Arc`): é assim que o
//! shell passa o terminal e os pipes para os programas que executa.

use crate::erro::*;
use crate::processo;
use crate::spin::Mutex;
use crate::tty;
use crate::vfs::{self, Cred, Dispositivo, Meta, Vnode};
use crate::rtc;
use alloc::collections::VecDeque;
use alloc::sync::Arc;
use core::sync::atomic::{AtomicUsize, Ordering};

pub const POLLIN: u16 = 0x1;
pub const POLLOUT: u16 = 0x4;
pub const POLLERR: u16 = 0x8;
pub const POLLHUP: u16 = 0x10;
pub const POLLNVAL: u16 = 0x20;

pub const O_ACESSO: u64 = 3;
pub const O_RDONLY: u64 = 0;
pub const O_WRONLY: u64 = 1;
pub const O_CREAT: u64 = 0o100;
pub const O_EXCL: u64 = 0o200;
pub const O_TRUNC: u64 = 0o1000;
pub const O_APPEND: u64 = 0o2000;
pub const O_NOCTTY: u64 = 0o400;
pub const O_NONBLOCK: u64 = 0o4000;
pub const O_DIRECTORY: u64 = 0o200000;

pub enum Tipo {
    No(Vnode),
    LeituraPipe(Arc<Pipe>),
    EscritaPipe(Arc<Pipe>),
    /// Um terminal aberto (console ou escravo de pseudo-terminal), pelo id em `tty`.
    Terminal(Vnode, tty::IdTerminal),
    /// O lado mestre de um pseudo-terminal (o que /dev/ptmx devolve).
    PtyMestre(Vnode, tty::IdTerminal),
}

pub struct Arquivo {
    pub tipo: Tipo,
    pub posicao: Mutex<u64>,
    flags: u64,
}

impl Arquivo {
    /// `permissoes`: as de um arquivo novo, se `O_CREAT` precisar criá-lo.
    pub fn abrir(caminho: &str, flags: u64, cred: Cred, permissoes: u32) -> Result<Arc<Arquivo>, i64> {
        let escrita = flags & O_ACESSO != O_RDONLY;
        let no = match vfs::buscar_como(caminho, cred) {
            Ok(_) if flags & O_CREAT != 0 && flags & O_EXCL != 0 => return Err(EEXIST),
            Ok(no) => {
                let acesso = match flags & O_ACESSO {
                    O_RDONLY => vfs::LER,
                    O_WRONLY => vfs::ESCREVER,
                    _ => vfs::LER | vfs::ESCREVER,
                };
                no.permite(cred, acesso)?;
                no
            }
            Err(ENOENT) if flags & O_CREAT != 0 => {
                // Criar um arquivo é escrever no diretório.
                let (pai, _) = vfs::separar(caminho)?;
                vfs::buscar_como(pai, cred)?.permite(cred, vfs::ESCREVER | vfs::EXECUTAR)?;
                let meta = Meta { permissoes, uid: cred.uid, gid: cred.gid, modificado: rtc::agora() };
                vfs::criar(caminho, false, meta)?
            }
            Err(e) => return Err(e),
        };
        let tipo = no.tipo()?;
        if tipo == vfs::Tipo::Fifo {
            return Pipe::abrir_fifo(vfs::pipe_do_fifo(&no), flags);
        }
        if let vfs::Tipo::Dispositivo(d) = tipo {
            let terminal = match d {
                // /dev/tty é sempre o terminal de controle de quem abre.
                Dispositivo::Terminal => processo::atual().terminal.ok_or(ENXIO)?,
                Dispositivo::Console(n) => n as tty::IdTerminal,
                Dispositivo::PtyEscravo(id) => id,
                Dispositivo::Ptmx => {
                    let id = tty::abrir_mestre(cred.uid, cred.gid)?;
                    let tipo = Tipo::PtyMestre(no, id);
                    return Ok(Arc::new(Arquivo { tipo, posicao: Mutex::new(0), flags }));
                }
                _ => return Ok(Arc::new(Arquivo { tipo: Tipo::No(no), posicao: Mutex::new(0), flags })),
            };
            tty::abrir(terminal, flags & O_NOCTTY != 0)?;
            return Ok(Arc::new(Arquivo { tipo: Tipo::Terminal(no, terminal), posicao: Mutex::new(0), flags }));
        }
        match no.tipo()? {
            vfs::Tipo::Diretorio if escrita => return Err(EISDIR),
            vfs::Tipo::Diretorio => {}
            _ if flags & O_DIRECTORY != 0 => return Err(ENOTDIR),
            vfs::Tipo::Arquivo if escrita && flags & O_TRUNC != 0 => no.truncar()?,
            _ => {}
        }
        Ok(Arc::new(Arquivo { tipo: Tipo::No(no), posicao: Mutex::new(0), flags }))
    }

    pub fn ler(&self, buf: &mut [u8]) -> Result<usize, i64> {
        if self.flags & O_ACESSO == O_WRONLY {
            return Err(EBADF);
        }
        let no = match &self.tipo {
            Tipo::LeituraPipe(pipe) => return pipe.ler(buf),
            Tipo::EscritaPipe(_) => return Err(EBADF),
            // Ler um terminal pode dormir até alguém digitar.
            Tipo::Terminal(_, id) => return tty::ler(*id, buf),
            Tipo::PtyMestre(_, id) => return tty::ler_mestre(*id, buf),
            Tipo::No(no) => no,
        };
        match no.tipo()? {
            vfs::Tipo::Arquivo => {
                let mut posicao = self.posicao.lock();
                let n = no.ler(*posicao, buf)?;
                *posicao += n as u64;
                Ok(n)
            }
            vfs::Tipo::Diretorio => Err(EISDIR),
            vfs::Tipo::Link | vfs::Tipo::Fifo => Err(EINVAL), // o open segue links e abre FIFOs como pipe
            vfs::Tipo::Dispositivo(Dispositivo::Nulo) => Ok(0),
            vfs::Tipo::Dispositivo(Dispositivo::Zero) => {
                buf.fill(0);
                Ok(buf.len())
            }
            vfs::Tipo::Dispositivo(Dispositivo::Aleatorio) => {
                crate::aleatorio::preencher(buf);
                Ok(buf.len())
            }
            vfs::Tipo::Dispositivo(Dispositivo::Bloco(i)) => {
                let bloco = crate::blocos::listar().get(i as usize).cloned().ok_or(EIO)?;
                let mut posicao = self.posicao.lock();
                let n = bloco.ler_bytes(*posicao, buf)?;
                *posicao += n as u64;
                Ok(n)
            }
            vfs::Tipo::Dispositivo(_) => Err(ENXIO),
        }
    }

    pub fn escrever(&self, dados: &[u8]) -> Result<usize, i64> {
        if self.flags & O_ACESSO == O_RDONLY {
            return Err(EBADF);
        }
        let no = match &self.tipo {
            Tipo::EscritaPipe(pipe) => return pipe.escrever(dados),
            Tipo::LeituraPipe(_) => return Err(EBADF),
            Tipo::Terminal(_, id) => return tty::escrever(*id, dados),
            Tipo::PtyMestre(_, id) => return tty::escrever_mestre(*id, dados),
            Tipo::No(no) => no,
        };
        match no.tipo()? {
            vfs::Tipo::Arquivo => {
                let mut posicao = self.posicao.lock();
                if self.flags & O_APPEND != 0 {
                    *posicao = no.tamanho()?;
                }
                let n = no.escrever(*posicao, dados)?;
                *posicao += n as u64;
                Ok(n)
            }
            vfs::Tipo::Diretorio => Err(EISDIR),
            vfs::Tipo::Link | vfs::Tipo::Fifo => Err(EINVAL),
            vfs::Tipo::Dispositivo(Dispositivo::Bloco(i)) => {
                let bloco = crate::blocos::listar().get(i as usize).cloned().ok_or(EIO)?;
                let mut posicao = self.posicao.lock();
                let n = bloco.gravar_bytes(*posicao, dados)?;
                *posicao += n as u64;
                Ok(n)
            }
            vfs::Tipo::Dispositivo(_) => Ok(dados.len()),
        }
    }

    pub fn flags(&self) -> u64 {
        self.flags
    }

    /// O nó do sistema de arquivos por trás (não existe para os pipes).
    pub fn vnode(&self) -> Option<&Vnode> {
        match &self.tipo {
            Tipo::No(no) | Tipo::Terminal(no, _) | Tipo::PtyMestre(no, _) => Some(no),
            _ => None,
        }
    }

    /// O terminal, se for um: (id, é o lado mestre?).
    pub fn terminal(&self) -> Option<(tty::IdTerminal, bool)> {
        match self.tipo {
            Tipo::Terminal(_, id) => Some((id, false)),
            Tipo::PtyMestre(_, id) => Some((id, true)),
            _ => None,
        }
    }

    /// Para o `poll`: o que dá para fazer agora sem bloquear (POLLIN, POLLOUT...).
    pub fn prontidao(&self) -> u16 {
        match &self.tipo {
            Tipo::LeituraPipe(pipe) => pipe.prontidao(false),
            Tipo::EscritaPipe(pipe) => pipe.prontidao(true),
            Tipo::Terminal(_, id) => if tty::pode_ler(*id) { POLLIN | POLLOUT } else { POLLOUT },
            Tipo::PtyMestre(_, id) => if tty::pode_ler_mestre(*id) { POLLIN | POLLOUT } else { POLLOUT },
            Tipo::No(no) => match no.tipo() {
                Ok(_) => POLLIN | POLLOUT, // arquivos comuns nunca bloqueiam
                Err(_) => POLLERR,
            },
        }
    }

    /// `lseek`: 0 = a partir do início, 1 = da posição atual, 2 = do fim.
    pub fn posicionar(&self, deslocamento: i64, referencia: u64) -> Result<u64, i64> {
        let Tipo::No(no) = &self.tipo else { return Err(ESPIPE) };
        let tamanho = match no.tipo()? {
            vfs::Tipo::Arquivo => no.tamanho()? as i64,
            vfs::Tipo::Diretorio | vfs::Tipo::Link => 0,
            vfs::Tipo::Fifo => return Err(ESPIPE),
            vfs::Tipo::Dispositivo(Dispositivo::Bloco(i)) => {
                crate::blocos::listar().get(i as usize).map_or(0, |b| b.tamanho() as i64)
            }
            vfs::Tipo::Dispositivo(_) => return Err(ESPIPE),
        };
        let mut posicao = self.posicao.lock();
        let base = match referencia {
            0 => 0,
            1 => *posicao as i64,
            2 => tamanho,
            _ => return Err(EINVAL),
        };
        let nova = base.checked_add(deslocamento).filter(|&p| p >= 0).ok_or(EINVAL)?;
        *posicao = nova as u64;
        Ok(nova as u64)
    }
}

impl Drop for Arquivo {
    /// Quando a última referência a uma ponta de pipe some, a outra ponta
    /// precisa saber (leitura recebe fim de arquivo; escrita recebe EPIPE).
    fn drop(&mut self) {
        match &self.tipo {
            Tipo::LeituraPipe(pipe) => {
                pipe.leitores.fetch_sub(1, Ordering::Relaxed);
                pipe.esvaziar_se_ninguem_usa();
                processo::acordar(pipe.canal());
            }
            Tipo::EscritaPipe(pipe) => {
                pipe.escritores.fetch_sub(1, Ordering::Relaxed);
                pipe.esvaziar_se_ninguem_usa();
                processo::acordar(pipe.canal());
            }
            Tipo::Terminal(_, id) => tty::fechou(*id),
            Tipo::PtyMestre(_, id) => tty::fechar_mestre(*id),
            Tipo::No(_) => {}
        }
    }
}

// -----------------------------------------------------------------------------
// Pipes
// -----------------------------------------------------------------------------

const CAPACIDADE_PIPE: usize = 4096;

/// Um buffer circular entre dois processos. Quem lê de um pipe vazio dorme
/// até alguém escrever; quem escreve num pipe cheio dorme até alguém ler.
pub struct Pipe {
    dados: Mutex<VecDeque<u8>>,
    leitores: AtomicUsize,
    escritores: AtomicUsize,
    /// Quantas vezes cada ponta já foi aberta (para o open de um FIFO saber
    /// que alguém apareceu, mesmo que já tenha ido embora).
    aberturas: [AtomicUsize; 2],
}

impl Pipe {
    /// (ponta de leitura, ponta de escrita)
    pub fn novo() -> (Arc<Arquivo>, Arc<Arquivo>) {
        let pipe = Arc::new(Pipe {
            dados: Mutex::new(VecDeque::new()),
            leitores: AtomicUsize::new(1),
            escritores: AtomicUsize::new(1),
            aberturas: [AtomicUsize::new(1), AtomicUsize::new(1)],
        });
        let ponta = |tipo, flags| Arc::new(Arquivo { tipo, posicao: Mutex::new(0), flags });
        (
            ponta(Tipo::LeituraPipe(pipe.clone()), O_RDONLY),
            ponta(Tipo::EscritaPipe(pipe), O_WRONLY),
        )
    }

    /// Num FIFO, quando todo mundo fecha, o que sobrou no pipe se perde (como no Unix).
    fn esvaziar_se_ninguem_usa(&self) {
        if self.leitores.load(Ordering::Relaxed) == 0 && self.escritores.load(Ordering::Relaxed) == 0 {
            self.dados.lock().clear();
        }
    }

    fn prontidao(&self, escrita: bool) -> u16 {
        let tamanho = self.dados.lock().len();
        if escrita {
            if self.leitores.load(Ordering::Relaxed) == 0 {
                POLLERR // escrever agora daria EPIPE
            } else if tamanho < CAPACIDADE_PIPE {
                POLLOUT
            } else {
                0
            }
        } else {
            let mut r = if tamanho > 0 { POLLIN } else { 0 };
            if self.escritores.load(Ordering::Relaxed) == 0 {
                r |= POLLHUP; // fim de arquivo: ler devolve 0 na hora
            }
            r
        }
    }

    /// O pipe de um FIFO: começa sem ninguém nas pontas.
    pub fn vazio() -> Arc<Pipe> {
        Arc::new(Pipe {
            dados: Mutex::new(VecDeque::new()),
            leitores: AtomicUsize::new(0),
            escritores: AtomicUsize::new(0),
            aberturas: [AtomicUsize::new(0), AtomicUsize::new(0)],
        })
    }

    /// Abrir um FIFO: quem abre para ler espera alguém abrir para escrever, e
    /// vice-versa (com O_NONBLOCK, ler não espera e escrever sem leitor dá ENXIO).
    pub fn abrir_fifo(pipe: Arc<Pipe>, flags: u64) -> Result<Arc<Arquivo>, i64> {
        let escrita = match flags & O_ACESSO {
            O_RDONLY => false,
            O_WRONLY => true,
            _ => return Err(EINVAL),
        };
        if escrita && flags & O_NONBLOCK != 0 && pipe.leitores.load(Ordering::Relaxed) == 0 {
            return Err(ENXIO);
        }
        let minha_ponta = if escrita { &pipe.escritores } else { &pipe.leitores };
        let outras_aberturas = &pipe.aberturas[!escrita as usize];
        let antes = outras_aberturas.load(Ordering::Relaxed);
        minha_ponta.fetch_add(1, Ordering::Relaxed);
        pipe.aberturas[escrita as usize].fetch_add(1, Ordering::Relaxed);
        processo::acordar(pipe.canal());
        // Montado já agora: se a espera for interrompida, o Drop desfaz a contagem.
        let tipo = if escrita { Tipo::EscritaPipe(pipe.clone()) } else { Tipo::LeituraPipe(pipe.clone()) };
        let arquivo = Arc::new(Arquivo { tipo, posicao: Mutex::new(0), flags: flags & O_ACESSO });
        let outra = if escrita { &pipe.leitores } else { &pipe.escritores };
        // Espera alguém na outra ponta — ou que alguém tenha aparecido (e talvez
        // já ido embora) enquanto esperávamos.
        while flags & O_NONBLOCK == 0
            && outra.load(Ordering::Relaxed) == 0
            && outras_aberturas.load(Ordering::Relaxed) == antes
        {
            if processo::tem_sinal_pendente() {
                return Err(EINTR);
            }
            processo::dormir(pipe.canal());
        }
        Ok(arquivo)
    }

    /// O "canal" onde leitores e escritores dormem: o endereço do próprio pipe.
    fn canal(&self) -> usize {
        self as *const Pipe as usize
    }

    fn ler(&self, buf: &mut [u8]) -> Result<usize, i64> {
        loop {
            {
                let mut dados = self.dados.lock();
                if !dados.is_empty() {
                    let n = buf.len().min(dados.len());
                    for (destino, byte) in buf.iter_mut().zip(dados.drain(..n)) {
                        *destino = byte;
                    }
                    processo::acordar(self.canal());
                    return Ok(n);
                }
            }
            if self.escritores.load(Ordering::Relaxed) == 0 {
                return Ok(0); // ninguém mais vai escrever: fim de arquivo
            }
            if processo::tem_sinal_pendente() {
                return Err(EINTR);
            }
            processo::dormir(self.canal());
        }
    }

    fn escrever(&self, dados: &[u8]) -> Result<usize, i64> {
        let mut escritos = 0;
        while escritos < dados.len() {
            if self.leitores.load(Ordering::Relaxed) == 0 {
                // Escrever num pipe sem leitor mata o processo com SIGPIPE,
                // como no Unix (é o que encerra `programa | head`).
                processo::enviar_sinal(processo::atual().pid, processo::SIGPIPE).ok();
                return if escritos > 0 { Ok(escritos) } else { Err(EPIPE) };
            }
            {
                let mut fila = self.dados.lock();
                let espaco = CAPACIDADE_PIPE - fila.len();
                if espaco > 0 {
                    let n = espaco.min(dados.len() - escritos);
                    fila.extend(&dados[escritos..escritos + n]);
                    escritos += n;
                    processo::acordar(self.canal());
                    continue;
                }
            }
            if processo::tem_sinal_pendente() {
                return if escritos > 0 { Ok(escritos) } else { Err(EINTR) };
            }
            processo::dormir(self.canal());
        }
        Ok(escritos)
    }
}
