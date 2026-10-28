//! ext2: o sistema de arquivos clássico do Linux, lido e gravado no disco.
//!
//! O disco é dividido em blocos (1, 2 ou 4 KiB) e os blocos em grupos. O
//! superbloco (no byte 1024) descreve o todo; cada grupo tem um bitmap de
//! blocos, um bitmap de inodes e uma tabela de inodes. Um inode guarda os
//! metadados de um arquivo e onde estão os dados: 12 ponteiros diretos para
//! blocos, um indireto (um bloco cheio de ponteiros), um duplamente e um
//! triplamente indireto. Um diretório é um arquivo com uma lista de
//! entradas (inode, tamanho do registro, nome).
//!
//! Cada alteração vai para o disco na hora (write-through), na ordem que
//! deixa o disco mais consistente se a energia cair no meio: o inode antes
//! da entrada de diretório que aponta para ele. Imagens criadas com
//! `mkfs.ext2` (veja scripts/disco.sh) podem ser montadas e verificadas
//! com `e2fsck` no Linux.

use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::sync::Arc;
use alloc::vec;
use alloc::vec::Vec;
use core::any::Any;

use crate::block::{BlockDevice, SECTOR_SIZE};
use crate::rtc;
use crate::sync::SpinLock;
use crate::syscall::{EEXIST, EFBIG, EINVAL, EIO, EISDIR, ENOENT, ENOSPC, ENOTDIR, ENOTEMPTY, EROFS, EXDEV, Errno};
use crate::vfs::{DirEntry, Device, Inode, Kind, NewNode, NodeRef};

const MAGIC: u16 = 0xEF53;
const ROOT_INO: u32 = 2;
const DIRECT_BLOCKS: u64 = 12;
/// Blocos guardados no cache (o resto é relido do disco).
const CACHE_BLOCKS: usize = 2048;
/// Links simbólicos curtos ficam dentro do próprio inode ("fast symlinks").
const FAST_SYMLINK_MAX: usize = 60;

// Tipos no campo de modo do inode.
const S_IFMT: u16 = 0xF000;
const S_IFREG: u16 = 0x8000;
const S_IFDIR: u16 = 0x4000;
const S_IFLNK: u16 = 0xA000;
const S_IFCHR: u16 = 0x2000;

// Recursos (features). Sem os obrigatórios que não conhecemos, não dá para
// montar; sem os de "só leitura", dá para ler mas não para gravar.
const INCOMPAT_FILETYPE: u32 = 0x2;
const RO_COMPAT_SUPPORTED: u32 = 0x1 | 0x2; // sparse_super, large_file
/// Diretório com índice em árvore (dir_index). Não o mantemos: ao alterar
/// um diretório indexado, a marca é tirada e o Linux volta à busca linear.
const INDEX_FLAG: u32 = 0x1000;

const STATE_CLEAN: u16 = 1;

// --- Bytes em little-endian ------------------------------------------------------

fn u16_at(data: &[u8], offset: usize) -> u16 {
    u16::from_le_bytes([data[offset], data[offset + 1]])
}

fn u32_at(data: &[u8], offset: usize) -> u32 {
    u32::from_le_bytes(data[offset..offset + 4].try_into().unwrap())
}

fn put_u16(data: &mut [u8], offset: usize, value: u16) {
    data[offset..offset + 2].copy_from_slice(&value.to_le_bytes());
}

