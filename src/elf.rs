//! Leitura de executáveis ELF64 estáticos (o formato dos binários do Linux).

use alloc::vec::Vec;

use crate::paging::USER_TOP;

const PT_LOAD: u32 = 1;
const PT_INTERP: u32 = 3;
const ET_EXEC: u16 = 2;
const ET_DYN: u16 = 3;
const EM_X86_64: u16 = 0x3E;

pub const PF_X: u32 = 1;
pub const PF_W: u32 = 2;
pub const PF_R: u32 = 4;

/// Um segmento a carregar: `file_size` bytes do arquivo a partir de
/// `offset` vão para `vaddr`, e o resto até `mem_size` é zerado (o .bss).
pub struct Segment {
    pub vaddr: u64,
    pub mem_size: u64,
    pub offset: usize,
    pub file_size: usize,
    pub flags: u32,
}

pub struct Image {
    pub entry: u64,
    pub segments: Vec<Segment>,
}

fn u16_at(data: &[u8], at: usize) -> u16 {
    u16::from_le_bytes(data[at..at + 2].try_into().unwrap())
}

fn u32_at(data: &[u8], at: usize) -> u32 {
    u32::from_le_bytes(data[at..at + 4].try_into().unwrap())
}

fn u64_at(data: &[u8], at: usize) -> u64 {
    u64::from_le_bytes(data[at..at + 8].try_into().unwrap())
}

pub fn is_elf(data: &[u8]) -> bool {
    data.starts_with(b"\x7fELF")
}

pub fn parse(data: &[u8]) -> Result<Image, &'static str> {
    if data.len() < 64 || !is_elf(data) {
        return Err("não é um executável ELF");
    }
    if data[4] != 2 || data[5] != 1 {
        return Err("só ELF de 64 bits little-endian é suportado");
    }
    if u16_at(data, 18) != EM_X86_64 {
        return Err("o executável não é para x86_64");
    }
    match u16_at(data, 16) {
        ET_EXEC => {}
        ET_DYN => return Err("executáveis PIE/dinâmicos ainda não são suportados (use -static -no-pie)"),
        _ => return Err("o ELF não é um executável"),
    }

    let entry = u64_at(data, 24);
    if entry >= USER_TOP {
        return Err("ponto de entrada fora do espaço de usuário");
    }
    let ph_offset = u64_at(data, 32) as usize;
    let ph_size = u16_at(data, 54) as usize;
    let ph_count = u16_at(data, 56) as usize;
    if ph_size < 56 || ph_offset.checked_add(ph_size * ph_count).is_none_or(|end| end > data.len()) {
        return Err("tabela de cabeçalhos de programa inválida");
    }

    let mut segments = Vec::new();
    for i in 0..ph_count {
        let ph = &data[ph_offset + i * ph_size..];
        match u32_at(ph, 0) {
            PT_INTERP => return Err("o executável precisa de ligação dinâmica"),
            PT_LOAD => {}
            _ => continue,
        }
        let segment = Segment {
            flags: u32_at(ph, 4),
            offset: u64_at(ph, 8) as usize,
            vaddr: u64_at(ph, 16),
            file_size: u64_at(ph, 32) as usize,
            mem_size: u64_at(ph, 40),
        };
        let file_end = segment.offset.checked_add(segment.file_size);
        let mem_end = segment.vaddr.checked_add(segment.mem_size);
        if file_end.is_none_or(|end| end > data.len())
            || segment.file_size as u64 > segment.mem_size
            || segment.vaddr < 0x1000
            || mem_end.is_none_or(|end| end > USER_TOP)
        {
            return Err("segmento ELF inválido");
        }
        segments.push(segment);
    }
    if segments.is_empty() {
        return Err("o executável não tem segmentos carregáveis");
    }
    Ok(Image { entry, segments })
}
