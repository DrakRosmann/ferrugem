//! Syscalls de arquivos: E/S, descritores, pipes, diretórios e metadados.

use alloc::string::String;
use alloc::sync::Arc;
use alloc::vec;
use alloc::vec::Vec;

use super::{
    EINVAL, EISDIR, ELOOP, ENOENT, ENOTDIR, ENOTTY, ERANGE, Errno, SysResult, read_cstr, read_u64, user_slice,
    with_process, write_user,
};
use crate::file::{
    FileKind, O_ACCMODE, O_CLOEXEC, O_CREAT, O_DIRECTORY, O_EXCL, O_NONBLOCK, O_RDONLY, O_TRUNC, O_WRONLY, OpenFile,
    Stat, node_stat,
};
use crate::vfs::Kind;
use crate::{pipe, rtc, tty, vfs};

pub const AT_FDCWD: i64 = -100;
pub const AT_REMOVEDIR: u64 = 0x200;
pub const AT_SYMLINK_NOFOLLOW: u64 = 0x100;
pub const AT_EMPTY_PATH: u64 = 0x1000;
const O_NOFOLLOW: u32 = 0x2_0000;
/// Permissões de um arquivo novo: as pedidas menos as do `umask`.
fn creation_mode(mode: u64) -> Result<u32, Errno> {
    let umask = with_process(|p| Ok(p.umask))?;
    Ok(mode as u32 & 0o7777 & !umask)
}

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

