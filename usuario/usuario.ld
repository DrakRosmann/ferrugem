/*
 * Linker script dos programas de usuário.
 * Todo programa começa em 1 GiB: abaixo disso fica o kernel (inacessível
 * para o programa). Cada seção começa numa página nova, para que o kernel
 * possa dar permissões diferentes (código só leitura, dados graváveis).
 */
ENTRY(_start)

SECTIONS
{
    . = 0x40000000;

    .text   : ALIGN(4K) { *(.text .text.*) }
    .rodata : ALIGN(4K) { *(.rodata .rodata.*) }
    .data   : ALIGN(4K) { *(.data .data.*) }
    .bss    : ALIGN(4K) { *(.bss .bss.*) *(COMMON) }

    /DISCARD/ : { *(.eh_frame*) *(.comment) *(.note*) }
}
