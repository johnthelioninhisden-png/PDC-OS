//! Validated syscall boundary for PDC-OS processes.
//!
//! The ABI is reached through int 0x80.  RAX is the syscall number and
//! RBX/RCX/RDX carry up to three arguments.  Results are returned in RAX.
//! Negative values are reserved for the errno-style ABI errors below.

/// Syscall ABI generation.  Numbers above remain numerically stable within a
/// major version; entries may be deprecated only by minor or patch releases.
pub const ABI_MAJOR: u16 = 0;
pub const ABI_MINOR: u16 = 2;
pub const ABI_PATCH: u16 = 0;
pub const ABI_VERSION: crate::kernel::core::InterfaceVersion =
    crate::kernel::core::InterfaceVersion {
        major: ABI_MAJOR,
        minor: ABI_MINOR,
        patch: ABI_PATCH,
    };

pub const SYS_WRITE: usize = 1;
pub const SYS_EXIT: usize = 2;
pub const SYS_GETPID: usize = 3;
pub const SYS_GETTICKS: usize = 4;
pub const SYS_ALLOC_FRAME: usize = 5;
pub const SYS_FORK: usize = 6;
pub const SYS_WAIT: usize = 7;
pub const SYS_EXEC_ELF: usize = 8;
pub const SYS_OPEN: usize = 9;
pub const SYS_READ: usize = 10;
pub const SYS_CLOSE: usize = 11;
pub const SYS_IPC_SEND: usize = 12;
pub const SYS_IPC_RECV: usize = 13;
pub const SYS_SIGNAL_SEND: usize = 14;
pub const SYS_SIGNAL_RECV: usize = 15;
pub const SYS_GETUID: usize = 16;
pub const SYS_SETUID: usize = 17;
pub const SYS_SHM_CREATE: usize = 18;
pub const SYS_SHM_ATTACH: usize = 19;
pub const SYS_SHM_WRITE: usize = 20;
pub const SYS_SHM_READ: usize = 21;
pub const SYS_CONSOLE_FOREGROUND_SET: usize = 22;
pub const SYS_CONSOLE_FOREGROUND_GET: usize = 23;

// Descriptive aliases keep the ABI easy to use while retaining numeric stability.
pub const SYS_KILL: usize = SYS_SIGNAL_SEND;
pub const SYS_SIGRECV: usize = SYS_SIGNAL_RECV;
pub const SYS_PIPE: usize = SYS_IPC_SEND;

pub const ENOSYS: isize = -38;
pub const EBADF: isize = -9;
pub const EFAULT: isize = -14;
pub const ENOMEM: isize = -12;
pub const ESRCH: isize = -3;
pub const ENOENT: isize = -2;
pub const ENOTDIR: isize = -20;
pub const EACCES: isize = -13;
pub const EPERM: isize = -1;
pub const EINVAL: isize = -22;

#[derive(Clone, Copy)]
pub struct SyscallInfo {
    pub number: usize,
    pub name: &'static str,
    pub argument_count: u8,
}

const SYSCALL_TABLE: [SyscallInfo; 23] = [
    SyscallInfo {
        number: SYS_WRITE,
        name: "write",
        argument_count: 2,
    },
    SyscallInfo {
        number: SYS_EXIT,
        name: "exit",
        argument_count: 1,
    },
    SyscallInfo {
        number: SYS_GETPID,
        name: "getpid",
        argument_count: 0,
    },
    SyscallInfo {
        number: SYS_GETTICKS,
        name: "getticks",
        argument_count: 0,
    },
    SyscallInfo {
        number: SYS_ALLOC_FRAME,
        name: "alloc_frame",
        argument_count: 0,
    },
    SyscallInfo {
        number: SYS_FORK,
        name: "fork",
        argument_count: 0,
    },
    SyscallInfo {
        number: SYS_WAIT,
        name: "wait",
        argument_count: 1,
    },
    SyscallInfo {
        number: SYS_EXEC_ELF,
        name: "exec_elf",
        argument_count: 2,
    },
    SyscallInfo {
        number: SYS_OPEN,
        name: "open",
        argument_count: 2,
    },
    SyscallInfo {
        number: SYS_READ,
        name: "read",
        argument_count: 3,
    },
    SyscallInfo {
        number: SYS_CLOSE,
        name: "close",
        argument_count: 1,
    },
    SyscallInfo {
        number: SYS_IPC_SEND,
        name: "ipc_send",
        argument_count: 3,
    },
    SyscallInfo {
        number: SYS_IPC_RECV,
        name: "ipc_recv",
        argument_count: 3,
    },
    SyscallInfo {
        number: SYS_SIGNAL_SEND,
        name: "signal_send",
        argument_count: 2,
    },
    SyscallInfo {
        number: SYS_SIGNAL_RECV,
        name: "signal_recv",
        argument_count: 1,
    },
    SyscallInfo {
        number: SYS_GETUID,
        name: "getuid",
        argument_count: 0,
    },
    SyscallInfo {
        number: SYS_SETUID,
        name: "setuid",
        argument_count: 1,
    },
    SyscallInfo {
        number: SYS_SHM_CREATE,
        name: "shm_create",
        argument_count: 2,
    },
    SyscallInfo {
        number: SYS_SHM_ATTACH,
        name: "shm_attach",
        argument_count: 1,
    },
    SyscallInfo {
        number: SYS_SHM_WRITE,
        name: "shm_write",
        argument_count: 3,
    },
    SyscallInfo {
        number: SYS_SHM_READ,
        name: "shm_read",
        argument_count: 3,
    },
    SyscallInfo {
        number: SYS_CONSOLE_FOREGROUND_SET,
        name: "console_foreground_set",
        argument_count: 1,
    },
    SyscallInfo {
        number: SYS_CONSOLE_FOREGROUND_GET,
        name: "console_foreground_get",
        argument_count: 0,
    },
];

