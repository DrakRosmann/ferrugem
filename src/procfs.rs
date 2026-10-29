//! `/proc`: um sistema de arquivos que não guarda nada.
//!
//! Cada "arquivo" daqui é gerado na hora em que é lido, a partir do estado do
//! kernel. É assim que, no Linux, programas como `ps`, `top` e `free` sabem o
//! que está acontecendo: leem texto do `/proc` em vez de usar chamadas especiais.
//!
//! ```text
//!   /proc/meminfo  uptime  version  cpuinfo  mounts  loadavg
//!   /proc/self                       → link para o diretório do próprio processo
//!   /proc/<pid>/status  stat  cmdline  cwd → ...  exe → ...
//! ```

use crate::erro::*;
use crate::processo::{self, Pid, Resumo};
use crate::vfs::{self, Meta, Tipo};
use crate::{heap, pit, quadros, rtc};
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use core::arch::x86_64::__cpuid;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Global {
    Meminfo,
    Uptime,
    Version,
    Cpuinfo,
    Mounts,
    Loadavg,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Item {
    Status,
    Stat,
    Cmdline,
    Cwd,
    Exe,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum NoProc {
    Raiz,
    /// `/proc/self`
    Eu,
    Global(Global),
    /// `/proc/<pid>`
    Processo(Pid),
    DoProcesso(Pid, Item),
}

const GLOBAIS: [(&str, Global); 6] = [
    ("cpuinfo", Global::Cpuinfo),
    ("loadavg", Global::Loadavg),
    ("meminfo", Global::Meminfo),
    ("mounts", Global::Mounts),
    ("uptime", Global::Uptime),
    ("version", Global::Version),
];

const ITENS: [(&str, Item); 5] = [
    ("cmdline", Item::Cmdline),
    ("cwd", Item::Cwd),
    ("exe", Item::Exe),
    ("stat", Item::Stat),
    ("status", Item::Status),
];

fn processo(pid: Pid) -> Result<Resumo, i64> {
    processo::resumo(pid).ok_or(ENOENT)
}

impl NoProc {
    pub fn tipo(&self) -> Tipo {
        match self {
            NoProc::Raiz | NoProc::Processo(_) => Tipo::Diretorio,
            NoProc::Eu | NoProc::DoProcesso(_, Item::Cwd | Item::Exe) => Tipo::Link,
            _ => Tipo::Arquivo,
        }
    }

    pub fn id(&self) -> u64 {
        let base = 0x2_0000_0000;
        match self {
            NoProc::Raiz => base + 1,
            NoProc::Eu => base + 2,
            NoProc::Global(g) => base + 10 + *g as u64,
            NoProc::Processo(pid) => base + 0x1000 * (*pid as u64 + 1),
            NoProc::DoProcesso(pid, item) => base + 0x1000 * (*pid as u64 + 1) + 1 + *item as u64,
        }
    }

    pub fn meta(&self) -> Meta {
        let (uid, gid) = match self {
            NoProc::Processo(pid) | NoProc::DoProcesso(pid, _) => {
                processo::resumo(*pid).map_or((0, 0), |r| (r.uid, r.gid))
            }
            _ => (0, 0),
        };
        let permissoes = match self.tipo() {
            Tipo::Diretorio => 0o555,
            Tipo::Link => 0o777,
            _ => 0o444,
        };
        Meta { permissoes, uid, gid, modificado: rtc::agora() }
    }

    pub fn filho(&self, nome: &str) -> Result<NoProc, i64> {
        match self {
            NoProc::Raiz => {
                if nome == "self" {
                    return Ok(NoProc::Eu);
                }
                if let Some((_, g)) = GLOBAIS.iter().find(|(n, _)| *n == nome) {
                    return Ok(NoProc::Global(*g));
                }
                let pid: Pid = nome.parse().map_err(|_| ENOENT)?;
                processo(pid).map(|_| NoProc::Processo(pid))
            }
            NoProc::Processo(pid) => {
                processo(*pid)?;
                let (_, item) = ITENS.iter().find(|(n, _)| *n == nome).ok_or(ENOENT)?;
                Ok(NoProc::DoProcesso(*pid, *item))
            }
            _ => Err(ENOTDIR),
        }
    }

    pub fn listar(&self) -> Result<Vec<(String, u64, Tipo)>, i64> {
        let mut lista = Vec::new();
        let mut por = |no: NoProc, nome: String| lista.push((nome, no.id(), no.tipo()));
        match self {
            NoProc::Raiz => {
                por(NoProc::Eu, String::from("self"));
                for (nome, g) in GLOBAIS {
                    por(NoProc::Global(g), String::from(nome));
                }
                for r in processo::listar().into_iter().skip(1) {
                    por(NoProc::Processo(r.pid), format!("{}", r.pid));
                }
            }
            NoProc::Processo(pid) => {
                processo(*pid)?;
                for (nome, item) in ITENS {
                    por(NoProc::DoProcesso(*pid, item), String::from(nome));
                }
            }
            _ => return Err(ENOTDIR),
        }
        Ok(lista)
    }

    pub fn ler_link(&self) -> Result<String, i64> {
        match self {
            NoProc::Eu => Ok(format!("{}", processo::atual().pid)),
            NoProc::DoProcesso(pid, Item::Cwd) => Ok(processo(*pid)?.diretorio),
            NoProc::DoProcesso(pid, Item::Exe) => Ok(processo(*pid)?.executavel),
            _ => Err(EINVAL),
        }
    }

    /// O conteúdo de um arquivo do /proc, gerado agora.
    pub fn conteudo(&self) -> Result<Vec<u8>, i64> {
        let texto = match self {
            NoProc::Global(g) => global(*g),
            NoProc::DoProcesso(pid, item) => {
                let r = processo(*pid)?;
                match item {
                    Item::Status => status(&r),
                    Item::Stat => stat(&r),
                    Item::Cmdline => {
                        // argumentos separados (e terminados) por \0
                        let mut bytes = Vec::new();
                        for a in &r.argumentos {
                            bytes.extend_from_slice(a.as_bytes());
                            bytes.push(0);
                        }
                        return Ok(bytes);
                    }
                    _ => return Err(EINVAL),
                }
            }
            _ => return Err(EISDIR),
        };
        Ok(texto.into_bytes())
    }
}

fn letra_do_estado(estado: u32) -> (char, &'static str) {
    match estado {
        0 | 1 => ('R', "running"),
        2 => ('S', "sleeping"),
        3 => ('Z', "zombie"),
        _ => ('T', "stopped"),
    }
}

fn status(r: &Resumo) -> String {
    let (letra, nome) = letra_do_estado(r.estado);
    format!(
        "Name:\t{}\nState:\t{} ({})\nTgid:\t{}\nPid:\t{}\nPPid:\t{}\nUid:\t{}\t{}\t{}\t{}\nGid:\t{}\t{}\t{}\t{}\n\
         VmRSS:\t{} kB\nThreads:\t{}\n",
        r.nome, letra, nome, r.tgid, r.pid, r.pai, r.uid, r.euid, r.euid, r.euid, r.gid, r.egid, r.egid, r.egid,
        r.memoria_kib, r.threads
    )
}

/// O formato de uma linha só do Linux, que o `ps` do busybox e o `top` leem.
fn stat(r: &Resumo) -> String {
    let (letra, _) = letra_do_estado(r.estado);
    let inicio = r.inicio_ms / 10; // em tiques de 1/100 s
    format!(
        "{} ({}) {} {} {} {} 0 {} 0 0 0 0 0 {} 0 0 0 20 0 {} 0 {} {} {} 0 0 0 0 0 0 0 0 0 0 0 0 0 17 0 0 0 0 0 0 0 0 0 0 0 0 0 0\n",
        r.pid,
        r.nome,
        letra,
        r.pai,
        r.pgid,
        r.sid,
        r.pgid,
        r.tiques,
        r.threads,
        inicio,
        r.paginas * 4096,
        r.paginas
    )
}

fn marca_da_cpu() -> String {
    #[allow(unused_unsafe)]
    let maximo = unsafe { __cpuid(0x8000_0000) }.eax;
    if maximo < 0x8000_0004 {
        return String::from("processador x86_64");
    }
    let mut bytes = Vec::new();
    for folha in 0x8000_0002..=0x8000_0004u32 {
        #[allow(unused_unsafe)]
        let r = unsafe { __cpuid(folha) };
        for registro in [r.eax, r.ebx, r.ecx, r.edx] {
            bytes.extend_from_slice(&registro.to_le_bytes());
        }
    }
    String::from_utf8_lossy(&bytes).trim_matches(|c: char| c == '\0' || c == ' ').into()
}

fn global(g: Global) -> String {
    match g {
        Global::Meminfo => {
            let (total, livres) = quadros::estatisticas();
            format!(
                "MemTotal:       {:>8} kB\nMemFree:        {:>8} kB\nMemAvailable:   {:>8} kB\nBuffers:        {:>8} kB\n\
                 Cached:         {:>8} kB\nSwapTotal:      {:>8} kB\nSwapFree:       {:>8} kB\nKernelHeap:     {:>8} kB\n",
                total * 4,
                livres * 4,
                livres * 4,
                0,
                0,
                0,
                0,
                heap::usado() / 1024
            )
        }
        Global::Uptime => {
            let ms = pit::milissegundos();
            let ocioso = processo::resumo(0).map_or(0, |r| r.tiques) * 10;
            format!("{}.{:02} {}.{:02}\n", ms / 1000, ms % 1000 / 10, ocioso / 1000, ocioso % 1000 / 10)
        }
        Global::Version => String::from("FerrugemOS version 0.5.0 (rustc, sem std) #1 x86_64\n"),
        Global::Cpuinfo => format!(
            "processor\t: 0\nmodel name\t: {}\nrdrand\t\t: {}\n\n",
            marca_da_cpu(),
            if crate::aleatorio::tem_hardware() { "sim" } else { "não" }
        ),
        Global::Mounts => {
            let mut t = String::new();
            for (dispositivo, ponto, tipo) in vfs::montagens() {
                t.push_str(&format!("{} {} {} rw 0 0\n", dispositivo, ponto, tipo));
            }
            t
        }
        Global::Loadavg => {
            let lista = processo::listar();
            let rodando = lista.iter().filter(|r| r.estado <= 1).count().saturating_sub(1);
            let ultimo = lista.iter().map(|r| r.pid).max().unwrap_or(0);
            format!("0.00 0.00 0.00 {}/{} {}\n", rodando, lista.len() - 1, ultimo)
        }
    }
}
