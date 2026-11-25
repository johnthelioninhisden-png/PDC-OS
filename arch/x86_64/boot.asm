; 32-bit Multiboot entry that constructs a minimal identity map and enters
; long mode.  The Rust kernel receives control with interrupts disabled.

BITS 32

SECTION .multiboot
align 4
; Multiboot v1 header: request a 512x512x32 linear framebuffer.
dd 0x1BADB002
dd 0x00000004
dd -(0x1BADB002 + 0x00000004)
; Reserve the five optional address-field slots so the graphics fields remain
; at their fixed Multiboot v1 offsets even though address loading is unused.
dd 0, 0, 0, 0, 0
dd 0
dd 512
dd 512
dd 32

SECTION .text
global _start
extern kernel_main

_start:
    cli
    ; Preserve the Multiboot magic and information pointer while 32-bit
    ; bootstrap code performs page-table and CPU initialization.
    mov [multiboot_magic], eax
    mov [multiboot_info], ebx
    ; Use the bootstrap stack before any Rust code is entered.
    mov esp, stack_top

    ; Initialize COM1 before CPU feature checks so bootstrap failures are visible.
    call serial_init_early
    mov esi, boot_message
    call serial_write_early

    ; Reject CPUs without PAE or the extended long-mode feature.
    mov eax, 1
    cpuid
    test edx, 1 << 6
    jz .no_long_mode
    mov eax, 0x80000001
    cpuid
    test edx, 1 << 29
    jz .no_long_mode

    ; Clear the 64-bit bootstrap page tables.
    mov edi, pml4
    mov ecx, 16384 / 4
    xor eax, eax
    rep stosd

    ; PML4[0] -> PDPT; PDPT[0] -> PD0; PDPT[1] -> PD1.
    mov dword [pml4], pdpt + 0x003
    mov dword [pdpt], pd0 + 0x003
    mov dword [pdpt + 8], pd1 + 0x003

    ; Identity-map the first 64 MiB using 2 MiB pages.
    mov ecx, 32
    mov ebx, pd0
    xor eax, eax
.map_page:
    mov edx, eax
    or edx, 0x083
    mov [ebx], edx
    mov dword [ebx + 4], 0
    add ebx, 8
    add eax, 0x200000
    loop .map_page

    ; Map the bootloader's framebuffer at the fixed 1 GiB kernel alias. Two
    ; adjacent 2 MiB pages cover the requested 512x512x32 mode and its offset.
    mov esi, [multiboot_info]
    test esi, esi
    jz .no_framebuffer
    test dword [esi], 1 << 12
    jz .no_framebuffer
    cmp dword [esi + 92], 0
    jne .no_framebuffer
    mov eax, [esi + 88]
    and eax, 0xFFE00000
    or eax, 0x09B
    mov [pd1], eax
    mov eax, [esi + 88]
    and eax, 0xFFE00000
    add eax, 0x200000
    or eax, 0x09B
    mov [pd1 + 8], eax
.no_framebuffer:

    ; Install page-table root and enable PAE.
    mov eax, pml4
    mov cr3, eax
    mov eax, cr4
    or eax, 1 << 5
    mov cr4, eax

    ; Set IA32_EFER.LME, then enable paging.
    mov ecx, 0xC0000080
    rdmsr
    or eax, 1 << 8
    or eax, 1 << 11
    wrmsr
    mov eax, cr0
    or eax, 1 << 31
    mov cr0, eax

    lgdt [gdt_pointer]
    jmp 0x08:start64

.no_long_mode:
    ; Report the CPU feature failure on serial and show a VGA error pattern.
    mov esi, long_mode_error
    call serial_write_early
    mov dword [0xB8000], 0x4F204F20
    hlt
    jmp .no_long_mode

; Minimal polled COM1 setup for diagnostics before Rust initialization.
serial_init_early:
    mov dx, 0x3F9
    xor al, al
    out dx, al
    mov dx, 0x3FB
    mov al, 0x80
    out dx, al
    mov dx, 0x3F8
    mov al, 3
    out dx, al
    mov dx, 0x3F9
    xor al, al
    out dx, al
    mov dx, 0x3FB
    mov al, 0x03
    out dx, al
    mov dx, 0x3FA
    mov al, 0xC7
    out dx, al
    mov dx, 0x3FC
    mov al, 0x0B
    out dx, al
    ret

; Write the NUL-terminated string at ESI to COM1.
serial_write_early:
.next:
    lodsb
    test al, al
    jz .done
    mov bl, al
.wait:
    mov dx, 0x3FD
    in al, dx
    test al, 0x20
    jz .wait
    mov dx, 0x3F8
    mov al, bl
    out dx, al
    jmp .next
.done:
    ret

BITS 64
start64:
    mov edi, dword [rel multiboot_magic]
    mov esi, dword [rel multiboot_info]
    ; Load the kernel data selector into each data segment before using the stack.
    mov ax, 0x10
    mov ds, ax
    mov es, ax
    mov fs, ax
    mov gs, ax
    mov ss, ax
    mov rsp, stack_top
    ; Protect read-only mappings from supervisor writes and prevent writable
    ; page-table contents from being modified through supervisor writes.
    mov rax, cr0
    or rax, 1 << 16
    mov cr0, rax
    ; EDI/ESI were preserved from the 32-bit entry and now contain the
    ; Multiboot magic and information pointer.  In 64-bit mode they occupy
    ; the low halves of RDI/RSI, as required by the kernel_main ABI.
    call kernel_main

; kernel_main is specified not to return; retain a safe halt loop if it does.
.halt64:
    cli
    hlt
    jmp .halt64

SECTION .bss
; Page tables are page-aligned and cleared at boot before use.
multiboot_magic:
    resd 1
multiboot_info:
    resd 1
alignb 4096
pml4:
    resb 4096
pdpt:
    resb 4096
pd0:
    resb 4096
pd1:
    resb 4096

alignb 16
; Reserve a 64 KiB stack for bootstrap and early kernel execution.
stack_bottom:
    resb 65536
stack_top:

SECTION .rodata
boot_message:
    db "PDC-OS boot: entering x86-64 kernel", 13, 10, 0
long_mode_error:
    db "PDC-OS boot failure: CPU lacks required long-mode features", 13, 10, 0

align 8
; Null, 64-bit code, and 64-bit data descriptors used for the long-mode jump.
gdt64:
    dq 0
    dq 0x00AF9A000000FFFF
    dq 0x00AF92000000FFFF
gdt_pointer:
    dw gdt_pointer - gdt64 - 1
    dq gdt64

SECTION .note.GNU-stack noalloc noexec nowrite progbits
