//! Sistema de arquivos virtual: uma árvore em memória de diretórios,
//! arquivos regulares e dispositivos, com caminhos ao estilo Unix.
//!
//! ```text
//! /bin   programas      /dev   null, zero, tty, console
//! /home  arquivos       /tmp   área temporária
//! ```

use alloc::collections::BTreeMap;
use alloc::string::{String, ToString};
use alloc::sync::Arc;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicU64, Ordering};

use crate::sync::SpinLock;
use crate::syscall::{EEXIST, EISDIR, ENAMETOOLONG, ENOENT, ENOTDIR, ENOTEMPTY, EPERM, Errno};

// Programas de usuário compilados pelo build.rs (pasta user/).
include!(concat!(env!("OUT_DIR"), "/programs.rs"));

const MAX_NAME: usize = 255;

pub type NodeRef = Arc<Node>;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Device {
    Null,
    Zero,
    Tty,
}

pub enum NodeKind {
    File(SpinLock<Vec<u8>>),
    Directory(SpinLock<BTreeMap<String, NodeRef>>),
    Device(Device),
}

pub struct Node {
    pub ino: u64,
    /// Permissões (os 9 bits rwx); não são verificadas, só informadas.
    pub mode: u32,
    pub kind: NodeKind,
}

static NEXT_INO: AtomicU64 = AtomicU64::new(1);
static ROOT: SpinLock<Option<NodeRef>> = SpinLock::new(None);

impl Node {
    fn new(kind: NodeKind, mode: u32) -> NodeRef {
        Arc::new(Node { ino: NEXT_INO.fetch_add(1, Ordering::Relaxed), mode, kind })
    }

    pub fn new_file(data: Vec<u8>, mode: u32) -> NodeRef {
        Self::new(NodeKind::File(SpinLock::new(data)), mode)
    }

    pub fn new_dir() -> NodeRef {
        Self::new(NodeKind::Directory(SpinLock::new(BTreeMap::new())), 0o755)
    }

    pub fn is_dir(&self) -> bool {
        matches!(self.kind, NodeKind::Directory(_))
    }

    pub fn size(&self) -> u64 {
        match &self.kind {
            NodeKind::File(data) => data.lock().len() as u64,
            NodeKind::Directory(entries) => entries.lock().len() as u64,
            NodeKind::Device(_) => 0,
        }
    }

    fn entries(&self) -> Result<&SpinLock<BTreeMap<String, NodeRef>>, Errno> {
        match &self.kind {
            NodeKind::Directory(entries) => Ok(entries),
            _ => Err(ENOTDIR),
        }
    }

    pub fn lookup(&self, name: &str) -> Result<NodeRef, Errno> {
        self.entries()?.lock().get(name).cloned().ok_or(ENOENT)
    }

    /// Lista o diretório (sem "." e "..").
    pub fn list(&self) -> Result<Vec<(String, NodeRef)>, Errno> {
        Ok(self.entries()?.lock().iter().map(|(n, node)| (n.clone(), node.clone())).collect())
    }
}

pub fn root() -> NodeRef {
    ROOT.lock().clone().expect("VFS não inicializado")
}

/// Junta `path` ao diretório `cwd` e resolve "." e "..", sem consultar a
/// árvore. O resultado é absoluto e sem barras repetidas.
pub fn normalize(cwd: &str, path: &str) -> String {
    let mut parts: Vec<&str> = Vec::new();
    let full = if path.starts_with('/') { [path, ""] } else { [cwd, path] };
    for component in full.iter().flat_map(|p| p.split('/')) {
        match component {
            "" | "." => {}
            ".." => {
                parts.pop();
            }
            name => parts.push(name),
        }
    }
    let mut result = String::new();
    for part in &parts {
        result.push('/');
        result.push_str(part);
    }
    if result.is_empty() {
        result.push('/');
    }
    result
}

pub fn resolve(cwd: &str, path: &str) -> Result<NodeRef, Errno> {
    if path.is_empty() {
        return Err(ENOENT);
    }
    let mut node = root();
    for name in normalize(cwd, path).split('/').filter(|c| !c.is_empty()) {
        node = node.lookup(name)?;
    }
    Ok(node)
}

