/* testec.c — testa a libc (musl) rodando no FerrugemOS: stdio, malloc
 * grande (mmap), arquivos, processos, pipes, sinais, tempo e números
 * de ponto flutuante. Tudo com as funções padrão do C/POSIX. */
#include <dirent.h>
#include <errno.h>
#include <poll.h>
#include <pthread.h>
#include <sys/syscall.h>
#include <sys/select.h>
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

static long agora_ms(void) {
    struct timespec t;
    clock_gettime(CLOCK_MONOTONIC, &t);
    return t.tv_sec * 1000 + t.tv_nsec / 1000000;
}

static void testar_poll_e_select(void) {
    int p[2];
    pipe(p);
    struct pollfd pf = { .fd = p[0], .events = POLLIN };
    long antes = agora_ms();
    int r = poll(&pf, 1, 100);
    long ms = agora_ms() - antes;
    CONFERIR(r == 0 && ms >= 100 && ms < 200, "poll sem dados espera o prazo (100 ms)");

    write(p[1], "x", 1);
    r = poll(&pf, 1, -1);
    CONFERIR(r == 1 && (pf.revents & POLLIN), "poll vê dados no pipe");

    fd_set conjunto;
    FD_ZERO(&conjunto);
    FD_SET(p[0], &conjunto);
    struct timeval zero = {0, 0};
    CONFERIR(select(p[0] + 1, &conjunto, NULL, NULL, &zero) == 1 && FD_ISSET(p[0], &conjunto), "select");

    char c;
    read(p[0], &c, 1);
    close(p[1]);
    pf.revents = 0;
    CONFERIR(poll(&pf, 1, 0) == 1 && (pf.revents & POLLHUP), "poll vê o fim do pipe (POLLHUP)");
    close(p[0]);

    /* poll dorme e acorda quando outro processo escreve */
    pipe(p);
    pid_t filho = fork();
    if (filho == 0) {
        usleep(50000);
        write(p[1], "y", 1);
        _exit(0);
    }
    pf.fd = p[0];
    pf.revents = 0;
    antes = agora_ms();
    r = poll(&pf, 1, 2000);
    ms = agora_ms() - antes;
    waitpid(filho, NULL, 0);
    CONFERIR(r == 1 && ms >= 50 && ms < 500, "poll acorda quando outro processo escreve");
    close(p[0]);
    close(p[1]);
}

/* ------------------------------------------------------------- threads */

#define THREADS 8
#define VOLTAS 20000
static pthread_mutex_t trava = PTHREAD_MUTEX_INITIALIZER;
static long contador = 0;
static __thread int meu_numero = -1; /* cada thread tem o seu */

static void *somar(void *arg) {
    meu_numero = (int)(long)arg;
    for (int i = 0; i < VOLTAS; i++) {
        pthread_mutex_lock(&trava);
        contador++;
        pthread_mutex_unlock(&trava);
        if (i % 1000 == 0) sched_yield();
    }
    /* devolve o próprio número, lido da variável por thread */
    return (void *)(long)(meu_numero * 10);
}

static pthread_mutex_t fila_trava = PTHREAD_MUTEX_INITIALIZER;
static pthread_cond_t fila_tem_item = PTHREAD_COND_INITIALIZER;
static int fila[64], fila_tamanho = 0, produzidos = 0;

static void *produzir(void *arg) {
    (void)arg;
    for (int i = 1; i <= 50; i++) {
        pthread_mutex_lock(&fila_trava);
        fila[fila_tamanho++ % 64] = i;
        produzidos++;
        pthread_cond_signal(&fila_tem_item);
        pthread_mutex_unlock(&fila_trava);
        if (i % 10 == 0) usleep(1000);
    }
    return NULL;
}

static void *so_sair(void *arg) {
    (void)arg;
    pthread_exit((void *)7);
}

static void *sair_com_5(void *arg) {
    (void)arg;
    usleep(20000);
    exit(5); /* exit_group: leva o processo inteiro, inclusive a thread principal */
}

