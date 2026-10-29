/* libsaudacao.so — uma biblioteca compartilhada de exemplo.
 *
 * Ela fica em /lib e o programa `dinamico` a usa sem ter o código dela: o
 * carregador (ld-ferrugem.so) a põe na memória e liga as chamadas. Tem um
 * pouco de tudo que exige relocação: funções exportadas, uma variável
 * global exportada, uma tabela de ponteiros (relocações RELATIVE) e um
 * inicializador que roda antes do main do programa. */
#include "sys.h"

int contador = 40;                /* exportada: o programa lê e escreve */
static const char *cores[] = {"vermelho", "verde", "azul"};

static void __attribute__((constructor)) ao_carregar(void) {
    contador += 1; /* roda antes do programa: ele deve ver 41 */
}

int soma(int a, int b) { return a + b; }

const char *cor(int i) { return cores[i % 3]; }

void incrementar(void) { contador++; }

void saudar(const char *nome) {
    escrever("olá, ");
    escrever(nome);
    escrever(", da libsaudacao.so (carregada em ");
    escrever_hex((u64)&saudar);
    escrever(")\n");
}