fn put_u32(data: &mut [u8], offset: usize, value: u32) {
    data[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
}

// --- Inode cru ---------------------------------------------------------------------

/// Um inode como está no disco. Os campos que não usamos são preservados.
struct RawInode(Vec<u8>);

impl RawInode {
    fn mode(&self) -> u16 {
        u16_at(&self.0, 0)
    }
    fn file_type(&self) -> u16 {
        self.mode() & S_IFMT
    }
    fn size(&self) -> u64 {
        let high = if self.file_type() == S_IFREG { u32_at(&self.0, 108) as u64 } else { 0 };
        u32_at(&self.0, 4) as u64 | high << 32
    }
    fn set_size(&mut self, size: u64) {
        put_u32(&mut self.0, 4, size as u32);
        if self.file_type() == S_IFREG {
            put_u32(&mut self.0, 108, (size >> 32) as u32);
        }
    }
    fn mtime(&self) -> u32 {
        u32_at(&self.0, 16)
    }
    /// Dono: 16 bits baixos no começo do inode, altos na área "osd2".
    fn uid(&self) -> u32 {
        u16_at(&self.0, 2) as u32 | (u16_at(&self.0, 120) as u32) << 16
    }
    fn gid(&self) -> u32 {
        u16_at(&self.0, 24) as u32 | (u16_at(&self.0, 122) as u32) << 16
    }
    fn set_owner(&mut self, uid: u32, gid: u32) {
        put_u16(&mut self.0, 2, uid as u16);
        put_u16(&mut self.0, 120, (uid >> 16) as u16);
        put_u16(&mut self.0, 24, gid as u16);
        put_u16(&mut self.0, 122, (gid >> 16) as u16);
    }
    /// Marca uma alteração: muda mtime e ctime.
    fn touch(&mut self) {
        let now = rtc::now() as u32;
        put_u32(&mut self.0, 12, now);
        put_u32(&mut self.0, 16, now);
    }
    fn links(&self) -> u16 {
        u16_at(&self.0, 26)
    }
    fn set_links(&mut self, links: u16) {
        put_u16(&mut self.0, 26, links);
    }
    /// Espaço ocupado, em setores de 512 bytes (inclui os blocos indiretos).
    fn sectors(&self) -> u32 {
        u32_at(&self.0, 28)
    }
    fn add_sectors(&mut self, delta: i64) {
        let sectors = (self.sectors() as i64 + delta) as u32;
        put_u32(&mut self.0, 28, sectors);
    }
    fn flags(&self) -> u32 {
        u32_at(&self.0, 32)
    }
    fn block(&self, index: usize) -> u32 {
        u32_at(&self.0, 40 + index * 4)
    }
    fn set_block(&mut self, index: usize, block: u32) {
        put_u32(&mut self.0, 40 + index * 4, block);
    }
    /// Link simbólico com o destino guardado nos ponteiros de bloco.
    fn is_fast_symlink(&self) -> bool {
        let acl_sectors = if u32_at(&self.0, 104) != 0 { 1 } else { 0 };
        self.file_type() == S_IFLNK && self.sectors() <= acl_sectors && self.size() < FAST_SYMLINK_MAX as u64
    }
}

// --- O sistema de arquivos -----------------------------------------------------------

struct Fs {
    disk: Arc<dyn BlockDevice>,
    block_size: usize,
    /// Ponteiros de bloco que cabem num bloco indireto.
    per_block: u64,
    superblock: Vec<u8>,
    /// Descritores de grupo crus (32 bytes cada).
    groups: Vec<[u8; 32]>,
    blocks_per_group: u32,
    inodes_per_group: u32,
    inode_size: usize,
    first_data_block: u32,
    blocks_count: u32,
    filetype: bool,
    cache: BTreeMap<u32, Vec<u8>>,
}

pub struct Ext2 {
    fs: SpinLock<Fs>,
    dev: u64,
    read_only: bool,
}

pub struct Ext2Node {
    fs: Arc<Ext2>,
    ino: u32,
}

/// Informações para o monitor do kernel.
pub struct Info {
    pub label: String,
    pub block_size: usize,
    pub blocks: u64,
    pub free_blocks: u64,
    pub inodes: u64,
    pub free_inodes: u64,
    pub read_only: bool,
}

impl Fs {
    // --- Blocos -----------------------------------------------------------------

    fn block(&mut self, block: u32) -> Result<&[u8], Errno> {
        if block == 0 || block >= self.blocks_count {
            return Err(EIO);
        }
        if !self.cache.contains_key(&block) {
            let mut data = vec![0; self.block_size];
            self.disk.read(block as u64 * (self.block_size / SECTOR_SIZE) as u64, &mut data)?;
            if self.cache.len() >= CACHE_BLOCKS {
                self.cache.pop_first();
            }
            self.cache.insert(block, data);
        }
        Ok(&self.cache[&block])
    }

    fn write_block(&mut self, block: u32, data: &[u8]) -> Result<(), Errno> {
        if block == 0 || block >= self.blocks_count {
            return Err(EIO);
        }
        self.disk.write(block as u64 * (self.block_size / SECTOR_SIZE) as u64, data)?;
        if let Some(cached) = self.cache.get_mut(&block) {
            cached.copy_from_slice(data);
        }
        Ok(())
    }

    /// Altera um trecho de um bloco (lê, muda, grava).
    fn update_block(&mut self, block: u32, f: impl FnOnce(&mut [u8])) -> Result<(), Errno> {
        let mut data = self.block(block)?.to_vec();
        f(&mut data);
        self.write_block(block, &data)
    }

    // --- Superbloco e grupos ----------------------------------------------------------

    fn write_superblock(&mut self) -> Result<(), Errno> {
        self.disk.write(2, &self.superblock)
    }

    fn write_group(&mut self, group: usize) -> Result<(), Errno> {
        let offset = group * 32;
        let block = self.first_data_block + 1 + (offset / self.block_size) as u32;
        let descriptor = self.groups[group];
        let within = offset % self.block_size;
        self.update_block(block, |data| data[within..within + 32].copy_from_slice(&descriptor))
    }

    /// Ajusta os contadores de livres (e de diretórios) de um grupo e do
    /// superbloco, e grava os dois.
    fn adjust_counts(&mut self, group: usize, blocks: i32, inodes: i32, dirs: i32) -> Result<(), Errno> {
        let descriptor = &mut self.groups[group];
        for (offset, delta) in [(12, blocks), (14, inodes), (16, dirs)] {
            let value = u16_at(descriptor, offset) as i32 + delta;
            put_u16(descriptor, offset, value as u16);
        }
        self.write_group(group)?;
        for (offset, delta) in [(12, blocks), (16, inodes)] {
            let value = u32_at(&self.superblock, offset) as i64 + delta as i64;
            put_u32(&mut self.superblock, offset, value as u32);
        }
        self.write_superblock()
    }

    fn group_count(&self) -> usize {
        self.groups.len()
    }

    // --- Alocação ---------------------------------------------------------------------

    /// Procura um bit livre no bitmap (em `bitmap_offset` do descritor) de
    /// algum grupo, começando por `start`, e o marca. Devolve (grupo, bit).
    fn allocate_bit(&mut self, start: usize, bitmap_offset: usize, free_offset: usize, limit: impl Fn(usize) -> u32) -> Result<(usize, u32), Errno> {
        for i in 0..self.group_count() {
            let group = (start + i) % self.group_count();
            if u16_at(&self.groups[group], free_offset) == 0 {
                continue;
            }
            let bitmap = u32_at(&self.groups[group], bitmap_offset);
            let bits = limit(group);
            let data = self.block(bitmap)?;
            let Some(bit) = (0..bits).find(|&bit| data[bit as usize / 8] & (1 << (bit % 8)) == 0) else {
                continue; // contador e bitmap discordam: tenta outro grupo
            };
            self.update_block(bitmap, |data| data[bit as usize / 8] |= 1 << (bit % 8))?;
            return Ok((group, bit));
        }
        Err(ENOSPC)
    }

    fn release_bit(&mut self, group: usize, bitmap_offset: usize, bit: u32) -> Result<(), Errno> {
        let bitmap = u32_at(&self.groups[group], bitmap_offset);
        self.update_block(bitmap, |data| data[bit as usize / 8] &= !(1 << (bit % 8)))
    }

    /// Reserva um bloco, de preferência no grupo `near`, e o zera.
    fn allocate_block(&mut self, near: usize) -> Result<u32, Errno> {
        let (per_group, first, total) = (self.blocks_per_group, self.first_data_block, self.blocks_count);
        let (group, bit) =
            self.allocate_bit(near, 0, 12, |group| per_group.min(total - first - group as u32 * per_group))?;
        self.adjust_counts(group, -1, 0, 0)?;
        let block = first + group as u32 * per_group + bit;
        let zeros = vec![0; self.block_size];
        self.write_block(block, &zeros)?;
        Ok(block)
    }

    fn free_block(&mut self, block: u32) -> Result<(), Errno> {
        let index = block - self.first_data_block;
        let group = (index / self.blocks_per_group) as usize;
        self.release_bit(group, 0, index % self.blocks_per_group)?;
        self.cache.remove(&block);
        self.adjust_counts(group, 1, 0, 0)
    }

    fn allocate_inode(&mut self, near: usize, directory: bool) -> Result<u32, Errno> {
        let per_group = self.inodes_per_group;
        let (group, bit) = self.allocate_bit(near, 4, 14, |_| per_group)?;
        self.adjust_counts(group, 0, -1, directory as i32)?;
        Ok(group as u32 * per_group + bit + 1)
    }

    fn free_inode(&mut self, ino: u32, directory: bool) -> Result<(), Errno> {
        let group = ((ino - 1) / self.inodes_per_group) as usize;
        self.release_bit(group, 4, (ino - 1) % self.inodes_per_group)?;
        self.adjust_counts(group, 0, 1, -(directory as i32))
    }

    fn group_of(&self, ino: u32) -> usize {
        ((ino - 1) / self.inodes_per_group) as usize
    }

    // --- Inodes -------------------------------------------------------------------------

    fn inode_location(&self, ino: u32) -> (u32, usize) {
        let group = self.group_of(ino);
        let index = ((ino - 1) % self.inodes_per_group) as usize;
        let table = u32_at(&self.groups[group], 8);
        let offset = index * self.inode_size;
        (table + (offset / self.block_size) as u32, offset % self.block_size)
    }

    fn read_inode(&mut self, ino: u32) -> Result<RawInode, Errno> {
        let (block, offset) = self.inode_location(ino);
        let size = self.inode_size;
        Ok(RawInode(self.block(block)?[offset..offset + size].to_vec()))
    }

    fn write_inode(&mut self, ino: u32, inode: &RawInode) -> Result<(), Errno> {
        let (block, offset) = self.inode_location(ino);
        self.update_block(block, |data| data[offset..offset + inode.0.len()].copy_from_slice(&inode.0))
    }

    // --- Blocos de um arquivo ----------------------------------------------------------

    /// O bloco do disco com o `index`-ésimo bloco do arquivo. Com `allocate`,
    /// cria o que faltar (o bloco e os indiretos); sem, 0 indica um buraco.
    fn map_block(&mut self, ino: u32, inode: &mut RawInode, index: u64, allocate: bool) -> Result<u32, Errno> {
        let near = self.group_of(ino);
        if index < DIRECT_BLOCKS {
            let mut block = inode.block(index as usize);
            if block == 0 && allocate {
                block = self.allocate_block(near)?;
                inode.set_block(index as usize, block);
                inode.add_sectors((self.block_size / SECTOR_SIZE) as i64);
            }
            return Ok(block);
        }
        // Qual árvore de indiretos (1, 2 ou 3 níveis) e a posição nela.
        let mut index = index - DIRECT_BLOCKS;
        let mut levels = 1;
        let mut span = self.per_block;
        while index >= span {
            index -= span;
            levels += 1;
            span *= self.per_block;
            if levels > 3 {
                return Err(EFBIG);
            }
        }
        let slot = 11 + levels;
        let mut block = inode.block(slot);
        if block == 0 {
            if !allocate {
                return Ok(0);
            }
            block = self.allocate_block(near)?;
            inode.set_block(slot, block);
            inode.add_sectors((self.block_size / SECTOR_SIZE) as i64);
        }
        for level in (0..levels).rev() {
            let span = self.per_block.pow(level as u32);
            let entry = (index / span) as usize;
            index %= span;
            let mut next = u32_at(self.block(block)?, entry * 4);
            if next == 0 {
                if !allocate {
                    return Ok(0);
                }
                next = self.allocate_block(near)?;
                inode.add_sectors((self.block_size / SECTOR_SIZE) as i64);
                self.update_block(block, |data| put_u32(data, entry * 4, next))?;
            }
            block = next;
        }
        Ok(block)
    }

    /// Libera os blocos de dados a partir do bloco `keep` do arquivo (e os
    /// indiretos que ficarem vazios).
    fn free_blocks_from(&mut self, inode: &mut RawInode, keep: u64) -> Result<(), Errno> {
        let mut first = 0;
        for slot in 0..15usize {
            let level = slot.saturating_sub(11) as u32;
            let span = self.per_block.pow(level);
            let mut block = inode.block(slot);
            let freed = self.free_tree(&mut block, level, first, keep)?;
            inode.set_block(slot, block);
            inode.add_sectors(-((freed * self.block_size / SECTOR_SIZE) as i64));
            first += span;
        }
        Ok(())
    }

    /// Libera a subárvore de `*block` (de `level` níveis de indireção, que
    /// cobre os blocos do arquivo a partir de `first`) que estiver a partir
    /// de `keep`. Devolve quantos blocos foram liberados.
    fn free_tree(&mut self, block: &mut u32, level: u32, first: u64, keep: u64) -> Result<usize, Errno> {
        if *block == 0 || first + self.per_block.pow(level) <= keep {
            return Ok(0);
        }
        let mut freed = 0;
        if level > 0 {
            let child_span = self.per_block.pow(level - 1);
            let mut table = self.block(*block)?.to_vec();
            for entry in 0..self.per_block as usize {
                let mut child = u32_at(&table, entry * 4);
                freed += self.free_tree(&mut child, level - 1, first + entry as u64 * child_span, keep)?;
                put_u32(&mut table, entry * 4, child);
            }
            if first < keep {
                // Parte da subárvore continua: o indireto fica, atualizado.
                self.write_block(*block, &table)?;
                return Ok(freed);
            }
        }
        self.free_block(*block)?;
        *block = 0;
        Ok(freed + 1)
    }

    // --- Conteúdo -----------------------------------------------------------------------

    fn read(&mut self, ino: u32, offset: u64, buf: &mut [u8]) -> Result<usize, Errno> {
        let mut inode = self.read_inode(ino)?;
        let size = inode.size();
        if offset >= size {
            return Ok(0);
        }
        let len = buf.len().min((size - offset) as usize);
        let bs = self.block_size as u64;
        let mut done = 0;
        while done < len {
            let position = offset + done as u64;
            let within = (position % bs) as usize;
            let n = (self.block_size - within).min(len - done);
            let block = self.map_block(ino, &mut inode, position / bs, false)?;
            if block == 0 {
                buf[done..done + n].fill(0); // buraco num arquivo esparso
            } else {
                buf[done..done + n].copy_from_slice(&self.block(block)?[within..within + n]);
            }
            done += n;
        }
        Ok(len)
    }

    fn write(&mut self, ino: u32, offset: u64, data: &[u8]) -> Result<usize, Errno> {
        let mut inode = self.read_inode(ino)?;
        let bs = self.block_size as u64;
        let mut done = 0;
        let mut result = Ok(());
        while done < data.len() {
            let position = offset + done as u64;
            let within = (position % bs) as usize;
            let n = (self.block_size - within).min(data.len() - done);
            let block = match self.map_block(ino, &mut inode, position / bs, true) {
                Ok(block) => block,
                Err(errno) => {
                    result = Err(errno); // disco cheio: grava o que deu
                    break;
                }
            };
            let chunk = &data[done..done + n];
            if n == self.block_size {
                self.write_block(block, chunk)?;
            } else {
                self.update_block(block, |bytes| bytes[within..within + n].copy_from_slice(chunk))?;
            }
            done += n;
        }
        if offset + done as u64 > inode.size() {
            inode.set_size(offset + done as u64);
        }
        inode.touch();
        self.write_inode(ino, &inode)?;
        match result {
            Err(errno) if done == 0 => Err(errno),
            _ => Ok(done),
        }
    }

    fn truncate(&mut self, ino: u32, size: u64) -> Result<(), Errno> {
        let mut inode = self.read_inode(ino)?;
        if size < inode.size() {
            let bs = self.block_size as u64;
            self.free_blocks_from(&mut inode, size.div_ceil(bs))?;
            // O resto do último bloco precisa ler como zero se o arquivo crescer.
            if size % bs != 0 {
                let block = self.map_block(ino, &mut inode, size / bs, false)?;
                if block != 0 {
                    let within = (size % bs) as usize;
                    self.update_block(block, |data| data[within..].fill(0))?;
                }
            }
        }
        inode.set_size(size);
        inode.touch();
        self.write_inode(ino, &inode)
    }

    // --- Diretórios ----------------------------------------------------------------------

    /// Percorre as entradas de um diretório: `visit(bloco, posição, inode,
    /// nome)` devolve `true` para parar. Devolve onde parou.
    fn scan_dir(
        &mut self,
        ino: u32,
        mut visit: impl FnMut(u32, usize, u32, &[u8]) -> bool,
    ) -> Result<Option<(u32, usize)>, Errno> {
        let mut inode = self.read_inode(ino)?;
        if inode.file_type() != S_IFDIR {
            return Err(ENOTDIR);
        }
        let blocks = inode.size() / self.block_size as u64;
        for index in 0..blocks {
            let block = self.map_block(ino, &mut inode, index, false)?;
            if block == 0 {
                continue;
            }
            let data = self.block(block)?.to_vec();
            let mut position = 0;
            while position + 8 <= self.block_size {
                let entry_ino = u32_at(&data, position);
                let record = u16_at(&data, position + 4) as usize;
                let name_len = data[position + 6] as usize;
                if record < 8 || position + record > self.block_size {
                    return Err(EIO); // entrada corrompida
                }
                if entry_ino != 0 && visit(block, position, entry_ino, &data[position + 8..position + 8 + name_len]) {
                    return Ok(Some((block, position)));
                }
                position += record;
            }
        }
        Ok(None)
    }

    fn find_entry(&mut self, dir: u32, name: &str) -> Result<u32, Errno> {
        let mut found = 0;
        self.scan_dir(dir, |_, _, ino, entry| {
            found = ino;
            entry == name.as_bytes()
        })?
        .map(|_| found)
        .ok_or(ENOENT)
    }

    fn type_code(mode: u16) -> u8 {
        match mode & S_IFMT {
            S_IFREG => 1,
            S_IFDIR => 2,
            S_IFCHR => 3,
            0x6000 => 4,
            0x1000 => 5,
            0xC000 => 6,
            S_IFLNK => 7,
            _ => 0,
        }
    }

    fn put_entry(&self, data: &mut [u8], position: usize, ino: u32, record: usize, name: &str, mode: u16) {
        put_u32(data, position, ino);
        put_u16(data, position + 4, record as u16);
        data[position + 6] = name.len() as u8;
        data[position + 7] = if self.filetype { Self::type_code(mode) } else { 0 };
        data[position + 8..position + 8 + name.len()].copy_from_slice(name.as_bytes());
    }

    /// Marca o diretório como alterado (e tira o índice dir_index, que não
    /// mantemos).
    fn touch_dir(&mut self, dir: u32, inode: &mut RawInode) -> Result<(), Errno> {
        let flags = inode.flags() & !INDEX_FLAG;
        put_u32(&mut inode.0, 32, flags);
        inode.touch();
        self.write_inode(dir, inode)
    }

    fn add_entry(&mut self, dir: u32, name: &str, ino: u32, mode: u16) -> Result<(), Errno> {
        let needed = (8 + name.len()).next_multiple_of(4);
        let mut inode = self.read_inode(dir)?;
        let blocks = inode.size() / self.block_size as u64;
        // Primeiro, sobra de espaço numa entrada existente.
        for index in 0..blocks {
            let block = self.map_block(dir, &mut inode, index, false)?;
            if block == 0 {
                continue;
            }
            let mut data = self.block(block)?.to_vec();
            let mut position = 0;
            while position + 8 <= self.block_size {
                let entry_ino = u32_at(&data, position);
                let record = u16_at(&data, position + 4) as usize;
                if record < 8 {
                    return Err(EIO);
                }
                let used = if entry_ino == 0 { 0 } else { (8 + data[position + 6] as usize).next_multiple_of(4) };
                if record - used >= needed {
                    if used == 0 {
                        self.put_entry(&mut data, position, ino, record, name, mode);
                    } else {
                        put_u16(&mut data, position + 4, used as u16);
                        self.put_entry(&mut data, position + used, ino, record - used, name, mode);
                    }
                    self.write_block(block, &data)?;
                    return self.touch_dir(dir, &mut inode);
                }
                position += record;
            }
        }
        // Senão, um bloco novo no fim do diretório.
        let block = self.map_block(dir, &mut inode, blocks, true)?;
        let mut data = vec![0; self.block_size];
        self.put_entry(&mut data, 0, ino, self.block_size, name, mode);
        self.write_block(block, &data)?;
        inode.set_size(inode.size() + self.block_size as u64);
        self.touch_dir(dir, &mut inode)
    }

    /// Tira a entrada `name` do diretório e devolve o inode dela.
    fn remove_entry(&mut self, dir: u32, name: &str) -> Result<u32, Errno> {
        let mut inode = self.read_inode(dir)?;
        let blocks = inode.size() / self.block_size as u64;
        for index in 0..blocks {
            let block = self.map_block(dir, &mut inode, index, false)?;
            if block == 0 {
                continue;
            }
            let mut data = self.block(block)?.to_vec();
            // A entrada fisicamente anterior no bloco (mesmo se apagada).
            let mut previous: Option<usize> = None;
            let mut position = 0;
            while position + 8 <= self.block_size {
                let entry_ino = u32_at(&data, position);
                let record = u16_at(&data, position + 4) as usize;
                let name_len = data[position + 6] as usize;
                if record < 8 || position + record > self.block_size {
                    return Err(EIO);
                }
                if entry_ino != 0 && &data[position + 8..position + 8 + name_len] == name.as_bytes() {
                    match previous {
                        // A anterior absorve o espaço desta.
                        Some(previous) => {
                            let merged = u16_at(&data, previous + 4) + record as u16;
                            put_u16(&mut data, previous + 4, merged);
                        }
                        None => put_u32(&mut data, position, 0),
                    }
                    self.write_block(block, &data)?;
                    self.touch_dir(dir, &mut inode)?;
                    return Ok(entry_ino);
                }
                previous = Some(position);
                position += record;
            }
        }
        Err(ENOENT)
    }

    fn is_empty_dir(&mut self, ino: u32) -> Result<bool, Errno> {
        let mut empty = true;
        self.scan_dir(ino, |_, _, _, name| {
            empty = name == b"." || name == b"..";
            !empty
        })?;
        Ok(empty)
    }

    /// Solta um link do inode; no último, libera os blocos e o inode.
    fn drop_link(&mut self, ino: u32) -> Result<(), Errno> {
        let mut inode = self.read_inode(ino)?;
        let directory = inode.file_type() == S_IFDIR;
        let links = if directory { 0 } else { inode.links().saturating_sub(1) };
        inode.set_links(links);
        if links > 0 {
            inode.touch();
            return self.write_inode(ino, &inode);
        }
        // Em dispositivos e links curtos, os "ponteiros" não são blocos.
        let has_blocks = matches!(inode.file_type(), S_IFREG | S_IFDIR) || inode.file_type() == S_IFLNK && !inode.is_fast_symlink();
        if has_blocks {
            self.free_blocks_from(&mut inode, 0)?;
        }
        put_u32(&mut inode.0, 20, rtc::now() as u32); // dtime: inode apagado
        self.write_inode(ino, &inode)?;
        self.free_inode(ino, directory)
    }

    fn adjust_links(&mut self, ino: u32, delta: i32) -> Result<(), Errno> {
        let mut inode = self.read_inode(ino)?;
        inode.set_links((inode.links() as i32 + delta) as u16);
        inode.touch();
        self.write_inode(ino, &inode)
    }

    fn create(&mut self, dir: u32, name: &str, node: &NewNode, permissions: u32) -> Result<u32, Errno> {
        if name.len() > 255 {
            return Err(EINVAL);
        }
        if self.find_entry(dir, name).is_ok() {
            return Err(EEXIST);
        }
        let directory = matches!(node, NewNode::Directory);
        let file_type = match node {
            NewNode::File => S_IFREG,
            NewNode::Directory => S_IFDIR,
            NewNode::Symlink(_) => S_IFLNK,
        };
        let mode = file_type | (permissions & 0o7777) as u16;
        let ino = self.allocate_inode(self.group_of(dir), directory)?;
        let mut inode = RawInode(vec![0; self.inode_size]);
        put_u16(&mut inode.0, 0, mode);
        put_u32(&mut inode.0, 8, rtc::now() as u32); // atime
        inode.touch();
        inode.set_links(if directory { 2 } else { 1 });
        if self.inode_size > 128 {
            put_u16(&mut inode.0, 128, 32); // i_extra_isize, como o Linux grava
        }
        match node {
            NewNode::Directory => {
                let block = self.map_block(ino, &mut inode, 0, true)?;
                let mut data = vec![0; self.block_size];
                self.put_entry(&mut data, 0, ino, 12, ".", S_IFDIR);
                self.put_entry(&mut data, 12, dir, self.block_size - 12, "..", S_IFDIR);
                self.write_block(block, &data)?;
                inode.set_size(self.block_size as u64);
            }
            NewNode::Symlink(target) if target.len() < FAST_SYMLINK_MAX => {
                inode.0[40..40 + target.len()].copy_from_slice(target.as_bytes());
                inode.set_size(target.len() as u64);
            }
            _ => {}
        }
        self.write_inode(ino, &inode)?;
        if let NewNode::Symlink(target) = node
            && target.len() >= FAST_SYMLINK_MAX
        {
            self.write(ino, 0, target.as_bytes())?;
        }
        if directory {
            self.adjust_links(dir, 1)?; // o ".." do novo diretório
        }
        self.add_entry(dir, name, ino, mode)?;
        Ok(ino)
    }

    fn unlink(&mut self, dir: u32, name: &str, directory: bool) -> Result<(), Errno> {
        let ino = self.find_entry(dir, name)?;
        let is_dir = self.read_inode(ino)?.file_type() == S_IFDIR;
        match (directory, is_dir) {
            (false, true) => return Err(EISDIR),
            (true, false) => return Err(ENOTDIR),
            (true, true) if !self.is_empty_dir(ino)? => return Err(ENOTEMPTY),
            _ => {}
        }
        self.remove_entry(dir, name)?;
        if is_dir {
            self.adjust_links(dir, -1)?;
        }
        self.drop_link(ino)
    }

    fn rename(&mut self, dir: u32, name: &str, new_dir: u32, new_name: &str) -> Result<(), Errno> {
        let ino = self.find_entry(dir, name)?;
        let mode = self.read_inode(ino)?.mode();
        self.add_entry(new_dir, new_name, ino, mode)?;
        self.remove_entry(dir, name)?;
        if mode & S_IFMT == S_IFDIR && dir != new_dir {
            // O ".." do diretório movido passa a apontar para o novo pai.
            if let Some((block, position)) = self.scan_dir(ino, |_, _, _, entry| entry == b"..")? {
                self.update_block(block, |data| put_u32(data, position, new_dir))?;
            }
            self.adjust_links(dir, -1)?;
            self.adjust_links(new_dir, 1)?;
        }
        Ok(())
    }
}

impl Ext2 {
    /// Monta o ext2 de `disk`. Devolve a raiz, ou por que não deu.
    pub fn mount(disk: Arc<dyn BlockDevice>, dev: u64) -> Result<NodeRef, &'static str> {
        let mut superblock = vec![0; 1024];
        disk.read(2, &mut superblock).map_err(|_| "erro de leitura no disco")?;
        if u16_at(&superblock, 56) != MAGIC {
            return Err("não é um ext2");
        }
        let block_size = 1024usize << u32_at(&superblock, 24);
        let rev = u32_at(&superblock, 76);
        let inode_size = if rev >= 1 { u16_at(&superblock, 88) as usize } else { 128 };
        let incompat = if rev >= 1 { u32_at(&superblock, 96) } else { 0 };
        let ro_compat = if rev >= 1 { u32_at(&superblock, 100) } else { 0 };
        if incompat & !INCOMPAT_FILETYPE != 0 {
            return Err("recursos não suportados (ext3/ext4?)");
        }
        let read_only = ro_compat & !RO_COMPAT_SUPPORTED != 0;
        let blocks_count = u32_at(&superblock, 4);
        let first_data_block = u32_at(&superblock, 20);
        let blocks_per_group = u32_at(&superblock, 32);
        let inodes_per_group = u32_at(&superblock, 40);
        if block_size > 4096 || blocks_per_group == 0 || inodes_per_group == 0 || inode_size < 128 {
            return Err("superbloco inválido");
        }
        let group_count = (blocks_count - first_data_block).div_ceil(blocks_per_group) as usize;

        let mut fs = Fs {
            disk,
            block_size,
            per_block: block_size as u64 / 4,
            superblock,
            groups: Vec::new(),
            blocks_per_group,
            inodes_per_group,
            inode_size,
            first_data_block,
            blocks_count,
            filetype: incompat & INCOMPAT_FILETYPE != 0,
            cache: BTreeMap::new(),
        };
        let table_blocks = (group_count * 32).div_ceil(block_size);
        let mut table = Vec::new();
        for i in 0..table_blocks {
            let data = fs.block(first_data_block + 1 + i as u32).map_err(|_| "erro ao ler os grupos")?;
            table.extend_from_slice(data);
        }
        fs.groups = (0..group_count).map(|g| table[g * 32..g * 32 + 32].try_into().unwrap()).collect();

        if !read_only {
            // Enquanto montado, o sistema não está "limpo": se a energia cair,
            // o e2fsck saberá que precisa verificar.
            let state = u16_at(&fs.superblock, 58) & !STATE_CLEAN;
            put_u16(&mut fs.superblock, 58, state);
            let mounts = u16_at(&fs.superblock, 52);
            put_u16(&mut fs.superblock, 52, mounts.wrapping_add(1));
            put_u32(&mut fs.superblock, 44, rtc::now() as u32); // s_mtime
            fs.write_superblock().map_err(|_| "erro ao gravar o superbloco")?;
        }
        let ext2 = Arc::new(Ext2 { fs: SpinLock::new(fs), dev, read_only });
        Ok(Arc::new(Ext2Node { fs: ext2, ino: ROOT_INO }))
    }

    fn writable(&self) -> Result<(), Errno> {
        if self.read_only { Err(EROFS) } else { Ok(()) }
    }

    pub fn info(root: &NodeRef) -> Option<Info> {
        let node = root.as_any().downcast_ref::<Ext2Node>()?;
        let fs = node.fs.fs.lock();
        let sb = &fs.superblock;
        let label = &sb[120..136];
        let label_len = label.iter().position(|&b| b == 0).unwrap_or(16);
        Some(Info {
            label: String::from_utf8_lossy(&label[..label_len]).into_owned(),
            block_size: fs.block_size,
            blocks: u32_at(sb, 4) as u64,
            free_blocks: u32_at(sb, 12) as u64,
            inodes: u32_at(sb, 0) as u64,
            free_inodes: u32_at(sb, 16) as u64,
            read_only: node.fs.read_only,
        })
    }
}

