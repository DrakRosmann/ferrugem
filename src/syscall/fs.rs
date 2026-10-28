//! Syscalls de arquivos: E/S, descritores, pipes, diretórios e metadados.

use alloc::string::String;
use alloc::sync::Arc;
use alloc::vec::Vec;

use super::{
    EINVAL, EISDIR, ENOTDIR, ENOTTY, ERANGE, Errno, SysResult, read_cstr, read_u64, user_slice, with_process,
    write_user,
};
use crate::file::{
    FileKind, O_ACCMODE, O_CLOEXEC, O_CREAT, O_DIRECTORY, O_EXCL, O_NONBLOCK, O_RDONLY, O_TRUNC, O_WRONLY, OpenFile,
    Stat, node_stat,
};
use crate::{pipe, tty, vfs};

pub const AT_FDCWD: i64 = -100;
pub const AT_REMOVEDIR: u64 = 0x200;
const AT_EMPTY_PATH: u64 = 0x1000;

fn get_file(fd: u64) -> Result<Arc<OpenFile>, Errno> {
    with_process(|p| p.fds.get(fd as i32))
}

/// Diretório a partir do qual resolver `path` nas syscalls `*at`.
fn base_dir(dirfd: i64, path: &str) -> Result<String, Errno> {
    if path.starts_with('/') || dirfd as i32 == AT_FDCWD as i32 {
        return with_process(|p| Ok(p.cwd.clone()));
    }
    let file = get_file(dirfd as u64)?;
    file.path().map(String::from).ok_or(ENOTDIR)
}

pub fn read(fd: u64, buf: u64, len: u64) -> SysResult {
    let file = get_file(fd)?;
    user_slice(buf, len, true)?;
    let data = file.read(len as usize)?;
    write_user(buf, &data)?;
    Ok(data.len() as i64)
}

pub fn write(fd: u64, buf: u64, len: u64) -> SysResult {
    let file = get_file(fd)?;
    let data = user_slice(buf, len, false)?.to_vec();
    Ok(file.write(&data)? as i64)
}

/// Lê o vetor de `struct iovec` (base, tamanho) de readv/writev.
fn iovecs(iov: u64, count: u64) -> Result<Vec<(u64, u64)>, Errno> {
    if count > 1024 {
        return Err(EINVAL);
    }
    (0..count).map(|i| Ok((read_u64(iov + i * 16)?, read_u64(iov + i * 16 + 8)?))).collect()
}

pub fn readv(fd: u64, iov: u64, count: u64) -> SysResult {
    let mut total = 0;
    for (base, len) in iovecs(iov, count)? {
        let n = read(fd, base, len)?;
        total += n;
        if (n as u64) < len {
            break; // leitura curta: não espera encher os próximos
        }
    }
    Ok(total)
}

pub fn writev(fd: u64, iov: u64, count: u64) -> SysResult {
    let mut total = 0;
    for (base, len) in iovecs(iov, count)? {
        total += write(fd, base, len)?;
    }
    Ok(total)
}

pub fn openat(dirfd: i64, path: u64, flags: u64, _mode: u64) -> SysResult {
    let path = read_cstr(path)?;
    let flags = flags as u32;
    let base = base_dir(dirfd, &path)?;
    let node = if flags & O_CREAT != 0 {
        vfs::create_file(&base, &path, flags & O_EXCL != 0)?
    } else {
        vfs::resolve(&base, &path)?
    };
    let writes = flags & O_ACCMODE != O_RDONLY;
    if flags & O_DIRECTORY != 0 && !node.is_dir() {
        return Err(ENOTDIR);
    }
    if node.is_dir() && writes {
        return Err(EISDIR);
    }
    let file = OpenFile::open_node(node, vfs::normalize(&base, &path), flags);
    if flags & O_TRUNC != 0 && writes {
        file.truncate();
    }
    with_process(|p| p.fds.insert(file, flags & O_CLOEXEC != 0, 0)).map(i64::from)
}

