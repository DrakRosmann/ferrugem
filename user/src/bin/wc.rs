//! wc [-l] [-w] [-c] [arquivo...]: conta linhas, palavras e bytes.

#![no_std]
#![no_main]

extern crate alloc;

use alloc::string::String;
use alloc::vec::Vec;
use ferrugem_user::{entry, eprintln, println, rt, sys};

entry!(main);

#[derive(Default)]
struct Counts {
    lines: usize,
    words: usize,
    bytes: usize,
}

fn count(fd: u64) -> sys::Result<Counts> {
    let mut counts = Counts::default();
    let mut buf = [0u8; 4096];
    let mut in_word = false;
    loop {
        let n = sys::read(fd, &mut buf);
        if n < 0 {
            return Err(-n as sys::Errno);
        }
        if n == 0 {
            return Ok(counts);
        }
        for &byte in &buf[..n as usize] {
            counts.bytes += 1;
            if byte == b'\n' {
                counts.lines += 1;
            }
            let space = byte.is_ascii_whitespace();
            if !space && !in_word {
                counts.words += 1;
            }
            in_word = !space;
        }
    }
}

fn main() -> i32 {
    let mut fields: Vec<char> = Vec::new();
    let mut files: Vec<&str> = Vec::new();
    for arg in rt::args().skip(1) {
        match arg.strip_prefix('-') {
            Some(flags) if !flags.is_empty() => fields.extend(flags.chars()),
            _ => files.push(arg),
        }
    }
    if fields.is_empty() {
        fields = alloc::vec!['l', 'w', 'c'];
    }
    let show = |counts: &Counts, name: &str| {
        let mut line = String::new();
        for field in ['l', 'w', 'c'] {
            if fields.contains(&field) {
                let value = match field {
                    'l' => counts.lines,
                    'w' => counts.words,
                    _ => counts.bytes,
                };
                line.push_str(&alloc::format!("{value:>7} "));
            }
        }
        println!("{}{name}", line);
    };
    if files.is_empty() {
        return match count(0) {
            Ok(counts) => {
                show(&counts, "");
                0
            }
            Err(_) => 1,
        };
    }
    let mut status = 0;
    for path in files {
        match sys::open(path, sys::O_RDONLY, 0).and_then(|fd| {
            let counts = count(fd);
            let _ = sys::close(fd);
            counts
        }) {
            Ok(counts) => show(&counts, path),
            Err(errno) => {
                eprintln!("wc: {path}: {}", sys::strerror(errno));
                status = 1;
            }
        }
    }
    status
}