impl Ext2Node {
    fn node(&self, ino: u32) -> NodeRef {
        Arc::new(Ext2Node { fs: self.fs.clone(), ino })
    }

    fn inode(&self) -> Result<RawInode, Errno> {
        self.fs.fs.lock().read_inode(self.ino)
    }

    /// O inode de outro nó deste mesmo sistema de arquivos.
    fn same_fs(&self, other: &NodeRef) -> Result<u32, Errno> {
        let other = other.as_any().downcast_ref::<Ext2Node>().ok_or(EXDEV)?;
        if Arc::ptr_eq(&self.fs, &other.fs) { Ok(other.ino) } else { Err(EXDEV) }
    }
}

impl Inode for Ext2Node {
    fn ino(&self) -> u64 {
        self.ino as u64
    }

    fn dev(&self) -> u64 {
        self.fs.dev
    }

    fn kind(&self) -> Kind {
        let Ok(inode) = self.inode() else { return Kind::Special };
        match inode.file_type() {
            S_IFREG => Kind::File,
            S_IFDIR => Kind::Directory,
            S_IFLNK => Kind::Symlink,
            S_IFCHR => match Device::from_rdev(self.rdev()) {
                Some(device) => Kind::Device(device),
                None => Kind::Special,
            },
            _ => Kind::Special,
        }
    }

