//! ext2: o sistema de arquivos clássico do Linux, com leitura e escrita.
//!
//! O disco é dividido em blocos (aqui, de 1 KiB) e os blocos em *grupos*.
//! Cada grupo tem um bitmap de blocos livres, um bitmap de inodes livres e
//! uma tabela de inodes. Um **inode** descreve um arquivo: tipo, tamanho,
//! número de links e onde estão os dados:
//!
//! ```text
//!   i_block[0..12]  12 blocos diretos
//!   i_block[12]     um bloco cheio de números de blocos       (indireto)
//!   i_block[13]     um bloco de blocos de números de blocos   (duplo indireto)
//! ```
//!
//! Um **diretório** é um arquivo cujo conteúdo é uma lista de entradas
//! `[inode | tamanho do registro | tamanho do nome | tipo | nome]`.
//! O inode 2 é sempre a raiz.
//!
//! Como o formato é o mesmo do Linux, dá para criar o disco com `mkfs.ext2`,
//! conferir com `e2fsck` e montar no Linux para ver o que o FerrugemOS gravou.

use crate::ata;
use crate::rtc;
use crate::erro::*;
use crate::spin::Mutex;
use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::sync::Arc;
use alloc::vec;
use alloc::vec::Vec;

pub const RAIZ: u32 = 2;

const MODO_DIRETORIO: u16 = 0x4000;
const MODO_ARQUIVO: u16 = 0x8000;
const TIPO_ENTRADA_ARQUIVO: u8 = 1;
const TIPO_ENTRADA_DIRETORIO: u8 = 2;
const MAX_CACHE: usize = 512;

fn rd16(b: &[u8], o: usize) -> u16 {
    u16::from_le_bytes([b[o], b[o + 1]])
}
fn rd32(b: &[u8], o: usize) -> u32 {
    u32::from_le_bytes(b[o..o + 4].try_into().unwrap())
}
fn wr16(b: &mut [u8], o: usize, v: u16) {
    b[o..o + 2].copy_from_slice(&v.to_le_bytes());
}
fn wr32(b: &mut [u8], o: usize, v: u32) {
    b[o..o + 4].copy_from_slice(&v.to_le_bytes());
}

/// Os 128 primeiros bytes de um inode (o resto, se houver, não usamos).
#[derive(Clone)]
struct Inode([u8; 128]);

impl Inode {
    fn modo(&self) -> u16 {
        rd16(&self.0, 0)
    }
    fn eh_diretorio(&self) -> bool {
        self.modo() & 0xF000 == MODO_DIRETORIO
    }
    fn tamanho(&self) -> u64 {
        rd32(&self.0, 4) as u64
    }
    fn definir_tamanho(&mut self, t: u64) {
        wr32(&mut self.0, 4, t as u32);
    }
    fn links(&self) -> u16 {
        rd16(&self.0, 26)
    }
    fn definir_links(&mut self, n: u16) {
        wr16(&mut self.0, 26, n);
    }
    /// Em setores de 512 bytes, contando também os blocos de índice.
    fn somar_setores(&mut self, n: i64) {
        let atual = rd32(&self.0, 28) as i64;
        wr32(&mut self.0, 28, (atual + n) as u32);
    }
    /// Marca a hora de acesso, de mudança e de modificação.
    fn tocar(&mut self) {
        let agora = rtc::agora() as u32;
        for deslocamento in [8, 12, 16] {
            wr32(&mut self.0, deslocamento, agora);
        }
    }
    fn modificado_em(&self) -> u64 {
        rd32(&self.0, 16) as u64
    }
    fn info(&self) -> InfoInode {
        InfoInode {
            diretorio: self.eh_diretorio(),
            tamanho: self.tamanho(),
            modificado: self.modificado_em(),
            permissoes: (self.modo() & 0o7777) as u32,
            uid: rd16(&self.0, 2) as u32,
            gid: rd16(&self.0, 24) as u32,
        }
    }
    fn bloco(&self, i: usize) -> u32 {
        rd32(&self.0, 40 + i * 4)
    }
    fn definir_bloco(&mut self, i: usize, b: u32) {
        wr32(&mut self.0, 40 + i * 4, b);
    }
}