pub fn close(fd: u64) -> SysResult {
    // O descritor é descartado aqui, fora da trava do escalonador.
    let descriptor = with_process(|p| p.fds.close(fd as i32))?;
    drop(descriptor);
    Ok(0)
}

pub fn lseek(fd: u64, offset: u64, whence: u64) -> SysResult {
    Ok(get_file(fd)?.seek(offset as i64, whence as u32)? as i64)
}

/// Grava o `struct stat` do Linux x86_64 (144 bytes).
fn write_stat(buf: u64, stat: &Stat) -> SysResult {
    let mut out = [0u8; 144];
    out[0..8].copy_from_slice(&1u64.to_le_bytes()); // st_dev
    out[8..16].copy_from_slice(&stat.ino.to_le_bytes());
    out[16..24].copy_from_slice(&stat.nlink.to_le_bytes());
    out[24..28].copy_from_slice(&stat.mode.to_le_bytes());
    out[40..48].copy_from_slice(&stat.rdev.to_le_bytes());
    out[48..56].copy_from_slice(&stat.size.to_le_bytes());
    out[56..64].copy_from_slice(&4096u64.to_le_bytes()); // st_blksize
    out[64..72].copy_from_slice(&stat.size.div_ceil(512).to_le_bytes()); // st_blocks
    write_user(buf, &out)?;
    Ok(0)
}

pub fn fstat(fd: u64, buf: u64) -> SysResult {
    write_stat(buf, &get_file(fd)?.stat())
}

pub fn fstatat(dirfd: i64, path: u64, buf: u64, flags: u64) -> SysResult {
    let path = read_cstr(path)?;
    if path.is_empty() && flags & AT_EMPTY_PATH != 0 {
        return fstat(dirfd as u64, buf);
    }
    let base = base_dir(dirfd, &path)?;
    write_stat(buf, &node_stat(&vfs::resolve(&base, &path)?))
}

/// Grava entradas `struct linux_dirent64` até encher o buffer.
pub fn getdents64(fd: u64, buf: u64, size: u64) -> SysResult {
    let file = get_file(fd)?;
    let entries = file.dir_entries()?;
    let mut out = Vec::new();
    let mut used = 0;
    for (index, (ino, kind, name)) in entries.iter().enumerate() {
        let record = (19 + name.len() + 1).next_multiple_of(8);
        if out.len() + record > size as usize {
            break;
        }
        let start = out.len();
        out.extend_from_slice(&ino.to_le_bytes());
        out.extend_from_slice(&(index as i64 + 1).to_le_bytes()); // d_off
        out.extend_from_slice(&(record as u16).to_le_bytes());
        out.push(*kind);
        out.extend_from_slice(name.as_bytes());
        out.resize(start + record, 0);
        used += 1;
    }
    if used == 0 && !entries.is_empty() {
        return Err(EINVAL); // buffer pequeno demais para uma entrada
    }
    file.advance(used);
    write_user(buf, &out)?;
    Ok(out.len() as i64)
}

pub fn dup(fd: u64) -> SysResult {
    with_process(|p| {
        let file = p.fds.get(fd as i32)?;
        p.fds.insert(file, false, 0)
    })
    .map(i64::from)
}

/// dup2 e dup3: `newfd` passa a apontar para o arquivo de `oldfd`.
pub fn dup3(oldfd: u64, newfd: u64, flags: u64, is_dup2: bool) -> SysResult {
    if flags & !(O_CLOEXEC as u64) != 0 {
        return Err(EINVAL);
    }
    if oldfd as i32 == newfd as i32 {
        return if is_dup2 { get_file(oldfd).map(|_| newfd as i64) } else { Err(EINVAL) };
    }
    let replaced = with_process(|p| {
        let file = p.fds.get(oldfd as i32)?;
        p.fds.install(newfd as i32, file, flags != 0)
    })?;
    drop(replaced);
    Ok(newfd as i32 as i64)
}

