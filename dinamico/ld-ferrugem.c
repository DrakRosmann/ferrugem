/* ld-ferrugem.so — o carregador dinâmico do FerrugemOS.
 *
 * Um programa ligado a bibliotecas compartilhadas não traz o código delas:
 * traz só uma lista ("preciso de libsaudacao.so", entradas DT_NEEDED) e
 * buracos ("aqui vai o endereço de soma", as relocações). O kernel vê, no
 * PT_INTERP do programa, que ele precisa deste carregador, carrega os dois e
 * começa por aqui. Nós então:
 *
 *   1. achamos o programa na memória (pelo vetor auxiliar: AT_PHDR, AT_ENTRY);
 *   2. carregamos cada biblioteca pedida, de /lib, numa base qualquer;
 *   3. preenchemos os buracos: para cada relocação, procuramos o símbolo
 *      (primeiro no programa, depois nas bibliotecas, na ordem) pela tabela
 *      hash do ELF e escrevemos o endereço;
 *   4. rodamos os inicializadores das bibliotecas e pulamos para o programa.
 *
 * Tudo antes de existir qualquer libc: por isso só chamadas de sistema, e
 * nenhum ponteiro em dado global (nós mesmos não somos relocados por ninguém).
 *
 * Com LD_TRACE_LOADED_OBJECTS=1 no ambiente, só lista as bibliotecas e sai
 * (é o que o `ldd` usa).
 */
#include "sys.h"

typedef unsigned int u32;
typedef unsigned short u16;
typedef unsigned char u8;

typedef struct { u32 tipo, flags; u64 deslocamento, virt, fisico, no_arquivo, na_memoria, alinhamento; } Phdr;
typedef struct { i64 tag; u64 valor; } Dyn;
typedef struct { u32 nome; u8 info, outro; u16 secao; u64 valor, tamanho; } Sym;
typedef struct { u64 onde, info; i64 adendo; } Rela;

enum { PT_LOAD = 1, PT_DYNAMIC = 2, PT_PHDR = 6 };
enum { DT_NULL = 0, DT_NEEDED = 1, DT_PLTRELSZ = 2, DT_HASH = 4, DT_STRTAB = 5, DT_SYMTAB = 6, DT_RELA = 7,
       DT_RELASZ = 8, DT_INIT = 12, DT_JMPREL = 23, DT_INIT_ARRAY = 25, DT_INIT_ARRAYSZ = 27 };
enum { R_64 = 1, R_COPY = 5, R_GLOB_DAT = 6, R_JUMP_SLOT = 7, R_RELATIVE = 8 };

#define MAX_OBJETOS 16

/* Um objeto carregado: o programa (índice 0) ou uma biblioteca. */
typedef struct {
    char nome[64];
    u64 base;
    Dyn *dinamico;
    const char *strings;
    Sym *simbolos;
    u32 *hash;
} Objeto;

static Objeto objetos[MAX_OBJETOS];
static int quantos;
static int listar; /* LD_TRACE_LOADED_OBJECTS */

static void falhar(const char *a, const char *b) {
    escrever("ld-ferrugem: ");
    escrever(a);
    escrever(b);
    escrever("\n");
    sys_exit(127);
}

static int iguais(const char *a, const char *b) {
    while (*a && *a == *b) a++, b++;
    return *a == *b;
}

static void copiar(void *d, const void *o, u64 n) {
    for (u64 i = 0; i < n; i++) ((u8 *)d)[i] = ((const u8 *)o)[i];
}

static u64 valor_dinamico(Objeto *o, i64 tag) {
    for (Dyn *d = o->dinamico; d->tag != DT_NULL; d++)
        if (d->tag == tag) return d->valor;
    return 0;
}

static void preparar(Objeto *o) {
    o->strings = (const char *)(o->base + valor_dinamico(o, DT_STRTAB));
    o->simbolos = (Sym *)(o->base + valor_dinamico(o, DT_SYMTAB));
    u64 h = valor_dinamico(o, DT_HASH);
    o->hash = h ? (u32 *)(o->base + h) : 0;
}

/* A função de hash do ELF (System V). */
static u32 hash_elf(const char *s) {
    u32 h = 0;
    while (*s) {
        h = (h << 4) + (u8)*s++;
        u32 g = h & 0xf0000000;
        if (g) h ^= g >> 24;
        h &= ~g;
    }
    return h;
}

/* Procura um símbolo *definido* no objeto, pela tabela hash: baldes e correntes. */
static Sym *procurar_em(Objeto *o, const char *nome) {
    if (!o->hash) return 0;
    u32 baldes = o->hash[0];
    u32 *balde = o->hash + 2, *corrente = balde + baldes;
    for (u32 i = balde[hash_elf(nome) % baldes]; i; i = corrente[i]) {
        Sym *s = &o->simbolos[i];
        if (s->secao != 0 && iguais(o->strings + s->nome, nome)) return s;
    }
    return 0;
}

