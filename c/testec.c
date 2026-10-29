/* testec.c — testa a libc (musl) rodando no FerrugemOS: stdio, malloc
 * grande (mmap), arquivos, processos, pipes, sinais, tempo e números
 * de ponto flutuante. Tudo com as funções padrão do C/POSIX. */
#include <errno.h>
#include <fcntl.h>
#include <math.h>
#include <signal.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/stat.h>
#include <sys/time.h>
#include <sys/wait.h>
#include <time.h>
#include <unistd.h>

static int falhas = 0;
#define CONFERIR(cond, nome) do { \
    if (cond) printf("  [ OK ] %s\n", nome); \
    else { printf("  [FALHOU] %s (errno %d: %s)\n", nome, errno, strerror(errno)); falhas++; } \
} while (0)

static volatile sig_atomic_t recebido = 0;
static void ao_receber(int s) { recebido = s; }

static int compara(const void *a, const void *b) { return *(const int *)a - *(const int *)b; }

int main(void) {
    printf("Testando a libc em C...\n");

    /* malloc pequeno e grande (o grande a musl pede com mmap) */
    char *pequeno = malloc(100);
    size_t n = 8 * 1024 * 1024;
    unsigned char *grande = malloc(n);
    for (size_t i = 0; i < n; i++) grande[i] = (unsigned char)(i * 7);
    unsigned long soma = 0;
    for (size_t i = 0; i < n; i++) soma += grande[i];
    CONFERIR(pequeno && grande && soma == 1069547520UL, "malloc de 100 bytes e de 8 MiB");
    free(grande);
    free(pequeno);

    /* ponto flutuante (registradores SSE) */
    double r = sqrt(2.0) * sin(1.0) + pow(2.0, 10);
    char texto[64];
    snprintf(texto, sizeof texto, "%.6f", r);
    CONFERIR(strcmp(texto, "1025.190020") == 0, "sqrt, sin, pow e printf com double");
    CONFERIR(strtod("3.25e2", NULL) == 325.0, "strtod");

    int numeros[] = {5, 3, 9, 1, 7};
    qsort(numeros, 5, sizeof(int), compara);
    CONFERIR(numeros[0] == 1 && numeros[4] == 9, "qsort");

    /* arquivos com stdio */
    FILE *f = fopen("/tmp/testec.txt", "w");
    CONFERIR(f != NULL, "fopen para escrita");
    fprintf(f, "linha %d\nlinha %d\n", 1, 2);
    fclose(f);
    f = fopen("/tmp/testec.txt", "r");
    char linha[32];
    int linhas = 0;
    while (fgets(linha, sizeof linha, f)) linhas++;
    fclose(f);
    CONFERIR(linhas == 2, "fgets lê o que fprintf escreveu");
    struct stat st;
    CONFERIR(stat("/tmp/testec.txt", &st) == 0 && st.st_size == 16 && S_ISREG(st.st_mode), "stat");
    CONFERIR(access("/tmp/testec.txt", R_OK | W_OK) == 0, "access");
    CONFERIR(unlink("/tmp/testec.txt") == 0 && access("/tmp/testec.txt", F_OK) == -1 && errno == ENOENT,
             "unlink e errno = ENOENT");

    /* processos e pipe */
    int fds[2];
    CONFERIR(pipe(fds) == 0, "pipe");
    pid_t pid = fork();
    if (pid == 0) {
        close(fds[0]);
        dup2(fds[1], 1);
        execl("/bin/echo", "echo", "o filho falou", (char *)NULL);
        _exit(127);
    }
    close(fds[1]);
    char buf[64] = {0};
    ssize_t lidos = read(fds[0], buf, sizeof buf - 1);
    close(fds[0]);
    int status;
    waitpid(pid, &status, 0);
    CONFERIR(lidos > 0 && strcmp(buf, "o filho falou\n") == 0 && WIFEXITED(status) && WEXITSTATUS(status) == 0,
             "fork + execl + pipe + waitpid");

    /* system() e popen(): a musl cria o filho com clone() */
    CONFERIR(system("exit 3") == 3 << 8, "system() devolve o código de saída");
    FILE *p = popen("echo pelo popen", "r");
    char saida[64] = {0};
    if (p) {
        fgets(saida, sizeof saida, p);
        pclose(p);
    }
    CONFERIR(strcmp(saida, "pelo popen\n") == 0, "popen() lê a saída de outro programa");

    /* sinais pela libc */
    signal(SIGUSR1, ao_receber);
    raise(SIGUSR1);
    CONFERIR(recebido == SIGUSR1, "signal + raise");

    /* tempo */
    time_t agora = time(NULL);
    struct tm *tm = gmtime(&agora);
    CONFERIR(tm->tm_year + 1900 >= 2024, "time + gmtime");
    struct timespec antes, depois;
    clock_gettime(CLOCK_MONOTONIC, &antes);
    usleep(100000);
    clock_gettime(CLOCK_MONOTONIC, &depois);
    long ms = (depois.tv_sec - antes.tv_sec) * 1000 + (depois.tv_nsec - antes.tv_nsec) / 1000000;
    CONFERIR(ms >= 100 && ms < 200, "usleep de 100 ms");

    char data[32];
    strftime(data, sizeof data, "%Y-%m-%d %H:%M", tm);
    printf("Data (UTC): %s\n", data);

    if (falhas == 0) printf("Todos os testes da libc passaram.\n");
    else printf("%d testes da libc falharam.\n", falhas);
    return falhas;
}