/// O que o VFS precisa saber de um inode.
pub struct InfoInode {
    pub diretorio: bool,
    pub tamanho: u64,
    pub modificado: u64,
    pub permissoes: u32,
    pub uid: u32,
    pub gid: u32,
}

struct Grupo {
    mapa_blocos: u32,
    mapa_inodes: u32,
    tabela_inodes: u32,
    blocos_livres: u16,
    inodes_livres: u16,
    diretorios: u16,
    sujo: bool,
}

struct Estado {
    tamanho_bloco: usize,
    primeiro_bloco: u32,
    blocos_por_grupo: u32,
    inodes_por_grupo: u32,
    tamanho_inode: usize,
    total_blocos: u32,
    superbloco: Vec<u8>,
    superbloco_sujo: bool,
    grupos: Vec<Grupo>,
    cache: BTreeMap<u32, Vec<u8>>,
}

pub struct Ext2 {
    estado: Mutex<Estado>,
}

/// Uma entrada de diretório, com onde ela está (para poder alterá-la).
struct Entrada {
    inode: u32,
    nome: String,
    tipo: u8,
}

impl Ext2 {
    /// Lê o superbloco e a tabela de grupos do disco ATA.
    pub fn montar() -> Result<Arc<Ext2>, &'static str> {
        let mut superbloco = vec![0u8; 1024];
        ata::ler(2, &mut superbloco).map_err(|_| "erro lendo o superbloco")?;
        if rd16(&superbloco, 56) != 0xEF53 {
            return Err("o disco não tem um sistema ext2");
        }
        // Só aceitamos recursos que sabemos gravar sem estragar nada.
        let incompativeis = rd32(&superbloco, 96);
        let so_leitura = rd32(&superbloco, 100);
        if incompativeis & !0x2 != 0 || so_leitura & !0x3 != 0 {
            return Err("o ext2 usa recursos que não suportamos (crie com `make disco`)");
        }
        let revisao = rd32(&superbloco, 76);
        let mut e = Estado {
            tamanho_bloco: 1024 << rd32(&superbloco, 24),
            primeiro_bloco: rd32(&superbloco, 20),
            blocos_por_grupo: rd32(&superbloco, 32),
            inodes_por_grupo: rd32(&superbloco, 40),
            tamanho_inode: if revisao >= 1 { rd16(&superbloco, 88) as usize } else { 128 },
            total_blocos: rd32(&superbloco, 4),
            superbloco,
            superbloco_sujo: false,
            grupos: Vec::new(),
            cache: BTreeMap::new(),
        };
        let quantos = (e.total_blocos - e.primeiro_bloco).div_ceil(e.blocos_por_grupo);
        for g in 0..quantos as usize {
            let (bloco, deslocamento) = e.posicao_grupo(g);
            let dados = e.ler_bloco(bloco).map_err(|_| "erro lendo a tabela de grupos")?;
            let d = &dados[deslocamento..deslocamento + 32];
            e.grupos.push(Grupo {
                mapa_blocos: rd32(d, 0),
                mapa_inodes: rd32(d, 4),
                tabela_inodes: rd32(d, 8),
                blocos_livres: rd16(d, 12),
                inodes_livres: rd16(d, 14),
                diretorios: rd16(d, 16),
                sujo: false,
            });
        }
        Ok(Arc::new(Ext2 { estado: Mutex::new(e) }))
    }

    /// Roda uma operação e grava no disco os metadados que ela mudou.
    fn operar<T>(&self, f: impl FnOnce(&mut Estado) -> Result<T, i64>) -> Result<T, i64> {
        let mut e = self.estado.lock();
        let resultado = f(&mut e);
        e.sincronizar()?;
        resultado
    }

    pub fn buscar(&self, dir: u32, nome: &str) -> Result<u32, i64> {
        self.operar(|e| e.buscar(dir, nome))
    }

    pub fn info(&self, inode: u32) -> Result<InfoInode, i64> {
        self.operar(|e| e.ler_inode(inode).map(|i| i.info()))
    }

    /// `chmod`/`chown`: troca as permissões e/ou o dono.
    pub fn alterar(&self, inode: u32, permissoes: Option<u32>, uid: Option<u32>, gid: Option<u32>) -> Result<(), i64> {
        self.operar(|e| {
            let mut i = e.ler_inode(inode)?;
            if let Some(p) = permissoes {
                let modo = (i.modo() & 0xF000) | (p & 0o7777) as u16;
                wr16(&mut i.0, 0, modo);
            }
            if let Some(u) = uid {
                wr16(&mut i.0, 2, u as u16);
            }
            if let Some(g) = gid {
                wr16(&mut i.0, 24, g as u16);
            }
            wr32(&mut i.0, 12, rtc::agora() as u32); // ctime: o inode mudou
            e.escrever_inode(inode, &i)
        })
    }

    pub fn ler(&self, inode: u32, posicao: u64, buf: &mut [u8]) -> Result<usize, i64> {
        self.operar(|e| e.ler_dados(inode, posicao, buf))
    }

    pub fn escrever(&self, inode: u32, posicao: u64, dados: &[u8]) -> Result<usize, i64> {
        self.operar(|e| e.escrever_dados(inode, posicao, dados))
    }

    pub fn truncar(&self, inode: u32) -> Result<(), i64> {
        self.operar(|e| {
            let mut i = e.ler_inode(inode)?;
            e.liberar_dados(&mut i)?;
            i.tocar();
            e.escrever_inode(inode, &i)
        })
    }

    /// (nome, inode, é diretório?) de cada entrada, sem `.` e `..`.
    pub fn listar(&self, dir: u32) -> Result<Vec<(String, u32, bool)>, i64> {
        self.operar(|e| {
            Ok(e.entradas(dir)?
                .into_iter()
                .filter(|x| x.nome != "." && x.nome != "..")
                .map(|x| (x.nome, x.inode, x.tipo == TIPO_ENTRADA_DIRETORIO))
                .collect())
        })
    }

    pub fn criar(&self, dir: u32, nome: &str, diretorio: bool, permissoes: u32, uid: u32, gid: u32) -> Result<u32, i64> {
        self.operar(|e| {
            let numero = e.criar(dir, nome, diretorio)?;
            let mut i = e.ler_inode(numero)?;
            let modo = (i.modo() & 0xF000) | (permissoes & 0o7777) as u16;
            wr16(&mut i.0, 0, modo);
            wr16(&mut i.0, 2, uid as u16);
            wr16(&mut i.0, 24, gid as u16);
            e.escrever_inode(numero, &i)?;
            Ok(numero)
        })
    }

    pub fn remover(&self, dir: u32, nome: &str, diretorio: bool) -> Result<(), i64> {
        self.operar(|e| e.remover(dir, nome, diretorio))
    }

    pub fn renomear(&self, dir_de: u32, nome_de: &str, dir_para: u32, nome_para: &str) -> Result<(), i64> {
        self.operar(|e| e.renomear(dir_de, nome_de, dir_para, nome_para))
    }

    /// (blocos no total, blocos livres, tamanho do bloco)
    pub fn estatisticas(&self) -> (u64, u64, u64) {
        let e = self.estado.lock();
        (e.total_blocos as u64, rd32(&e.superbloco, 12) as u64, e.tamanho_bloco as u64)
    }
}

