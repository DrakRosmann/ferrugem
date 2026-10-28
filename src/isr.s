/*
 * Pontos de entrada das 256 interrupções.
 *
 * Cada stub ocupa exatamente 16 bytes (isr_stubs + 16 * vetor). Ele empilha
 * um código de erro falso (quando a CPU não empilha um) e o número do vetor,
 * e salta para isr_common, que salva os registradores e chama
 * interrupt_dispatch(frame). O valor retornado é o frame a restaurar: é
 * assim que o escalonador troca de tarefa. A entrada de `syscall`
 * (syscall.rs) monta o mesmo frame e também termina em isr_common.
 */

.section .text.isr, "ax"
.code64
.balign 16
.global isr_stubs
isr_stubs:
.set vec, 0
.rept 256
    .balign 16
    .if (vec == 8) || (vec == 10) || (vec == 11) || (vec == 12) || (vec == 13) || (vec == 14) || (vec == 17) || (vec == 21) || (vec == 29) || (vec == 30)
    .else
    pushq $0
    .endif
    pushq $vec
    jmp isr_common
    .set vec, vec + 1
.endr

.balign 16
.global isr_common
isr_common:
    pushq %rax
    pushq %rbx
    pushq %rcx
    pushq %rdx
    pushq %rsi
    pushq %rdi
    pushq %rbp
    pushq %r8
    pushq %r9
    pushq %r10
    pushq %r11
    pushq %r12
    pushq %r13
    pushq %r14
    pushq %r15

    movq %rsp, %rdi
    cld
    call interrupt_dispatch
    movq %rax, %rsp

    popq %r15
    popq %r14
    popq %r13
    popq %r12
    popq %r11
    popq %r10
    popq %r9
    popq %r8
    popq %rbp
    popq %rdi
    popq %rsi
    popq %rdx
    popq %rcx
    popq %rbx
    popq %rax
    addq $16, %rsp                    /* vetor + código de erro */
    iretq
