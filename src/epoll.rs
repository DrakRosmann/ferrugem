//! `epoll`: esperar muitos descritores sem repassar a lista toda a cada vez.
//!
//! Com `poll`, o programa manda a lista inteira de descritores em cada
//! chamada. Com `epoll`, ele cria um "conjunto de interesse" uma vez
//! (`epoll_create1`), adiciona e remove descritores aos poucos (`epoll_ctl`)
//! e só pergunta "quem está pronto?" (`epoll_wait`). É o que servidores e
//! laços de eventos (o servidor de janelas, por exemplo) usam.
//!
//! ```text
//!   ep = epoll_create1(0);
//!   epoll_ctl(ep, EPOLL_CTL_ADD, sock, {EPOLLIN, .data = 7});
//!   n = epoll_wait(ep, eventos, 16, -1);   // eventos[0].data == 7
//! ```
//!
//! O modo "por borda" (EPOLLET) é aproximado: um descritor só é relatado de
//! novo depois de ter deixado de estar pronto (ou de mudar o que está pronto).

use crate::arquivo::{Arquivo, Tipo, POLLERR, POLLHUP};
use crate::erro::*;
use crate::processo;
use crate::spin::Mutex;
use crate::syscall::{arquivo, escrever_usuario, ler_usuario, novo_descritor, Resultado};
use crate::pit;
use alloc::collections::BTreeMap;
use alloc::sync::{Arc, Weak};
use alloc::vec::Vec;

const EPOLL_CLOEXEC: u64 = 0o2000000;
const EPOLL_CTL_ADD: u64 = 1;
const EPOLL_CTL_DEL: u64 = 2;
const EPOLL_CTL_MOD: u64 = 3;
const EPOLLONESHOT: u32 = 1 << 30;
const EPOLLET: u32 = 1 << 31;

struct Interesse {
    arquivo: Weak<Arquivo>,
    eventos: u32,
    dados: u64,
    /// EPOLLONESHOT já disparou: fica quieto até um EPOLL_CTL_MOD.
    desligado: bool,
    /// EPOLLET: o que estava pronto da última vez que olhamos.
    ultimo: u16,
}

impl Interesse {
    /// O que está pronto e interessa (POLLERR e POLLHUP sempre interessam).
    fn prontos(&self) -> Option<u16> {
        let arq = self.arquivo.upgrade()?;
        Some(arq.prontidao() & (self.eventos as u16 | POLLERR | POLLHUP))
    }
}

pub struct Epoll {
    interesses: Mutex<BTreeMap<i32, Interesse>>,
}

impl Epoll {
    /// Para um `poll` no próprio epoll: tem alguém pronto?
    pub fn tem_prontos(&self) -> bool {
        self.interesses.lock().values().any(|i| !i.desligado && i.prontos().is_some_and(|r| r != 0))
    }

    /// Recolhe até `maximo` eventos prontos: (eventos, dados).
    fn colher(&self, maximo: usize) -> Vec<(u32, u64)> {
        let mut prontos = Vec::new();
        let mut interesses = self.interesses.lock();
        interesses.retain(|_, i| i.arquivo.strong_count() > 0); // arquivos que fecharam saem sozinhos
        for i in interesses.values_mut() {
            if prontos.len() >= maximo {
                break;
            }
            if i.desligado {
                continue;
            }
            let r = i.prontos().unwrap_or(0);
            let anterior = core::mem::replace(&mut i.ultimo, r);
            if r == 0 || i.eventos & EPOLLET != 0 && r == anterior {
                continue;
            }
            if i.eventos & EPOLLONESHOT != 0 {
                i.desligado = true;
            }
            prontos.push((r as u32, i.dados));
        }
        prontos
    }
}

fn do_descritor(fd: u64) -> Result<Arc<Epoll>, i64> {
    let arq = arquivo(fd)?;
    let Tipo::Epoll(e) = &arq.tipo else { return Err(EINVAL) };
    Ok(e.clone())
}

/// `epoll_create1(flags)` (e o antigo `epoll_create`).
pub fn criar(flags: u64) -> Resultado {
    let e = Arc::new(Epoll { interesses: Mutex::new(BTreeMap::new()) });
    let fd = novo_descritor(Arquivo::novo(Tipo::Epoll(e), crate::arquivo::O_RDONLY))?;
    if flags & EPOLL_CLOEXEC != 0 {
        processo::atual().arquivos().fecha_no_exec |= 1 << fd;
    }
    Ok(fd)
}

/// `epoll_ctl(ep, op, fd, evento)`: struct epoll_event { u32 eventos; u64 dados } (12 bytes, sem buraco).
pub fn controlar(epfd: u64, op: u64, fd: u64, ptr: u64) -> Resultado {
    let e = do_descritor(epfd)?;
    let alvo = arquivo(fd)?;
    match &alvo.tipo {
        Tipo::Epoll(outro) if Arc::ptr_eq(outro, &e) => return Err(EINVAL),
        // Como no Linux: arquivos e diretórios comuns estão sempre "prontos",
        // não faz sentido esperar por eles.
        Tipo::No(no) if matches!(no.tipo(), Ok(crate::vfs::Tipo::Arquivo | crate::vfs::Tipo::Diretorio)) => {
            return Err(EPERM);
        }
        _ => {}
    }
    let fd = fd as i32;
    let mut interesses = e.interesses.lock();
    let existe = interesses.get(&fd).is_some_and(|i| i.arquivo.strong_count() > 0);
    match op {
        EPOLL_CTL_DEL => {
            if !existe {
                return Err(ENOENT);
            }
            interesses.remove(&fd);
        }
        EPOLL_CTL_ADD | EPOLL_CTL_MOD => {
            if op == EPOLL_CTL_ADD && existe {
                return Err(EEXIST);
            }
            if op == EPOLL_CTL_MOD && !existe {
                return Err(ENOENT);
            }
            let b = ler_usuario(ptr, 12)?;
            let eventos = u32::from_le_bytes(b[..4].try_into().unwrap());
            let dados = u64::from_le_bytes(b[4..12].try_into().unwrap());
            interesses.insert(fd, Interesse { arquivo: Arc::downgrade(&alvo), eventos, dados, desligado: false, ultimo: 0 });
        }
        _ => return Err(EINVAL),
    }
    drop(interesses);
    processo::acordar(processo::CANAL_POLL); // quem já espera neste epoll confere de novo
    Ok(0)
}

/// `epoll_wait(ep, eventos, máximo, prazo)`: dorme até algo ficar pronto.
pub fn esperar(epfd: u64, ptr: u64, maximo: u64, prazo_ms: Option<u64>) -> Resultado {
    let e = do_descritor(epfd)?;
    if maximo == 0 || maximo > 4096 {
        return Err(EINVAL);
    }
    let limite = prazo_ms.map(|ms| pit::milissegundos() + ms);
    loop {
        let prontos = e.colher(maximo as usize);
        if !prontos.is_empty() || limite.is_some_and(|l| pit::milissegundos() >= l) {
            let mut bytes = Vec::with_capacity(prontos.len() * 12);
            for (eventos, dados) in &prontos {
                bytes.extend_from_slice(&eventos.to_le_bytes());
                bytes.extend_from_slice(&dados.to_le_bytes());
            }
            escrever_usuario(ptr, &bytes)?;
            return Ok(prontos.len() as u64);
        }
        if processo::tem_sinal_pendente() {
            return Err(EINTR);
        }
        processo::atual().prazo_ms = limite.unwrap_or(0);
        processo::dormir(processo::CANAL_POLL);
        processo::atual().prazo_ms = 0;
    }
}
