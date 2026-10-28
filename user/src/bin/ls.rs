//! ls [-a] [-l] [caminho...]: lista diretórios. Com -l: tipo, permissões,
//! links, tamanho, data de modificação (UTC) e o destino dos links.

#![no_std]
#![no_main]

extern crate alloc;

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use ferrugem_user::{entry, eprintln, print, println, rt, sys};

entry!(main);

struct Options {
    all: bool,
    long: bool,
    color: bool,
}

fn type_char(mode: u32) -> char {
    match mode & sys::S_IFMT {
        sys::S_IFDIR => 'd',
        sys::S_IFCHR => 'c',
        sys::S_IFIFO => 'p',
        sys::S_IFLNK => 'l',
        _ => '-',
    }
}

fn permissions(mode: u32) -> String {
    let mut text = String::new();
    for shift in [6, 3, 0] {
        let bits = mode >> shift & 7;
        text.push(if bits & 4 != 0 { 'r' } else { '-' });
        text.push(if bits & 2 != 0 { 'w' } else { '-' });
        text.push(if bits & 1 != 0 { 'x' } else { '-' });
    }
    text
}

fn decorate(name: &str, mode: u32, options: &Options) -> String {
    if !options.color {
        return String::from(name);
    }
    let color = match mode & sys::S_IFMT {
        sys::S_IFDIR => "1;34",
        sys::S_IFLNK => "1;36",
        sys::S_IFCHR => "33",
        _ if mode & 0o111 != 0 => "1;32",
        _ => return String::from(name),
    };
    format!("\x1b[{color}m{name}\x1b[0m")
}

fn long_line(path: &str, name: &str, info: &sys::Stat, options: &Options) {
    let (year, month, day, hour, minute, _) = sys::civil_time(info.mtime);
    print!(
        "{}{} {:>2} {:>8} {year}-{month:02}-{day:02} {hour:02}:{minute:02} {}",
        type_char(info.mode),
        permissions(info.mode),
        info.nlink,
        info.size,
        decorate(name, info.mode, options)
    );
    if info.is_symlink() {
        print!(" -> {}", sys::readlink(path).unwrap_or_default());
    }
    println!();
}

fn list(path: &str, options: &Options) -> sys::Result<()> {
    // Um link para diretório é listado como diretório, exceto com -l.
    let info = match sys::lstat(path)? {
        link if link.is_symlink() && !options.long => sys::stat(path).unwrap_or(link),
        info => info,
    };
    if !info.is_dir() {
        if options.long {
            long_line(path, path, &info, options);
        } else {
            println!("{}", decorate(path, info.mode, options));
        }
        return Ok(());
    }
    let fd = sys::open(path, sys::O_RDONLY | sys::O_DIRECTORY, 0)?;
    let mut names = sys::read_dir(fd)?;
    let _ = sys::close(fd);
    names.retain(|n| options.all || !n.starts_with('.'));
    names.sort();

    let mut column = 0;
    for name in &names {
        let full = format!("{}/{name}", path.trim_end_matches('/'));
        let info = sys::lstat(&full).unwrap_or_default();
        if options.long {
            long_line(&full, name, &info, options);
        } else if options.color {
            // Terminal: nomes lado a lado, quebrando antes da coluna 80.
            let width = name.chars().count() + 2;
            if column + width > 80 {
                println!();
                column = 0;
            }
            print!("{}  ", decorate(name, info.mode, options));
            column += width;
        } else {
            println!("{name}");
        }
    }
    if column > 0 {
        println!();
    }
    Ok(())
}

fn main() -> i32 {
    let mut options = Options { all: false, long: false, color: sys::isatty(1) };
    let mut paths: Vec<&str> = Vec::new();
    for arg in rt::args().skip(1) {
        match arg.strip_prefix('-') {
            Some(flags) if !flags.is_empty() => {
                for flag in flags.chars() {
                    match flag {
                        'a' => options.all = true,
                        'l' => options.long = true,
                        _ => {
                            eprintln!("ls: opção inválida -{flag}");
                            return 2;
                        }
                    }
                }
            }
            _ => paths.push(arg),
        }
    }
    if paths.is_empty() {
        paths.push(".");
    }
    let mut status = 0;
    for (i, path) in paths.iter().enumerate() {
        // Com vários caminhos, cada diretório ganha um título.
        if paths.len() > 1 && sys::stat(path).is_ok_and(|info| info.is_dir()) {
            println!("{}{path}:", if i > 0 { "\n" } else { "" });
        }
        if let Err(errno) = list(path, &options) {
            eprintln!("ls: {path}: {}", sys::strerror(errno));
            status = 1;
        }
    }
    status
}