pub fn openat(dirfd: i64, path: u64, flags: u64, mode: u64) -> SysResult {
    let path = read_cstr(path)?;
    let flags = flags as u32;
    let base = base_dir(dirfd, &path)?;
    let resolved = if flags & O_CREAT != 0 {
        vfs::create_file(&base, &path, flags & O_EXCL != 0, creation_mode(mode)?)?
    } else {
        vfs::lookup(&base, &path, flags & O_NOFOLLOW == 0)?
    };
    let node = resolved.node;
    let writes = flags & O_ACCMODE != O_RDONLY;
    if node.kind() == Kind::Symlink {
        return Err(ELOOP); // só acontece com O_NOFOLLOW
    }
    if flags & O_DIRECTORY != 0 && !node.is_dir() {
        return Err(ENOTDIR);
    }
    if node.is_dir() && writes {
        return Err(EISDIR);
    }
    let file = OpenFile::open_node(node, resolved.path, flags)?;
    if flags & O_TRUNC != 0 && writes && file.node().is_some() {
        file.truncate(0)?;
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
    out[0..8].copy_from_slice(&stat.dev.to_le_bytes());
    out[8..16].copy_from_slice(&stat.ino.to_le_bytes());
    out[16..24].copy_from_slice(&stat.nlink.to_le_bytes());
    out[24..28].copy_from_slice(&stat.mode.to_le_bytes());
    out[28..32].copy_from_slice(&stat.uid.to_le_bytes());
    out[32..36].copy_from_slice(&stat.gid.to_le_bytes());
    out[40..48].copy_from_slice(&stat.rdev.to_le_bytes());
    out[48..56].copy_from_slice(&stat.size.to_le_bytes());
    out[56..64].copy_from_slice(&4096u64.to_le_bytes()); // st_blksize
    out[64..72].copy_from_slice(&stat.size.div_ceil(512).to_le_bytes()); // st_blocks
    for offset in [72, 88, 104] {
        out[offset..offset + 8].copy_from_slice(&stat.mtime.to_le_bytes()); // atime, mtime, ctime
    }
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
    let follow = flags & AT_SYMLINK_NOFOLLOW == 0;
    write_stat(buf, &node_stat(&vfs::lookup(&base, &path, follow)?.node))
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
    let resolved = vfs::lookup(&cwd, &path, true)?;
    if !resolved.node.is_dir() {
        return Err(ENOTDIR);
    }
    with_process(|p| {
        p.cwd = resolved.path;
        Ok(0)
    })
}

pub fn mkdirat(dirfd: i64, path: u64, mode: u64) -> SysResult {
    let path = read_cstr(path)?;
    vfs::mkdir(&base_dir(dirfd, &path)?, &path, creation_mode(mode)?).map(|()| 0)
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

pub fn renameat(old_dirfd: i64, old: u64, new_dirfd: i64, new: u64, flags: u64) -> SysResult {
    if flags != 0 {
        return Err(EINVAL); // RENAME_NOREPLACE e afins ainda não
    }
    let (old, new) = (read_cstr(old)?, read_cstr(new)?);
    let (old_base, new_base) = (base_dir(old_dirfd, &old)?, base_dir(new_dirfd, &new)?);
    // O VFS resolve os dois a partir de um diretório só: usa caminhos absolutos.
    let old = vfs::lookup_parent(&old_base, &old).map(|(dir, name)| join(&dir.path, &name))?;
    let new = vfs::lookup_parent(&new_base, &new).map(|(dir, name)| join(&dir.path, &name))?;
    vfs::rename("/", &old, &new).map(|()| 0)
}

pub fn linkat(old_dirfd: i64, old: u64, new_dirfd: i64, new: u64) -> SysResult {
    let (old, new) = (read_cstr(old)?, read_cstr(new)?);
    let old = vfs::lookup(&base_dir(old_dirfd, &old)?, &old, false)?.path;
    let new = vfs::lookup_parent(&base_dir(new_dirfd, &new)?, &new).map(|(dir, name)| join(&dir.path, &name))?;
    vfs::link("/", &old, &new).map(|()| 0)
}

fn join(dir: &str, name: &str) -> String {
    if dir == "/" { alloc::format!("/{name}") } else { alloc::format!("{dir}/{name}") }
}

pub fn symlinkat(target: u64, dirfd: i64, path: u64) -> SysResult {
    let (target, path) = (read_cstr(target)?, read_cstr(path)?);
    if target.is_empty() {
        return Err(ENOENT);
    }
    vfs::symlink(&target, &base_dir(dirfd, &path)?, &path).map(|()| 0)
}

pub fn readlinkat(dirfd: i64, path: u64, buf: u64, size: u64) -> SysResult {
    let path = read_cstr(path)?;
    let target = vfs::readlink(&base_dir(dirfd, &path)?, &path)?;
    // Como no Linux: sem o zero no fim, e cortado se não couber.
    let n = target.len().min(size as usize);
    write_user(buf, &target.as_bytes()[..n])?;
    Ok(n as i64)
}

pub fn ftruncate(fd: u64, size: u64) -> SysResult {
    get_file(fd)?.truncate(size).map(|()| 0)
}

pub fn truncate(path: u64, size: u64) -> SysResult {
    let path = read_cstr(path)?;
    let cwd = with_process(|p| Ok(p.cwd.clone()))?;
    vfs::resolve(&cwd, &path)?.truncate(size).map(|()| 0)
}

/// `utimensat`: só a hora de modificação existe; "agora" ou um valor dado.
pub fn utimensat(dirfd: i64, path: u64, times: u64, flags: u64) -> SysResult {
    const UTIME_NOW: u64 = 0x3FFF_FFFF;
    const UTIME_OMIT: u64 = 0x3FFF_FFFE;
    let node = if path == 0 {
        get_file(dirfd as u64)?.node().cloned().ok_or(EINVAL)?
    } else {
        let path = read_cstr(path)?;
        vfs::lookup(&base_dir(dirfd, &path)?, &path, flags & AT_SYMLINK_NOFOLLOW == 0)?.node
    };
    // times[1] é a modificação (times[0], o acesso, não é guardado).
    let mtime = if times == 0 {
        rtc::now()
    } else {
        match read_u64(times + 24)? {
            UTIME_OMIT => return Ok(0),
            UTIME_NOW => rtc::now(),
            _ => read_u64(times + 16)?,
        }
    };
    node.set_mtime(mtime).map(|()| 0)
}

pub fn pread(fd: u64, buf: u64, len: u64, offset: u64) -> SysResult {
    let file = get_file(fd)?;
    user_slice(buf, len, true)?;
    let data = file.read_at(offset, len as usize)?;
    write_user(buf, &data)?;
    Ok(data.len() as i64)
}

pub fn pwrite(fd: u64, buf: u64, len: u64, offset: u64) -> SysResult {
    let file = get_file(fd)?;
    let data = user_slice(buf, len, false)?.to_vec();
    Ok(file.write_at(offset, &data)? as i64)
}

/// `sendfile`: copia de um descritor para outro dentro do kernel.
pub fn sendfile(out_fd: u64, in_fd: u64, offset_ptr: u64, count: u64) -> SysResult {
    let (input, output) = (get_file(in_fd)?, get_file(out_fd)?);
    let mut offset = if offset_ptr != 0 { Some(read_u64(offset_ptr)?) } else { None };
    let mut total = 0u64;
    while total < count {
        let chunk = (count - total).min(64 * 1024) as usize;
        let data = match offset {
            Some(position) => input.read_at(position, chunk)?,
            None => input.read(chunk)?,
        };
        if data.is_empty() {
            break;
        }
        let written = output.write(&data)?;
        total += written as u64;
        if let Some(position) = offset.as_mut() {
            *position += written as u64;
        }
        if written < data.len() {
            break;
        }
    }
    if let Some(position) = offset {
        write_user(offset_ptr, &position.to_le_bytes())?;
    }
    Ok(total as i64)
}

/// Espera, de 10 em 10 ms, até `ready` achar algo, o prazo vencer ou chegar
/// um sinal. `timeout_ms` negativo espera para sempre.
fn wait_until(timeout_ms: i64, mut ready: impl FnMut() -> Result<i64, Errno>) -> SysResult {
    let deadline = (timeout_ms >= 0).then(|| crate::timer::uptime_ms() + timeout_ms as u64);
    loop {
        let count = ready()?;
        if count > 0 {
            return Ok(count);
        }
        let now = crate::timer::uptime_ms();
        if deadline.is_some_and(|d| now >= d) {
            return Ok(0);
        }
        if crate::task::signal_pending() {
            return Err(super::EINTR);
        }
        crate::task::sleep_ms(deadline.map_or(10, |d| (d - now).min(10)));
    }
}

/// Lê um `struct timespec` como milissegundos (-1 se o ponteiro é nulo).
fn timespec_ms(addr: u64) -> Result<i64, Errno> {
    if addr == 0 {
        return Ok(-1);
    }
    let (seconds, nanos) = (read_u64(addr)? as i64, read_u64(addr + 8)? as i64);
    if seconds < 0 || !(0..1_000_000_000).contains(&nanos) {
        return Err(EINVAL);
    }
    Ok(seconds.saturating_mul(1000).saturating_add((nanos as u64).div_ceil(1_000_000) as i64))
}

/// `poll`: `struct pollfd { int fd; short events; short revents; }`.
pub fn poll(fds: u64, count: u64, timeout_ms: i64) -> SysResult {
    const POLLNVAL: u16 = 0x20;
    const ALWAYS: u16 = 0x8 | 0x10; // POLLERR e POLLHUP valem mesmo sem pedir
    if count > 1024 {
        return Err(EINVAL);
    }
    user_slice(fds, count * 8, true)?;
    let entries: Vec<(i32, u16)> = (0..count)
        .map(|i| {
            let raw = read_u64(fds + i * 8)?;
            Ok((raw as u32 as i32, (raw >> 32) as u16))
        })
        .collect::<Result<_, Errno>>()?;
    wait_until(timeout_ms, || {
        let mut ready = 0;
        for (i, &(fd, events)) in entries.iter().enumerate() {
            let revents = if fd < 0 {
                0
            } else {
                match get_file(fd as u64) {
                    Ok(file) => file.poll() & (events | ALWAYS),
                    Err(_) => POLLNVAL,
                }
            };
            write_user(fds + i as u64 * 8 + 6, &revents.to_le_bytes())?;
            if revents != 0 {
                ready += 1;
            }
        }
        Ok(ready)
    })
}

pub fn ppoll(fds: u64, count: u64, timeout: u64) -> SysResult {
    poll(fds, count, timespec_ms(timeout)?)
}

/// `select`/`pselect6`: três conjuntos de bits (leitura, escrita, exceção).
pub fn select(nfds: u64, sets: [u64; 3], timeout_ms: i64) -> SysResult {
    const POLLIN: u16 = 0x1;
    const POLLOUT: u16 = 0x4;
    const POLLERR: u16 = 0x8;
    const POLLHUP: u16 = 0x10;
    if nfds > 1024 {
        return Err(EINVAL);
    }
    let bytes = nfds.div_ceil(64) * 8;
    let wanted: Vec<Vec<u8>> = sets
        .iter()
        .map(|&set| if set == 0 { Ok(vec![0; bytes as usize]) } else { user_slice(set, bytes, true).map(|s| s.to_vec()) })
        .collect::<Result<_, Errno>>()?;
    let is_set = |bits: &[u8], fd: usize| bits[fd / 8] & (1 << (fd % 8)) != 0;
    for fd in 0..nfds as usize {
        if (0..3).any(|k| is_set(&wanted[k], fd)) {
            get_file(fd as u64)?; // descritor inválido: EBADF
        }
    }
    wait_until(timeout_ms, || {
        let mut result = vec![vec![0u8; bytes as usize]; 3];
        let mut ready = 0;
        for fd in 0..nfds as usize {
            if !(0..3).any(|k| is_set(&wanted[k], fd)) {
                continue;
            }
            let events = get_file(fd as u64)?.poll();
            let hits = [events & (POLLIN | POLLHUP) != 0, events & (POLLOUT | POLLERR) != 0, false];
            for k in 0..3 {
                if hits[k] && is_set(&wanted[k], fd) {
                    result[k][fd / 8] |= 1 << (fd % 8);
                    ready += 1;
                }
            }
        }
        // Os conjuntos voltam só com os descritores prontos (todos zerados
        // se o prazo vencer).
        for k in 0..3 {
            if sets[k] != 0 {
                write_user(sets[k], &result[k])?;
            }
        }
        Ok(ready)
    })
}

/// `struct timeval` (segundos, microssegundos) em milissegundos.
pub fn timeval_ms(addr: u64) -> Result<i64, Errno> {
    if addr == 0 {
        return Ok(-1);
    }
    let (seconds, micros) = (read_u64(addr)? as i64, read_u64(addr + 8)? as i64);
    if seconds < 0 || !(0..1_000_000).contains(&micros) {
        return Err(EINVAL);
    }
    Ok(seconds * 1000 + (micros as u64).div_ceil(1000) as i64)
}

pub fn pselect(nfds: u64, sets: [u64; 3], timeout: u64) -> SysResult {
    select(nfds, sets, timespec_ms(timeout)?)
}

/// O nó de `path` (ou do descritor `dirfd`, com AT_EMPTY_PATH ou caminho nulo).
fn node_at(dirfd: i64, path: u64, flags: u64) -> Result<crate::vfs::NodeRef, Errno> {
    let path = if path == 0 { String::new() } else { read_cstr(path)? };
    if path.is_empty() {
        if path_required(flags) {
            return Err(ENOENT);
        }
        return get_file(dirfd as u64)?.node().cloned().ok_or(EINVAL);
    }
    Ok(vfs::lookup(&base_dir(dirfd, &path)?, &path, flags & AT_SYMLINK_NOFOLLOW == 0)?.node)
}

fn path_required(flags: u64) -> bool {
    flags & AT_EMPTY_PATH == 0
}

pub fn fchmodat(dirfd: i64, path: u64, mode: u64) -> SysResult {
    node_at(dirfd, path, 0)?.set_permissions(mode as u32).map(|()| 0)
}

pub fn fchmod(fd: u64, mode: u64) -> SysResult {
    get_file(fd)?.node().ok_or(EINVAL)?.set_permissions(mode as u32).map(|()| 0)
}

/// `chown` e afins: -1 mantém o dono ou o grupo.
pub fn fchownat(dirfd: i64, path: u64, uid: u64, gid: u64, flags: u64) -> SysResult {
    let keep = |id: u64| (id as u32 != u32::MAX).then_some(id as u32);
    node_at(dirfd, path, flags)?.set_owner(keep(uid), keep(gid)).map(|()| 0)
}

pub fn fchown(fd: u64, uid: u64, gid: u64) -> SysResult {
    let keep = |id: u64| (id as u32 != u32::MAX).then_some(id as u32);
    get_file(fd)?.node().ok_or(EINVAL)?.set_owner(keep(uid), keep(gid)).map(|()| 0)
}

pub fn umask(mask: u64) -> SysResult {
    with_process(|p| Ok(core::mem::replace(&mut p.umask, mask as u32 & 0o777) as i64))
}