    fn mode(&self) -> u32 {
        self.inode().map_or(0, |inode| inode.mode() as u32)
    }

    fn size(&self) -> u64 {
        self.inode().map_or(0, |inode| inode.size())
    }

    fn nlink(&self) -> u64 {
        self.inode().map_or(0, |inode| inode.links() as u64)
    }

    fn mtime(&self) -> u64 {
        self.inode().map_or(0, |inode| inode.mtime() as u64)
    }

    fn set_mtime(&self, seconds: u64) -> Result<(), Errno> {
        self.fs.writable()?;
        let mut fs = self.fs.fs.lock();
        let mut inode = fs.read_inode(self.ino)?;
        put_u32(&mut inode.0, 16, seconds as u32);
        fs.write_inode(self.ino, &inode)
    }

    fn uid(&self) -> u32 {
        self.inode().map_or(0, |inode| inode.uid())
    }

    fn gid(&self) -> u32 {
        self.inode().map_or(0, |inode| inode.gid())
    }

    fn set_permissions(&self, permissions: u32) -> Result<(), Errno> {
        self.fs.writable()?;
        let mut fs = self.fs.fs.lock();
        let mut inode = fs.read_inode(self.ino)?;
        let mode = inode.file_type() | (permissions & 0o7777) as u16;
        put_u16(&mut inode.0, 0, mode);
        put_u32(&mut inode.0, 12, rtc::now() as u32); // ctime
        fs.write_inode(self.ino, &inode)
    }

