//! Um motor de JavaScript escrito do zero: o texto vira fichas (`lexico`), a
//! árvore com os escopos (`sintaxe`), bytecode (`compilador`), e uma máquina
//! virtual roda o bytecode (`vm`, `interprete`), com coletor de lixo e os
//! objetos embutidos da linguagem (`nativos`, `nativos2`, `regex`).
//!
//! Só depende de `alloc`: os testes rápidos rodam também no Linux.

pub mod compilador;
pub mod interprete;
pub mod lexico;
pub mod nativos;
pub mod nativos2;
pub mod regex;
pub mod sintaxe;
pub mod vm;

pub use interprete::{relatar_erro, rodar_microtarefas};
pub use vm::{Res, Valor, Vm};
