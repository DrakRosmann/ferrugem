//! Sistema de arquivos virtual (VFS): a árvore única de caminhos do Unix,
//! montada a partir de vários sistemas de arquivos.
//!
//! Cada sistema de arquivos (ext2 no disco, memfs na memória) implementa o
//! trait `Inode`. A tabela de montagem diz qual sistema aparece em cada
//! diretório; com um disco, por exemplo:
//!
//! ```text
//! /      ext2 (disco ATA)     /dev   memfs com null, zero, tty, console
//! /tmp   memfs (some ao desligar)
//! ```
//!
//! Sem disco, a raiz é um memfs com os programas embutidos no kernel.

use alloc::string::{String, ToString};
use alloc::sync::Arc;
use alloc::vec::Vec;
use core::any::Any;

use crate::sync::SpinLock;
use crate::syscall::{
    EBUSY, EEXIST, EINVAL, EISDIR, ELOOP, ENAMETOOLONG, ENOENT, ENOTDIR, EPERM, EXDEV, Errno,
};

const MAX_NAME: usize = 255;
/// Quantos links simbólicos seguir num caminho antes de desistir (ELOOP).
const MAX_SYMLINKS: usize = 40;

pub type NodeRef = Arc<dyn Inode>;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Device {
    Null,
    Zero,
    Tty,
}

impl Device {
    /// Número do dispositivo (major << 8 | minor), como no Linux.
    pub fn rdev(self) -> u64 {
        match self {
            Device::Null => 1 << 8 | 3,
            Device::Zero => 1 << 8 | 5,
            Device::Tty => 5 << 8,
        }
    }

