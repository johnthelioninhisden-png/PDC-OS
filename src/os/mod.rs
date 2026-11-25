//! User-facing PDC-OS programs and terminal services.
//!
//! Userspace-facing services and programs.
//!
//! The shell and core utilities are owned by this module instead of by the
//! kernel boot path.  They communicate through the syscall and VFS APIs;
//! hardware access and process policy remain in kernel modules.

pub mod commands;
pub mod keyboard;
pub mod syscalls;
pub mod terminal;
pub mod services;
pub mod userspace;
