# PDC-OS x86-64 memory layout

The supported bootstrap layout is:

- `0x0000000000001000` and above: canonical userspace pages
- `0x0000000000100000`: kernel/GRUB load area
- `0x0000000000400000` through `0x0000000000400000 + 64 MiB`: bootstrap
  identity mapping
- `0x0000000000000000` through `0x0000000000000fff`: userspace null page
- `0xffff800000000000` and above: reserved canonical kernel address range

Kernel and userspace mappings are separated in page-table permissions. Kernel
pages do not carry the x86 user bit. User mappings use the user and write
permission bits explicitly and executable mappings remain supported only when
requested. 4 KiB frames are the ownership unit.

The bootstrap PML4 may remain shared only for kernel mappings. A process
address space has its own PML4 root and must clean all private intermediate
tables and leaves it owns.