static void testar_threads(void) {
    pthread_t t[THREADS];
    for (long i = 0; i < THREADS; i++) pthread_create(&t[i], NULL, somar, (void *)i);

    char linha[64];
    int threads_vistas = 0;
    FILE *f = fopen("/proc/self/status", "r");
    while (f && fgets(linha, sizeof linha, f))
        if (sscanf(linha, "Threads:\t%d", &threads_vistas) == 1) break;
    if (f) fclose(f);

    long soma_retornos = 0;
    for (int i = 0; i < THREADS; i++) {
        void *r;
        pthread_join(t[i], &r);
        soma_retornos += (long)r;
    }
    CONFERIR(contador == THREADS * VOLTAS, "8 threads somando com mutex (160000)");
    CONFERIR(soma_retornos == 280 && meu_numero == -1, "pthread_join e variáveis __thread separadas");
    CONFERIR(threads_vistas >= 2, "/proc/self/status conta as threads");

    pthread_t produtor;
    pthread_create(&produtor, NULL, produzir, NULL);
    int consumidos = 0, soma = 0;
    while (consumidos < 50) {
        pthread_mutex_lock(&fila_trava);
        while (consumidos == produzidos) pthread_cond_wait(&fila_tem_item, &fila_trava);
        soma += fila[consumidos++ % 64];
        pthread_mutex_unlock(&fila_trava);
    }
    pthread_join(produtor, NULL);
    CONFERIR(soma == 1275, "produtor e consumidor com variável de condição");

    pthread_t saiu;
    void *r = NULL;
    pthread_create(&saiu, NULL, so_sair, NULL);
    pthread_join(saiu, &r);
    long tid = syscall(SYS_gettid);
    CONFERIR(r == (void *)7 && tid == getpid(), "pthread_exit não derruba o processo; gettid");

    pid_t filho = fork();
    if (filho == 0) {
        pthread_t th;
        pthread_create(&th, NULL, sair_com_5, NULL);
        for (;;) pause(); /* a thread principal fica esperando para sempre */
    }
    int status = 0;
    waitpid(filho, &status, 0);
    CONFERIR(WIFEXITED(status) && WEXITSTATUS(status) == 5, "exit() numa thread encerra o processo todo");
}

static void testar_proc_e_aleatorio(void) {
    unsigned char a[16], b[16];
    int fd = open("/dev/urandom", O_RDONLY);
    CONFERIR(fd >= 0 && read(fd, a, 16) == 16 && read(fd, b, 16) == 16 && memcmp(a, b, 16) != 0, "/dev/urandom");
    close(fd);

    char linha[128];
    int pid_lido = -1;
    FILE *f = fopen("/proc/self/status", "r");
    while (f && fgets(linha, sizeof linha, f))
        if (sscanf(linha, "Pid:\t%d", &pid_lido) == 1) break;
    if (f) fclose(f);
    CONFERIR(pid_lido == getpid(), "/proc/self/status");

    char alvo[32] = {0};
    readlink("/proc/self", alvo, sizeof alvo - 1);
    CONFERIR(atoi(alvo) == getpid(), "/proc/self é um link para o próprio pid");

    f = fopen("/proc/meminfo", "r");
    long total = 0;
    if (f) {
        fscanf(f, "MemTotal: %ld kB", &total);
        fclose(f);
    }
    CONFERIR(total > 100000, "/proc/meminfo");

    f = fopen("/proc/self/cmdline", "r");
    char cmd[32] = {0};
    if (f) {
        fread(cmd, 1, sizeof cmd - 1, f);
        fclose(f);
    }
    CONFERIR(strcmp(cmd, "testec") == 0, "/proc/self/cmdline");

    int processos = 0;
    DIR *d = opendir("/proc");
    struct dirent *e;
    while (d && (e = readdir(d)))
        if (e->d_name[0] >= '1' && e->d_name[0] <= '9') processos++;
    if (d) closedir(d);
    CONFERIR(processos >= 3, "readdir do /proc lista os processos");
}

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

    testar_poll_e_select();
    testar_proc_e_aleatorio();
    testar_threads();

    char data[32];
    strftime(data, sizeof data, "%Y-%m-%d %H:%M", tm);
    printf("Data (UTC): %s\n", data);

    if (falhas == 0) printf("Todos os testes da libc passaram.\n");
    else printf("%d testes da libc falharam.\n", falhas);
    return falhas;
}
