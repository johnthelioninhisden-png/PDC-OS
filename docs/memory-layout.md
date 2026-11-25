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

## Region ownership

- Physical frames are 4 KiB allocation units below `0x001000000000` (4 GiB).
- Frame zero is reserved and is never a valid allocation or typed address.
- The address `0x1000` starts user mappings; `0x0000800000000000` is the
  exclusive upper user boundary.
- Canonical addresses at or above `0xffff800000000000` are kernel mappings.
- Address-space PML4 roots and private 4-level intermediate tables are owned
  by the address-space object.
- Mapped 4 KiB leaf frames are reclaimed together with private address spaces;
  kernel/shared mappings must be registered separately before such cleanup is
  permitted.
- Unmapping leaves the physical frame available to explicit frame ownership;
  it invalidates only the removed virtual page.
- Address-space destruction switches an active root back to the bootstrap CR3
  before reclaiming PML4 frames.
- Userspace thread stacks are 4 KiB mappings in the canonical userspace region.
  Kernel thread stacks are separate 4 KiB physical frames with a second guard
  frame reserved for the ownership model.
