//! Códigos de erro (errno). Os mesmos números do Linux: uma chamada de sistema
//! que falha devolve o código negativo em RAX (ex.: -2 = "arquivo não existe").

pub const EPERM: i64 = 1;
pub const ENOENT: i64 = 2;
pub const ESRCH: i64 = 3;
pub const EINTR: i64 = 4;
pub const EIO: i64 = 5;
pub const ENXIO: i64 = 6;
pub const ENOEXEC: i64 = 8;
pub const EBADF: i64 = 9;
pub const ECHILD: i64 = 10;
pub const EAGAIN: i64 = 11;
pub const ENOMEM: i64 = 12;
pub const EACCES: i64 = 13;
pub const EFAULT: i64 = 14;
pub const EBUSY: i64 = 16;
pub const EEXIST: i64 = 17;
pub const EXDEV: i64 = 18;
pub const ENODEV: i64 = 19;
pub const ENOTDIR: i64 = 20;
pub const EISDIR: i64 = 21;
pub const EINVAL: i64 = 22;
pub const EMFILE: i64 = 24;
pub const ENOTTY: i64 = 25;
pub const EFBIG: i64 = 27;
pub const ENOSPC: i64 = 28;
pub const ESPIPE: i64 = 29;
pub const EPIPE: i64 = 32;
pub const ERANGE: i64 = 34;
pub const ENAMETOOLONG: i64 = 36;
pub const ENOSYS: i64 = 38;
pub const ENOTEMPTY: i64 = 39;
pub const ELOOP: i64 = 40;
pub const ENOTSOCK: i64 = 88;
pub const EDESTADDRREQ: i64 = 89;
pub const EMSGSIZE: i64 = 90;
pub const EPROTOTYPE: i64 = 91;
pub const ENOPROTOOPT: i64 = 92;
pub const ESOCKTNOSUPPORT: i64 = 94;
pub const EOPNOTSUPP: i64 = 95;
pub const EAFNOSUPPORT: i64 = 97;
pub const EADDRINUSE: i64 = 98;
pub const EISCONN: i64 = 106;
pub const ENOTCONN: i64 = 107;
pub const ECONNREFUSED: i64 = 111;
pub const ETIMEDOUT: i64 = 110;

/// Interno do kernel (o programa nunca vê): "refaça esta chamada de sistema
/// quando o processo voltar a rodar". Usado quando ele é parado no meio dela.
pub const ERESTARTSYS: i64 = 512;