/// Look up ABI metadata without dispatching a syscall or constructing pointers.
pub fn syscall_info(number: usize) -> Option<SyscallInfo> {
    SYSCALL_TABLE
        .iter()
        .find(|entry| entry.number == number)
        .copied()
}

#[derive(Clone, Copy, PartialEq)]
pub enum ProcessState {
    Empty,
    Running,
    Exited,
}

#[derive(Clone, Copy)]
pub struct Process {
    pub pid: usize,
    pub parent_pid: usize,
    pub address_space: usize,
    pub entry: usize,
    pub exit_code: isize,
    pub state: ProcessState,
    pub uid: u16,
}

struct ProcessTable {
    processes: [Process; 16],
    next_pid: usize,
}

impl ProcessTable {
    const fn new() -> Self {
        const EMPTY: Process = Process {
            pid: 0,
            parent_pid: 0,
            address_space: 0,
            entry: 0,
            exit_code: 0,
            state: ProcessState::Empty,
            uid: 0,
        };
        Self {
            processes: [EMPTY; 16],
            next_pid: 2,
        }
    }
}

use crate::interrupts::IrqLock;

static PROCESSES: IrqLock<ProcessTable> = IrqLock::new(ProcessTable::new());
static CURRENT_PID: IrqLock<usize> = IrqLock::new(1);
static FOREGROUND_CONSOLE_PID: IrqLock<usize> = IrqLock::new(2);

/// Initialize the first process before the terminal starts dispatching commands.
pub fn init() {
    let mut processes = PROCESSES.lock();
    processes.processes[0] = Process {
        pid: 1,
        parent_pid: 0,
        address_space: 0,
        entry: 0,
        exit_code: 0,
        state: ProcessState::Running,
        uid: 0,
    };
    drop(processes);
    // Publish the kernel-owned process record after the fixed syscall table
    // has established PID 1.  The bootstrap process has no private CR3 yet.
    let _ = crate::kernel::process::PROCESS_TABLE.create_process_with_id(1, 0, 0, 0);
    let _ = crate::kernel::process::PROCESS_TABLE
        .set_process_state(1, crate::kernel::process::ProcessState::Running);
}

/// Register a userspace or service process behind the syscall/process ABI.
/// This is used by startup services and shell/process launchers.
pub fn spawn_process(parent_pid: usize, _name: &str, uid: u16) -> isize {
    let mut processes = PROCESSES.lock();
    if processes
        .processes
        .iter()
        .any(|process| process.pid == parent_pid)
    {
        // Reserve the process-table slot before allocating an address space.
        // This prevents a full process table from leaking a page-table root.
        let Some(slot_index) = processes
            .processes
            .iter()
            .position(|process| process.state == ProcessState::Empty)
        else {
            return ESRCH;
        };
        let address_space = match crate::memory::create_address_space() {
            Some(address_space) => address_space,
            None => return ENOMEM,
        };
        let pid = processes.next_pid;
        processes.next_pid = processes.next_pid.saturating_add(1);
        processes.processes[slot_index] = Process {
            pid,
            parent_pid,
            address_space,
            entry: 0,
            exit_code: 0,
            state: ProcessState::Running,
            uid,
        };
        let _ = crate::kernel::process::PROCESS_TABLE.create_process_with_id(
            pid,
            parent_pid,
            address_space,
            uid,
        );
        let _ = crate::kernel::process::PROCESS_TABLE
            .set_process_state(pid, crate::kernel::process::ProcessState::Running);
        return pid as isize;
    }
    ESRCH
}

