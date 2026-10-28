/*
 * O kernel é carregado no endereço físico 1 MiB, mas ligado na "metade
 * alta" (KERNEL_VMA + endereço físico), deixando a metade baixa do espaço
 * de endereçamento livre para os processos de usuário.
 */
ENTRY(_start)

KERNEL_VMA = 0xFFFFFFFF80000000;

PHDRS
{
    text   PT_LOAD FLAGS(5);   /* R-X */
    rodata PT_LOAD FLAGS(4);   /* R-- */
    data   PT_LOAD FLAGS(6);   /* RW- */
}

SECTIONS
{
    . = KERNEL_VMA + 1M;
    __kernel_start = .;

    .text : AT(ADDR(.text) - KERNEL_VMA)
    {
        KEEP(*(.multiboot))
        *(.text.boot)
        *(.text .text.*)
    } :text

    .rodata : AT(ADDR(.rodata) - KERNEL_VMA) ALIGN(4K)
    {
        *(.rodata .rodata.*)
        *(.eh_frame .eh_frame_hdr)
    } :rodata

    .data : AT(ADDR(.data) - KERNEL_VMA) ALIGN(4K)
    {
        *(.data .data.*)
        *(.got .got.plt)
    } :data
    __data_end = .;

    .bss : AT(ADDR(.bss) - KERNEL_VMA) ALIGN(4K)
    {
        *(COMMON)
        *(.bss .bss.*)
    } :data

    . = ALIGN(4K);
    __kernel_end = .;

    /DISCARD/ : { *(.comment) *(.note .note.*) }
}