    fn set_owner(&self, uid: Option<u32>, gid: Option<u32>) -> Result<(), Errno> {
        self.fs.writable()?;
        let mut fs = self.fs.fs.lock();
        let mut inode = fs.read_inode(self.ino)?;
        let (uid, gid) = (uid.unwrap_or(inode.uid()), gid.unwrap_or(inode.gid()));
        inode.set_owner(uid, gid);
        put_u32(&mut inode.0, 12, rtc::now() as u32); // ctime
        fs.write_inode(self.ino, &inode)
    }

    fn rdev(&self) -> u64 {
        let Ok(inode) = self.inode() else { return 0 };
        if inode.file_type() != S_IFCHR {
            return 0;
        }
        // Formato antigo (major:minor de 8 bits) no 1º ponteiro, ou o novo no 2º.
        match (inode.block(0), inode.block(1)) {
            (0, new) => ((new & 0xFFF00) >> 8 << 8 | (new & 0xFF)) as u64,
            (old, _) => old as u64 & 0xFFFF,
        }
    }

    fn read_at(&self, offset: u64, buf: &mut [u8]) -> Result<usize, Errno> {
        if self.kind() == Kind::Directory {
            return Err(EISDIR);
        }
        self.fs.fs.lock().read(self.ino, offset, buf)
    }