/// Dispatch a syscall number to the current in-kernel placeholder operation.
///
/// The ABI is not yet safe for untrusted ring-3 callers: argument validation,
/// process permissions, and a real entry mechanism must be added before that.
pub fn dispatch(number: usize, arg: usize) -> isize {
    // Unknown calls use a negative result, matching the current simple error
    // convention. A mature ABI should define stable error codes centrally.
    match number {
        SYS_WRITE => write_bytes(arg as *const u8, arg.wrapping_shr(32) as usize),
        SYS_EXIT => sys_exit(arg as isize),
        SYS_GETPID => sys_getpid(),
        SYS_GETTICKS => crate::interrupts::tick_count() as isize,
        SYS_ALLOC_FRAME => crate::memory::allocate_frame().map_or(-1, |address| address as isize),
        SYS_FORK => sys_fork(),
        SYS_WAIT => sys_wait(arg),
        SYS_EXEC_ELF => ENOSYS,
        SYS_OPEN => ENOSYS,
        SYS_READ => ENOSYS,
        SYS_CLOSE => ENOSYS,
        _ => ENOSYS,
    }
}

/// Dispatch the full int 0x80 ABI, validating pointer-shaped arguments here.
pub fn dispatch_args(number: usize, arg1: usize, arg2: usize, _arg3: usize) -> isize {
    match number {
        SYS_WRITE => sys_write(arg1, arg2),
        SYS_EXIT => sys_exit(arg1 as isize),
        SYS_GETPID => sys_getpid(),
        SYS_GETTICKS => crate::interrupts::tick_count() as isize,
        SYS_ALLOC_FRAME => {
            crate::memory::allocate_frame().map_or(ENOMEM, |address| address as isize)
        }
        SYS_FORK => sys_fork(),
        SYS_WAIT => sys_wait(arg1),
        SYS_EXEC_ELF => sys_exec_elf(arg1, arg2),
        SYS_OPEN => sys_open(arg1, arg2),
        SYS_READ => sys_read(arg1, arg2, _arg3),
        SYS_CLOSE => sys_close(arg1),
        SYS_IPC_SEND => sys_ipc_send(arg1, arg2, _arg3),
        SYS_IPC_RECV => sys_ipc_recv(arg1, arg2, _arg3),
        SYS_SIGNAL_SEND => sys_signal_send(arg1, arg2),
        SYS_SIGNAL_RECV => sys_signal_recv(arg1),
        SYS_GETUID => sys_getuid(),
        SYS_SETUID => sys_setuid(arg1),
        SYS_SHM_CREATE => sys_shm_create(arg1, arg2),
        SYS_SHM_ATTACH => sys_shm_attach(arg1),
        SYS_SHM_WRITE => sys_shm_write(arg1, arg2, _arg3),
        SYS_SHM_READ => sys_shm_read(arg1, arg2, _arg3),
        SYS_CONSOLE_FOREGROUND_SET => sys_console_foreground_set(arg1),
        SYS_CONSOLE_FOREGROUND_GET => sys_console_foreground_get(),
        _ => ENOSYS,
    }
}

fn valid_user_range(pointer: usize, length: usize) -> bool {
    // The current ABI has no Ring 3 entry or per-process pointer policy yet.
    // Checked endpoint arithmetic prevents wraparound even in the in-kernel
    // prototype, and the canonical user boundary remains non-negotiable.
    pointer >= crate::memory::USER_VIRTUAL_START
        && pointer < crate::memory::USER_VIRTUAL_END
        && pointer
            .checked_add(length)
            .is_some_and(|end| end <= crate::memory::USER_VIRTUAL_END)
}

fn sys_write(pointer: usize, length: usize) -> isize {
    // Handle 1 is the kernel console in this prototype.  Handles 0 and 2 are
    // also accepted by the console ABI and return the byte count on success.
    if length == 0 {
        return 0;
    }
    // Console output belongs to the foreground process.  This turns a stray
    // background write into an explicit permission error instead of silently
    // mixing its text into the owner-visible shell line.
    if current_pid() != *FOREGROUND_CONSOLE_PID.lock() {
        return EACCES;
    }
    if !valid_user_range(pointer, length) {
        return EFAULT;
    }
    let bytes = unsafe { core::slice::from_raw_parts(pointer as *const u8, length) };
    for byte in bytes {
        crate::print!("{}", *byte as char);
    }
    length as isize
}

