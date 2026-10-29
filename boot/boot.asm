; =============================================================================
;  FerrugemOS — código de boot (x86, assembly NASM)
;
;  O bootloader (GRUB ou o próprio QEMU com -kernel) segue o padrão Multiboot 1
;  e nos entrega a CPU em MODO PROTEGIDO de 32 bits, sem paginação.
;  Nosso kernel Rust é 64 bits, então este arquivo precisa:
;
;    1. Declarar o header Multiboot (para o bootloader nos reconhecer)
;    2. Montar uma pilha
;    3. Verificar se a CPU suporta modo longo (64 bits)
;    4. Criar tabelas de página (identidade: endereço virtual == físico)
;    5. Ligar PAE, modo longo e paginação
;    6. Carregar uma GDT de 64 bits e dar um "far jump" para código 64 bits
;    7. Ligar SSE e chamar kmain() em Rust
; =============================================================================

global _start
extern kmain

MB_MAGIC    equ 0x1BADB002
MB_FLAGS    equ 0x00000003          ; bit 0: alinhar módulos | bit 1: pedir mapa de memória
MB_CHECKSUM equ -(MB_MAGIC + MB_FLAGS)

; -----------------------------------------------------------------------------
; Header Multiboot: precisa estar nos primeiros 8 KiB do arquivo
; (o linker script coloca esta seção primeiro).
; -----------------------------------------------------------------------------
section .multiboot
align 4
    dd MB_MAGIC
    dd MB_FLAGS
    dd MB_CHECKSUM

; -----------------------------------------------------------------------------
; Memória não inicializada: tabelas de página e pilha
; -----------------------------------------------------------------------------
section .bss
align 4096
pml4:   resb 4096                   ; Page Map Level 4      (nível 4)
pdpt:   resb 4096                   ; Page Directory Pointer (nível 3)
pd:     resb 4096                   ; Page Directory         (nível 2)
stack_bottom:
        resb 64 * 1024              ; 64 KiB de pilha
stack_top:

; -----------------------------------------------------------------------------
; GDT de 64 bits. Em modo longo a segmentação é quase toda ignorada,
; mas a CPU ainda exige um descritor de código com o bit "L" (64 bits).
; -----------------------------------------------------------------------------
section .rodata
align 8
gdt64:
    dq 0                                                    ; descritor nulo (obrigatório)
.code: equ $ - gdt64
    dq (1 << 43) | (1 << 44) | (1 << 47) | (1 << 53)        ; executável | código/dados | presente | 64 bits
.pointer:
    dw $ - gdt64 - 1                                        ; tamanho - 1
    dq gdt64                                                ; endereço

; =============================================================================
;  Código 32 bits
; =============================================================================
section .text
bits 32

_start:
    mov esp, stack_top

    ; O bootloader deixa: EAX = número mágico, EBX = ponteiro para a struct de info.
    ; Guardamos em EDI/ESI, que serão o 1º e 2º argumento de kmain (convenção System V).
    mov edi, eax
    mov esi, ebx

    cmp eax, 0x2BADB002
    jne .erro_multiboot

    call checar_modo_longo
    call montar_tabelas_de_pagina
    call ligar_paginacao

    lgdt [gdt64.pointer]
    jmp gdt64.code:modo_longo       ; far jump: recarrega CS com o seletor de 64 bits

.erro_multiboot:
    mov al, 'M'
    jmp erro

; Usa CPUID estendido para ver se o bit LM (Long Mode) está ligado.
checar_modo_longo:
    mov eax, 0x80000000
    cpuid
    cmp eax, 0x80000001             ; a CPU tem a função estendida 0x80000001?
    jb .sem_modo_longo
    mov eax, 0x80000001
    cpuid
    test edx, 1 << 29               ; bit 29 = Long Mode
    jz .sem_modo_longo
    ret
.sem_modo_longo:
    mov al, 'L'
    jmp erro

; Mapeia o primeiro 1 GiB de memória usando páginas "huge" de 2 MiB.
;   PML4[0] -> PDPT
;   PDPT[0] -> PD
;   PD[i]   -> i * 2 MiB  (512 entradas = 1 GiB)
montar_tabelas_de_pagina:
    mov eax, pdpt
    or eax, 0b11                    ; presente + gravável
    mov [pml4], eax

    mov eax, pd
    or eax, 0b11
    mov [pdpt], eax

    xor ecx, ecx
.proxima:
    mov eax, 0x200000               ; 2 MiB
    mul ecx                         ; eax = 2 MiB * ecx
    or eax, 0b10000011              ; presente + gravável + huge page
    mov [pd + ecx * 8], eax
    inc ecx
    cmp ecx, 512
    jne .proxima
    ret

ligar_paginacao:
    mov eax, pml4
    mov cr3, eax                    ; CR3 aponta para a tabela de nível mais alto

    mov eax, cr4
    or eax, 1 << 5                  ; PAE (Physical Address Extension)
    mov cr4, eax

    mov ecx, 0xC0000080             ; MSR EFER
    rdmsr
    or eax, 1 << 8                  ; LME (Long Mode Enable)
    wrmsr

    mov eax, cr0
    or eax, 1 << 31                 ; PG (paginação) — a partir daqui estamos em modo de compatibilidade
    mov cr0, eax
    ret

; Escreve "ERRO: X" no canto da tela (VGA texto em 0xB8000) e trava.
; AL = código do erro.
erro:
    mov dword [0xB8000], 0x4F524F45 ; "ER" em branco sobre vermelho
    mov dword [0xB8004], 0x4F4F4F52 ; "RO"
    mov dword [0xB8008], 0x4F204F3A ; ": "
    mov byte  [0xB800A], al
    mov byte  [0xB800B], 0x4F
.trava:
    hlt
    jmp .trava

; =============================================================================
;  Código 64 bits
; =============================================================================
bits 64
modo_longo:
    ; Zera os registradores de segmento de dados (ignorados em 64 bits, mas
    ; podem conter lixo do modo protegido).
    xor ax, ax
    mov ss, ax
    mov ds, ax
    mov es, ax
    mov fs, ax
    mov gs, ax

    ; Liga SSE. O Rust pode gerar instruções SSE (cópias de memória, floats).
    mov rax, cr0
    and ax, 0xFFFB                  ; limpa EM (emulação de FPU)
    or ax, 0x2                      ; liga MP (monitor de coprocessador)
    mov cr0, rax
    mov rax, cr4
    or ax, 3 << 9                   ; OSFXSR + OSXMMEXCPT
    mov cr4, rax

    ; Os 32 bits de cima de RDI/RSI são indefinidos depois da troca de modo:
    ; "mov r32, r32" zera a parte alta.
    mov edi, edi
    mov esi, esi

    mov rsp, stack_top
    and rsp, -16                    ; a ABI exige pilha alinhada em 16 bytes
    call kmain                      ; kmain(magic: u32, info: u32) -> !

.trava:
    cli
    hlt
    jmp .trava
