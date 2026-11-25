//! Built-in terminal commands.
//!
//! The command parser is intentionally small.  Each command receives argument
//! text and writes output through the terminal.  This is the correct place to
//! add commands such as `ls`, `cat`, `ps`, and `mount` before filesystem code.
//! For larger commands, return an exit status and keep kernel-only operations
//! behind `os::syscalls.rs` rather than exposing framebuffer or hardware APIs.

use crate::println;

const DEFAULT_COLOR: u8 = 0x07;
const INFO_COLOR: u8 = 0x0B;
const DIRECTORY_COLOR: u8 = 0x0A;
const ERROR_COLOR: u8 = 0x0C;

fn set_color(color: u8) {
    crate::userspace::terminal::set_color(color);
}

fn print_directory_entries(path: &str, entries: &[crate::fs::DirEntry], count: usize) {
    if count == 0 {
        println!("{}: directory is empty", path);
        return;
    }
    for entry in entries.iter().take(count) {
        if entry.kind == crate::fs::NodeKind::Directory {
            set_color(DIRECTORY_COLOR);
            crate::print!("{}/  ", entry.name());
            set_color(DEFAULT_COLOR);
        } else {
            crate::print!("{}  ", entry.name());
        }
    }
    println!();
}

fn print_fs_error(command: &str, path: &str, error: crate::fs::FsError) {
    set_color(ERROR_COLOR);
    match error {
        crate::fs::FsError::NotFound => {
            println!("{}: {}: no such file or directory", command, path)
        }
        crate::fs::FsError::NotDirectory => println!("{}: {}: not a directory", command, path),
        crate::fs::FsError::NotFile => println!("{}: {}: is a directory", command, path),
        crate::fs::FsError::Exists => println!("{}: mount point already exists", command),
        crate::fs::FsError::NameTooLong => println!("{}: {}: name too long", command, path),
        crate::fs::FsError::TableFull => println!("{}: mount table full", command),
        crate::fs::FsError::NotMounted => println!("{}: storage source is not mounted", command),
        crate::fs::FsError::TooLarge => println!("{}: {}: file too large", command, path),
    }
    set_color(DEFAULT_COLOR);
}

fn read_file_operand(command: &str, args: &str, buffer: &mut [u8]) -> Option<usize> {
    if args.is_empty() {
        set_color(ERROR_COLOR);
        println!("{}: missing file operand", command);
        set_color(DEFAULT_COLOR);
        return None;
    }

    let mut operands = args.split_whitespace();
    let operand = operands.next().unwrap_or("");
    if operands.next().is_some() {
        set_color(ERROR_COLOR);
        println!("{}: expected one file operand", command);
        set_color(DEFAULT_COLOR);
        return None;
    }
    let mut absolute = [0u8; crate::fs::NAME_LIMIT + 1];
    let path = if operand.starts_with('/') {
        if operand.len() > crate::fs::NAME_LIMIT {
            set_color(ERROR_COLOR);
            println!("{}: path too long", command);
            set_color(DEFAULT_COLOR);
            return None;
        }
        operand
    } else {
        if operand.len() >= crate::fs::NAME_LIMIT {
            set_color(ERROR_COLOR);
            println!("{}: path too long", command);
            set_color(DEFAULT_COLOR);
            return None;
        }
        absolute[0] = b'/';
        absolute[1..1 + operand.len()].copy_from_slice(operand.as_bytes());
        unsafe { core::str::from_utf8_unchecked(&absolute[..1 + operand.len()]) }
    };

    match crate::fs::with(|vfs| vfs.read(path, buffer)) {
        Ok(length) => Some(length),
        Err(error) => {
            print_fs_error(command, path, error);
            None
        }
    }
}

