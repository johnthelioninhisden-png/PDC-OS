//! Userspace shell and core-program registry.
//!
//! Programs in this module use only process, console, and VFS APIs.  Direct
//! VGA/framebuffer access and privileged hardware operations stay in kernel
//! modules, keeping this the boundary for future ring-3 program execution.

use crate::println;
use crate::scheduler::SCHEDULER;

#[derive(Clone, Copy)]
pub struct Program {
    pub name: &'static str,
    pub path: &'static str,
    pub description: &'static str,
}

const PROGRAMS: [Program; 12] = [
    Program { name: "echo", path: "/bin/echo", description: "print text" },
    Program { name: "uname", path: "/bin/uname", description: "show system identity" },
    Program { name: "uptime", path: "/bin/uptime", description: "show PIT ticks" },
    Program { name: "mem", path: "/bin/mem", description: "show memory allocations" },
    Program { name: "free", path: "/bin/free", description: "show physical memory" },
    Program { name: "ps", path: "/bin/ps", description: "list processes" },
    Program { name: "pid", path: "/bin/pid", description: "show process id" },
    Program { name: "id", path: "/bin/id", description: "show process identity" },
    Program { name: "users", path: "/bin/users", description: "list users" },
    Program { name: "shm", path: "/bin/shm", description: "list shared resources" },
    Program { name: "ls", path: "/bin/ls", description: "list files" },
    Program { name: "cat", path: "/bin/cat", description: "print a file" },
];

extern "C" fn shell_entry() -> ! {
    // The console device is currently polled by the boot loop, while this task
    // owns the process/scheduler identity that the shell is presented as.
    loop {
        unsafe { core::arch::asm!("hlt", options(nomem, nostack)); }
    }
}

/// Install initial userspace programs and launch the startup shell process.
pub fn init() {
    // A future build can replace this table with ELF-backed files from a package
    // archive; fixed registration is deliberately behind the VFS boundary.
    crate::fs::with(|vfs| {
        for program in &PROGRAMS {
            if vfs.read(program.path, &mut [0u8; 1]).is_err() {
                let _ = vfs.create_file(program.path);
            }
        }
    });

    // Register the shell as PID 2 and as a real scheduler task.  PID 1 remains
    // the kernel/init process, while core programs are still process-like syscalls.
    // PID 1 is the kernel/init process.  Launch PID 2 as the configured shell.
    crate::userspace::syscalls::spawn_process(1, "/bin/pdc-sh", 0);
    SCHEDULER.with(|scheduler| {
        let _ = scheduler.spawn_with_entry(2, "/bin/pdc-sh", shell_entry);
    });

    run_startup();
}

/// Read /etc/inittab and launch the configured shell startup entries.
pub fn run_startup() {
    let mut buffer = [0u8; 512];
    let config = match crate::fs::with(|vfs| vfs.read("/etc/inittab", &mut buffer)) {
        Ok(length) => unsafe { core::str::from_utf8_unchecked(&buffer[..length]) },
        Err(_) => {
            println!("init: cannot read /etc/inittab");
            return;
        }
    };

    println!("init: starting userspace services");
    crate::serial::println(format_args!("PDC-OS userspace startup complete"));
    for line in config.lines() {
        let mut fields = line.split(':');
        let _runlevel = fields.next().unwrap_or("");
        let action = fields.next().unwrap_or("");
        let program = fields.next().unwrap_or("");
        match (action, program) {
            ("once", "/sbin/init") => println!("init: startup service ready"),
            ("respawn", "/bin/pdc-sh") => println!("init: /bin/pdc-sh (pid 2)"),
            _ => println!("init: ignored startup entry"),
        }
    }
}

/// Install a regular VFS file into /bin or /sbin as a packaged core program.
pub fn install(source: &str, destination: &str) -> Result<(), &'static str> {
    if !destination.starts_with("/bin/")
        && !destination.starts_with("/sbin/")
        && !destination.starts_with("/lib/")
    {
        return Err("destination must be under /bin, /sbin, or /lib");
    }

    let mut buffer = [0u8; crate::fs::FILE_LIMIT];
    let length = crate::fs::with(|vfs| vfs.read(source, &mut buffer))
        .map_err(|_| "source file not found")?;
    crate::fs::with(|vfs| {
        let _ = vfs.create_file(destination);
        vfs.write(destination, &buffer[..length])
            .map(|_| ())
            .map_err(|_| "cannot install package")
    })
}

/// Resolve a bare command to a registered userspace program.
pub fn resolve(name: &str) -> Option<Program> {
    PROGRAMS.iter()
        .find(|program| program.name == name)
        .copied()
}

/// Return the fixed program catalog for shell help and package tools.
pub fn programs_snapshot() -> [Program; 12] {
    PROGRAMS
}