#[derive(Clone, Copy)]
struct OpenHandle {
    fd: usize,
    owner_pid: usize,
    path: [u8; crate::fs::NAME_LIMIT],
    path_len: usize,
}

impl OpenHandle {
    const fn empty() -> Self {
        Self {
            fd: 0,
            owner_pid: 0,
            path: [0; crate::fs::NAME_LIMIT],
            path_len: 0,
        }
    }
}

static OPEN_HANDLES: IrqLock<[OpenHandle; 16]> = IrqLock::new([const { OpenHandle::empty() }; 16]);
static NEXT_FD: IrqLock<usize> = IrqLock::new(3);

fn sys_open(pointer: usize, length: usize) -> isize {
    if !valid_user_range(pointer, length) || length == 0 {
        return EFAULT;
    }
    let bytes = unsafe { core::slice::from_raw_parts(pointer as *const u8, length) };
    if bytes.len() > crate::fs::NAME_LIMIT {
        return ENOENT;
    }
    let path = match core::str::from_utf8(bytes) {
        Ok(path) => path,
        Err(_) => return ENOENT,
    };
    let mut absolute = [0u8; crate::fs::NAME_LIMIT];
    let normalized;
    if path.starts_with('/') {
        if path.len() > crate::fs::NAME_LIMIT {
            return ENOENT;
        }
        absolute[..path.len()].copy_from_slice(path.as_bytes());
        normalized = unsafe { core::str::from_utf8_unchecked(&absolute[..path.len()]) };
    } else {
        if path.len() + 1 > crate::fs::NAME_LIMIT {
            return ENOENT;
        }
        absolute[0] = b'/';
        absolute[1..1 + path.len()].copy_from_slice(path.as_bytes());
        normalized = unsafe { core::str::from_utf8_unchecked(&absolute[..1 + path.len()]) };
    }

    // `/dev/console` is a kernel-owned character device rather than a VFS
    // regular file.  VFS paths continue to use the normal existence probe.
    let exists = if normalized == "/dev/console" {
        true
    } else {
        let mut probe = [0u8; crate::fs::FILE_LIMIT];
        crate::fs::with(|vfs| vfs.read(normalized, &mut probe).is_ok())
    };
    if !exists {
        return ENOENT;
    }

    let mut handles = OPEN_HANDLES.lock();
    let slot = match handles.iter_mut().position(|handle| handle.fd == 0) {
        Some(slot) => slot,
        None => return ENOMEM,
    };
    let mut next_fd = NEXT_FD.lock();
    let fd = *next_fd;
    *next_fd = next_fd.saturating_add(1);
    handles[slot].fd = fd;
    handles[slot].owner_pid = current_pid();
    handles[slot].path_len = normalized.len();
    handles[slot].path[..normalized.len()].copy_from_slice(normalized.as_bytes());
    fd as isize
}

fn sys_read(fd: usize, pointer: usize, length: usize) -> isize {
    if !valid_user_range(pointer, length) {
        return EFAULT;
    }
    if length == 0 {
        return 0;
    }
    // Standard input and an explicitly opened `/dev/console` both read the
    // foreground keyboard stream.  Navigation and control keys are exposed as
    // terminal escape/control bytes rather than hardware scancodes.
    let is_console = fd == 0 || {
        let handles = OPEN_HANDLES.lock();
        handles
            .iter()
            .find(|handle| handle.fd == fd && handle.owner_pid == current_pid())
            .is_some_and(|handle| {
                handle.path_len == "/dev/console".len()
                    && &handle.path[..handle.path_len] == b"/dev/console"
            })
    };
    if is_console {
        if current_pid() != *FOREGROUND_CONSOLE_PID.lock() {
            return EACCES;
        }
        let mut copied = 0usize;
        while copied < length {
            let Some(key) = crate::userspace::keyboard::try_pop() else {
                break;
            };
            let byte = match key {
                crate::userspace::keyboard::Key::Character(byte) => Some(byte),
                crate::userspace::keyboard::Key::Enter => Some(b'\n'),
                crate::userspace::keyboard::Key::Backspace => Some(0x08),
                crate::userspace::keyboard::Key::ControlC => Some(3),
                crate::userspace::keyboard::Key::ControlL => Some(12),
                crate::userspace::keyboard::Key::Escape => Some(0x1B),
                _ => None,
            };
            if let Some(byte) = byte {
                unsafe {
                    (pointer as *mut u8).add(copied).write(byte);
                }
                copied += 1;
            }
        }
        return copied as isize;
    }
    let handles = OPEN_HANDLES.lock();
    let handle = match handles
        .iter()
        .find(|handle| handle.fd == fd && handle.owner_pid == current_pid())
    {
        Some(handle) => *handle,
        None => return EBADF,
    };
    let path = unsafe { core::str::from_utf8_unchecked(&handle.path[..handle.path_len]) };
    let mut data = [0u8; crate::fs::FILE_LIMIT];
    let read = match crate::fs::with(|vfs| vfs.read(path, &mut data)) {
        Ok(read) => read,
        Err(error) => match error {
            crate::fs::FsError::NotFile => return ENOTDIR,
            _ => return ENOENT,
        },
    };
    let copy_length = read.min(length);
    unsafe {
        core::ptr::copy_nonoverlapping(data.as_ptr(), pointer as *mut u8, copy_length);
    }
    copy_length as isize
}

