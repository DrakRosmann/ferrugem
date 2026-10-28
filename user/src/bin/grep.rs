//! grep [-v] [-c] [-i] [-n] padrão [arquivo...]: linhas que contêm o padrão.
//! Sai com 0 se encontrou algo, 1 se não, 2 em erro.

#![no_std]
#![no_main]

extern crate alloc;

use alloc::string::String;
use alloc::vec::Vec;
use ferrugem_user::{entry, eprintln, println, rt, sys};

entry!(main);

struct Options {
    invert: bool,
    count: bool,
    ignore_case: bool,
    numbers: bool,
}

fn read_all(fd: u64) -> sys::Result<Vec<u8>> {
    let mut data = Vec::new();
    let mut buf = [0u8; 4096];
    loop {
        let n = sys::read(fd, &mut buf);
        if n < 0 {
            return Err(-n as sys::Errno);
        }
        if n == 0 {
            return Ok(data);
        }
        data.extend_from_slice(&buf[..n as usize]);
    }
}

fn search(text: &str, pattern: &str, options: &Options, prefix: &str) -> usize {
    let mut matches = 0;
    for (number, line) in text.lines().enumerate() {
        let found = if options.ignore_case {
            line.to_lowercase().contains(&pattern.to_lowercase())
        } else {
            line.contains(pattern)
        };
        if found != options.invert {
            matches += 1;
            if !options.count {
                let number = if options.numbers { alloc::format!("{}:", number + 1) } else { String::new() };
                println!("{prefix}{number}{line}");
            }
        }
    }
    matches
}

fn main() -> i32 {
    let mut options = Options { invert: false, count: false, ignore_case: false, numbers: false };
    let mut rest: Vec<&str> = Vec::new();
    for arg in rt::args().skip(1) {
        match arg {
            "-v" => options.invert = true,
            "-c" => options.count = true,
            "-i" => options.ignore_case = true,
            "-n" => options.numbers = true,
            _ => rest.push(arg),
        }
    }
    let Some((pattern, files)) = rest.split_first() else {
        eprintln!("uso: grep [-v] [-c] [-i] [-n] padrão [arquivo...]");
        return 2;
    };
    let mut total = 0;
    let mut status = 0;
    let sources: Vec<&str> = if files.is_empty() { alloc::vec!["-"] } else { files.to_vec() };
    for source in &sources {
        let data = if *source == "-" {
            read_all(0)
        } else {
            sys::open(source, sys::O_RDONLY, 0).and_then(|fd| {
                let data = read_all(fd);
                let _ = sys::close(fd);
                data
            })
        };
        match data {
            Ok(data) => {
                let prefix = if sources.len() > 1 { alloc::format!("{source}:") } else { String::new() };
                let found = search(&String::from_utf8_lossy(&data), pattern, &options, &prefix);
                if options.count {
                    println!("{prefix}{found}");
                }
                total += found;
            }
            Err(errno) => {
                eprintln!("grep: {source}: {}", sys::strerror(errno));
                status = 2;
            }
        }
    }
    if status != 0 { status } else if total > 0 { 0 } else { 1 }
}
