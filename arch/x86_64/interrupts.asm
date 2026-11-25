; Generates fixed interrupt stubs.  The common handler reads the pushed vector
; and dispatches to the Rust interrupt subsystem.

BITS 64

extern interrupt_dispatch
global interrupt_common

%macro ISR_NO_ERROR 1
global isr_stub_%1
isr_stub_%1:
    ; Normalize the stack shape: these stubs supply a dummy error code, then
    ; push the vector so the shared handler can use one dispatch convention.
    push qword 0
    push qword %1
    jmp interrupt_common
%endmacro

%macro ISR_ERROR 1
global isr_stub_%1
isr_stub_%1:
    ; The processor already pushed an error code for these vectors. Only
    ; normalize the vector slot above it before entering the shared handler.
    push qword %1
    jmp interrupt_common
%endmacro

%assign i 0
%rep 64
%if i == 8 || i == 10 || i == 11 || i == 12 || i == 13 || i == 14 || i == 17 || i == 21 || i == 29 || i == 30
    ISR_ERROR i
%else
    ISR_NO_ERROR i
%endif
%assign i i+1
%endrep

; Vector 128 provides the PDC-OS int 0x80 syscall ABI to ring 3.
global isr_stub_128
isr_stub_128:
    push qword 0
    push qword 128
    jmp interrupt_common

interrupt_common:
    ; Preserve every general-purpose register that the Rust handler may clobber.
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

    mov rdi, [rsp + 15*8]
    mov rsi, [rsp + 16*8]
    mov rdx, cr2
    mov rcx, rsp
    ; The System V x86-64 ABI passes vector, error code, CR2, and the register
    ; frame pointer in RDI/RSI/RDX/RCX.
    call interrupt_dispatch
    ; Timer preemption can replace the interrupt frame in place and return
    ; the stack pointer to be restored below.
    mov rsp, rax

    ; Restore the interrupted register state, discard vector/error slots, and
    ; return through the processor's saved interrupt frame.
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
    add rsp, 16
    iretq

SECTION .note.GNU-stack noalloc noexec nowrite progbits
