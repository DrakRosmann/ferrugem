/* ola.c — um programa em C comum, compilado com a musl (a libc do Alpine Linux).
 * Nada aqui sabe que está rodando no FerrugemOS: só usa a libc padrão. */
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <unistd.h>
#include <sys/utsname.h>

int main(int argc, char **argv) {
    struct utsname u;
    uname(&u);
    printf("Olá do C! Rodando em %s %s (%s), pid %d\n", u.sysname, u.release, u.machine, getpid());
    printf("argc = %d:", argc);
    for (int i = 0; i < argc; i++) printf(" [%s]", argv[i]);
    printf("\nHOME = %s, PATH = %s\n", getenv("HOME") ? getenv("HOME") : "(nada)", getenv("PATH") ? getenv("PATH") : "(nada)");

    char *texto = malloc(64);
    snprintf(texto, 64, "malloc e snprintf funcionam: %.3f", 3.14159);
    puts(texto);
    free(texto);
    return 0;
}