impl Estado {
    // ------------------------------------------------------------------ blocos

    fn setores_por_bloco(&self) -> u32 {
        (self.tamanho_bloco / 512) as u32
    }

    fn ler_bloco(&mut self, bloco: u32) -> Result<Vec<u8>, i64> {
        if let Some(dados) = self.cache.get(&bloco) {
            return Ok(dados.clone());
        }
        let mut dados = vec![0u8; self.tamanho_bloco];
        ata::ler(bloco * self.setores_por_bloco(), &mut dados)?;
        if self.cache.len() >= MAX_CACHE {
            self.cache.pop_first();
        }
        self.cache.insert(bloco, dados.clone());
        Ok(dados)
    }

    /// Grava direto no disco ("write-through"): o cache nunca tem nada pendente.
    fn escrever_bloco(&mut self, bloco: u32, dados: &[u8]) -> Result<(), i64> {
        ata::gravar(bloco * self.setores_por_bloco(), dados)?;
        if self.cache.len() >= MAX_CACHE {
            self.cache.pop_first();
        }
        self.cache.insert(bloco, dados.to_vec());
        Ok(())
    }

    fn posicao_grupo(&self, g: usize) -> (u32, usize) {
        let byte = g * 32;
        (self.primeiro_bloco + 1 + (byte / self.tamanho_bloco) as u32, byte % self.tamanho_bloco)
    }

