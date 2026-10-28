/*
 * Boot do FerrugemOS.
 *
 * O carregador Multiboot (GRUB ou o `-kernel` do QEMU) nos entrega a CPU em
 * modo protegido de 32 bits, sem paginação, executando no endereço físico.
 * O kernel, porém, é ligado na metade alta (KERNEL_VMA), então até a
 * paginação estar ligada todo endereço absoluto aqui é "símbolo - KERNEL_VMA".
 *
 * Tabelas de página montadas (todas com páginas enormes de 2 MiB):
 *   P4[0]   -> 0..4 GiB físicos 1:1   (temporário, só para atravessar a troca)
 *   P4[256] -> 0..4 GiB em 0xFFFF800000000000  (mapa direto da memória física)
 *   P4[511] -> 0..1 GiB em 0xFFFFFFFF80000000  (imagem do kernel)
 */

.set KERNEL_VMA,  0xFFFFFFFF80000000
.set MB_MAGIC,    0x1BADB002
.set MB_FLAGS,    0x00010003          /* alinhar, mapa de memória, endereços no cabeçalho */
.set MB_CHECKSUM, -(MB_MAGIC + MB_FLAGS)

.section .multiboot, "a"
.balign 4
multiboot_header:
    .long MB_MAGIC
    .long MB_FLAGS
    .long MB_CHECKSUM
    /* Como a imagem é um binário plano, dizemos ao carregador onde ela vai. */
    .long multiboot_header - KERNEL_VMA  /* header_addr */
    .long __kernel_start - KERNEL_VMA    /* load_addr */
    .long __data_end - KERNEL_VMA        /* load_end_addr */
    .long __kernel_end - KERNEL_VMA      /* bss_end_addr */
    .long _start - KERNEL_VMA            /* entry_addr */

.section .bss.boot, "aw", @nobits
.balign 4096
.global boot_p4
boot_p4:      .skip 4096
boot_p3_low:  .skip 4096
boot_p3_high: .skip 4096
boot_p2:      .skip 4096 * 4
.balign 16
boot_stack_bottom: .skip 64 * 1024
.global boot_stack_top
boot_stack_top:

.section .text.boot, "ax"
.balign 16
gdt64:
    .quad 0                           /* nulo */
    .quad 0x00AF9A000000FFFF          /* código 64 bits, anel 0 */
    .quad 0x00CF92000000FFFF          /* dados, anel 0 */
gdt64_end:
gdt64_ptr:
    .word gdt64_end - gdt64 - 1
    .quad gdt64 - KERNEL_VMA

.code32
.global _start
_start:
    cli
    cld
    movl $(boot_stack_top - KERNEL_VMA), %esp
    movl %eax, %edi                   /* magic -> 1º argumento */
    movl %ebx, %esi                   /* info  -> 2º argumento */

    cmpl $0x2BADB002, %eax
    jne .Lerr_multiboot

    /* A CPU suporta modo longo? (CPUID 0x80000001, EDX bit 29) */
    movl $0x80000000, %eax
    cpuid
    cmpl $0x80000001, %eax
    jb .Lerr_long_mode
    movl $0x80000001, %eax
    cpuid
    testl $(1 << 29), %edx
    jz .Lerr_long_mode

    /* P4[0] e P4[256] -> P3 baixo; P4[511] -> P3 alto */
    movl $(boot_p3_low - KERNEL_VMA), %eax
    orl $0x3, %eax
    movl %eax, (boot_p4 - KERNEL_VMA)
    movl %eax, (boot_p4 - KERNEL_VMA + 256 * 8)
    movl $(boot_p3_high - KERNEL_VMA), %eax
    orl $0x3, %eax
    movl %eax, (boot_p4 - KERNEL_VMA + 511 * 8)

    /* P3 baixo[0..4] -> quatro P2; P3 alto[510] -> primeiro P2 (1 GiB) */
    movl $(boot_p2 - KERNEL_VMA), %eax
    orl $0x3, %eax
    movl %eax, (boot_p3_high - KERNEL_VMA + 510 * 8)
    xorl %ecx, %ecx
.Lfill_p3:
    movl %eax, (boot_p3_low - KERNEL_VMA)(,%ecx,8)
    addl $4096, %eax
    incl %ecx
    cmpl $4, %ecx
    jne .Lfill_p3

    /* 2048 entradas P2 de 2 MiB = 4 GiB */
    xorl %ecx, %ecx
.Lfill_p2:
    movl %ecx, %eax
    shll $21, %eax
    orl $0x83, %eax                   /* presente | gravável | página enorme */
    movl %eax, (boot_p2 - KERNEL_VMA)(,%ecx,8)
    movl $0, (boot_p2 - KERNEL_VMA + 4)(,%ecx,8)
    incl %ecx
    cmpl $2048, %ecx
    jne .Lfill_p2

    movl $(boot_p4 - KERNEL_VMA), %eax
    movl %eax, %cr3

    movl %cr4, %eax
    orl $0x620, %eax                  /* PAE | OSFXSR | OSXMMEXCPT (SSE) */
    movl %eax, %cr4

    movl $0xC0000080, %ecx            /* EFER */
    rdmsr
    orl $0x901, %eax                  /* SCE (syscall) | LME | NXE */
    wrmsr

    movl %cr0, %eax
    andl $~0x4, %eax                  /* sem emulação de FPU */
    orl $0x80010002, %eax             /* PG | WP | MP */
    movl %eax, %cr0

    lgdt (gdt64_ptr - KERNEL_VMA)
    pushl $0x08
    pushl $(long_mode_low - KERNEL_VMA)
    lret

.Lerr_multiboot:
    movl $0x4f4d4f45, 0xb8000         /* "EM" em vermelho */
    jmp .Lhalt32
.Lerr_long_mode:
    movl $0x4f4c4f45, 0xb8000         /* "EL" em vermelho */
.Lhalt32:
    hlt
    jmp .Lhalt32

.code64
long_mode_low:
    /* Ainda executando no endereço físico: salta para a metade alta. */
    movabsq $long_mode_high, %rax
    jmp *%rax

long_mode_high:
    movw $0x10, %ax
    movw %ax, %ds
    movw %ax, %es
    movw %ax, %ss
    xorw %ax, %ax
    movw %ax, %fs
    movw %ax, %gs

    /* Os 32 bits altos dos registradores são indefinidos após a troca de modo. */
    movl %edi, %edi
    movl %esi, %esi
    leaq boot_stack_top(%rip), %rsp
    xorl %ebp, %ebp
    call kernel_main
.Lhalt64:
    cli
    hlt
    jmp .Lhalt64
