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
use crate::syscall::{EBADF, EINVAL, EISDIR, EMFILE, ENOTDIR, ESPIPE, Errno};
use crate::tty;
use crate::vfs::{Device, NodeKind, NodeRef};

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
    pub ino: u64,
    pub mode: u32,
    pub size: u64,
    pub rdev: u64,
    pub nlink: u64,
}

pub const S_IFIFO: u32 = 0o010000;
pub const S_IFCHR: u32 = 0o020000;
pub const S_IFDIR: u32 = 0o040000;
pub const S_IFREG: u32 = 0o100000;

pub fn node_stat(node: &NodeRef) -> Stat {
    let (kind, rdev) = match &node.kind {
        NodeKind::File(_) => (S_IFREG, 0),
        NodeKind::Directory(_) => (S_IFDIR, 0),
        NodeKind::Device(Device::Null) => (S_IFCHR, 1 << 8 | 3),
        NodeKind::Device(Device::Zero) => (S_IFCHR, 1 << 8 | 5),
        NodeKind::Device(Device::Tty) => (S_IFCHR, 5 << 8),
    };
    let nlink = if node.is_dir() { 2 } else { 1 };
    Stat { ino: node.ino, mode: kind | node.mode, size: node.size(), rdev, nlink }
}

impl OpenFile {
    pub fn new(kind: FileKind, flags: u32) -> Arc<Self> {
        let flags = flags & (O_ACCMODE | O_APPEND | O_NONBLOCK);
        Arc::new(OpenFile { kind, flags: AtomicU32::new(flags), offset: SpinLock::new(0) })
    }

    /// Abre um nó da árvore (dispositivos viram o arquivo do dispositivo).
    pub fn open_node(node: NodeRef, path: String, flags: u32) -> Arc<Self> {
        let kind = match node.kind {
            NodeKind::Device(device) => FileKind::Device(device),
            _ => FileKind::Node { node, path },
        };
        Self::new(kind, flags)
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
            FileKind::Node { node, .. } => match &node.kind {
                NodeKind::File(data) => {
                    let data = data.lock();
                    let mut offset = self.offset.lock();
                    let start = (*offset as usize).min(data.len());
                    let end = start.saturating_add(max).min(data.len());
                    *offset = end as u64;
                    Ok(data[start..end].to_vec())
                }
                _ => Err(EISDIR),
            },
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
            FileKind::Node { node, .. } => match &node.kind {
                NodeKind::File(data) => {
                    let mut data = data.lock();
                    let mut offset = self.offset.lock();
                    if self.flags() & O_APPEND != 0 {
                        *offset = data.len() as u64;
                    }
                    let start = *offset as usize;
                    let end = start + bytes.len();
                    if data.len() < end {
                        data.resize(end, 0);
                    }
                    data[start..end].copy_from_slice(bytes);
                    *offset = end as u64;
                    Ok(bytes.len())
                }
                _ => Err(EISDIR),
            },
            FileKind::Pipe(end) if end.is_writer() => end.write(bytes, self.nonblocking()),
            FileKind::Pipe(_) => Err(EBADF),
            FileKind::Device(Device::Null | Device::Zero) => Ok(bytes.len()),
            FileKind::Device(Device::Tty) => Ok(tty::write(bytes)),
        }
    }

    pub fn truncate(&self) {
        if let FileKind::Node { node, .. } = &self.kind
            && let NodeKind::File(data) = &node.kind
        {
            data.lock().clear();
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

    pub fn stat(&self) -> Stat {
        match &self.kind {
            FileKind::Node { node, .. } => node_stat(node),
            FileKind::Pipe(_) => Stat { ino: 0, mode: S_IFIFO | 0o600, size: 0, rdev: 0, nlink: 1 },
            FileKind::Device(device) => {
                let rdev = match device {
                    Device::Null => 1 << 8 | 3,
                    Device::Zero => 1 << 8 | 5,
                    Device::Tty => 5 << 8,
                };
                Stat { ino: 0, mode: S_IFCHR | 0o666, size: 0, rdev, nlink: 1 }
            }
        }
    }

    /// Próximas entradas de um diretório, como (inode, tipo DT_*, nome). A
    /// posição do arquivo conta entradas já lidas; `advance` a avança.
    pub fn dir_entries(&self) -> Result<Vec<(u64, u8, String)>, Errno> {
        const DT_CHR: u8 = 2;
        const DT_DIR: u8 = 4;
        const DT_REG: u8 = 8;
        let FileKind::Node { node, .. } = &self.kind else { return Err(ENOTDIR) };
        let mut entries = vec![(node.ino, DT_DIR, String::from(".")), (node.ino, DT_DIR, String::from(".."))];
        for (name, child) in node.list()? {
            let kind = match child.kind {
                NodeKind::File(_) => DT_REG,
                NodeKind::Directory(_) => DT_DIR,
                NodeKind::Device(_) => DT_CHR,
            };
            entries.push((child.ino, kind, name));
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
