//! Memória compartilhada do System V: `shmget`, `shmat`, `shmdt`, `shmctl`.
//!
//! Dois processos quaisquer (não precisam ser pai e filho) combinam uma
//! *chave* — um número — e pedem ao kernel um segmento com ela. Cada um o
//! "anexa" (`shmat`) ao seu espaço de endereçamento, e a partir daí os dois
//! enxergam as mesmas páginas físicas: o que um escreve, o outro lê na hora,
//! sem chamada de sistema nenhuma no meio.
//!
//! ```text
//!   id = shmget(1234, 4096, IPC_CREAT | 0600);
//!   char *p = shmat(id, NULL, 0);     // processo A escreve em p[0]...
//!   char *q = shmat(id, NULL, 0);     // ...e o processo B lê em q[0]
//!   shmctl(id, IPC_RMID, NULL);       // some quando o último se desanexar
//! ```

use crate::erro::*;
use crate::paginacao::{Fonte, Regiao, Segmento};
use crate::processo;
use crate::rtc;
use crate::spin::Mutex;
use crate::syscall::{escrever_usuario, Resultado};
use alloc::sync::Arc;
use alloc::vec::Vec;

const IPC_PRIVATE: i32 = 0;
const IPC_CREAT: u64 = 0o1000;
const IPC_EXCL: u64 = 0o2000;
const IPC_RMID: u64 = 0;
const IPC_SET: u64 = 1;
const IPC_STAT: u64 = 2;
const SHM_RDONLY: u64 = 0o10000;

struct Entrada {
    id: i32,
    chave: i32,
    segmento: Arc<Segmento>,
    tamanho: u64,
    uid: u32,
    gid: u32,
    modo: u32,
    criador: u32,
    ultimo: u32,
    criado: u64,
}

impl Entrada {
    /// Quantos processos o têm anexado: cada região é mais um `Arc`, e a
    /// própria tabela segura um.
    fn anexos(&self) -> u64 {
        Arc::strong_count(&self.segmento) as u64 - 1
    }

    fn permite(&self, escrita: bool) -> Result<(), i64> {
        let p = processo::atual();
        if p.euid == 0 {
            return Ok(());
        }
        let trio = if p.euid == self.uid {
            self.modo >> 6
        } else if p.egid == self.gid {
            self.modo >> 3
        } else {
            self.modo
        };
        let precisa = if escrita { 0o6 } else { 0o4 };
        if trio & precisa == precisa { Ok(()) } else { Err(EACCES) }
    }
}

static SEGMENTOS: Mutex<Vec<Entrada>> = Mutex::new(Vec::new());
static PROXIMO_ID: Mutex<i32> = Mutex::new(1);

/// `shmget(chave, tamanho, flags)`: acha (ou cria) o segmento da chave.
pub fn shmget(chave: i32, tamanho: u64, flags: u64) -> Resultado {
    let mut lista = SEGMENTOS.lock();
    if chave != IPC_PRIVATE {
        if let Some(e) = lista.iter().find(|e| e.chave == chave) {
            if flags & IPC_CREAT != 0 && flags & IPC_EXCL != 0 {
                return Err(EEXIST);
            }
            if tamanho > e.tamanho {
                return Err(EINVAL);
            }
            e.permite(flags & 0o222 != 0)?;
            return Ok(e.id as u64);
        }
        if flags & IPC_CREAT == 0 {
            return Err(ENOENT);
        }
    }
    if tamanho == 0 || tamanho > 1 << 30 {
        return Err(EINVAL);
    }
    let p = processo::atual();
    let id = {
        let mut proximo = PROXIMO_ID.lock();
        *proximo += 1;
        *proximo - 1
    };
    lista.push(Entrada {
        id,
        chave,
        segmento: Segmento::novo(tamanho),
        tamanho,
        uid: p.euid,
        gid: p.egid,
        modo: (flags & 0o777) as u32,
        criador: p.tgid,
        ultimo: 0,
        criado: rtc::agora(),
    });
    Ok(id as u64)
}