/// Resolve o diretório pai de `path` e devolve (pai, último nome).
pub fn resolve_parent(cwd: &str, path: &str) -> Result<(NodeRef, String), Errno> {
    let full = normalize(cwd, path);
    let (parent, name) = full.rsplit_once('/').unwrap();
    if name.is_empty() {
        return Err(EPERM); // a raiz não tem pai
    }
    if name.len() > MAX_NAME {
        return Err(ENAMETOOLONG);
    }
    let parent = resolve("/", if parent.is_empty() { "/" } else { parent })?;
    parent.entries()?;
    Ok((parent, name.to_string()))
}

/// Cria (ou devolve, se já existir) o arquivo regular `path`.
pub fn create_file(cwd: &str, path: &str, exclusive: bool) -> Result<NodeRef, Errno> {
    let (parent, name) = resolve_parent(cwd, path)?;
    let mut entries = parent.entries()?.lock();
    if let Some(existing) = entries.get(&name) {
        return if exclusive { Err(EEXIST) } else { Ok(existing.clone()) };
    }
    let node = Node::new_file(Vec::new(), 0o644);
    entries.insert(name, node.clone());
    Ok(node)
}

pub fn mkdir(cwd: &str, path: &str) -> Result<(), Errno> {
    let (parent, name) = resolve_parent(cwd, path)?;
    let mut entries = parent.entries()?.lock();
    if entries.contains_key(&name) {
        return Err(EEXIST);
    }
    entries.insert(name, Node::new_dir());
    Ok(())
}

/// Remove um arquivo (`directory = false`) ou um diretório vazio.
pub fn remove(cwd: &str, path: &str, directory: bool) -> Result<(), Errno> {
    let (parent, name) = resolve_parent(cwd, path)?;
    let mut entries = parent.entries()?.lock();
    let node = entries.get(&name).ok_or(ENOENT)?;
    match (directory, node.is_dir()) {
        (false, true) => return Err(EISDIR),
        (true, false) => return Err(ENOTDIR),
        (true, true) if node.size() > 0 => return Err(ENOTEMPTY),
        _ => {}
    }
    entries.remove(&name);
    Ok(())
}

fn add(dir: &NodeRef, name: &str, node: NodeRef) {
    dir.entries().unwrap().lock().insert(name.to_string(), node);
}

pub fn init() {
    let root = Node::new_dir();
    *ROOT.lock() = Some(root.clone());

    let bin = Node::new_dir();
    for (name, binary) in PROGRAMS {
        add(&bin, name, Node::new_file(binary.to_vec(), 0o755));
    }
    add(&root, "bin", bin);

    let dev = Node::new_dir();
    add(&dev, "null", Node::new(NodeKind::Device(Device::Null), 0o666));
    add(&dev, "zero", Node::new(NodeKind::Device(Device::Zero), 0o666));
    let tty = Node::new(NodeKind::Device(Device::Tty), 0o666);
    add(&dev, "tty", tty.clone());
    add(&dev, "console", tty);
    add(&root, "dev", dev);

    let home = Node::new_dir();
    let readme = "Bem-vindo ao FerrugemOS!\n\
         Este sistema roda inteiramente em memória: os arquivos somem ao desligar.\n\
         No sh, experimente: ls /bin, cat leiame.txt | grep sistema, yes | head -n 3,\n\
         sleep 100 & e depois jobs, fg (Ctrl+Z suspende, Ctrl+C interrompe).\n";
    add(&home, "leiame.txt", Node::new_file(readme.as_bytes().to_vec(), 0o644));
    add(&home, "rust.txt", Node::new_file(b"Feito em Rust, sem std, sem crates externos.\n".to_vec(), 0o644));
    add(&root, "home", home);
    add(&root, "tmp", Node::new_dir());
}

// Atalhos usados pelo monitor do kernel (caminhos relativos a `cwd`).

pub fn read_file(cwd: &str, path: &str) -> Result<Vec<u8>, Errno> {
    match &resolve(cwd, path)?.kind {
        NodeKind::File(data) => Ok(data.lock().clone()),
        NodeKind::Directory(_) => Err(EISDIR),
        NodeKind::Device(_) => Ok(Vec::new()),
    }
}

pub fn write_file(cwd: &str, path: &str, data: &[u8], append: bool) -> Result<(), Errno> {
    let node = create_file(cwd, path, false)?;
    match &node.kind {
        NodeKind::File(contents) => {
            let mut contents = contents.lock();
            if !append {
                contents.clear();
            }
            contents.extend_from_slice(data);
            Ok(())
        }
        _ => Err(EISDIR),
    }
}