fn sys_close(fd: usize) -> isize {
    let mut handles = OPEN_HANDLES.lock();
    if let Some(handle) = handles
        .iter_mut()
        .find(|handle| handle.fd == fd && handle.owner_pid == current_pid())
    {
        *handle = OpenHandle::empty();
        return 0;
    }
    EBADF
}

fn process_uid(pid: usize) -> Option<u16> {
    let processes = PROCESSES.lock();
    processes
        .processes
        .iter()
        .find(|process| process.pid == pid)
        .map(|process| process.uid)
}

fn sys_getuid() -> isize {
    current_uid() as isize
}

fn sys_setuid(uid: usize) -> isize {
    if uid > u16::MAX as usize {
        return EINVAL;
    }
    let requested = uid as u16;
    // The root identity may assign identities; ordinary processes cannot
    // change their credentials through this syscall.
    if current_uid() != 0 {
        return EPERM;
    }
    crate::userspace::services::with(|services| {
        if !services.users.exists(requested) {
            return EPERM;
        }
        if set_current_uid(requested) {
            0
        } else {
            ESRCH
        }
    })
}

fn sys_ipc_send(receiver: usize, pointer: usize, length: usize) -> isize {
    if !valid_user_range(pointer, length) {
        return EFAULT;
    }
    if length > crate::userspace::services::MESSAGE_LIMIT {
        return EINVAL;
    }
    let sender = current_pid();
    let sender_uid = current_uid();
    let Some(receiver_uid) = process_uid(receiver) else {
        return ESRCH;
    };
    let bytes = unsafe { core::slice::from_raw_parts(pointer as *const u8, length) };
    crate::userspace::services::with(|services| {
        services
            .ipc
            .send(sender, sender_uid, receiver, receiver_uid, bytes)
    })
}

fn sys_ipc_recv(pointer: usize, length: usize, _unused: usize) -> isize {
    if !valid_user_range(pointer, length) {
        return EFAULT;
    }
    let mut buffer = [0u8; crate::userspace::services::MESSAGE_LIMIT];
    let received = crate::userspace::services::with(|services| {
        services.ipc.receive(current_pid(), &mut buffer)
    });
    if received > 0 {
        let copy_length = received as usize;
        if copy_length > length {
            return EINVAL;
        }
        unsafe {
            core::ptr::copy_nonoverlapping(buffer.as_ptr(), pointer as *mut u8, copy_length);
        }
    }
    received
}

fn sys_signal_send(pid: usize, signal: usize) -> isize {
    let Some(target_uid) = process_uid(pid) else {
        return ESRCH;
    };
    crate::userspace::services::with(|services| {
        services
            .signals
            .send(current_uid(), target_uid, pid, signal as u32)
    })
}

fn sys_signal_recv(pid: usize) -> isize {
    if pid != 0 && pid != current_pid() {
        return EPERM;
    }
    let target = if pid == 0 { current_pid() } else { pid };
    crate::userspace::services::with(|services| services.signals.take(target))
}

fn sys_shm_create(key: usize, mode: usize) -> isize {
    if mode > u16::MAX as usize {
        return EINVAL;
    }
    let uid = current_uid();
    crate::userspace::services::with(|services| services.shared.create(key, uid, mode as u16))
}

fn sys_shm_attach(key: usize) -> isize {
    crate::userspace::services::with(|services| services.shared.attach(key, current_uid()))
}

