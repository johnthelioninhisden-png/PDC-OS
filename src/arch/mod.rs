//! Architecture backends.
//!
//! Architecture-specific code lives below this module.  Architecture-neutral
//! kernel subsystems must use [`crate::hal`] contracts rather than this module.

pub mod x86_64;
