/* Keep the Multiboot header in the executable load segment. */
ENTRY(_start)

PHDRS
{
    text PT_LOAD FLAGS(5);   /* Read and execute. */
    rodata PT_LOAD FLAGS(4); /* Read-only constants. */
    data PT_LOAD FLAGS(6);   /* Writable data and zero-filled BSS. */
}

SECTIONS
{
    /* Multiboot kernels are normally loaded at 1 MiB. */
    . = 1M;

    .text ALIGN(4K) :
    {
        /* Keep the Multiboot header early so bootloaders can locate it. */
        KEEP(*(.multiboot))
        /* Collect executable code, including compiler-generated subsections. */
        *(.text .text.*)
    } :text

    .rodata ALIGN(4K) :
    {
        /* Read-only constants and lookup tables. */
        *(.rodata .rodata.*)
    } :rodata

    .data ALIGN(4K) :
    {
        /* Initialized writable globals. */
        *(.data .data.*)
    } :data

    .bss ALIGN(4K) :
    {
        /* Zero-initialized globals and common symbols occupy memory, not file data. */
        *(COMMON)
        *(.bss .bss.*)
    } :data

    /DISCARD/ :
    {
        /* Unwind metadata is unnecessary because kernel panics abort. */
        *(.eh_frame)
        *(.comment)
    }
}
