# Subsystem dependencies

| Subsystem | Depends on |
| --- | --- |
| serial/recovery | assembly COM1 setup |
| CPU/HAL | serial |
| memory | boot memory map, HAL CPU |
| scheduler | memory allocator |
| interrupts | memory, PIC/PIT, scheduler |
| filesystem | memory, VFS |
| services | process tables, interrupt-safe locks |
| userspace | process tables, filesystem, terminal |
| terminal | framebuffer/VGA, keyboard, interrupt-safe lock |

Hardware-specific code must not be called from architecture-independent
scheduler, process, syscall, or filesystem implementation.
