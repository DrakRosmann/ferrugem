//! Arquivos abertos e a tabela de descritores de cada processo.
//!
//! Como no Unix, um descritor (número) aponta para uma descrição de arquivo
//! aberto (`OpenFile`), que guarda a posição atual e os flags. `dup` e
//! `fork` copiam descritores que apontam para a mesma descrição, e por isso
//! compartilham a posição.

use alloc::string::String;
use alloc::sync::Arc;
use alloc::vec;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicU32, Ordering};

use crate::pipe::PipeEnd;
use crate::sync::SpinLock;
use crate::syscall::{EBADF, EINVAL, EISDIR, EMFILE, ENOTDIR, ENXIO, ESPIPE, Errno};
use crate::tty;
use crate::vfs::{DT_DIR, Device, Kind, NodeRef, S_IFCHR, S_IFIFO};

pub const O_ACCMODE: u32 = 3;
pub const O_RDONLY: u32 = 0;
pub const O_WRONLY: u32 = 1;
pub const O_RDWR: u32 = 2;
pub const O_CREAT: u32 = 0x40;
pub const O_EXCL: u32 = 0x80;
pub const O_TRUNC: u32 = 0x200;
pub const O_APPEND: u32 = 0x400;
pub const O_NONBLOCK: u32 = 0x800;
pub const O_DIRECTORY: u32 = 0x1_0000;
pub const O_CLOEXEC: u32 = 0x8_0000;

const MAX_FDS: usize = 256;

pub enum FileKind {
    /// Arquivo ou diretório da árvore, com seu caminho absoluto.
    Node { node: NodeRef, path: String },
    Pipe(PipeEnd),
    Device(Device),
}

pub struct OpenFile {
    pub kind: FileKind,
    /// Modo de acesso e flags de estado (O_APPEND, O_NONBLOCK).
    flags: AtomicU32,
    offset: SpinLock<u64>,
}

/// Os campos do `struct stat` do Linux que fazem sentido aqui.
pub struct Stat {
    pub dev: u64,
    pub ino: u64,
    pub mode: u32,
    pub uid: u32,
    pub gid: u32,
    pub size: u64,
    pub rdev: u64,
    pub nlink: u64,
    pub mtime: u64,
}

pub fn node_stat(node: &NodeRef) -> Stat {
    Stat {
        dev: node.dev(),
        ino: node.ino(),
        mode: node.mode(),
        uid: node.uid(),
        gid: node.gid(),
        size: node.size(),
        rdev: node.rdev(),
        nlink: node.nlink(),
        mtime: node.mtime(),
    }
}

impl OpenFile {
    pub fn new(kind: FileKind, flags: u32) -> Arc<Self> {
        let flags = flags & (O_ACCMODE | O_APPEND | O_NONBLOCK);
        Arc::new(OpenFile { kind, flags: AtomicU32::new(flags), offset: SpinLock::new(0) })
    }

    /// Abre um nó da árvore (dispositivos viram o arquivo do dispositivo).
    pub fn open_node(node: NodeRef, path: String, flags: u32) -> Result<Arc<Self>, Errno> {
        let kind = match node.kind() {
            Kind::Device(device) => FileKind::Device(device),
            Kind::Special => return Err(ENXIO),
            _ => FileKind::Node { node, path },
        };
        Ok(Self::new(kind, flags))
    }

    pub fn console() -> Arc<Self> {
        Self::new(FileKind::Device(Device::Tty), O_RDWR)
    }

    pub fn flags(&self) -> u32 {
        self.flags.load(Ordering::Relaxed)
    }

    /// F_SETFL: só O_APPEND e O_NONBLOCK podem mudar.
    pub fn set_status_flags(&self, flags: u32) {
        let keep = self.flags() & O_ACCMODE;
        self.flags.store(keep | (flags & (O_APPEND | O_NONBLOCK)), Ordering::Relaxed);
    }

    fn readable(&self) -> bool {
        self.flags() & O_ACCMODE != O_WRONLY
    }

    fn writable(&self) -> bool {
        self.flags() & O_ACCMODE != O_RDONLY
    }

    fn nonblocking(&self) -> bool {
        self.flags() & O_NONBLOCK != 0
    }

    pub fn is_tty(&self) -> bool {
        matches!(self.kind, FileKind::Device(Device::Tty))
    }

    /// Caminho absoluto, para arquivos e diretórios da árvore.
    pub fn path(&self) -> Option<&str> {
        match &self.kind {
            FileKind::Node { path, .. } => Some(path),
            _ => None,
        }
    }