    fn sincronizar(&mut self) -> Result<(), i64> {
        for g in 0..self.grupos.len() {
            if !self.grupos[g].sujo {
                continue;
            }
            let (bloco, o) = self.posicao_grupo(g);
            let mut dados = self.ler_bloco(bloco)?;
            let grupo = &self.grupos[g];
            wr16(&mut dados, o + 12, grupo.blocos_livres);
            wr16(&mut dados, o + 14, grupo.inodes_livres);
            wr16(&mut dados, o + 16, grupo.diretorios);
            self.escrever_bloco(bloco, &dados)?;
            self.grupos[g].sujo = false;
        }
        if self.superbloco_sujo {
            ata::gravar(2, &self.superbloco)?;
            self.superbloco_sujo = false;
        }
        ata::sincronizar()
    }

    fn somar_superbloco(&mut self, deslocamento: usize, n: i32) {
        let atual = rd32(&self.superbloco, deslocamento) as i64;
        wr32(&mut self.superbloco, deslocamento, (atual + n as i64) as u32);
        self.superbloco_sujo = true;
    }

    /// Procura um bit zero num bitmap, liga e devolve o índice.
    fn ocupar_bit(&mut self, bloco_mapa: u32, limite: u32) -> Result<Option<u32>, i64> {
        let mut mapa = self.ler_bloco(bloco_mapa)?;
        for i in 0..limite {
            let (byte, bit) = ((i / 8) as usize, i % 8);
            if mapa[byte] & (1 << bit) == 0 {
                mapa[byte] |= 1 << bit;
                self.escrever_bloco(bloco_mapa, &mapa)?;
                return Ok(Some(i));
            }
        }
        Ok(None)
    }

    fn soltar_bit(&mut self, bloco_mapa: u32, i: u32) -> Result<(), i64> {
        let mut mapa = self.ler_bloco(bloco_mapa)?;
        mapa[(i / 8) as usize] &= !(1 << (i % 8));
        self.escrever_bloco(bloco_mapa, &mapa)
    }

    fn alocar_bloco(&mut self) -> Result<u32, i64> {
        for g in 0..self.grupos.len() {
            if self.grupos[g].blocos_livres == 0 {
                continue;
            }
            let inicio = self.primeiro_bloco + g as u32 * self.blocos_por_grupo;
            let limite = self.blocos_por_grupo.min(self.total_blocos - inicio);
            if let Some(i) = self.ocupar_bit(self.grupos[g].mapa_blocos, limite)? {
                self.grupos[g].blocos_livres -= 1;
                self.grupos[g].sujo = true;
                self.somar_superbloco(12, -1);
                let bloco = inicio + i;
                let zeros = vec![0u8; self.tamanho_bloco];
                self.escrever_bloco(bloco, &zeros)?;
                return Ok(bloco);
            }
        }
        Err(ENOSPC)
    }

    fn liberar_bloco(&mut self, bloco: u32) -> Result<(), i64> {
        let relativo = bloco - self.primeiro_bloco;
        let g = (relativo / self.blocos_por_grupo) as usize;
        self.soltar_bit(self.grupos[g].mapa_blocos, relativo % self.blocos_por_grupo)?;
        self.grupos[g].blocos_livres += 1;
        self.grupos[g].sujo = true;
        self.somar_superbloco(12, 1);
        self.cache.remove(&bloco);
        Ok(())
    }

    // ------------------------------------------------------------------ inodes

    fn posicao_inode(&self, inode: u32) -> (u32, usize) {
        let g = ((inode - 1) / self.inodes_por_grupo) as usize;
        let byte = ((inode - 1) % self.inodes_por_grupo) as usize * self.tamanho_inode;
        (self.grupos[g].tabela_inodes + (byte / self.tamanho_bloco) as u32, byte % self.tamanho_bloco)
    }

    fn ler_inode(&mut self, inode: u32) -> Result<Inode, i64> {
        if inode == 0 || ((inode - 1) / self.inodes_por_grupo) as usize >= self.grupos.len() {
            return Err(EIO);
        }
        let (bloco, o) = self.posicao_inode(inode);
        let dados = self.ler_bloco(bloco)?;
        Ok(Inode(dados[o..o + 128].try_into().unwrap()))
    }