fn sys_shm_write(key: usize, pointer: usize, length: usize) -> isize {
    if !valid_user_range(pointer, length) {
        return EFAULT;
    }
    if length > crate::userspace::services::SHARED_RESOURCE_LIMIT {
        return EINVAL;
    }
    let bytes = unsafe { core::slice::from_raw_parts(pointer as *const u8, length) };
    crate::userspace::services::with(|services| services.shared.write(key, current_uid(), bytes))
}

fn sys_shm_read(key: usize, pointer: usize, length: usize) -> isize {
    if !valid_user_range(pointer, length) {
        return EFAULT;
    }
    let mut buffer = [0u8; crate::userspace::services::SHARED_RESOURCE_LIMIT];
    let read = crate::userspace::services::with(|services| {
        services.shared.read(key, current_uid(), &mut buffer)
    });
    if read > 0 {
        let copy_length = read as usize;
        if copy_length > length {
            return EINVAL;
        }
        unsafe {
            core::ptr::copy_nonoverlapping(buffer.as_ptr(), pointer as *mut u8, copy_length);
        }
    }
    read
}

fn write_bytes(pointer: *const u8, length: usize) -> isize {
    if pointer.is_null() {
        return EFAULT;
    }
    sys_write(pointer as usize, length)
}

fn sys_getpid() -> isize {
    *CURRENT_PID.lock() as isize
}

pub fn current_pid() -> usize {
    *CURRENT_PID.lock()
}

/// Get the process that owns `/dev/console` for keyboard input and console output.
pub fn foreground_console_pid() -> usize {
    *FOREGROUND_CONSOLE_PID.lock()
}

/// Change foreground console ownership after verifying the target process exists.
pub fn set_foreground_console_pid(pid: usize) -> bool {
    let processes = PROCESSES.lock();
    if !processes
        .processes
        .iter()
        .any(|process| process.pid == pid && process.state == ProcessState::Running)
    {
        return false;
    }
    *FOREGROUND_CONSOLE_PID.lock() = pid;
    crate::userspace::terminal::set_foreground_pid(pid);
    true
}

fn sys_console_foreground_set(pid: usize) -> isize {
    if pid == 0 {
        return EINVAL;
    }
    if set_foreground_console_pid(pid) {
        0
    } else {
        ESRCH
    }
}

fn sys_console_foreground_get() -> isize {
    foreground_console_pid() as isize
}

pub fn current_uid() -> u16 {
    let pid = current_pid();
    let processes = PROCESSES.lock();
    processes
        .processes
        .iter()
        .find(|process| process.pid == pid)
        .map(|process| process.uid)
        .unwrap_or(0)
}

pub fn set_current_uid(uid: u16) -> bool {
    let pid = current_pid();
    let mut processes = PROCESSES.lock();
    if let Some(process) = processes
        .processes
        .iter_mut()
        .find(|process| process.pid == pid)
    {
        process.uid = uid;
        true
    } else {
        false
    }
}

fn sys_exit(exit_code: isize) -> isize {
    let pid = *CURRENT_PID.lock();
    let mut processes = PROCESSES.lock();
    for process in processes.processes.iter_mut() {
        if process.pid == pid && process.state == ProcessState::Running {
            process.exit_code = exit_code;
            process.state = ProcessState::Exited;
            crate::scheduler::SCHEDULER.with(|scheduler| scheduler.cleanup(pid));
            crate::userspace::services::with(|services| services.signals.clear(pid));
            // Destroy the kernel process record now: it owns the address
            // space and every thread stack, so cleanup must not be duplicated.
            crate::kernel::process::PROCESS_TABLE
                .set_process_state(pid, crate::kernel::process::ProcessState::Exiting);
            crate::kernel::process::PROCESS_TABLE.destroy_process(pid);
            if pid == *FOREGROUND_CONSOLE_PID.lock() {
                *FOREGROUND_CONSOLE_PID.lock() = process.parent_pid;
                crate::userspace::terminal::set_foreground_pid(process.parent_pid);
            }
            return 0;
        }
    }
    0
}

