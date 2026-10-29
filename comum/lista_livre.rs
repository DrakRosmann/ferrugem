//! Alocador de memória por lista livre (first-fit, juntando blocos vizinhos).
//!
//! Usado tanto pelo heap do kernel quanto pelo `malloc` dos programas de
//! usuário (este arquivo é incluído pelos dois). A memória livre é uma lista
//! ligada, em ordem de endereço, de blocos `[tamanho | próximo]` guardados
//! dentro da própria memória livre: o alocador não precisa de memória extra.

use core::alloc::Layout;
use core::mem::size_of;
use core::ptr::null_mut;

struct Bloco {
    tamanho: usize,
    proximo: *mut Bloco,
}

/// Todo bloco tem pelo menos este tamanho e começa alinhado a ele (16 bytes).
const MINIMO: usize = size_of::<Bloco>();

pub struct ListaLivre {
    cabeca: *mut Bloco,
    livre: usize,
}

// A lista só é usada por trás de um lock (kernel) ou num programa de uma thread só.
unsafe impl Send for ListaLivre {}

const fn arredondar(valor: usize, alinhamento: usize) -> usize {
    (valor + alinhamento - 1) & !(alinhamento - 1)
}

fn tamanho_real(layout: &Layout) -> usize {
    arredondar(layout.size().max(1), MINIMO)
}

impl ListaLivre {
    pub const fn new() -> Self {
        Self { cabeca: null_mut(), livre: 0 }
    }

    /// Bytes livres no momento.
    #[allow(dead_code)]
    pub fn livre(&self) -> usize {
        self.livre
    }

    /// Doa uma região de memória para o alocador.
    ///
    /// # Safety
    /// A região precisa ser memória válida que ninguém mais usa.
    pub unsafe fn adicionar(&mut self, inicio: usize, tamanho: usize) {
        let comeco = arredondar(inicio, MINIMO);
        if inicio + tamanho < comeco + MINIMO {
            return;
        }
        let tamanho = (inicio + tamanho - comeco) & !(MINIMO - 1);
        unsafe { self.inserir(comeco, tamanho) }
    }

    /// Põe um bloco na lista, na posição certa, e junta com os vizinhos colados nele.
    unsafe fn inserir(&mut self, inicio: usize, tamanho: usize) {
        self.livre += tamanho;
        unsafe {
            let mut anterior: *mut Bloco = null_mut();
            let mut atual = self.cabeca;
            while !atual.is_null() && (atual as usize) < inicio {
                anterior = atual;
                atual = (*atual).proximo;
            }

            let novo = inicio as *mut Bloco;
            novo.write(Bloco { tamanho, proximo: atual });

            if !atual.is_null() && inicio + tamanho == atual as usize {
                (*novo).tamanho += (*atual).tamanho;
                (*novo).proximo = (*atual).proximo;
            }

            if anterior.is_null() {
                self.cabeca = novo;
            } else if anterior as usize + (*anterior).tamanho == inicio {
                (*anterior).tamanho += (*novo).tamanho;
                (*anterior).proximo = (*novo).proximo;
            } else {
                (*anterior).proximo = novo;
            }
        }
    }

    /// Procura o primeiro bloco onde a alocação cabe (já alinhada) e o divide.
    /// Devolve nulo se não houver espaço.
    pub fn alocar(&mut self, layout: Layout) -> *mut u8 {
        let tamanho = tamanho_real(&layout);
        let alinhamento = layout.align().max(MINIMO);
        unsafe {
            // `elo` aponta para o ponteiro que aponta para o bloco atual: assim
            // dá para tirar o bloco da lista sem guardar o anterior.
            let mut elo: *mut *mut Bloco = &raw mut self.cabeca;
            while !(*elo).is_null() {
                let bloco = *elo;
                let inicio_bloco = bloco as usize;
                let fim_bloco = inicio_bloco + (*bloco).tamanho;

                let mut inicio = arredondar(inicio_bloco, alinhamento);
                // A sobra na frente precisa caber um bloco, ou ela se perderia.
                if inicio != inicio_bloco && inicio - inicio_bloco < MINIMO {
                    inicio = arredondar(inicio_bloco + MINIMO, alinhamento);
                }

                if inicio + tamanho <= fim_bloco {
                    *elo = (*bloco).proximo;
                    self.livre -= fim_bloco - inicio_bloco;
                    if fim_bloco > inicio + tamanho {
                        self.inserir(inicio + tamanho, fim_bloco - inicio - tamanho);
                    }
                    if inicio > inicio_bloco {
                        self.inserir(inicio_bloco, inicio - inicio_bloco);
                    }
                    return inicio as *mut u8;
                }
                elo = &raw mut (*bloco).proximo;
            }
        }
        null_mut()
    }

    /// # Safety
    /// `ptr` precisa ter vindo de `alocar` com o mesmo `layout`.
    pub unsafe fn liberar(&mut self, ptr: *mut u8, layout: Layout) {
        unsafe { self.inserir(ptr as usize, tamanho_real(&layout)) }
    }
}