    pub fn read(&self, max: usize) -> Result<Vec<u8>, Errno> {
        if !self.readable() {
            return Err(EBADF);
        }
        match &self.kind {
            FileKind::Node { node, .. } => {
                let mut offset = self.offset.lock();
                let remaining = node.size().saturating_sub(*offset);
                let mut data = vec![0; (max as u64).min(remaining).min(1 << 20) as usize];
                let n = node.read_at(*offset, &mut data)?;
                data.truncate(n);
                *offset += n as u64;
                Ok(data)
            }
            FileKind::Pipe(end) if !end.is_writer() => end.read(max, self.nonblocking()),
            FileKind::Pipe(_) => Err(EBADF),
            FileKind::Device(Device::Null) => Ok(Vec::new()),
            FileKind::Device(Device::Zero) => Ok(vec![0; max.min(64 * 1024)]),
            FileKind::Device(Device::Tty) => tty::read(max, self.nonblocking()),
        }
    }

    pub fn write(&self, bytes: &[u8]) -> Result<usize, Errno> {
        if !self.writable() {
            return Err(EBADF);
        }
        match &self.kind {
            FileKind::Node { node, .. } => {
                let mut offset = self.offset.lock();
                if self.flags() & O_APPEND != 0 {
                    *offset = node.size();
                }
                let n = node.write_at(*offset, bytes)?;
                *offset += n as u64;
                Ok(n)
            }
            FileKind::Pipe(end) if end.is_writer() => end.write(bytes, self.nonblocking()),
            FileKind::Pipe(_) => Err(EBADF),
            FileKind::Device(Device::Null | Device::Zero) => Ok(bytes.len()),
            FileKind::Device(Device::Tty) => Ok(tty::write(bytes)),
        }
    }

    pub fn truncate(&self, size: u64) -> Result<(), Errno> {
        match &self.kind {
            FileKind::Node { node, .. } if node.is_dir() => Err(EISDIR),
            FileKind::Node { node, .. } if self.writable() => node.truncate(size),
            _ => Err(EINVAL),
        }
    }

    /// O nó da árvore, para arquivos e diretórios.
    pub fn node(&self) -> Option<&NodeRef> {
        match &self.kind {
            FileKind::Node { node, .. } => Some(node),
            _ => None,
        }
    }

    pub fn seek(&self, offset: i64, whence: u32) -> Result<u64, Errno> {
        const SEEK_SET: u32 = 0;
        const SEEK_CUR: u32 = 1;
        const SEEK_END: u32 = 2;
        let FileKind::Node { node, .. } = &self.kind else { return Err(ESPIPE) };
        let mut current = self.offset.lock();
        let base = match whence {
            SEEK_SET => 0,
            SEEK_CUR => *current as i64,
            SEEK_END => node.size() as i64,
            _ => return Err(EINVAL),
        };
        let target = base.checked_add(offset).filter(|&t| t >= 0).ok_or(EINVAL)?;
        *current = target as u64;
        Ok(target as u64)
    }

    /// Eventos de `poll` prontos agora (POLLIN, POLLOUT, POLLHUP, POLLERR).
    pub fn poll(&self) -> u16 {
        const POLLIN: u16 = 0x1;
        const POLLOUT: u16 = 0x4;
        const POLLERR: u16 = 0x8;
        const POLLHUP: u16 = 0x10;
        match &self.kind {
            FileKind::Pipe(end) => {
                let (readable, writable, closed) = end.readiness();
                let mut events = 0;
                if readable {
                    events |= POLLIN;
                }
                if writable {
                    events |= POLLOUT;
                }
                if closed {
                    events |= if end.is_writer() { POLLERR } else { POLLHUP };
                }
                events
            }
            FileKind::Device(Device::Tty) => POLLOUT | if tty::readable() { POLLIN } else { 0 },
            _ => POLLIN | POLLOUT, // arquivos e /dev/null nunca bloqueiam
        }
    }

    /// `pread64`: lê de uma posição sem mexer na posição do arquivo.
    pub fn read_at(&self, offset: u64, max: usize) -> Result<Vec<u8>, Errno> {
        let FileKind::Node { node, .. } = &self.kind else { return Err(ESPIPE) };
        if !self.readable() {
            return Err(EBADF);
        }
        let mut data = vec![0; (max as u64).min(node.size().saturating_sub(offset)).min(1 << 20) as usize];
        let n = node.read_at(offset, &mut data)?;
        data.truncate(n);
        Ok(data)
    }

