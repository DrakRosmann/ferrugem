; =============================================================================
;  FerrugemOS — pontos de entrada das interrupções (assembly NASM, 64 bits)
;
;  A IDT tem 256 entradas, e cada uma precisa apontar para algum código.
;  Aqui geramos 256 "stubs" quase iguais. Cada um:
;
;    1. empilha um código de erro falso (0), se a CPU não empilhou um de verdade
;       — assim a pilha fica com o mesmo formato para todos os vetores;
;    2. empilha o número do vetor;
;    3. pula para `isr_comum`, que salva os registradores e chama o Rust.
;
;  Pilha quando `tratar_interrupcao` é chamada (endereços crescendo para baixo):
;
;      SS, RSP, RFLAGS, CS, RIP     <- empilhados pela CPU
;      código de erro               <- CPU ou stub
;      vetor                        <- stub
;      RAX ... R15                  <- isr_comum
;                                   <- RSP = ponteiro para `Contexto` (Rust)
; =============================================================================

global tabela_isr
global isr_sair
global troca_contexto
global entrada_syscall
global comeco_de_processo
extern tratar_interrupcao
extern processo_comecou

section .text
bits 64

; Só estas exceções empilham um código de erro (manual da Intel, vol. 3, 6.13).
%define TEM_CODIGO_DE_ERRO(v) ((v) == 8 || ((v) >= 10 && (v) <= 14) || (v) == 17 || (v) == 21 || (v) == 29 || (v) == 30)

%assign i 0
%rep 256
isr_%[i]:
  %if TEM_CODIGO_DE_ERRO(i) == 0
    push qword 0
  %endif
    push qword i
    jmp isr_comum
%assign i i+1
%endrep

isr_comum:
    ; Veio de um programa (o CS empilhado tem anel 3)? Então o GS ainda é o
    ; dele: `swapgs` traz o do kernel, que aponta para a `Cpu` desta CPU (smp.rs).
    ; [rsp] = vetor, [rsp+8] = código de erro, [rsp+16] = RIP, [rsp+24] = CS
    test qword [rsp + 24], 3
    jz isr_salvar
    swapgs
isr_salvar:
    ; A ordem aqui é o inverso da ordem dos campos de `Contexto` em interrupcoes.rs
    push rax
    push rbx
    push rcx
    push rdx
    push rsi
    push rdi
    push rbp
    push r8
    push r9
    push r10
    push r11
    push r12
    push r13
    push r14
    push r15

    ; A CPU alinha RSP em 16 antes de empilhar o quadro de interrupção, e
    ; empilhamos 22 * 8 = 176 bytes (múltiplo de 16): a pilha já está alinhada
    ; como a ABI exige na hora do `call`.
    mov rdi, rsp                    ; 1º argumento: &mut Contexto
    cld                             ; a ABI garante DF = 0 na entrada de funções
    call tratar_interrupcao

    ; O Rust devolve qual contexto restaurar (hoje, sempre o mesmo).
    mov rsp, rax

; Um processo novo começa a vida aqui: seu primeiro `troca_contexto` "retorna"
; para este ponto, com RSP apontando para um `Contexto` preparado pelo kernel.
isr_sair:
    pop r15
    pop r14
    pop r13
    pop r12
    pop r11
    pop r10
    pop r9
    pop r8
    pop rbp
    pop rdi
    pop rsi
    pop rdx
    pop rcx
    pop rbx
    pop rax
    test qword [rsp + 24], 3        ; voltando para um programa? devolve o GS dele
    jz .para_o_kernel
    swapgs
.para_o_kernel:
    add rsp, 16                     ; descarta vetor e código de erro
    iretq                           ; restaura RIP, CS, RFLAGS, RSP e SS

; Um processo novo começa a vida aqui: o primeiro `troca_contexto` para ele
; "retorna" para este ponto. Ele vai direto para o modo usuário, então
; precisa soltar a trava do kernel que herdou de quem o escalonou.
comeco_de_processo:
    call processo_comecou
    jmp isr_sair

; -----------------------------------------------------------------------------
; troca_contexto(salvar_rsp_em: *mut u64 [RDI], novo_rsp: u64 [RSI])
;
; O coração da multitarefa. Salva os registradores que a ABI manda preservar
; na pilha atual, guarda RSP, troca para a pilha de outro processo e restaura
; os registradores dele. O `ret` final volta para onde *aquele* processo
; chamou troca_contexto — ou para `isr_sair`, se ele nunca rodou.
; -----------------------------------------------------------------------------
troca_contexto:
    push rbp
    push rbx
    push r12
    push r13
    push r14
    push r15
    mov [rdi], rsp
    mov rsp, rsi
    pop r15
    pop r14
    pop r13
    pop r12
    pop rbx
    pop rbp
    ret

; -----------------------------------------------------------------------------
; Entrada da instrução `syscall`.
;
; Diferente de uma interrupção, `syscall` não troca de pilha nem empilha nada:
; só guarda RIP em RCX e RFLAGS em R11. Então trocamos para a pilha de kernel
; do processo e montamos à mão o mesmo quadro que a CPU montaria num
; `int 0x80`. Daí em diante o caminho é o mesmo de qualquer interrupção,
; e a volta é por `iretq`.
; -----------------------------------------------------------------------------
entrada_syscall:
    swapgs                          ; GS → a `Cpu` desta CPU (smp.rs)
    mov [gs:8], rsp                 ; Cpu.rsp_usuario
    mov rsp, [gs:0]                 ; Cpu.pilha_kernel: a do processo atual
    push qword 0x1B                 ; SS  = dados de usuário  (0x18 | anel 3)
    push qword [gs:8]               ; RSP do usuário
    push r11                        ; RFLAGS
    push qword 0x23                 ; CS  = código de usuário (0x20 | anel 3)
    push rcx                        ; RIP de volta
    push qword 0                    ; código de erro
    push qword 0x80                 ; vetor: o mesmo do `int 0x80`
    jmp isr_salvar                  ; (o swapgs já foi feito)

; -----------------------------------------------------------------------------
; Tabela com o endereço de cada stub; o Rust usa para preencher a IDT.
; -----------------------------------------------------------------------------
section .rodata
align 8
tabela_isr:
%assign i 0
%rep 256
    dq isr_%[i]
%assign i i+1
%endrep
