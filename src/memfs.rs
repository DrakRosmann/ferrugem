//! memfs: um sistema de arquivos que vive só na memória (como o tmpfs do
//! Linux). Serve de /tmp, de /dev e, quando não há disco, de raiz com os
//! programas embutidos no kernel.

use alloc::collections::BTreeMap;
use alloc::string::{String, ToString};
use alloc::sync::Arc;
use alloc::vec::Vec;
use core::any::Any;
use core::sync::atomic::{AtomicU32, AtomicU64, Ordering};

use crate::rtc;
use crate::sync::SpinLock;
use crate::syscall::{EEXIST, EFBIG, EINVAL, EISDIR, ENOENT, ENOTDIR, ENOTEMPTY, EXDEV, Errno};
use crate::vfs::{DirEntry, Device, Inode, Kind, NewNode, NodeRef};

// Programas de usuário compilados pelo build.rs (pasta user/).
include!(concat!(env!("OUT_DIR"), "/programs.rs"));

/// Cada memfs ganha um número de dispositivo próprio (o `st_dev`).
static NEXT_DEV: AtomicU64 = AtomicU64::new(0x100);
/// Limite de um arquivo: o heap do kernel é pequeno.
const MAX_FILE: u64 = 64 << 20;

struct Shared {
    dev: u64,
    next_ino: AtomicU64,
}

enum Data {
    File(SpinLock<Vec<u8>>),
    Directory(SpinLock<BTreeMap<String, NodeRef>>),
    Symlink(String),
    Device(Device),
}

pub struct MemNode {
    fs: Arc<Shared>,
    ino: u64,
    permissions: AtomicU32,
    uid: AtomicU32,
    gid: AtomicU32,
    links: AtomicU64,
    mtime: AtomicU64,
    data: Data,
}

impl MemNode {
    fn new_in(fs: &Arc<Shared>, data: Data, permissions: u32) -> Arc<MemNode> {
        Arc::new(MemNode {
            fs: fs.clone(),
            ino: fs.next_ino.fetch_add(1, Ordering::Relaxed),
            permissions: AtomicU32::new(permissions),
            uid: AtomicU32::new(0),
            gid: AtomicU32::new(0),
            links: AtomicU64::new(1),
            mtime: AtomicU64::new(rtc::now()),
            data,
        })
    }

    fn directory(&self) -> Result<&SpinLock<BTreeMap<String, NodeRef>>, Errno> {
        match &self.data {
            Data::Directory(entries) => Ok(entries),
            _ => Err(ENOTDIR),
        }
    }

    fn touch(&self) {
        self.mtime.store(rtc::now(), Ordering::Relaxed);
    }

    fn insert(&self, name: &str, node: NodeRef) -> Result<(), Errno> {
        let mut entries = self.directory()?.lock();
        if entries.contains_key(name) {
            return Err(EEXIST);
        }
        entries.insert(name.to_string(), node);
        drop(entries);
        self.touch();
        Ok(())
    }
}

/// Um memfs novo e vazio; devolve a raiz.
pub fn new() -> NodeRef {
    let fs = Arc::new(Shared { dev: NEXT_DEV.fetch_add(1, Ordering::Relaxed), next_ino: AtomicU64::new(1) });
    MemNode::new_in(&fs, Data::Directory(SpinLock::new(BTreeMap::new())), 0o755)
}

fn concrete(node: &NodeRef) -> &MemNode {
    node.as_any().downcast_ref::<MemNode>().expect("nó de outro sistema de arquivos")
}

/// Cria o dispositivo `name` no diretório `dir` (um memfs).
pub fn add_device(dir: &NodeRef, name: &str, device: Device) {
    let dir = concrete(dir);
    let node: NodeRef = MemNode::new_in(&dir.fs, Data::Device(device), 0o666);
    dir.insert(name, node).expect("dispositivo duplicado");
}