    pub fn write_at(&self, offset: u64, bytes: &[u8]) -> Result<usize, Errno> {
        let FileKind::Node { node, .. } = &self.kind else { return Err(ESPIPE) };
        if !self.writable() {
            return Err(EBADF);
        }
        node.write_at(offset, bytes)
    }

    pub fn stat(&self) -> Stat {
        match &self.kind {
            FileKind::Node { node, .. } => node_stat(node),
            FileKind::Pipe(_) => Stat { dev: 0, ino: 0, mode: S_IFIFO | 0o600, uid: 0, gid: 0, size: 0, rdev: 0, nlink: 1, mtime: 0 },
            FileKind::Device(device) => {
                Stat {
                dev: 0,
                ino: 0,
                mode: S_IFCHR | 0o666,
                uid: 0,
                gid: 0,
                size: 0,
                rdev: device.rdev(),
                nlink: 1,
                mtime: 0,
            }
            }
        }
    }

    /// Próximas entradas de um diretório, como (inode, tipo DT_*, nome). A
    /// posição do arquivo conta entradas já lidas; `advance` a avança.
    pub fn dir_entries(&self) -> Result<Vec<(u64, u8, String)>, Errno> {
        let FileKind::Node { node, .. } = &self.kind else { return Err(ENOTDIR) };
        let mut entries = vec![(node.ino(), DT_DIR, String::from(".")), (node.ino(), DT_DIR, String::from(".."))];
        for entry in node.entries()? {
            entries.push((entry.ino, entry.dtype, entry.name));
        }
        let skip = *self.offset.lock() as usize;
        Ok(entries.into_iter().skip(skip).collect())
    }

    pub fn advance(&self, count: usize) {
        *self.offset.lock() += count as u64;
    }
}

#[derive(Clone)]
pub struct Descriptor {
    pub file: Arc<OpenFile>,
    pub close_on_exec: bool,
}

#[derive(Clone)]
pub struct FdTable(Vec<Option<Descriptor>>);

impl FdTable {
    /// 0, 1 e 2 apontando para o terminal.
    pub fn with_console() -> Self {
        let console = OpenFile::console();
        let fd = || Some(Descriptor { file: console.clone(), close_on_exec: false });
        FdTable(vec![fd(), fd(), fd()])
    }

    pub fn get(&self, fd: i32) -> Result<Arc<OpenFile>, Errno> {
        self.descriptor(fd).map(|d| d.file.clone())
    }

    pub fn descriptor(&self, fd: i32) -> Result<&Descriptor, Errno> {
        usize::try_from(fd).ok().and_then(|i| self.0.get(i)?.as_ref()).ok_or(EBADF)
    }

    pub fn set_close_on_exec(&mut self, fd: i32, value: bool) -> Result<(), Errno> {
        let slot = usize::try_from(fd).ok().and_then(|i| self.0.get_mut(i)?.as_mut()).ok_or(EBADF)?;
        slot.close_on_exec = value;
        Ok(())
    }

    /// Instala no menor descritor livre que seja >= `min`.
    pub fn insert(&mut self, file: Arc<OpenFile>, close_on_exec: bool, min: usize) -> Result<i32, Errno> {
        let index = (min..MAX_FDS).find(|&i| self.0.get(i).is_none_or(|slot| slot.is_none())).ok_or(EMFILE)?;
        if self.0.len() <= index {
            self.0.resize(index + 1, None);
        }
        self.0[index] = Some(Descriptor { file, close_on_exec });
        Ok(index as i32)
    }

    /// Instala em `fd`, devolvendo o que estava lá (para fechar fora da trava).
    pub fn install(&mut self, fd: i32, file: Arc<OpenFile>, close_on_exec: bool) -> Result<Option<Descriptor>, Errno> {
        let index = usize::try_from(fd).ok().filter(|&i| i < MAX_FDS).ok_or(EBADF)?;
        if self.0.len() <= index {
            self.0.resize(index + 1, None);
        }
        Ok(self.0[index].replace(Descriptor { file, close_on_exec }))
    }

    pub fn close(&mut self, fd: i32) -> Result<Descriptor, Errno> {
        usize::try_from(fd).ok().and_then(|i| self.0.get_mut(i)?.take()).ok_or(EBADF)
    }

    /// Remove os descritores marcados com O_CLOEXEC (no `execve`).
    pub fn take_close_on_exec(&mut self) -> Vec<Descriptor> {
        self.0.iter_mut().filter(|s| s.as_ref().is_some_and(|d| d.close_on_exec)).filter_map(Option::take).collect()
    }

    pub fn take_all(&mut self) -> Vec<Descriptor> {
        self.0.drain(..).flatten().collect()
    }
}