fn sys_fork() -> isize {
    let parent_pid = *CURRENT_PID.lock();
    let parent_uid = process_uid(parent_pid).unwrap_or(0);
    let mut processes = PROCESSES.lock();
    let slot_index = match processes
        .processes
        .iter()
        .position(|process| process.state == ProcessState::Empty)
    {
        Some(index) => index,
        None => return ESRCH,
    };
    let address_space = match crate::memory::create_address_space() {
        Some(address_space) => address_space,
        None => return ENOMEM,
    };
    let pid = processes.next_pid;
    processes.next_pid = processes.next_pid.saturating_add(1);
    processes.processes[slot_index] = Process {
        pid,
        parent_pid,
        address_space,
        entry: 0,
        exit_code: 0,
        state: ProcessState::Running,
        uid: parent_uid,
    };
    let _ = crate::kernel::process::PROCESS_TABLE.create_process_with_id(
        pid,
        parent_pid,
        address_space,
        parent_uid,
    );
    let _ = crate::kernel::process::PROCESS_TABLE
        .set_process_state(pid, crate::kernel::process::ProcessState::Running);
    pid as isize
}

fn sys_wait(pid: usize) -> isize {
    let current = current_pid();
    if pid == 0 {
        let processes = PROCESSES.lock();
        if let Some(process) = processes
            .processes
            .iter()
            .find(|process| process.state == ProcessState::Exited && process.parent_pid == current)
        {
            return process.exit_code;
        }
        return ESRCH;
    }
    let mut processes = PROCESSES.lock();
    for process in processes.processes.iter_mut() {
        if process.pid == pid {
            if process.parent_pid != current {
                return EPERM;
            }
            if process.state == ProcessState::Exited {
                let exit_code = process.exit_code;
                *process = Process {
                    pid: 0,
                    parent_pid: 0,
                    address_space: 0,
                    entry: 0,
                    exit_code: 0,
                    state: ProcessState::Empty,
                    uid: 0,
                };
                return exit_code;
            }
            return ESRCH;
        }
    }
    ESRCH
}

fn read_u16(bytes: &[u8], offset: usize) -> Option<u16> {
    let value = bytes.get(offset..offset.checked_add(2)?)?;
    Some(u16::from_le_bytes([value[0], value[1]]))
}

fn read_u32(bytes: &[u8], offset: usize) -> Option<u32> {
    let value = bytes.get(offset..offset.checked_add(4)?)?;
    Some(u32::from_le_bytes([value[0], value[1], value[2], value[3]]))
}

fn read_u64(bytes: &[u8], offset: usize) -> Option<u64> {
    let value = bytes.get(offset..offset.checked_add(8)?)?;
    Some(u64::from_le_bytes([
        value[0], value[1], value[2], value[3], value[4], value[5], value[6], value[7],
    ]))
}