    pub fn from_rdev(rdev: u64) -> Option<Device> {
        match rdev {
            0x103 => Some(Device::Null),
            0x105 => Some(Device::Zero),
            0x500 | 0x501 | 0x400 => Some(Device::Tty), // tty, console, tty0
            _ => None,
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Kind {
    File,
    Directory,
    Symlink,
    Device(Device),
    /// FIFOs, sockets e dispositivos que o kernel não conhece.
    Special,
}

/// O que criar num diretório.
pub enum NewNode<'a> {
    File,
    Directory,
    Symlink(&'a str),
}

pub struct DirEntry {
    pub name: String,
    pub ino: u64,
    /// Tipo no formato de `getdents64` (DT_REG, DT_DIR...).
    pub dtype: u8,
}

pub const DT_FIFO: u8 = 1;
pub const DT_CHR: u8 = 2;
pub const DT_DIR: u8 = 4;
pub const DT_REG: u8 = 8;
pub const DT_LNK: u8 = 10;

pub const S_IFIFO: u32 = 0o010000;
pub const S_IFCHR: u32 = 0o020000;
pub const S_IFDIR: u32 = 0o040000;
pub const S_IFREG: u32 = 0o100000;
pub const S_IFLNK: u32 = 0o120000;

impl Kind {
    pub fn dtype(self) -> u8 {
        match self {
            Kind::File => DT_REG,
            Kind::Directory => DT_DIR,
            Kind::Symlink => DT_LNK,
            Kind::Device(_) => DT_CHR,
            Kind::Special => 0,
        }
    }

    pub fn type_bits(self) -> u32 {
        match self {
            Kind::File => S_IFREG,
            Kind::Directory => S_IFDIR,
            Kind::Symlink => S_IFLNK,
            Kind::Device(_) => S_IFCHR,
            Kind::Special => S_IFIFO,
        }
    }
}

/// Um arquivo, diretório, link ou dispositivo de algum sistema de arquivos.
/// As operações que não fazem sentido para o tipo do nó têm um padrão que
/// devolve o erro adequado.
pub trait Inode: Send + Sync {
    fn ino(&self) -> u64;
    /// Identifica o sistema de arquivos (o `st_dev` do `stat`).
    fn dev(&self) -> u64;
    fn kind(&self) -> Kind;
    /// `st_mode` completo: tipo (S_IFREG...) e permissões.
    fn mode(&self) -> u32;
    fn size(&self) -> u64;
    fn nlink(&self) -> u64;
    /// Última modificação, em segundos desde 1970.
    fn mtime(&self) -> u64 {
        0
    }
    fn set_mtime(&self, _seconds: u64) -> Result<(), Errno> {
        Ok(())
    }
    fn rdev(&self) -> u64 {
        0
    }

    // Arquivos regulares.
    fn read_at(&self, _offset: u64, _buf: &mut [u8]) -> Result<usize, Errno> {
        Err(if self.kind() == Kind::Directory { EISDIR } else { EINVAL })
    }
    fn write_at(&self, _offset: u64, _data: &[u8]) -> Result<usize, Errno> {
        Err(if self.kind() == Kind::Directory { EISDIR } else { EINVAL })
    }
    fn truncate(&self, _size: u64) -> Result<(), Errno> {
        Err(if self.kind() == Kind::Directory { EISDIR } else { EINVAL })
    }

    // Diretórios. `name` nunca é "." nem "..": o VFS cuida deles.
    fn lookup(&self, _name: &str) -> Result<NodeRef, Errno> {
        Err(ENOTDIR)
    }
    /// As entradas, sem "." e "..".
    fn entries(&self) -> Result<Vec<DirEntry>, Errno> {
        Err(ENOTDIR)
    }
    fn create(&self, _name: &str, _node: NewNode, _permissions: u32) -> Result<NodeRef, Errno> {
        Err(ENOTDIR)
    }
    /// Nova entrada `name` para o nó `target` (link físico).
    fn link(&self, _name: &str, _target: &NodeRef) -> Result<(), Errno> {
        Err(ENOTDIR)
    }
    /// Remove um arquivo (`directory = false`) ou um diretório vazio.
    fn unlink(&self, _name: &str, _directory: bool) -> Result<(), Errno> {
        Err(ENOTDIR)
    }
    /// Move a entrada `name` para `new_name` em `new_dir`, que é do mesmo
    /// sistema de arquivos e ainda não tem essa entrada.
    fn rename(&self, _name: &str, _new_dir: &NodeRef, _new_name: &str) -> Result<(), Errno> {
        Err(ENOTDIR)
    }

    fn readlink(&self) -> Result<String, Errno> {
        Err(EINVAL)
    }

    /// Chamado ao desligar, na raiz de cada sistema montado.
    fn unmount(&self) {}

    /// Para um sistema de arquivos achar o tipo concreto de outro nó seu.
    fn as_any(&self) -> &dyn Any;
}

impl dyn Inode {
    pub fn is_dir(&self) -> bool {
        self.kind() == Kind::Directory
    }

    pub fn same_as(&self, other: &NodeRef) -> bool {
        self.dev() == other.dev() && self.ino() == other.ino()
    }
}

// --- Montagens ---------------------------------------------------------------

struct Mount {
    path: String,
    root: NodeRef,
    fstype: &'static str,
    source: String,
}

static MOUNTS: SpinLock<Vec<Mount>> = SpinLock::new(Vec::new());

/// Monta `root` em `path` (absoluto e normalizado).
pub fn mount(path: &str, root: NodeRef, fstype: &'static str, source: &str) {
    let mut mounts = MOUNTS.lock();
    mounts.retain(|m| m.path != path);
    mounts.push(Mount { path: path.to_string(), root, fstype, source: source.to_string() });
}

fn mounted_at(path: &str) -> Option<NodeRef> {
    MOUNTS.lock().iter().find(|m| m.path == path).map(|m| m.root.clone())
}

/// (origem, ponto de montagem, tipo) de cada montagem.
pub fn mounts() -> Vec<(String, String, &'static str)> {
    MOUNTS.lock().iter().map(|m| (m.source.clone(), m.path.clone(), m.fstype)).collect()
}

/// Avisa cada sistema montado de que o sistema vai desligar.
pub fn unmount_all() {
    let roots: Vec<NodeRef> = MOUNTS.lock().iter().map(|m| m.root.clone()).collect();
    for root in roots {
        root.unmount();
    }
}

pub fn root() -> NodeRef {
    mounted_at("/").expect("nada montado na raiz")
}

// --- Caminhos ------------------------------------------------------------------

/// Um caminho resolvido: o nó e o caminho canônico (absoluto, sem `.`, `..`
/// nem links simbólicos).
pub struct Resolved {
    pub node: NodeRef,
    pub path: String,
}

fn components(path: &str) -> impl DoubleEndedIterator<Item = &str> {
    path.split('/').filter(|c| !c.is_empty())
}

fn join(stack: &[String]) -> String {
    if stack.is_empty() {
        return String::from("/");
    }
    stack.iter().fold(String::new(), |path, name| path + "/" + name)
}

/// O nó de um caminho canônico (já sem links nem `..`).
fn node_at(stack: &[String]) -> Result<NodeRef, Errno> {
    let mut node = root();
    for depth in 1..=stack.len() {
        node = match mounted_at(&join(&stack[..depth])) {
            Some(mounted) => mounted,
            None => node.lookup(&stack[depth - 1])?,
        };
    }
    Ok(node)
}

/// Resolve `path` a partir do diretório `cwd` (canônico). Links simbólicos
/// no meio do caminho são sempre seguidos; o do fim, só com `follow`.
pub fn lookup(cwd: &str, path: &str, follow: bool) -> Result<Resolved, Errno> {
    if path.is_empty() {
        return Err(ENOENT);
    }
    let mut stack: Vec<String> =
        if path.starts_with('/') { Vec::new() } else { components(cwd).map(String::from).collect() };
    let mut node = node_at(&stack)?;
    // Componentes a visitar, em ordem inversa (o próximo fica no fim).
    let mut todo: Vec<String> = components(path).rev().map(String::from).collect();
    let mut links = 0;
    while let Some(name) = todo.pop() {
        match name.as_str() {
            "." => {}
            ".." => {
                stack.pop();
                node = node_at(&stack)?;
            }
            _ => {
                if !node.is_dir() {
                    return Err(ENOTDIR);
                }
                if name.len() > MAX_NAME {
                    return Err(ENAMETOOLONG);
                }
                stack.push(name);
                let child = match mounted_at(&join(&stack)) {
                    Some(mounted) => mounted,
                    None => node.lookup(stack.last().unwrap())?,
                };
                if child.kind() == Kind::Symlink && (follow || !todo.is_empty()) {
                    links += 1;
                    if links > MAX_SYMLINKS {
                        return Err(ELOOP);
                    }
                    stack.pop();
                    let target = child.readlink()?;
                    if target.starts_with('/') {
                        stack.clear();
                        node = root();
                    }
                    todo.extend(components(&target).rev().map(String::from));
                    continue;
                }
                node = child;
            }
        }
    }
    Ok(Resolved { node, path: join(&stack) })
}

pub fn resolve(cwd: &str, path: &str) -> Result<NodeRef, Errno> {
    lookup(cwd, path, true).map(|r| r.node)
}

/// Resolve o diretório que contém `path` e devolve (diretório, último nome).
pub fn lookup_parent(cwd: &str, path: &str) -> Result<(Resolved, String), Errno> {
    let trimmed = path.trim_end_matches('/');
    if trimmed.is_empty() {
        return Err(if path.is_empty() { ENOENT } else { EPERM }); // a raiz não tem pai
    }
    let (dir, name) = match trimmed.rsplit_once('/') {
        Some(("", name)) => ("/", name),
        Some((dir, name)) => (dir, name),
        None => (".", trimmed),
    };
    if name == "." || name == ".." {
        return Err(EINVAL);
    }
    if name.len() > MAX_NAME {
        return Err(ENAMETOOLONG);
    }
    let parent = lookup(cwd, dir, true)?;
    if !parent.node.is_dir() {
        return Err(ENOTDIR);
    }
    Ok((parent, name.to_string()))
}

fn child_path(parent: &Resolved, name: &str) -> String {
    if parent.path == "/" { alloc::format!("/{name}") } else { alloc::format!("{}/{name}", parent.path) }
}

fn is_mount_point(path: &str) -> bool {
    MOUNTS.lock().iter().any(|m| m.path == path)
}

// --- Operações sobre caminhos ----------------------------------------------------

/// Abre ou cria (com `O_CREAT`) o arquivo regular `path`.
pub fn create_file(cwd: &str, path: &str, exclusive: bool, permissions: u32) -> Result<Resolved, Errno> {
    match lookup(cwd, path, true) {
        Ok(_) if exclusive => Err(EEXIST),
        Ok(existing) => Ok(existing),
        Err(ENOENT) => {
            let (parent, name) = lookup_parent(cwd, path)?;
            let node = parent.node.create(&name, NewNode::File, permissions)?;
            Ok(Resolved { node, path: child_path(&parent, &name) })
        }
        Err(errno) => Err(errno),
    }
}

pub fn mkdir(cwd: &str, path: &str, permissions: u32) -> Result<(), Errno> {
    let (parent, name) = lookup_parent(cwd, path)?;
    if parent.node.lookup(&name).is_ok() || is_mount_point(&child_path(&parent, &name)) {
        return Err(EEXIST);
    }
    parent.node.create(&name, NewNode::Directory, permissions).map(|_| ())
}

pub fn symlink(target: &str, cwd: &str, path: &str) -> Result<(), Errno> {
    let (parent, name) = lookup_parent(cwd, path)?;
    if parent.node.lookup(&name).is_ok() {
        return Err(EEXIST);
    }
    parent.node.create(&name, NewNode::Symlink(target), 0o777).map(|_| ())
}

pub fn readlink(cwd: &str, path: &str) -> Result<String, Errno> {
    lookup(cwd, path, false)?.node.readlink()
}

/// Remove um arquivo (`directory = false`) ou um diretório vazio.
pub fn remove(cwd: &str, path: &str, directory: bool) -> Result<(), Errno> {
    let (parent, name) = lookup_parent(cwd, path)?;
    if is_mount_point(&child_path(&parent, &name)) {
        return Err(EBUSY);
    }
    parent.node.unlink(&name, directory)
}

/// `link`: `new` passa a ser outro nome do arquivo `old`.
pub fn link(cwd: &str, old: &str, new: &str) -> Result<(), Errno> {
    let target = lookup(cwd, old, false)?.node;
    if target.is_dir() {
        return Err(EPERM);
    }
    let (parent, name) = lookup_parent(cwd, new)?;
    if parent.node.lookup(&name).is_ok() {
        return Err(EEXIST);
    }
    if parent.node.dev() != target.dev() {
        return Err(EXDEV);
    }
    parent.node.link(&name, &target)
}

/// `rename`: move `old` para `new`, substituindo o que houver lá (um
/// arquivo, ou um diretório vazio se `old` também for um diretório).
pub fn rename(cwd: &str, old: &str, new: &str) -> Result<(), Errno> {
    let (old_parent, old_name) = lookup_parent(cwd, old)?;
    let (new_parent, new_name) = lookup_parent(cwd, new)?;
    let source = old_parent.node.lookup(&old_name)?;
    let old_path = child_path(&old_parent, &old_name);
    let new_path = child_path(&new_parent, &new_name);
    if is_mount_point(&old_path) || is_mount_point(&new_path) {
        return Err(EBUSY);
    }
    if old_parent.node.dev() != new_parent.node.dev() {
        return Err(EXDEV);
    }
    if source.is_dir() && new_path.starts_with(&(old_path.clone() + "/")) {
        return Err(EINVAL); // um diretório não pode ir para dentro de si mesmo
    }
    if let Ok(existing) = new_parent.node.lookup(&new_name) {
        if existing.same_as(&source) {
            return Ok(());
        }
        match (source.is_dir(), existing.is_dir()) {
            (true, false) => return Err(ENOTDIR),
            (false, true) => return Err(EISDIR),
            (directory, _) => new_parent.node.unlink(&new_name, directory)?,
        }
    }
    old_parent.node.rename(&old_name, &new_parent.node, &new_name)
}

/// Lê um arquivo inteiro (usado pelo carregador de programas e pelo monitor).
pub fn read_file(cwd: &str, path: &str) -> Result<Vec<u8>, Errno> {
    let node = resolve(cwd, path)?;
    match node.kind() {
        Kind::Directory => return Err(EISDIR),
        Kind::File => {}
        _ => return Ok(Vec::new()),
    }
    let mut data = alloc::vec![0; node.size() as usize];
    let mut done = 0;
    while done < data.len() {
        match node.read_at(done as u64, &mut data[done..])? {
            0 => break,
            n => done += n,
        }
    }
    data.truncate(done);
    Ok(data)
}

/// Cria ou substitui (ou acrescenta a) um arquivo (usado pelo monitor).
pub fn write_file(cwd: &str, path: &str, data: &[u8], append: bool) -> Result<(), Errno> {
    let node = create_file(cwd, path, false, 0o644)?.node;
    if !append {
        node.truncate(0)?;
    }
    node.write_at(if append { node.size() } else { 0 }, data).map(|_| ())
}