pub fn execute(line: &str) {
    // Parse the command name and the remaining argument text. A future parser
    // can add quoting, escape sequences, flags, or redirection here so all
    // built-in commands share the same input rules.
    let mut parts = line.split_whitespace();
    let command = parts.next().unwrap_or("");
    let args = line
        .strip_prefix(command)
        .unwrap_or("")
        .trim();

    // Dispatch built-in commands directly from this match. Each arm should own
    // one command; move complex behavior to a helper or module. Adding a command
    // requires an arm here and a matching entry in "help".
    match command {
        // An empty command is harmless and also makes this function robust when
        // called outside the terminal's normal Enter-key path.
        "" => {}
        "help" => {
            // This is the built-in command catalog. Future command registration
            // could generate both this list and the dispatch table.
            set_color(INFO_COLOR);
            println!("PDC-OS commands:");
            set_color(DEFAULT_COLOR);
            println!("  help       show this command list");
            println!("  clear      clear the VGA terminal");
            println!("  history    show recent commands");
            println!("  display    show the active display mode");
            println!("  echo TEXT  print text");
            println!("  uname      show kernel and architecture");
            println!("  uptime     show PIT tick count");
            println!("  pwd        show the current directory");
            println!("  perf       show scheduler, memory, syscall, I/O counters");
            println!("  security   run the bounded 10.7 adversarial review");
            println!("  bench      run bounded in-kernel performance smoke benchmarks");
            println!("  mem        show frame allocations");
            println!("  free       show physical memory summary");
            println!("  ps         list scheduler tasks");
            println!("  ls [PATH]  list a filesystem path");
            println!("  cat FILE   print a filesystem file");
            println!("  head FILE  print the first 10 lines");
            println!("  wc FILE    count lines and bytes");
            println!("  mount      show mounted filesystems");
            println!("  pid        show the current process id");
            println!("  id         show the current process identity");
            println!("  users      list registered kernel identities");
            println!("  shm        list shared resource keys");
            println!("  halt       halt the CPU");
            println!("  reboot     reset supported QEMU/PC hardware");
            println!("  shutdown   power off supported emulated hardware");
            println!("  run PROG   execute a registered userspace program");
            println!("  install SRC DEST  install a VFS file as a package");
            println!("  bin        list registered userspace programs");
        }
        // Terminal operations are delegated so command code does not manipulate
        // VGA internals directly.
        "clear" => crate::userspace::terminal::clear(),
        "history" => crate::userspace::terminal::print_history(),
        // Expose display capabilities only; mode changes and device access stay
        // in the kernel until a protected graphics ABI is designed.
        "display" => {
            let display = crate::drivers::display_info();
            println!(
                "Output: {} {}x{}; graphics {}",
                display.mode.name(),
                display.width,
                display.height,
                if display.graphics_enabled {
                    "enabled"
                } else {
                    "disabled"
                }
            );
        }
        // args intentionally remains one string, preserving spaces between words.
        // A future echo implementation could add newline flags or escape expansion.
        "echo" => println!("{}", args),
        // System identity is currently compile-time static text. Replace this
        // with kernel/build metadata once version and architecture configuration
        // are centralized.
        "uname" => println!("PDC-OS 0.1.0 x86_64"),
        // PIT tick reporting is read-only. A richer uptime command could convert
        // ticks to elapsed time after timer calibration is implemented.
        "uptime" => println!("PIT ticks: {}", crate::interrupts::tick_count()),
        "pwd" => println!("/"),
        "perf" => {
            let perf = crate::performance::snapshot();
            println!(
                "Scheduler: {} context switches; {} TSC total / {} max",
                perf.context_switches,
                perf.context_switch_total_tsc,
                perf.context_switch_max_tsc
            );
            println!(
                "IRQ: {} events; {} TSC wait / {} max",
                perf.irq_events,
                perf.irq_wait_total_tsc,
                perf.irq_wait_max_tsc
            );
            println!(
                "Syscalls: {} calls; {} TSC total / {} max",
                perf.syscalls,
                perf.syscall_total_tsc,
                perf.syscall_max_tsc
            );
            println!(
                "Disk cache: {} hits, {} misses",
                perf.disk_cache_hits,
                perf.disk_cache_misses
            );
            println!(
                "I/O: serial {} bytes/{} writes max={} TSC; VGA {} bytes/{} writes max={} TSC",
                perf.serial_bytes,
                perf.serial_writes,
                perf.serial_max_tsc,
                perf.vga_bytes,
                perf.vga_writes,
                perf.vga_max_tsc
            );
            println!(
                "Memory: frames alloc={} dealloc={} reclaim={} failures={}; page-switches={}; heap alloc={} dealloc={} fragments={}",
                crate::memory::frame_allocations(),
                crate::memory::frame_deallocations(),
                crate::memory::frame_reclaims(),
                crate::memory::frame_allocation_failures(),
                crate::memory::page_table_switches(),
                crate::memory::heap_allocations(),
                crate::memory::heap_deallocations(),
                crate::memory::heap_fragment_count()
            );
        }
        "security" => {
            let report = crate::security::run_adversarial_tests();
            println!(
                "Security: 10.7 cases={} passed={} failures={}",
                report.cases,
                report.passed,
                report.failed()
            );
        }
        "bench" => {
            // Fixed, small workloads exercise repeated process and file operations
            // without creating unbounded loops or relying on timing calibration.
            let start = crate::performance::timestamp();
            let mut file_hits = 0usize;
            let mut file_misses = 0usize;
            for _ in 0..32 {
                let mut data = [0u8; 128];
                if crate::fs::with(|vfs| vfs.read("/hello.txt", &mut data)).is_ok() {
                    file_hits += 1;
                } else {
                    file_misses += 1;
                }
            }
            let elapsed = crate::performance::timestamp().wrapping_sub(start);
            println!(
                "Bench: 32 file reads in {} TSC ({} ok, {} errors)",
                elapsed,
                file_hits,
                file_misses
            );
            println!(
                "Bench: {} PIT ticks, {} tasks",
                crate::interrupts::tick_count(),
                crate::scheduler::SCHEDULER.with(|scheduler| scheduler.task_count())
            );
        }
        // Memory reporting exposes allocator readouts only. Privileged allocation
        // or deallocation operations should remain behind syscalls and scheduler
        // boundaries rather than becoming shell commands.
        "mem" => println!(
            "Physical memory: {} bytes allocated of {} usable, {} KiB frame size",
            crate::memory::allocated_bytes(),
            crate::memory::usable_bytes(),
            crate::memory::frame_size() / 1024
        ),
        "free" => println!(
            "Physical: {} KiB total, {} KiB allocated, {} KiB available",
            crate::memory::usable_bytes() / 1024,
            crate::memory::allocated_bytes() / 1024,
            crate::memory::available_bytes() / 1024
        ),
        // Use a scheduler snapshot to keep task-table access bounded and
        // allocation-free. The fixed loop skips empty slots and prints only live
        // tasks. Future output can add PID filtering, sorting, CPU time, or
        // process arguments while reusing tasks_snapshot().
        "ps" => {
            crate::scheduler::SCHEDULER.with(|scheduler| {
                println!("PID  STATE  NAME");
                let tasks = scheduler.tasks_snapshot();
                for index in 0..16 {
                    let task = tasks[index];
                    if task.state != crate::scheduler::TaskState::Empty {
                        println!("{:>3}  {:?}  {}", task.id, task.state, task.name);
                    }
                }
            });
        }
        "ls" => {
            // A bare ls uses the root directory; relative operands are made
            // absolute for the VFS path resolver.
            let path = if args.is_empty() {
                "/"
            } else if args.starts_with('/') {
                args
            } else {
                // The kernel's in-memory shell currently has no cwd, so its
                // stable relative-path base is the filesystem root.
                if args.len() > 31 {
                    println!("ls: path too long");
                    return;
                }
                let mut absolute = [0u8; 32];
                absolute[0] = b'/';
                absolute[1..1 + args.len()].copy_from_slice(args.as_bytes());
                let path = unsafe { core::str::from_utf8_unchecked(&absolute[..1 + args.len()]) };
                let mut entries = [crate::fs::DirEntry {
                    name: [0; crate::fs::NAME_LIMIT],
                    name_len: 0,
                    kind: crate::fs::NodeKind::Directory,
                    size: 0,
                }; 16];
                match crate::fs::with(|vfs| vfs.list(path, &mut entries)) {
                    Ok(count) => print_directory_entries(path, &entries, count),
                    Err(error) => print_fs_error("ls", path, error),
                }
                return;
            };
            let mut entries = [crate::fs::DirEntry {
                name: [0; crate::fs::NAME_LIMIT],
                name_len: 0,
                kind: crate::fs::NodeKind::Directory,
                size: 0,
            }; 16];
            match crate::fs::with(|vfs| vfs.list(path, &mut entries)) {
                Ok(count) => print_directory_entries(path, &entries, count),
                Err(error) => print_fs_error("ls", path, error),
            }
        }
        "cat" => {
            if args.is_empty() {
                println!("cat: missing file operand");
            } else {
                let path = if args.starts_with('/') { args } else {
                    // Relative names are resolved from the root directory.
                    if args.len() > 31 {
                        println!("cat: path too long");
                        return;
                    }
                    let mut absolute = [0u8; 32];
                    absolute[0] = b'/';
                    absolute[1..1 + args.len()].copy_from_slice(args.as_bytes());
                    let path = unsafe { core::str::from_utf8_unchecked(&absolute[..1 + args.len()]) };
                    let mut buffer = [0u8; crate::fs::FILE_LIMIT];
                    match crate::fs::with(|vfs| vfs.read(path, &mut buffer)) {
                        Ok(length) => {
                            for byte in &buffer[..length] {
                                crate::print!("{}", *byte as char);
                            }
                        }
                        Err(error) => print_fs_error("cat", path, error),
                    }
                    return;
                };
                let mut buffer = [0u8; crate::fs::FILE_LIMIT];
                match crate::fs::with(|vfs| vfs.read(path, &mut buffer)) {
                    Ok(length) => {
                        for byte in &buffer[..length] {
                            crate::print!("{}", *byte as char);
                        }
                    }
                    Err(error) => print_fs_error("cat", path, error),
                }
            }
        }
        "head" => {
            let mut buffer = [0u8; crate::fs::FILE_LIMIT];
            if let Some(length) = read_file_operand("head", args, &mut buffer) {
                let mut lines = 0;
                for byte in &buffer[..length] {
                    crate::print!("{}", *byte as char);
                    if *byte == b'\n' {
                        lines += 1;
                        if lines == 10 {
                            break;
                        }
                    }
                }
            }
        }
        "wc" => {
            let mut buffer = [0u8; crate::fs::FILE_LIMIT];
            if let Some(length) = read_file_operand("wc", args, &mut buffer) {
                let lines = buffer[..length]
                    .iter()
                    .filter(|&&byte| byte == b'\n')
                    .count();
                println!("{} lines, {} bytes", lines, length);
            }
        }
        "mount" => {
            if args.is_empty() {
                crate::fs::with(|vfs| {
                    let (mounts, count) = vfs.mounts_snapshot();
                    if count == 0 {
                        println!("Mounted filesystems: none");
                    } else {
                        println!("SOURCE  TARGET");
                        for mount in mounts.iter().take(count) {
                            println!("{}  {}", mount.source(), mount.target());
                        }
                    }
                });
            } else {
                let mut parts = args.split_whitespace();
                let source = parts.next().unwrap_or("");
                let target = parts.next().unwrap_or("/");
                match crate::fs::with(|vfs| vfs.mount(source, target)) {
                    Ok(_) => println!("Mounted {} on {}", source, target),
                    Err(error) => print_fs_error("mount", target, error),
                }
            }
        }
        // Read the scheduler's current task through with() and expose only its
        // process identifier. This arm can later include parent/process metadata
        // or user/group identity as those concepts are implemented.
        "pid" => crate::scheduler::SCHEDULER.with(|scheduler| {
            let task = scheduler.current_task();
            println!("{}", task.id);
        }),
        "id" => println!(
            "uid={} pid={}",
            crate::userspace::syscalls::current_uid(),
            crate::userspace::syscalls::current_pid()
        ),
        "users" => crate::userspace::services::with(|services| {
            println!("UID  NAME");
            for user in services.users.snapshot().iter().filter(|user| user.active) {
                println!("{:>3}  {}", user.uid, user.name());
            }
        }),
        "shm" => crate::userspace::services::with(|services| {
            let resources = services.shared.snapshot();
            let mut count = 0;
            println!("KEY  OWNER  MODE  SIZE");
            for resource in resources.iter().filter(|resource| resource.active) {
                println!(
                    "{:>3}  {:>5}  {:>04o}  {}",
                    resource.key,
                    resource.permissions.owner,
                    resource.permissions.mode,
                    resource.size
                );
                count += 1;
            }
            if count == 0 {
                println!("No shared resources");
            }
        }),
        "bin" => {
            println!("NAME  PATH  DESCRIPTION");
            for program in crate::userspace::userspace::programs_snapshot() {
                println!("{}  {}  {}", program.name, program.path, program.description);
            }
        }
        "run" => {
            if args.is_empty() {
                println!("run: missing program operand");
            } else if let Some(program) = crate::userspace::userspace::resolve(args) {
                let pid = crate::userspace::syscalls::spawn_process(
                    crate::userspace::syscalls::current_pid(),
                    program.path,
                    crate::userspace::syscalls::current_uid(),
                );
                if pid > 0 {
                    println!("{}: started userspace process pid {}", program.name, pid);
                } else {
                    println!("run: cannot start {}", program.name);
                }
            } else {
                println!("run: unknown userspace program: {}", args);
            }
        }
        "install" => {
            let mut parts = args.split_whitespace();
            let source = parts.next().unwrap_or("");
            let destination = parts.next().unwrap_or("");
            if source.is_empty() || destination.is_empty() {
                println!("install: usage: install SOURCE DEST");
            } else {
                match crate::userspace::userspace::install(source, destination) {
                    Ok(_) => println!("Installed {} to {}", source, destination),
                    Err(error) => println!("install: {}", error),
                }
            }
        }
        "halt" => crate::recovery::halt("halt command requested"),
        "reboot" => crate::recovery::reboot(),
        "shutdown" => crate::recovery::shutdown(),
        // This fallback catches misspellings and commands that are not implemented.
        // Future shells can suggest close matches, expand aliases, or search an
        // external executable table before returning an unknown-command error.
        _ => {
            set_color(ERROR_COLOR);
            println!("Unknown command: {}. Try 'help'.", command);
            set_color(DEFAULT_COLOR);
        }
    }
}