/// /dev: os dispositivos de caractere que o kernel conhece.
pub fn devfs() -> NodeRef {
    let dev = new();
    add_device(&dev, "null", Device::Null);
    add_device(&dev, "zero", Device::Zero);
    add_device(&dev, "tty", Device::Tty);
    add_device(&dev, "console", Device::Tty);
    dev
}

/// A raiz usada quando não há disco: /bin com os programas embutidos e um
/// /home de exemplo. Tudo se perde ao desligar.
pub fn builtin_root() -> NodeRef {
    let root = new();
    let write = |dir: &NodeRef, name: &str, data: &[u8], permissions: u32| {
        let file = dir.create(name, NewNode::File, permissions).unwrap();
        file.write_at(0, data).unwrap();
    };
    let bin = root.create("bin", NewNode::Directory, 0o755).unwrap();
    for (name, binary) in PROGRAMS {
        write(&bin, name, binary, 0o755);
    }
    for dir in ["dev", "tmp", "mnt"] {
        root.create(dir, NewNode::Directory, 0o755).unwrap();
    }
    let home = root.create("home", NewNode::Directory, 0o755).unwrap();
    let readme = "Bem-vindo ao FerrugemOS!\n\
         Sem disco, este sistema roda inteiramente em memória: os arquivos somem ao desligar.\n\
         No sh, experimente: ls /bin, cat leiame.txt | grep sistema, yes | head -n 3,\n\
         sleep 100 & e depois jobs, fg (Ctrl+Z suspende, Ctrl+C interrompe).\n";
    write(&home, "leiame.txt", readme.as_bytes(), 0o644);
    write(&home, "rust.txt", b"Feito em Rust, sem std, sem crates externos.\n", 0o644);
    root
}

impl Inode for MemNode {
    fn ino(&self) -> u64 {
        self.ino
    }

    fn dev(&self) -> u64 {
        self.fs.dev
    }

    fn kind(&self) -> Kind {
        match &self.data {
            Data::File(_) => Kind::File,
            Data::Directory(_) => Kind::Directory,
            Data::Symlink(_) => Kind::Symlink,
            Data::Device(device) => Kind::Device(*device),
        }
    }

    fn mode(&self) -> u32 {
        self.kind().type_bits() | self.permissions.load(Ordering::Relaxed)
    }

    fn uid(&self) -> u32 {
        self.uid.load(Ordering::Relaxed)
    }

    fn gid(&self) -> u32 {
        self.gid.load(Ordering::Relaxed)
    }

    fn set_permissions(&self, permissions: u32) -> Result<(), Errno> {
        self.permissions.store(permissions & 0o7777, Ordering::Relaxed);
        Ok(())
    }

    fn set_owner(&self, uid: Option<u32>, gid: Option<u32>) -> Result<(), Errno> {
        if let Some(uid) = uid {
            self.uid.store(uid, Ordering::Relaxed);
        }
        if let Some(gid) = gid {
            self.gid.store(gid, Ordering::Relaxed);
        }
        Ok(())
    }

    fn size(&self) -> u64 {
        match &self.data {
            Data::File(data) => data.lock().len() as u64,
            Data::Directory(entries) => entries.lock().len() as u64,
            Data::Symlink(target) => target.len() as u64,
            Data::Device(_) => 0,
        }
    }

    fn nlink(&self) -> u64 {
        match &self.data {
            // "." e a entrada no pai, mais o ".." de cada subdiretório.
            Data::Directory(entries) => 2 + entries.lock().values().filter(|n| n.is_dir()).count() as u64,
            _ => self.links.load(Ordering::Relaxed),
        }
    }

    fn mtime(&self) -> u64 {
        self.mtime.load(Ordering::Relaxed)
    }

    fn set_mtime(&self, seconds: u64) -> Result<(), Errno> {
        self.mtime.store(seconds, Ordering::Relaxed);
        Ok(())
    }