/* O endereço de um símbolo, na ordem de busca global. `pular`: não olhar
 * esse objeto (uma relocação COPY procura o original, não a cópia). */
static u64 resolver(const char *nome, int fraco, Objeto *pular, u64 *tamanho_simbolo) {
    for (int i = 0; i < quantos; i++) {
        if (&objetos[i] == pular) continue;
        Sym *s = procurar_em(&objetos[i], nome);
        if (s) {
            if (tamanho_simbolo) *tamanho_simbolo = s->tamanho;
            return objetos[i].base + s->valor;
        }
    }
    if (!fraco) falhar("símbolo não encontrado: ", nome);
    return 0;
}

static void relocar_tabela(Objeto *o, Rela *r, u64 bytes) {
    for (u64 i = 0; i < bytes / sizeof(Rela); i++) {
        u32 tipo = (u32)r[i].info, indice = r[i].info >> 32;
        u64 *onde = (u64 *)(o->base + r[i].onde);
        Sym *s = &o->simbolos[indice];
        const char *nome = o->strings + s->nome;
        int fraco = (s->info >> 4) == 2;
        u64 tamanho_simbolo = 0;
        switch (tipo) {
        case R_RELATIVE: *onde = o->base + r[i].adendo; break;
        case R_64: *onde = resolver(nome, fraco, 0, 0) + r[i].adendo; break;
        case R_GLOB_DAT:
        case R_JUMP_SLOT: *onde = resolver(nome, fraco, 0, 0); break;
        case R_COPY: {
            u64 origem = resolver(nome, 0, o, &tamanho_simbolo);
            copiar(onde, (void *)origem, tamanho_simbolo);
            break;
        }
        default: falhar("relocação não suportada em ", o->nome);
        }
    }
}

static void relocar(Objeto *o) {
    relocar_tabela(o, (Rela *)(o->base + valor_dinamico(o, DT_RELA)), valor_dinamico(o, DT_RELASZ));
    relocar_tabela(o, (Rela *)(o->base + valor_dinamico(o, DT_JMPREL)), valor_dinamico(o, DT_PLTRELSZ));
}

/* Carrega /lib/<nome>: reserva o espaço todo, lê cada PT_LOAD para o lugar
 * dele e depois dá a cada segmento as permissões certas. */
static void carregar(const char *nome) {
    for (int i = 1; i < quantos; i++)
        if (iguais(objetos[i].nome, nome)) return; /* já carregada */
    if (quantos == MAX_OBJETOS) falhar("bibliotecas demais: ", nome);
    char caminho[80] = "/lib/";
    u64 n = tamanho(nome);
    if (n > 60) falhar("nome comprido demais: ", nome);
    copiar(caminho + 5, nome, n + 1);
    int fd = sys_open(caminho, 0);
    if (fd < 0) falhar("biblioteca não encontrada: ", caminho);

    u8 cabecalho[64];
    Phdr ph[16];
    if (sys_read(fd, cabecalho, 64) != 64 || *(u32 *)cabecalho != 0x464c457f || *(u16 *)(cabecalho + 16) != 3)
        falhar("não é uma biblioteca ELF: ", caminho);
    u16 nph = *(u16 *)(cabecalho + 56);
    if (nph > 16) falhar("cabeçalhos demais: ", caminho);
    sys_lseek(fd, *(u64 *)(cabecalho + 32), 0);
    sys_read(fd, ph, nph * sizeof(Phdr));

    u64 fim = 0;
    for (int i = 0; i < nph; i++)
        if (ph[i].tipo == PT_LOAD && ph[i].virt + ph[i].na_memoria > fim) fim = ph[i].virt + ph[i].na_memoria;
    i64 base = sys_mmap(0, (fim + 0xfff) & ~0xfffUL, 3, 0x22, -1, 0); /* MAP_PRIVATE | MAP_ANONYMOUS */
    if (base < 0) falhar("sem memória para ", caminho);

    Objeto *o = &objetos[quantos++];
    copiar(o->nome, nome, n + 1);
    o->base = base;
    for (int i = 0; i < nph; i++) {
        if (ph[i].tipo == PT_DYNAMIC) o->dinamico = (Dyn *)(base + ph[i].virt);
        if (ph[i].tipo != PT_LOAD) continue;
        sys_lseek(fd, ph[i].deslocamento, 0);
        if (sys_read(fd, (void *)(base + ph[i].virt), ph[i].no_arquivo) != (i64)ph[i].no_arquivo)
            falhar("erro lendo ", caminho);
    }
    sys_close(fd);
    if (!o->dinamico) falhar("sem PT_DYNAMIC: ", caminho);
    preparar(o);
    if (listar) {
        escrever("\t");
        escrever(nome);
        escrever(" => ");
        escrever(caminho);
        escrever(" (");
        escrever_hex(base);
        escrever(")\n");
    }
    /* As bibliotecas que ela pede também. */
    for (Dyn *d = o->dinamico; d->tag != DT_NULL; d++)
        if (d->tag == DT_NEEDED) carregar(o->strings + d->valor);
}