    fn escrever_inode(&mut self, inode: u32, valor: &Inode) -> Result<(), i64> {
        let (bloco, o) = self.posicao_inode(inode);
        let mut dados = self.ler_bloco(bloco)?;
        dados[o..o + 128].copy_from_slice(&valor.0);
        self.escrever_bloco(bloco, &dados)
    }

    fn alocar_inode(&mut self, diretorio: bool) -> Result<u32, i64> {
        for g in 0..self.grupos.len() {
            if self.grupos[g].inodes_livres == 0 {
                continue;
            }
            if let Some(i) = self.ocupar_bit(self.grupos[g].mapa_inodes, self.inodes_por_grupo)? {
                let grupo = &mut self.grupos[g];
                grupo.inodes_livres -= 1;
                grupo.diretorios += diretorio as u16;
                grupo.sujo = true;
                self.somar_superbloco(16, -1);
                return Ok(g as u32 * self.inodes_por_grupo + i + 1);
            }
        }
        Err(ENOSPC)
    }

    /// Apaga de vez: dados, inode e bit no bitmap.
    fn liberar_inode(&mut self, numero: u32, inode: &mut Inode) -> Result<(), i64> {
        let diretorio = inode.eh_diretorio();
        self.liberar_dados(inode)?;
        inode.definir_links(0);
        // dtime: a hora em que foi apagado. Precisa ser uma data de verdade, porque
        // valores pequenos o e2fsck interpreta como a lista de órfãos do ext3.
        wr32(&mut inode.0, 20, rtc::agora().max(1_000_000_000) as u32);
        self.escrever_inode(numero, inode)?;
        let g = ((numero - 1) / self.inodes_por_grupo) as usize;
        self.soltar_bit(self.grupos[g].mapa_inodes, (numero - 1) % self.inodes_por_grupo)?;
        let grupo = &mut self.grupos[g];
        grupo.inodes_livres += 1;
        grupo.diretorios -= diretorio as u16;
        grupo.sujo = true;
        self.somar_superbloco(16, 1);
        Ok(())
    }

    // ------------------------------------------------------- blocos de arquivo

    /// Número do bloco de disco que guarda o bloco `indice` do arquivo.
    /// 0 = buraco (nunca escrito). Com `alocar`, cria o que faltar.
    fn bloco_de(&mut self, inode: &mut Inode, indice: u32, alocar: bool) -> Result<u32, i64> {
        let por_bloco = (self.tamanho_bloco / 4) as u32;
        if indice < 12 {
            return self.ponteiro_no_inode(inode, indice as usize, alocar);
        }
        let i = indice - 12;
        if i < por_bloco {
            let tabela = self.ponteiro_no_inode(inode, 12, alocar)?;
            return if tabela == 0 { Ok(0) } else { self.ponteiro_na_tabela(inode, tabela, i, alocar) };
        }
        let i = i - por_bloco;
        if i < por_bloco * por_bloco {
            let t1 = self.ponteiro_no_inode(inode, 13, alocar)?;
            if t1 == 0 {
                return Ok(0);
            }
            let t2 = self.ponteiro_na_tabela(inode, t1, i / por_bloco, alocar)?;
            return if t2 == 0 { Ok(0) } else { self.ponteiro_na_tabela(inode, t2, i % por_bloco, alocar) };
        }
        Err(EFBIG)
    }

    fn ponteiro_no_inode(&mut self, inode: &mut Inode, i: usize, alocar: bool) -> Result<u32, i64> {
        let bloco = inode.bloco(i);
        if bloco != 0 || !alocar {
            return Ok(bloco);
        }
        let novo = self.alocar_bloco()?;
        inode.definir_bloco(i, novo);
        inode.somar_setores(self.setores_por_bloco() as i64);
        Ok(novo)
    }

    fn ponteiro_na_tabela(&mut self, inode: &mut Inode, tabela: u32, i: u32, alocar: bool) -> Result<u32, i64> {
        let mut dados = self.ler_bloco(tabela)?;
        let bloco = rd32(&dados, i as usize * 4);
        if bloco != 0 || !alocar {
            return Ok(bloco);
        }
        let novo = self.alocar_bloco()?;
        wr32(&mut dados, i as usize * 4, novo);
        self.escrever_bloco(tabela, &dados)?;
        inode.somar_setores(self.setores_por_bloco() as i64);
        Ok(novo)
    }

