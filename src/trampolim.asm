; trampolim.asm — onde cada CPU extra (AP) começa a vida.
;
; Uma CPU acordada pelo INIT-SIPI começa em modo real (16 bits, como um PC
; de 1981), no endereço que o SIPI mandou: 0x8000. Daqui ela precisa subir
; sozinha até o modo longo (64 bits) — o mesmo caminho do boot, em
; miniatura: GDT provisória → modo protegido → paginação com as tabelas do
; kernel → modo longo → salta para `ap_principal` (smp.rs) na pilha que a
; CPU que a acordou preparou.
;
; É montado como binário puro (nasm -f bin) e copiado para 0x8000 pelo
; kernel, que antes preenche os parâmetros logo no começo.

default abs
bits 16
org 0x8000

inicio:
    jmp short real
    align 8
; Parâmetros (o kernel escreve aqui; os deslocamentos estão em smp.rs).
param_cr3:     dq 0     ;  8: a tabela de páginas do kernel
param_cr4:     dq 0     ; 16: os mesmos recursos da CPU principal (PAE, SMEP, SMAP...)
param_cr0:     dq 0     ; 24
param_efer:    dq 0     ; 32: modo longo, NX, syscall
param_pilha:   dq 0     ; 40: topo da pilha desta CPU
param_entrada: dq 0     ; 48: ap_principal
param_cpu:     dq 0     ; 56: o número desta CPU (1, 2, ...)

real:
    cli
    cld
    xor ax, ax
    mov ds, ax
    mov es, ax
    mov ss, ax
    lgdt [gdt_ptr]
    mov eax, cr0
    or eax, 1                       ; PE: modo protegido
    mov cr0, eax
    jmp 0x08:protegido

bits 32
protegido:
    mov ax, 0x10
    mov ds, ax
    mov es, ax
    mov ss, ax
    mov eax, [param_cr4]
    and eax, ~(1 << 17)             ; PCIDE só pode ser ligado no modo longo
    mov cr4, eax
    mov eax, [param_cr3]
    mov cr3, eax
    mov ecx, 0xC0000080             ; EFER
    mov eax, [param_efer]
    mov edx, [param_efer + 4]
    wrmsr
    mov eax, [param_cr0]
    or eax, 0x80000001              ; paginação (e continua protegido)
    mov cr0, eax                    ; com EFER.LME, isto liga o modo longo
    jmp 0x18:longo

bits 64
longo:
    mov ax, 0x10
    mov ds, ax
    mov es, ax
    mov ss, ax
    mov rsp, [param_pilha]
    mov rdi, [param_cpu]
    mov rax, [param_entrada]
    call rax
.parada:
    hlt
    jmp .parada

align 8
gdt:
    dq 0
    dq 0x00CF9A000000FFFF           ; 0x08: código de 32 bits
    dq 0x00CF92000000FFFF           ; 0x10: dados
    dq 0x00AF9A000000FFFF           ; 0x18: código de 64 bits
gdt_ptr:
    dw gdt_ptr - gdt - 1
    dd gdt