pub fn fcntl(fd: u64, command: u64, arg: u64) -> SysResult {
    const F_DUPFD: u64 = 0;
    const F_GETFD: u64 = 1;
    const F_SETFD: u64 = 2;
    const F_GETFL: u64 = 3;
    const F_SETFL: u64 = 4;
    const F_DUPFD_CLOEXEC: u64 = 1030;
    const FD_CLOEXEC: u64 = 1;
    let fd = fd as i32;
    with_process(|p| match command {
        F_DUPFD | F_DUPFD_CLOEXEC => {
            let file = p.fds.get(fd)?;
            p.fds.insert(file, command == F_DUPFD_CLOEXEC, arg as usize).map(i64::from)
        }
        F_GETFD => Ok(p.fds.descriptor(fd)?.close_on_exec as i64),
        F_SETFD => p.fds.set_close_on_exec(fd, arg & FD_CLOEXEC != 0).map(|()| 0),
        F_GETFL => Ok(p.fds.get(fd)?.flags() as i64),
        F_SETFL => {
            p.fds.get(fd)?.set_status_flags(arg as u32);
            Ok(0)
        }
        _ => Err(EINVAL),
    })
}

pub fn pipe2(fds: u64, flags: u64) -> SysResult {
    let flags = flags as u32;
    if flags & !(O_CLOEXEC | O_NONBLOCK) != 0 {
        return Err(EINVAL);
    }
    user_slice(fds, 8, true)?;
    let (reader, writer) = pipe::new();
    let nonblock = flags & O_NONBLOCK;
    let reader = OpenFile::new(FileKind::Pipe(reader), O_RDONLY | nonblock);
    let writer = OpenFile::new(FileKind::Pipe(writer), O_WRONLY | nonblock);
    let cloexec = flags & O_CLOEXEC != 0;
    let (read_fd, write_fd) = with_process(|p| {
        let read_fd = p.fds.insert(reader, cloexec, 0)?;
        match p.fds.insert(writer, cloexec, 0) {
            Ok(write_fd) => Ok((read_fd, write_fd)),
            Err(errno) => {
                let _ = p.fds.close(read_fd);
                Err(errno)
            }
        }
    })?;
    let mut out = [0u8; 8];
    out[..4].copy_from_slice(&read_fd.to_le_bytes());
    out[4..].copy_from_slice(&write_fd.to_le_bytes());
    write_user(fds, &out)?;
    Ok(0)
}

pub fn ioctl(fd: u64, request: u64, arg: u64) -> SysResult {
    let file = get_file(fd)?;
    if !file.is_tty() {
        return Err(ENOTTY);
    }
    tty::ioctl(request, arg)
}

pub fn getcwd(buf: u64, size: u64) -> SysResult {
    let mut cwd = with_process(|p| Ok(p.cwd.clone()))?.into_bytes();
    cwd.push(0);
    if (size as usize) < cwd.len() {
        return Err(ERANGE);
    }
    write_user(buf, &cwd)?;
    Ok(cwd.len() as i64)
}

pub fn chdir(path: u64) -> SysResult {
    let path = read_cstr(path)?;
    let cwd = with_process(|p| Ok(p.cwd.clone()))?;
    if !vfs::resolve(&cwd, &path)?.is_dir() {
        return Err(ENOTDIR);
    }
    let new = vfs::normalize(&cwd, &path);
    with_process(|p| {
        p.cwd = new;
        Ok(0)
    })
}

pub fn mkdirat(dirfd: i64, path: u64) -> SysResult {
    let path = read_cstr(path)?;
    vfs::mkdir(&base_dir(dirfd, &path)?, &path).map(|()| 0)
}

pub fn unlinkat(dirfd: i64, path: u64, flags: u64) -> SysResult {
    let path = read_cstr(path)?;
    vfs::remove(&base_dir(dirfd, &path)?, &path, flags & AT_REMOVEDIR != 0).map(|()| 0)
}

/// Sem permissões de verdade: basta o arquivo existir.
pub fn faccessat(dirfd: i64, path: u64) -> SysResult {
    let path = read_cstr(path)?;
    vfs::resolve(&base_dir(dirfd, &path)?, &path).map(|_| 0)
}