    /// Devolve todos os blocos do arquivo (inclusive os de índice).
    fn liberar_dados(&mut self, inode: &mut Inode) -> Result<(), i64> {
        for i in 0..15 {
            let bloco = inode.bloco(i);
            if bloco != 0 {
                match i {
                    0..=11 => self.liberar_bloco(bloco)?,
                    _ => self.liberar_tabela(bloco, i - 11)?,
                }
            }
            inode.definir_bloco(i, 0);
        }
        inode.definir_tamanho(0);
        wr32(&mut inode.0, 28, 0);
        Ok(())
    }

    fn liberar_tabela(&mut self, tabela: u32, nivel: usize) -> Result<(), i64> {
        let dados = self.ler_bloco(tabela)?;
        for i in 0..self.tamanho_bloco / 4 {
            let bloco = rd32(&dados, i * 4);
            if bloco != 0 {
                if nivel > 1 { self.liberar_tabela(bloco, nivel - 1)? } else { self.liberar_bloco(bloco)? }
            }
        }
        self.liberar_bloco(tabela)
    }

    fn ler_dados(&mut self, numero: u32, posicao: u64, buf: &mut [u8]) -> Result<usize, i64> {
        let mut inode = self.ler_inode(numero)?;
        let tamanho = inode.tamanho();
        if posicao >= tamanho {
            return Ok(0);
        }
        let total = buf.len().min((tamanho - posicao) as usize);
        let tb = self.tamanho_bloco as u64;
        let mut feito = 0;
        while feito < total {
            let p = posicao + feito as u64;
            let o = (p % tb) as usize;
            let n = (self.tamanho_bloco - o).min(total - feito);
            let bloco = self.bloco_de(&mut inode, (p / tb) as u32, false)?;
            if bloco == 0 {
                buf[feito..feito + n].fill(0);
            } else {
                let dados = self.ler_bloco(bloco)?;
                buf[feito..feito + n].copy_from_slice(&dados[o..o + n]);
            }
            feito += n;
        }
        Ok(total)
    }

    fn escrever_dados(&mut self, numero: u32, posicao: u64, dados: &[u8]) -> Result<usize, i64> {
        let mut inode = self.ler_inode(numero)?;
        let tb = self.tamanho_bloco as u64;
        let mut feito = 0;
        let mut erro = None;
        while feito < dados.len() {
            let p = posicao + feito as u64;
            let o = (p % tb) as usize;
            let n = (self.tamanho_bloco - o).min(dados.len() - feito);
            let bloco = match self.bloco_de(&mut inode, (p / tb) as u32, true) {
                Ok(b) => b,
                Err(e) => {
                    erro = Some(e); // disco cheio: grava o que deu
                    break;
                }
            };
            let mut conteudo = if n == self.tamanho_bloco { vec![0u8; n] } else { self.ler_bloco(bloco)? };
            conteudo[o..o + n].copy_from_slice(&dados[feito..feito + n]);
            self.escrever_bloco(bloco, &conteudo)?;
            feito += n;
        }
        if posicao + feito as u64 > inode.tamanho() {
            inode.definir_tamanho(posicao + feito as u64);
        }
        inode.tocar();
        self.escrever_inode(numero, &inode)?;
        match erro {
            Some(e) if feito == 0 => Err(e),
            _ => Ok(feito),
        }
    }

    // ------------------------------------------------------------- diretórios

    fn entradas(&mut self, dir: u32) -> Result<Vec<Entrada>, i64> {
        let mut inode = self.ler_inode(dir)?;
        if !inode.eh_diretorio() {
            return Err(ENOTDIR);
        }
        let mut lista = Vec::new();
        for indice in 0..(inode.tamanho() / self.tamanho_bloco as u64) as u32 {
            let bloco = self.bloco_de(&mut inode, indice, false)?;
            if bloco == 0 {
                continue;
            }
            let dados = self.ler_bloco(bloco)?;
            let mut o = 0;
            while o + 8 <= self.tamanho_bloco {
                let numero = rd32(&dados, o);
                let registro = rd16(&dados, o + 4) as usize;
                let tamanho_nome = dados[o + 6] as usize;
                if registro < 8 || o + registro > self.tamanho_bloco {
                    break; // diretório corrompido: para por aqui
                }
                if numero != 0 {
                    lista.push(Entrada {
                        inode: numero,
                        nome: String::from_utf8_lossy(&dados[o + 8..o + 8 + tamanho_nome]).into_owned(),
                        tipo: dados[o + 7],
                    });
                }
                o += registro;
            }
        }
        Ok(lista)
    }

