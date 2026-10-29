/* dinamico — um programa ligado a uma biblioteca compartilhada.
 *
 * Ele é um PIE (vai para um endereço sorteado) e pede o carregador
 * /lib/ld-ferrugem.so (PT_INTERP) e a biblioteca libsaudacao.so (DT_NEEDED).
 * Nada de libc: só chamadas de sistema (sys.h). */
#include "sys.h"

extern int contador;
int soma(int a, int b);
const char *cor(int i);
void incrementar(void);
void saudar(const char *nome);

int main(int argc, char **argv) {
    saudar(argc > 1 ? argv[1] : "mundo");
    escrever("programa em ");
    escrever_hex((u64)&main);
    escrever("\nsoma(2, 3) = ");
    escrever_numero(soma(2, 3));
    escrever("\ncor(1) = ");
    escrever(cor(1));
    escrever("\ncontador = ");
    escrever_numero(contador);
    incrementar();
    escrever(" -> ");
    escrever_numero(contador);
    escrever("\n");
    return contador == 42 ? 0 : 1;
}

__asm__(".text\n"
        ".global _start\n"
        "_start:\n"
        "  mov (%rsp), %rdi\n"   /* argc */
        "  lea 8(%rsp), %rsi\n"  /* argv */
        "  and $-16, %rsp\n"
        "  call main\n"
        "  mov %eax, %edi\n"
        "  mov $231, %eax\n"     /* exit_group */
        "  syscall\n");
