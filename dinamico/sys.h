/* sys.h — chamadas de sistema sem libc, para o carregador dinâmico e os
 * exemplos de biblioteca compartilhada (que não podem depender de uma libc). */
#pragma once

typedef unsigned long u64;
typedef long i64;

static inline i64 sc(i64 n, i64 a, i64 b, i64 c, i64 d, i64 e, i64 f) {
    register i64 r10 __asm__("r10") = d;
    register i64 r8 __asm__("r8") = e;
    register i64 r9 __asm__("r9") = f;
    i64 r;
    __asm__ volatile("syscall" : "=a"(r) : "a"(n), "D"(a), "S"(b), "d"(c), "r"(r10), "r"(r8), "r"(r9)
                     : "rcx", "r11", "memory");
    return r;
}

static inline i64 sys_read(int fd, void *b, u64 n) { return sc(0, fd, (i64)b, n, 0, 0, 0); }
static inline i64 sys_write(int fd, const void *b, u64 n) { return sc(1, fd, (i64)b, n, 0, 0, 0); }
static inline i64 sys_open(const char *c, int f) { return sc(2, (i64)c, f, 0, 0, 0, 0); }
static inline i64 sys_close(int fd) { return sc(3, fd, 0, 0, 0, 0, 0); }
static inline i64 sys_lseek(int fd, i64 o, int w) { return sc(8, fd, o, w, 0, 0, 0); }
static inline i64 sys_mmap(void *a, u64 n, int p, int f, int fd, i64 o) { return sc(9, (i64)a, n, p, f, fd, o); }
static inline i64 sys_mprotect(void *a, u64 n, int p) { return sc(10, (i64)a, n, p, 0, 0, 0); }
static inline __attribute__((noreturn)) void sys_exit(int c) {
    sc(231, c, 0, 0, 0, 0, 0);
    __builtin_unreachable();
}

static inline u64 tamanho(const char *s) {
    u64 n = 0;
    while (s[n]) n++;
    return n;
}

static inline void escrever(const char *s) { sys_write(1, s, tamanho(s)); }

/* Um número em hexadecimal, para mostrar endereços. */
static inline void escrever_hex(u64 v) {
    char b[19] = "0x";
    for (int i = 0; i < 16; i++) b[2 + i] = "0123456789abcdef"[(v >> (60 - 4 * i)) & 15];
    b[18] = 0;
    escrever(b);
}

static inline void escrever_numero(i64 v) {
    char b[24];
    int i = 23;
    int negativo = v < 0;
    u64 u = negativo ? -(u64)v : (u64)v;
    b[i] = 0;
    do b[--i] = '0' + u % 10; while (u /= 10);
    if (negativo) b[--i] = '-';
    escrever(b + i);
}