    fn buscar(&mut self, dir: u32, nome: &str) -> Result<u32, i64> {
        self.entradas(dir)?.into_iter().find(|e| e.nome == nome).map(|e| e.inode).ok_or(ENOENT)
    }

    /// Acha espaço num bloco do diretório (sobra no fim de uma entrada, ou
    /// uma entrada vazia); se não houver, acrescenta um bloco novo.
    fn adicionar_entrada(&mut self, dir: u32, nome: &str, numero: u32, tipo: u8) -> Result<(), i64> {
        if nome.len() > 255 {
            return Err(ENAMETOOLONG);
        }
        let necessario = (8 + nome.len()).next_multiple_of(4);
        let escrever = |dados: &mut [u8], o: usize, registro: usize| {
            wr32(dados, o, numero);
            wr16(dados, o + 4, registro as u16);
            dados[o + 6] = nome.len() as u8;
            dados[o + 7] = tipo;
            dados[o + 8..o + 8 + nome.len()].copy_from_slice(nome.as_bytes());
        };

        let mut inode = self.ler_inode(dir)?;
        let blocos = (inode.tamanho() / self.tamanho_bloco as u64) as u32;
        for indice in 0..blocos {
            let bloco = self.bloco_de(&mut inode, indice, false)?;
            if bloco == 0 {
                continue;
            }
            let mut dados = self.ler_bloco(bloco)?;
            let mut o = 0;
            while o + 8 <= self.tamanho_bloco {
                let registro = rd16(&dados, o + 4) as usize;
                if registro < 8 {
                    break;
                }
                let usado = if rd32(&dados, o) == 0 { 0 } else { (8 + dados[o + 6] as usize).next_multiple_of(4) };
                if registro - usado >= necessario {
                    if usado == 0 {
                        escrever(&mut dados, o, registro);
                    } else {
                        wr16(&mut dados, o + 4, usado as u16);
                        escrever(&mut dados, o + usado, registro - usado);
                    }
                    return self.escrever_bloco(bloco, &dados);
                }
                o += registro;
            }
        }
        let bloco = self.bloco_de(&mut inode, blocos, true)?;
        let mut dados = vec![0u8; self.tamanho_bloco];
        escrever(&mut dados, 0, self.tamanho_bloco);
        self.escrever_bloco(bloco, &dados)?;
        inode.definir_tamanho(inode.tamanho() + self.tamanho_bloco as u64);
        self.escrever_inode(dir, &inode)
    }

    /// Tira a entrada: o registro anterior "engole" o espaço dela.
    fn remover_entrada(&mut self, dir: u32, nome: &str) -> Result<u32, i64> {
        let mut inode = self.ler_inode(dir)?;
        for indice in 0..(inode.tamanho() / self.tamanho_bloco as u64) as u32 {
            let bloco = self.bloco_de(&mut inode, indice, false)?;
            if bloco == 0 {
                continue;
            }
            let mut dados = self.ler_bloco(bloco)?;
            let mut o = 0;
            let mut anterior: Option<usize> = None;
            while o + 8 <= self.tamanho_bloco {
                let numero = rd32(&dados, o);
                let registro = rd16(&dados, o + 4) as usize;
                if registro < 8 {
                    break;
                }
                let tamanho_nome = dados[o + 6] as usize;
                if numero != 0 && &dados[o + 8..o + 8 + tamanho_nome] == nome.as_bytes() {
                    match anterior {
                        Some(a) => {
                            let novo = rd16(&dados, a + 4) + registro as u16;
                            wr16(&mut dados, a + 4, novo);
                        }
                        None => wr32(&mut dados, o, 0),
                    }
                    self.escrever_bloco(bloco, &dados)?;
                    return Ok(numero);
                }
                anterior = Some(o);
                o += registro;
            }
        }
        Err(ENOENT)
    }