    fn write_at(&self, offset: u64, data: &[u8]) -> Result<usize, Errno> {
        self.fs.writable()?;
        if self.kind() == Kind::Directory {
            return Err(EISDIR);
        }
        self.fs.fs.lock().write(self.ino, offset, data)
    }

    fn truncate(&self, size: u64) -> Result<(), Errno> {
        self.fs.writable()?;
        if self.kind() == Kind::Directory {
            return Err(EISDIR);
        }
        self.fs.fs.lock().truncate(self.ino, size)
    }

    fn lookup(&self, name: &str) -> Result<NodeRef, Errno> {
        let ino = self.fs.fs.lock().find_entry(self.ino, name)?;
        Ok(self.node(ino))
    }

    fn entries(&self) -> Result<Vec<DirEntry>, Errno> {
        let mut entries = Vec::new();
        let filetype = self.fs.fs.lock().filetype;
        let mut raw = Vec::new();
        self.fs.fs.lock().scan_dir(self.ino, |block, position, ino, name| {
            if name != b"." && name != b".." {
                raw.push((block, position, ino, String::from_utf8_lossy(name).into_owned()));
            }
            false
        })?;
        let mut fs = self.fs.fs.lock();
        for (block, position, ino, name) in raw {
            // Com o recurso filetype, o tipo está na própria entrada.
            let code = if filetype { fs.block(block)?[position + 7] } else { Fs::type_code(fs.read_inode(ino)?.mode()) };
            let dtype = match code {
                1 => crate::vfs::DT_REG,
                2 => crate::vfs::DT_DIR,
                3 => crate::vfs::DT_CHR,
                5 => crate::vfs::DT_FIFO,
                7 => crate::vfs::DT_LNK,
                _ => 0,
            };
            entries.push(DirEntry { name, ino: ino as u64, dtype });
        }
        Ok(entries)
    }

