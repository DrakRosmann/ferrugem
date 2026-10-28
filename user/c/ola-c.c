/*
 * Um programa em C sem libc, compilado pelo gcc do sistema:
 *
 *   gcc -static -nostdlib -no-pie -O2 -ffreestanding -o ola-c ola-c.c
 *
 * O mesmo binário roda no Linux e no FerrugemOS, porque os dois falam a
 * mesma língua: a ABI de chamadas de sistema do Linux x86_64.
 */

static long sys3(long number, long a, long b, long c)
{
    long result;
    __asm__ volatile("syscall"
                     : "=a"(result)
                     : "a"(number), "D"(a), "S"(b), "d"(c)
                     : "rcx", "r11", "memory");
    return result;
}

static unsigned long length(const char *s)
{
    unsigned long n = 0;
    while (s[n])
        n++;
    return n;
}

static void say(const char *s)
{
    sys3(1, 1, (long)s, (long)length(s));
}

struct utsname {
    char field[6][65];
};

void start_c(long *stack)
{
    long argc = stack[0];
    char **argv = (char **)(stack + 1);
    struct utsname system;

    sys3(63, (long)&system, 0, 0);
    say("Olá de um programa em C! Este kernel se chama ");
    say(system.field[0]);
    say(".\nArgumentos:");
    for (long i = 0; i < argc; i++) {
        say(" ");
        say(argv[i]);
    }
    say("\n");
    sys3(60, 0, 0, 0);
}

__asm__(".global _start\n"
        "_start:\n"
        "    xor %rbp, %rbp\n"
        "    mov %rsp, %rdi\n"
        "    and $-16, %rsp\n"
        "    call start_c\n"
        "    ud2\n");