    fn mudar_links(&mut self, numero: u32, delta: i32) -> Result<(), i64> {
        let mut inode = self.ler_inode(numero)?;
        inode.definir_links((inode.links() as i32 + delta) as u16);
        self.escrever_inode(numero, &inode)
    }

    fn criar(&mut self, dir: u32, nome: &str, diretorio: bool) -> Result<u32, i64> {
        if self.buscar(dir, nome).is_ok() {
            return Err(EEXIST);
        }
        let numero = self.alocar_inode(diretorio)?;
        let mut inode = Inode([0; 128]);
        inode.tocar();
        if diretorio {
            wr16(&mut inode.0, 0, MODO_DIRETORIO | 0o755);
            inode.definir_links(2); // a entrada no pai + o "." dele mesmo
            let bloco = self.bloco_de(&mut inode, 0, true)?;
            let mut dados = vec![0u8; self.tamanho_bloco];
            wr32(&mut dados, 0, numero);
            wr16(&mut dados, 4, 12);
            dados[6] = 1;
            dados[7] = TIPO_ENTRADA_DIRETORIO;
            dados[8] = b'.';
            wr32(&mut dados, 12, dir);
            wr16(&mut dados, 16, (self.tamanho_bloco - 12) as u16);
            dados[18] = 2;
            dados[19] = TIPO_ENTRADA_DIRETORIO;
            dados[20..22].copy_from_slice(b"..");
            self.escrever_bloco(bloco, &dados)?;
            inode.definir_tamanho(self.tamanho_bloco as u64);
        } else {
            wr16(&mut inode.0, 0, MODO_ARQUIVO | 0o644);
            inode.definir_links(1);
        }
        self.escrever_inode(numero, &inode)?;
        let tipo = if diretorio { TIPO_ENTRADA_DIRETORIO } else { TIPO_ENTRADA_ARQUIVO };
        self.adicionar_entrada(dir, nome, numero, tipo)?;
        if diretorio {
            self.mudar_links(dir, 1)?; // o ".." do filho aponta para o pai
        }
        Ok(numero)
    }

    fn remover(&mut self, dir: u32, nome: &str, diretorio: bool) -> Result<(), i64> {
        let numero = self.buscar(dir, nome)?;
        let mut inode = self.ler_inode(numero)?;
        match (inode.eh_diretorio(), diretorio) {
            (true, false) => return Err(EISDIR),
            (false, true) => return Err(ENOTDIR),
            (true, true) if self.entradas(numero)?.len() > 2 => return Err(ENOTEMPTY),
            _ => {}
        }
        self.remover_entrada(dir, nome)?;
        if inode.eh_diretorio() {
            self.mudar_links(dir, -1)?;
            return self.liberar_inode(numero, &mut inode);
        }
        // Um arquivo só some de verdade quando o último nome (link) for apagado.
        inode.definir_links(inode.links().saturating_sub(1));
        if inode.links() == 0 {
            self.liberar_inode(numero, &mut inode)
        } else {
            self.escrever_inode(numero, &inode)
        }
    }

    fn renomear(&mut self, dir_de: u32, nome_de: &str, dir_para: u32, nome_para: &str) -> Result<(), i64> {
        let numero = self.buscar(dir_de, nome_de)?;
        if let Ok(existente) = self.buscar(dir_para, nome_para) {
            if existente == numero {
                return Ok(());
            }
            self.remover(dir_para, nome_para, false)?; // substitui um arquivo
        }
        let diretorio = self.ler_inode(numero)?.eh_diretorio();
        let tipo = if diretorio { TIPO_ENTRADA_DIRETORIO } else { TIPO_ENTRADA_ARQUIVO };
        self.adicionar_entrada(dir_para, nome_para, numero, tipo)?;
        self.remover_entrada(dir_de, nome_de)?;
        if diretorio && dir_de != dir_para {
            // O ".." do diretório movido passa a apontar para o novo pai.
            self.remover_entrada(numero, "..")?;
            self.adicionar_entrada(numero, "..", dir_para, TIPO_ENTRADA_DIRETORIO)?;
            self.mudar_links(dir_de, -1)?;
            self.mudar_links(dir_para, 1)?;
        }
        Ok(())
    }
}