/// Load a bounded ELF64 image into a newly isolated process address space.
fn sys_exec_elf(pointer: usize, length: usize) -> isize {
    if !valid_user_range(pointer, length) {
        return EFAULT;
    }
    // Reject oversized inputs before parsing.  This keeps malformed or
    // adversarial ELF data bounded by the kernel ABI.
    const ELF_SIZE_LIMIT: usize = 4 * 1024 * 1024;
    if length == 0 || length > ELF_SIZE_LIMIT {
        return EINVAL;
    }
    let elf = unsafe { core::slice::from_raw_parts(pointer as *const u8, length) };

    // ELF64 ident: magic, class=64-bit, little-endian, and current ABI version.
    if elf.len() < 64 || &elf[0..4] != b"\x7fELF" || elf[4] != 2 || elf[5] != 1 || elf[6] != 1 {
        return EINVAL;
    }
    if read_u16(elf, 16) != Some(2) || read_u16(elf, 18) != Some(62) {
        return EINVAL;
    }

    let entry = match read_u64(elf, 24) {
        Some(entry) if entry >= 0x1000 => entry as usize,
        _ => return EINVAL,
    };
    let program_offset = match read_u64(elf, 32) {
        Some(offset) if offset <= usize::MAX as u64 => offset as usize,
        _ => return EINVAL,
    };
    let program_entry_size = match read_u16(elf, 54) {
        Some(size) if size as usize >= 56 => size as usize,
        _ => return EINVAL,
    };
    let program_count = match read_u16(elf, 56) {
        Some(count) => count as usize,
        _ => return EINVAL,
    };
    if program_count > 1024 {
        return EINVAL;
    }

    let address_space = match crate::memory::create_address_space() {
        Some(address_space) => address_space,
        None => return ENOMEM,
    };

    let mut loaded = false;
    let mut entry_executable = false;
    for index in 0..program_count {
        let index_offset = match index.checked_mul(program_entry_size) {
            Some(value) => value,
            None => {
                crate::memory::destroy_address_space(address_space);
                return EINVAL;
            }
        };
        let offset = match program_offset.checked_add(index_offset) {
            Some(offset) => offset,
            None => {
                crate::memory::destroy_address_space(address_space);
                return EINVAL;
            }
        };
        let header = match elf.get(offset..offset.saturating_add(program_entry_size)) {
            Some(header) => header,
            None => {
                crate::memory::destroy_address_space(address_space);
                return EINVAL;
            }
        };

        let segment_type = read_u32(header, 0).unwrap_or(0);
        if segment_type != 1 {
            continue;
        }
        let segment_flags = read_u32(header, 4).unwrap_or(0);
        let file_offset = read_u64(header, 8).unwrap_or(0) as usize;
        let virtual_address = read_u64(header, 16).unwrap_or(0) as usize;
        let file_size = read_u64(header, 32).unwrap_or(0) as usize;
        let memory_size = read_u64(header, 40).unwrap_or(0) as usize;

        if virtual_address < 0x1000
            || memory_size == 0
            || file_size > memory_size
            || file_offset.checked_add(file_size).is_none()
            || virtual_address.checked_add(memory_size).is_none()
        {
            crate::memory::destroy_address_space(address_space);
            return EINVAL;
        }
        if entry >= virtual_address
            && entry < virtual_address + memory_size
            && segment_flags & 1 != 0
        {
            entry_executable = true;
        }
        let source = match elf.get(file_offset..file_offset + file_size) {
            Some(source) => source,
            None => {
                crate::memory::destroy_address_space(address_space);
                return EINVAL;
            }
        };

        let start_page = virtual_address & !(crate::memory::frame_size() - 1);
        let Some(end_address) = virtual_address.checked_add(memory_size) else {
            crate::memory::destroy_address_space(address_space);
            return EINVAL;
        };
        let Some(end_page) = end_address
            .checked_add(crate::memory::frame_size() - 1)
            .map(|value| value & !(crate::memory::frame_size() - 1))
        else {
            crate::memory::destroy_address_space(address_space);
            return EINVAL;
        };
        for page_address in (start_page..end_page).step_by(crate::memory::frame_size()) {
            let Some(physical) = crate::memory::allocate_frame() else {
                crate::memory::destroy_address_space(address_space);
                return ENOMEM;
            };
            unsafe {
                core::ptr::write_bytes(physical as *mut u8, 0, crate::memory::frame_size());
            }

            let page_virtual_start = page_address;
            let destination_offset = if page_address < virtual_address {
                virtual_address - page_address
            } else {
                0
            };
            let available = crate::memory::frame_size().saturating_sub(destination_offset);
            let source_offset = page_address.checked_sub(virtual_address).unwrap_or(0);
            let copy_len = if source_offset < file_size {
                available.min(file_size - source_offset)
            } else {
                0
            };
            if copy_len > 0 {
                unsafe {
                    core::ptr::copy_nonoverlapping(
                        source.as_ptr().add(source_offset),
                        (physical + destination_offset) as *mut u8,
                        copy_len,
                    );
                }
            }

            let flags = if segment_flags & 2 != 0 {
                crate::memory::PageFlags::UserReadWrite
            } else if segment_flags & 1 != 0 {
                crate::memory::PageFlags::UserExecutable
            } else {
                crate::memory::PageFlags::UserReadOnly
            };
            if !crate::memory::map_page(address_space, page_virtual_start, physical, flags) {
                crate::memory::deallocate_frame(physical);
                crate::memory::destroy_address_space(address_space);
                return ENOMEM;
            }
        }
        loaded = true;
    }

    if !loaded || !entry_executable {
        crate::memory::destroy_address_space(address_space);
        return EINVAL;
    }

    let pid = *CURRENT_PID.lock();
    let mut processes = PROCESSES.lock();
    for process in processes.processes.iter_mut() {
        if process.pid == pid {
            process.address_space = address_space;
            process.entry = entry;
            let _ = crate::kernel::process::PROCESS_TABLE.replace_address_space(pid, address_space);
            return entry as isize;
        }
    }
    crate::memory::destroy_address_space(address_space);
    ESRCH
}

/// Return a bounded process snapshot for diagnostics and process-management code.
pub fn processes_snapshot() -> [Process; 16] {
    let mut result = [Process {
        pid: 0,
        parent_pid: 0,
        address_space: 0,
        entry: 0,
        exit_code: 0,
        state: ProcessState::Empty,
        uid: 0,
    }; 16];
    let processes = PROCESSES.lock();
    result.copy_from_slice(&processes.processes);
    result
}
