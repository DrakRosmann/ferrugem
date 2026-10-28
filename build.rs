//! Liga o kernel com linker.ld e compila os programas de usuário (user/),
//! que são embutidos na imagem e instalados no sistema de arquivos no boot.

use std::env;
use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

/// Os programas de usuário são binários estáticos para a ABI do Linux.
const USER_TARGET: &str = "x86_64-unknown-linux-gnu";

fn main() {
    let root = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());
    let out = PathBuf::from(env::var("OUT_DIR").unwrap());
    println!("cargo:rustc-link-arg=-T{}", root.join("linker.ld").display());
    println!("cargo:rerun-if-changed=linker.ld");
    println!("cargo:rerun-if-changed=user/src");
    println!("cargo:rerun-if-changed=user/c");
    println!("cargo:rerun-if-changed=user/Cargo.toml");

    let mut programs = build_rust_programs(&root);
    programs.extend(build_c_programs(&root, &out));

    let mut code = String::from("pub static PROGRAMS: &[(&str, &[u8])] = &[\n");
    for (name, path) in &programs {
        writeln!(code, "    ({name:?}, include_bytes!({:?})),", path.display().to_string()).unwrap();
    }
    code.push_str("];\n");
    fs::write(out.join("programs.rs"), code).unwrap();
}

fn build_rust_programs(root: &Path) -> Vec<(String, PathBuf)> {
    let target_dir = root.join("target/user");
    let flags = [
        "-Crelocation-model=static",
        "-Ctarget-feature=+crt-static",
        "-Clink-arg=-nostartfiles",
        "-Clink-arg=-nostdlib",
        "-Clink-arg=-static",
        "-Clink-arg=-no-pie",
    ];
    let status = Command::new(env::var("CARGO").unwrap())
        .current_dir(root.join("user"))
        .args(["build", "--release", "--target", USER_TARGET, "--target-dir"])
        .arg(&target_dir)
        .env_remove("RUSTFLAGS")
        .env("CARGO_ENCODED_RUSTFLAGS", flags.join("\x1f"))
        .status()
        .expect("falha ao executar o cargo para user/");
    assert!(status.success(), "falha ao compilar os programas de usuário");

    let bin_dir = target_dir.join(USER_TARGET).join("release");
    let mut programs: Vec<_> = fs::read_dir(root.join("user/src/bin"))
        .unwrap()
        .filter_map(|entry| {
            let path = entry.ok()?.path();
            let name = path.file_stem()?.to_str()?.to_string();
            (path.extension()? == "rs").then(|| (name.clone(), bin_dir.join(name)))
        })
        .collect();
    programs.sort();
    programs
}

/// Programas em C compilados com o gcc do sistema, sem libc. Opcionais:
/// sem gcc, o kernel é gerado sem eles.
fn build_c_programs(root: &Path, out: &Path) -> Vec<(String, PathBuf)> {
    let mut programs = Vec::new();
    let Ok(entries) = fs::read_dir(root.join("user/c")) else { return programs };
    for entry in entries.flatten() {
        let source = entry.path();
        if source.extension().is_none_or(|e| e != "c") {
            continue;
        }
        let name = source.file_stem().unwrap().to_str().unwrap().to_string();
        let output = out.join(&name);
        let result = Command::new("gcc")
            .args(["-static", "-nostdlib", "-no-pie", "-fno-pie", "-O2", "-ffreestanding"])
            .args(["-fno-stack-protector", "-fno-asynchronous-unwind-tables", "-o"])
            .arg(&output)
            .arg(&source)
            .status();
        match result {
            Ok(status) if status.success() => programs.push((name, output)),
            _ => println!("cargo:warning=não foi possível compilar {} (gcc ausente?)", source.display()),
        }
    }
    programs.sort();
    programs
}