    fn create(&self, name: &str, node: NewNode, permissions: u32) -> Result<NodeRef, Errno> {
        self.fs.writable()?;
        let ino = self.fs.fs.lock().create(self.ino, name, &node, permissions)?;
        Ok(self.node(ino))
    }

    fn link(&self, name: &str, target: &NodeRef) -> Result<(), Errno> {
        self.fs.writable()?;
        let target = self.same_fs(target)?;
        let mut fs = self.fs.fs.lock();
        let mode = fs.read_inode(target)?.mode();
        fs.adjust_links(target, 1)?;
        fs.add_entry(self.ino, name, target, mode)
    }

    fn unlink(&self, name: &str, directory: bool) -> Result<(), Errno> {
        self.fs.writable()?;
        self.fs.fs.lock().unlink(self.ino, name, directory)
    }

    fn rename(&self, name: &str, new_dir: &NodeRef, new_name: &str) -> Result<(), Errno> {
        self.fs.writable()?;
        let new_dir = self.same_fs(new_dir)?;
        self.fs.fs.lock().rename(self.ino, name, new_dir, new_name)
    }

    fn readlink(&self) -> Result<String, Errno> {
        let inode = self.inode()?;
        if inode.file_type() != S_IFLNK {
            return Err(EINVAL);
        }
        let size = inode.size() as usize;
        let bytes = if inode.is_fast_symlink() {
            inode.0[40..40 + size].to_vec()
        } else {
            let mut data = vec![0; size];
            let n = self.fs.fs.lock().read(self.ino, 0, &mut data)?;
            data.truncate(n);
            data
        };
        String::from_utf8(bytes).map_err(|_| EINVAL)
    }

    fn unmount(&self) {
        if self.fs.read_only {
            return;
        }
        let mut fs = self.fs.fs.lock();
        let state = u16_at(&fs.superblock, 58) | STATE_CLEAN;
        put_u16(&mut fs.superblock, 58, state);
        put_u32(&mut fs.superblock, 48, rtc::now() as u32); // s_wtime
        let _ = fs.write_superblock();
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}