/// `shmat(id, endereço, flags)`: mapeia o segmento; devolve onde ficou.
pub fn shmat(id: i32, endereco: u64, flags: u64) -> Resultado {
    let (segmento, tamanho) = {
        let mut lista = SEGMENTOS.lock();
        let e = lista.iter_mut().find(|e| e.id == id).ok_or(EINVAL)?;
        let escrita = flags & SHM_RDONLY == 0;
        e.permite(escrita)?;
        e.ultimo = processo::atual().tgid;
        (e.segmento.clone(), e.segmento.tamanho())
    };
    let p = processo::atual();
    let inicio = if endereco != 0 {
        if endereco & 0xFFF != 0 || endereco < crate::paginacao::USUARIO_INICIO {
            return Err(EINVAL);
        }
        endereco
    } else {
        let m = p.mem().ok_or(EINVAL)?;
        let inicio = m.mmap_topo - tamanho;
        if inicio < m.brk + 0x1000_0000 {
            return Err(ENOMEM);
        }
        m.mmap_topo = inicio;
        inicio
    };
    let prot = if flags & SHM_RDONLY != 0 { 1 } else { 3 };
    let espaco = p.espaco().ok_or(EINVAL)?;
    espaco.remover_faixa(inicio, inicio + tamanho);
    espaco.criar_regiao(Regiao {
        inicio,
        fim: inicio + tamanho,
        prot,
        compartilhada: true,
        fonte: Fonte::Segmento { segmento, pagina: 0 },
    });
    Ok(inicio)
}

/// `shmdt(endereço)`: desfaz o `shmat` que mapeou o segmento ali.
pub fn shmdt(endereco: u64) -> Resultado {
    let espaco = processo::atual().espaco().ok_or(EINVAL)?;
    let regiao = espaco.regiao_de(endereco).filter(|r| r.inicio == endereco).ok_or(EINVAL)?;
    let Fonte::Segmento { segmento, .. } = &regiao.fonte else { return Err(EINVAL) };
    let fim = endereco + segmento.tamanho();
    espaco.remover_faixa(endereco, fim);
    Ok(0)
}

/// `shmctl(id, comando, buf)`: IPC_STAT, IPC_SET e IPC_RMID.
pub fn shmctl(id: i32, comando: u64, buf: u64) -> Resultado {
    let comando = comando & !0x100; // IPC_64: o formato novo da struct (é o único que temos)
    let mut lista = SEGMENTOS.lock();
    let posicao = lista.iter().position(|e| e.id == id).ok_or(EINVAL)?;
    let p = processo::atual();
    match comando {
        IPC_STAT => {
            let e = &lista[posicao];
            e.permite(false)?;
            // struct shmid64_ds do Linux (112 bytes), começando pela ipc64_perm (48)
            let mut s = [0u8; 112];
            s[0..4].copy_from_slice(&e.chave.to_le_bytes());
            s[4..8].copy_from_slice(&e.uid.to_le_bytes());
            s[8..12].copy_from_slice(&e.gid.to_le_bytes());
            s[12..16].copy_from_slice(&e.uid.to_le_bytes());
            s[16..20].copy_from_slice(&e.gid.to_le_bytes());
            s[20..22].copy_from_slice(&(e.modo as u16).to_le_bytes());
            s[48..56].copy_from_slice(&e.tamanho.to_le_bytes());
            s[72..80].copy_from_slice(&e.criado.to_le_bytes());
            s[80..84].copy_from_slice(&e.criador.to_le_bytes());
            s[84..88].copy_from_slice(&e.ultimo.to_le_bytes());
            s[88..96].copy_from_slice(&e.anexos().to_le_bytes());
            drop(lista);
            escrever_usuario(buf, &s)?;
            Ok(0)
        }
        IPC_SET => {
            let dados = crate::syscall::ler_usuario(buf, 48)?;
            let e = &mut lista[posicao];
            if p.euid != 0 && p.euid != e.uid {
                return Err(EPERM);
            }
            e.uid = u32::from_le_bytes(dados[4..8].try_into().unwrap());
            e.gid = u32::from_le_bytes(dados[8..12].try_into().unwrap());
            e.modo = u16::from_le_bytes([dados[20], dados[21]]) as u32 & 0o777;
            Ok(0)
        }
        IPC_RMID => {
            let e = &lista[posicao];
            if p.euid != 0 && p.euid != e.uid {
                return Err(EPERM);
            }
            // Sai da tabela (ninguém mais o acha); as páginas ficam enquanto
            // houver alguém anexado — as regiões seguram o segmento.
            lista.remove(posicao);
            Ok(0)
        }
        _ => Err(EINVAL),
    }
}
