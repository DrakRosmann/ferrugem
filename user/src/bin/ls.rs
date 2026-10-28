//! ls [-a] [-l] [caminho...]: lista diretórios.

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
        sys::S_IFCHR => "33",
        _ if mode & 0o111 != 0 => "1;32",
        _ => return String::from(name),
    };
    format!("\x1b[{color}m{name}\x1b[0m")
}

fn list(path: &str, options: &Options) -> sys::Result<()> {
    let info = sys::stat(path)?;
    if !info.is_dir() {
        if options.long {
            let kind = type_char(info.mode);
            println!("{kind}{} {:>8} {}", permissions(info.mode), info.size, decorate(path, info.mode, options));
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
        let info = sys::stat(&full).unwrap_or(sys::Stat { ino: 0, mode: 0, size: 0 });
        if options.long {
            println!(
                "{}{} {:>8} {}",
                type_char(info.mode),
                permissions(info.mode),
                info.size,
                decorate(name, info.mode, options)
            );
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
        if paths.len() > 1 {
            println!("{}{path}:", if i > 0 { "\n" } else { "" });
        }
        if let Err(errno) = list(path, &options) {
            eprintln!("ls: {path}: {}", sys::strerror(errno));
            status = 1;
        }
    }
    status
}
