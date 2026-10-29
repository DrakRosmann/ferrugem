/*
 * Linker script do FerrugemOS.
 * Diz ao linker onde cada pedaço do kernel fica na memória.
 * O kernel é carregado a partir de 1 MiB (abaixo disso fica BIOS, VGA etc.).
 */
ENTRY(_start)

SECTIONS
{
    . = 1M;

    /* O header Multiboot precisa vir primeiro (bootloader procura nos primeiros 8 KiB) */
    .boot : { KEEP(*(.multiboot)) }

    .text   : ALIGN(4K) { *(.text .text.*) }
    .rodata : ALIGN(4K) { *(.rodata .rodata.*) }
    .data   : ALIGN(4K) { *(.data .data.*) }
    .bss    : ALIGN(4K) { *(COMMON) *(.bss .bss.*) }

    /* Daqui para cima a memória é do alocador de quadros */
    . = ALIGN(4K);
    __fim_kernel = .;

    /* Não temos unwinding nem precisamos de metadados do compilador */
    /DISCARD/ : { *(.eh_frame*) *(.comment) *(.note*) }
}