/* Depois de relocar: as permissões de cada segmento (código só leitura e execução...). */
static void proteger(Objeto *o) {
    u8 *cab = (u8 *)o->base;
    Phdr *ph = (Phdr *)(o->base + *(u64 *)(cab + 32));
    for (int i = 0; i < *(u16 *)(cab + 56); i++) {
        if (ph[i].tipo != PT_LOAD) continue;
        u64 inicio = (o->base + ph[i].virt) & ~0xfffUL;
        u64 fim = (o->base + ph[i].virt + ph[i].na_memoria + 0xfff) & ~0xfffUL;
        int prot = (ph[i].flags & 4 ? 1 : 0) | (ph[i].flags & 2 ? 2 : 0) | (ph[i].flags & 1 ? 4 : 0);
        sys_mprotect((void *)inicio, fim - inicio, prot);
    }
}

static void inicializar(Objeto *o) {
    u64 init = valor_dinamico(o, DT_INIT);
    if (init) ((void (*)(void))(o->base + init))();
    void (**lista)(void) = (void (**)(void))(o->base + valor_dinamico(o, DT_INIT_ARRAY));
    for (u64 i = 0; i < valor_dinamico(o, DT_INIT_ARRAYSZ) / 8; i++) lista[i]();
}

/* Chamada pelo _start com a pilha que o kernel montou; devolve onde pular. */
__attribute__((used)) static u64 comecar(u64 *pilha) {
    u64 argc = pilha[0];
    char **ambiente = (char **)(pilha + argc + 2);
    u64 *aux = pilha + argc + 2;
    while (*aux) aux++;
    aux++;
    u64 phdr = 0, nph = 0, entrada = 0;
    for (; aux[0]; aux += 2) {
        if (aux[0] == 3) phdr = aux[1];
        if (aux[0] == 5) nph = aux[1];
        if (aux[0] == 9) entrada = aux[1];
    }
    for (char **e = ambiente; *e; e++)
        if (iguais(*e, "LD_TRACE_LOADED_OBJECTS=1")) listar = 1;

    /* O programa: a base dele sai da diferença entre onde o PT_PHDR está e
     * onde ele diz que estaria (0 para um executável não-PIE). */
    Objeto *prog = &objetos[quantos++];
    copiar(prog->nome, "(programa)", 11);
    Phdr *ph = (Phdr *)phdr;
    for (u64 i = 0; i < nph; i++)
        if (ph[i].tipo == PT_PHDR) prog->base = phdr - ph[i].virt;
    for (u64 i = 0; i < nph; i++)
        if (ph[i].tipo == PT_DYNAMIC) prog->dinamico = (Dyn *)(prog->base + ph[i].virt);
    if (!prog->dinamico) falhar("o programa não é dinâmico", "");
    preparar(prog);
    for (Dyn *d = prog->dinamico; d->tag != DT_NULL; d++)
        if (d->tag == DT_NEEDED) carregar(prog->strings + d->valor);
    if (listar) sys_exit(0);

    /* As bibliotecas primeiro (de trás para a frente: quem é pedido por
     * outra fica pronto antes), o programa por último. */
    for (int i = quantos - 1; i >= 1; i--) relocar(&objetos[i]);
    relocar(prog);
    for (int i = quantos - 1; i >= 1; i--) proteger(&objetos[i]);
    for (int i = quantos - 1; i >= 1; i--) inicializar(&objetos[i]);
    return entrada;
}

/* O ponto de entrada: guarda a pilha original (o programa precisa dela
 * intacta: argc, argv, ambiente, vetor auxiliar) e pula para o programa. */
__asm__(".text\n"
        ".global _start\n"
        "_start:\n"
        "  mov %rsp, %rbx\n"
        "  mov %rsp, %rdi\n"
        "  and $-16, %rsp\n"
        "  call comecar\n"
        "  mov %rbx, %rsp\n"
        "  xor %edx, %edx\n" /* rdx: função de término (nenhuma) */
        "  jmp *%rax\n");
