#define _GNU_SOURCE /* struct ucred (SO_PEERCRED) */
/* testec.c — testa a libc (musl) rodando no FerrugemOS: stdio, malloc
 * grande (mmap), arquivos, processos, pipes, sinais, tempo e números
 * de ponto flutuante. Tudo com as funções padrão do C/POSIX. */
#include <dirent.h>
#include <errno.h>
#include <poll.h>
#include <pthread.h>
#include <stddef.h>
#include <sys/epoll.h>
#include <sys/shm.h>
#include <sys/socket.h>
#include <sys/syscall.h>
#include <sys/select.h>
#include <sys/un.h>
#include <syslog.h>
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

/* Sockets Unix, epoll e memória compartilhada: a comunicação entre processos. */
static void testar_ipc(void) {
    /* socketpair: um fluxo nos dois sentidos; shutdown dá fim de arquivo ao outro lado */
    int par[2];
    char buf[64] = {0};
    CONFERIR(socketpair(AF_UNIX, SOCK_STREAM, 0, par) == 0, "socketpair");
    write(par[0], "ida", 3);
    write(par[1], "volta", 5);
    int ok = read(par[1], buf, sizeof buf) == 3 && memcmp(buf, "ida", 3) == 0;
    ok = ok && read(par[0], buf, sizeof buf) == 5 && memcmp(buf, "volta", 5) == 0;
    shutdown(par[0], SHUT_WR);
    ok = ok && read(par[1], buf, sizeof buf) == 0;
    CONFERIR(ok, "socketpair: dados nos dois sentidos e shutdown");
    close(par[0]);
    close(par[1]);

    /* servidor num caminho: bind, listen, accept; o cliente é outro processo */
    const char *caminho = "/tmp/testec.sock";
    unlink(caminho);
    struct sockaddr_un end = {.sun_family = AF_UNIX};
    strcpy(end.sun_path, caminho);
    int servidor = socket(AF_UNIX, SOCK_STREAM, 0);
    ok = servidor >= 0 && bind(servidor, (struct sockaddr *)&end, sizeof end) == 0 && listen(servidor, 4) == 0;
    struct stat st;
    CONFERIR(ok && stat(caminho, &st) == 0 && S_ISSOCK(st.st_mode), "bind cria o socket no sistema de arquivos");
    int outro = socket(AF_UNIX, SOCK_STREAM, 0);
    CONFERIR(bind(outro, (struct sockaddr *)&end, sizeof end) == -1 && errno == EADDRINUSE, "bind no mesmo caminho: EADDRINUSE");
    close(outro);
    pid_t cliente = fork();
    if (cliente == 0) {
        int c = socket(AF_UNIX, SOCK_STREAM, 0);
        char r[16] = {0};
        if (connect(c, (struct sockaddr *)&end, sizeof end) != 0) _exit(1);
        send(c, "ping", 4, 0);
        if (recv(c, r, sizeof r, 0) != 4 || memcmp(r, "pong", 4) != 0) _exit(2);
        _exit(0);
    }
    int conexao = accept(servidor, NULL, NULL);
    struct ucred cred;
    socklen_t tamanho = sizeof cred;
    ok = conexao >= 0 && getsockopt(conexao, SOL_SOCKET, SO_PEERCRED, &cred, &tamanho) == 0 && cred.pid == cliente;
    memset(buf, 0, sizeof buf);
    ok = ok && recv(conexao, buf, sizeof buf, 0) == 4 && strcmp(buf, "ping") == 0;
    ok = ok && send(conexao, "pong", 4, 0) == 4;
    int status;
    waitpid(cliente, &status, 0);
    CONFERIR(ok && WIFEXITED(status) && WEXITSTATUS(status) == 0, "connect/accept entre dois processos (e SO_PEERCRED)");
    ok = read(conexao, buf, sizeof buf) == 0; /* o cliente saiu: fim de arquivo */
    signal(SIGPIPE, SIG_IGN);
    ok = ok && send(conexao, "x", 1, 0) == -1 && errno == EPIPE;
    signal(SIGPIPE, SIG_DFL);
    CONFERIR(ok, "o outro lado fechou: read dá 0, send dá EPIPE");
    close(conexao);
    close(servidor);
    int c = socket(AF_UNIX, SOCK_STREAM, 0);
    CONFERIR(connect(c, (struct sockaddr *)&end, sizeof end) == -1 && errno == ECONNREFUSED,
             "ninguém ouvindo: ECONNREFUSED");
    close(c);
    unlink(caminho);
    if (access("/disco", W_OK) == 0) {
        /* no ext2 também (fica lá: o e2fsck do `make teste` confere o inode) */
        struct sockaddr_un no_disco = {.sun_family = AF_UNIX};
        strcpy(no_disco.sun_path, "/disco/testec.sock");
        unlink(no_disco.sun_path);
        int s = socket(AF_UNIX, SOCK_STREAM, 0);
        ok = bind(s, (struct sockaddr *)&no_disco, sizeof no_disco) == 0;
        CONFERIR(ok && stat(no_disco.sun_path, &st) == 0 && S_ISSOCK(st.st_mode), "socket com nome no ext2");
        close(s);
    }

    /* datagramas num endereço abstrato: cada mensagem chega inteira e separada */
    struct sockaddr_un abstrato = {.sun_family = AF_UNIX};
    memcpy(abstrato.sun_path, "\0testec", 7);
    socklen_t tam_abstrato = offsetof(struct sockaddr_un, sun_path) + 7;
    int caixa = socket(AF_UNIX, SOCK_DGRAM, 0);
    ok = bind(caixa, (struct sockaddr *)&abstrato, tam_abstrato) == 0;
    pid_t remetente = fork();
    if (remetente == 0) {
        int d = socket(AF_UNIX, SOCK_DGRAM, 0);
        sendto(d, "um", 2, 0, (struct sockaddr *)&abstrato, tam_abstrato);
        sendto(d, "dois", 4, 0, (struct sockaddr *)&abstrato, tam_abstrato);
        sendto(d, "tres", 4, 0, (struct sockaddr *)&abstrato, tam_abstrato);
        _exit(0);
    }
    waitpid(remetente, &status, 0);
    char m[3][8] = {{0}};
    ok = ok && recv(caixa, m[0], 8, 0) == 2 && recv(caixa, m[1], 8, 0) == 4 && recv(caixa, m[2], 8, 0) == 4;
    CONFERIR(ok && !strcmp(m[0], "um") && !strcmp(m[1], "dois") && !strcmp(m[2], "tres"),
             "datagramas num endereço abstrato");

    /* O_NONBLOCK: sem nada para ler, EAGAIN na hora */
    fcntl(caixa, F_SETFL, fcntl(caixa, F_GETFL) | O_NONBLOCK);
    CONFERIR(recv(caixa, buf, sizeof buf, 0) == -1 && errno == EAGAIN && (fcntl(caixa, F_GETFL) & O_NONBLOCK),
             "fcntl(O_NONBLOCK): EAGAIN em vez de esperar");
    close(caixa);

    /* epoll: um pipe e um socket no mesmo conjunto */
    int p[2];
    pipe(p);
    socketpair(AF_UNIX, SOCK_STREAM, 0, par);
    int ep = epoll_create1(EPOLL_CLOEXEC);
    struct epoll_event ev = {.events = EPOLLIN, .data.u64 = 111};
    ok = ep >= 0 && epoll_ctl(ep, EPOLL_CTL_ADD, p[0], &ev) == 0;
    ev.data.u64 = 222;
    ok = ok && epoll_ctl(ep, EPOLL_CTL_ADD, par[1], &ev) == 0;
    ok = ok && epoll_ctl(ep, EPOLL_CTL_ADD, par[1], &ev) == -1 && errno == EEXIST;
    struct epoll_event prontos[4];
    ok = ok && epoll_wait(ep, prontos, 4, 0) == 0;
    long t0 = agora_ms();
    ok = ok && epoll_wait(ep, prontos, 4, 50) == 0 && agora_ms() - t0 >= 50;
    CONFERIR(ok, "epoll: nada pronto, o prazo de 50 ms é respeitado");
    write(par[0], "oi", 2);
    int n = epoll_wait(ep, prontos, 4, 1000);
    ok = n == 1 && prontos[0].data.u64 == 222 && (prontos[0].events & EPOLLIN);
    write(p[1], "x", 1);
    read(par[1], buf, sizeof buf);
    n = epoll_wait(ep, prontos, 4, 1000);
    ok = ok && n == 1 && prontos[0].data.u64 == 111;
    CONFERIR(ok, "epoll_wait diz quem ficou pronto (pelo data)");
    pid_t escritor = fork();
    if (escritor == 0) {
        usleep(50000);
        write(par[0], "depois", 6);
        _exit(0);
    }
    read(p[0], buf, 1);
    n = epoll_wait(ep, prontos, 4, 2000); /* dorme até o filho escrever */
    waitpid(escritor, &status, 0);
    CONFERIR(n == 1 && prontos[0].data.u64 == 222, "epoll_wait acorda quando outro processo escreve");
    close(ep);
    close(p[0]);
    close(p[1]);
    close(par[0]);
    close(par[1]);

    /* memória compartilhada do System V: dois processos, as mesmas páginas */
    key_t chave = 0x7e57;
    int id = shmget(chave, 8192, IPC_CREAT | 0600);
    pid_t filho = fork();
    if (filho == 0) {
        int meu = shmget(chave, 0, 0); /* acha pelo número, sem herdar nada */
        char *q = shmat(meu, NULL, 0);
        if (q == (void *)-1) _exit(1);
        strcpy(q + 5000, "escrito pelo filho");
        shmdt(q);
        _exit(0);
    }
    waitpid(filho, &status, 0);
    char *mem = shmat(id, NULL, 0);
    struct shmid_ds info;
    ok = mem != (void *)-1 && strcmp(mem + 5000, "escrito pelo filho") == 0;
    ok = ok && shmctl(id, IPC_STAT, &info) == 0 && info.shm_nattch == 1 && info.shm_segsz == 8192;
    CONFERIR(ok, "shmget/shmat: o pai lê o que o filho escreveu");
    ok = shmctl(id, IPC_RMID, NULL) == 0 && shmget(chave, 0, 0) == -1 && errno == ENOENT;
    ok = ok && strcmp(mem + 5000, "escrito pelo filho") == 0; /* ainda anexado: continua lá */
    ok = ok && shmdt(mem) == 0;
    CONFERIR(ok, "IPC_RMID: some da tabela, mas vale até o último shmdt");

    /* syslog() da libc: datagrama para /dev/log; o syslogd grava no registro */
    openlog("testec", LOG_PID, LOG_USER);
    syslog(LOG_INFO, "mensagem pela libc %d", 42);
    closelog();
    usleep(200000);
    char esperado[64], linha[256];
    snprintf(esperado, sizeof esperado, "testec[%d]: mensagem pela libc 42", getpid());
    FILE *reg = fopen("/var/log/mensagens", "r");
    ok = 0;
    while (reg && fgets(linha, sizeof linha, reg)) ok |= strstr(linha, esperado) != NULL;
    if (reg) fclose(reg);
    CONFERIR(ok, "syslog() chega ao /var/log/mensagens");
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
    testar_ipc();

    char data[32];
    strftime(data, sizeof data, "%Y-%m-%d %H:%M", tm);
    printf("Data (UTC): %s\n", data);

    if (falhas == 0) printf("Todos os testes da libc passaram.\n");
    else printf("%d testes da libc falharam.\n", falhas);
    return falhas;
}