    fn rdev(&self) -> u64 {
        match &self.data {
            Data::Device(device) => device.rdev(),
            _ => 0,
        }
    }

    fn read_at(&self, offset: u64, buf: &mut [u8]) -> Result<usize, Errno> {
        let Data::File(data) = &self.data else { return Err(EISDIR) };
        let data = data.lock();
        let start = (offset as usize).min(data.len());
        let end = start.saturating_add(buf.len()).min(data.len());
        buf[..end - start].copy_from_slice(&data[start..end]);
        Ok(end - start)
    }

    fn write_at(&self, offset: u64, bytes: &[u8]) -> Result<usize, Errno> {
        let Data::File(data) = &self.data else { return Err(EISDIR) };
        let end = offset.checked_add(bytes.len() as u64).filter(|&end| end <= MAX_FILE).ok_or(EFBIG)?;
        let mut data = data.lock();
        if (data.len() as u64) < end {
            data.resize(end as usize, 0);
        }
        data[offset as usize..end as usize].copy_from_slice(bytes);
        drop(data);
        self.touch();
        Ok(bytes.len())
    }

    fn truncate(&self, size: u64) -> Result<(), Errno> {
        let Data::File(data) = &self.data else { return Err(EISDIR) };
        if size > MAX_FILE {
            return Err(EFBIG);
        }
        data.lock().resize(size as usize, 0);
        self.touch();
        Ok(())
    }

    fn lookup(&self, name: &str) -> Result<NodeRef, Errno> {
        self.directory()?.lock().get(name).cloned().ok_or(ENOENT)
    }

    fn entries(&self) -> Result<Vec<DirEntry>, Errno> {
        let entries = self.directory()?.lock();
        Ok(entries
            .iter()
            .map(|(name, node)| DirEntry { name: name.clone(), ino: node.ino(), dtype: node.kind().dtype() })
            .collect())
    }

    fn create(&self, name: &str, node: NewNode, permissions: u32) -> Result<NodeRef, Errno> {
        let data = match node {
            NewNode::File => Data::File(SpinLock::new(Vec::new())),
            NewNode::Directory => Data::Directory(SpinLock::new(BTreeMap::new())),
            NewNode::Symlink(target) => Data::Symlink(target.to_string()),
        };
        let node: NodeRef = MemNode::new_in(&self.fs, data, permissions & 0o7777);
        self.insert(name, node.clone())?;
        Ok(node)
    }

    fn link(&self, name: &str, target: &NodeRef) -> Result<(), Errno> {
        let concrete = target.as_any().downcast_ref::<MemNode>().ok_or(EXDEV)?;
        self.insert(name, target.clone())?;
        concrete.links.fetch_add(1, Ordering::Relaxed);
        Ok(())
    }

    fn unlink(&self, name: &str, directory: bool) -> Result<(), Errno> {
        let mut entries = self.directory()?.lock();
        let node = entries.get(name).ok_or(ENOENT)?;
        match (directory, node.is_dir()) {
            (false, true) => return Err(EISDIR),
            (true, false) => return Err(ENOTDIR),
            (true, true) if node.size() > 0 => return Err(ENOTEMPTY),
            _ => {}
        }
        let node = entries.remove(name).unwrap();
        drop(entries);
        concrete(&node).links.fetch_sub(1, Ordering::Relaxed);
        self.touch();
        Ok(())
    }

    fn rename(&self, name: &str, new_dir: &NodeRef, new_name: &str) -> Result<(), Errno> {
        let new_dir = new_dir.as_any().downcast_ref::<MemNode>().ok_or(EXDEV)?;
        let node = self.directory()?.lock().remove(name).ok_or(ENOENT)?;
        self.touch();
        new_dir.insert(new_name, node)
    }

    fn readlink(&self) -> Result<String, Errno> {
        match &self.data {
            Data::Symlink(target) => Ok(target.clone()),
            _ => Err(EINVAL),
        }
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}
