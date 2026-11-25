//! Process and thread ownership model.
//!
//! Processes own address spaces and resources.  Threads own execution contexts
//! and kernel stacks; the scheduler only selects among runnable threads.

use crate::interrupts::IrqLock;
use core::sync::atomic::{AtomicUsize, Ordering};

pub const KERNEL_STACK_SIZE: usize = 4096;
pub const USER_STACK_SIZE: usize = 4096;
pub const USER_STACK_REGION_START: usize = 0x0007_ffff_0000;
pub const USER_STACK_REGION_END: usize = crate::memory::USER_VIRTUAL_END;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProcessState {
    Empty,
    Initializing,
    Running,
    Exiting,
    Reaped,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ThreadState {
    Empty,
    Initializing,
    Ready,
    Running,
    Blocked,
    Exiting,
    Reaped,
}

#[derive(Clone, Copy)]
pub struct Process {
    pub id: usize,
    pub parent_id: usize,
    pub state: ProcessState,
    pub address_space_id: usize,
    pub uid: u16,
    pub thread_count: usize,
}

#[derive(Clone, Copy)]
pub struct Thread {
    pub id: usize,
    pub process_id: usize,
    pub state: ThreadState,
    pub kernel_stack_base: usize,
    pub kernel_stack_top: usize,
    pub kernel_stack_guard: usize,
    pub user_stack_base: usize,
    pub user_stack_physical: usize,
}

pub struct ProcessTable {
    processes: IrqLock<[Process; 16]>,
    threads: IrqLock<[Thread; 32]>,
    next_process: AtomicUsize,
    next_thread: AtomicUsize,
    next_user_stack: AtomicUsize,
}

impl ProcessTable {
    pub const fn new() -> Self {
        const EMPTY_PROCESS: Process = Process {
            id: 0,
            parent_id: 0,
            state: ProcessState::Empty,
            address_space_id: 0,
            uid: 0,
            thread_count: 0,
        };
        const EMPTY_THREAD: Thread = Thread {
            id: 0,
            process_id: 0,
            state: ThreadState::Empty,
            kernel_stack_base: 0,
            kernel_stack_top: 0,
            kernel_stack_guard: 0,
            user_stack_base: 0,
            user_stack_physical: 0,
        };
        Self {
            processes: IrqLock::new([EMPTY_PROCESS; 16]),
            threads: IrqLock::new([EMPTY_THREAD; 32]),
            next_process: AtomicUsize::new(1),
            next_thread: AtomicUsize::new(1),
            next_user_stack: AtomicUsize::new(0),
        }
    }

    pub fn create_process(&self, address_space_id: usize) -> Option<usize> {
        self.create_process_with_parent(0, address_space_id)
    }

    /// Create a process with an explicit PID and parent for syscall integration.
    pub fn create_process_with_id(
        &self,
        id: usize,
        parent_id: usize,
        address_space_id: usize,
        uid: u16,
    ) -> Option<usize> {
        let mut processes = self.processes.lock();
        for process in processes.iter_mut() {
            if process.state == ProcessState::Empty {
                *process = Process {
                    id,
                    parent_id,
                    state: ProcessState::Initializing,
                    address_space_id,
                    uid,
                    thread_count: 0,
                };
                return Some(id);
            }
        }
        None
    }

    pub fn create_process_with_parent(
        &self,
        parent_id: usize,
        address_space_id: usize,
    ) -> Option<usize> {
        let id = self.next_process.fetch_add(1, Ordering::Relaxed);
        self.create_process_with_id(id, parent_id, address_space_id, 0)
    }

    pub fn allocate_user_stack(&self, address_space_id: usize) -> Option<(usize, usize)> {
        if address_space_id == 0
            || crate::memory::address_space_mapping_count(address_space_id).is_none()
        {
            return None;
        }
        let physical = crate::memory::allocate_frame()?;
        let slot = self.next_user_stack.fetch_add(1, Ordering::Relaxed);
        let base = USER_STACK_REGION_START
            .checked_add(slot.checked_mul(USER_STACK_SIZE)?)
            .filter(|address| *address < USER_STACK_REGION_END)?;
        if !crate::memory::map_page(
            address_space_id,
            base,
            physical,
            crate::memory::PageFlags::UserReadWrite,
        ) {
            let _ = crate::memory::deallocate_frame(physical);
            return None;
        }
        Some((base, physical))
    }

    pub fn create_thread(&self, process_id: usize) -> Option<usize> {
        let address_space_id = self.address_space(process_id)?;
        let stack = crate::memory::allocate_frame()?;
        let guard = match crate::memory::allocate_frame() {
            Some(frame) => frame,
            None => {
                let _ = crate::memory::deallocate_frame(stack);
                return None;
            }
        };
        let (user_stack_base, user_stack_physical) =
            match self.allocate_user_stack(address_space_id) {
                Some(stack) => stack,
                None => {
                    let _ = crate::memory::deallocate_frame(stack);
                    let _ = crate::memory::deallocate_frame(guard);
                    return None;
                }
            };
        let id = self.next_thread.fetch_add(1, Ordering::Relaxed);
        let mut threads = self.threads.lock();
        for thread in threads.iter_mut() {
            if thread.state == ThreadState::Empty {
                *thread = Thread {
                    id,
                    process_id,
                    state: ThreadState::Initializing,
                    kernel_stack_base: stack,
                    kernel_stack_top: stack + KERNEL_STACK_SIZE,
                    kernel_stack_guard: guard,
                    user_stack_base,
                    user_stack_physical,
                };
                let mut processes = self.processes.lock();
                if let Some(process) = processes
                    .iter_mut()
                    .find(|process| process.id == process_id)
                {
                    process.thread_count = process.thread_count.saturating_add(1);
                }
                return Some(id);
            }
        }
        // Roll back both frames when the fixed thread table is full.
        let _ = crate::memory::deallocate_frame(stack);
        let _ = crate::memory::deallocate_frame(guard);
        let _ = crate::memory::unmap_page(address_space_id, user_stack_base);
        let _ = crate::memory::deallocate_frame(user_stack_physical);
        None
    }

    pub fn set_process_state(&self, id: usize, state: ProcessState) -> bool {
        let mut processes = self.processes.lock();
        if let Some(process) = processes.iter_mut().find(|process| process.id == id) {
            let allowed = matches!(
                (process.state, state),
                (ProcessState::Empty, ProcessState::Initializing)
                    | (ProcessState::Initializing, ProcessState::Running)
                    | (ProcessState::Initializing, ProcessState::Exiting)
                    | (ProcessState::Running, ProcessState::Exiting)
                    | (ProcessState::Exiting, ProcessState::Reaped)
                    | (ProcessState::Reaped, ProcessState::Empty)
            );
            if !allowed {
                return false;
            }
            process.state = state;
            true
        } else {
            false
        }
    }

    pub fn set_thread_state(&self, id: usize, state: ThreadState) -> bool {
        let mut threads = self.threads.lock();
        if let Some(thread) = threads.iter_mut().find(|thread| thread.id == id) {
            let allowed = matches!(
                (thread.state, state),
                (ThreadState::Empty, ThreadState::Initializing)
                    | (ThreadState::Initializing, ThreadState::Ready)
                    | (ThreadState::Initializing, ThreadState::Exiting)
                    | (ThreadState::Ready, ThreadState::Running)
                    | (ThreadState::Ready, ThreadState::Blocked)
                    | (ThreadState::Ready, ThreadState::Exiting)
                    | (ThreadState::Running, ThreadState::Ready)
                    | (ThreadState::Running, ThreadState::Blocked)
                    | (ThreadState::Running, ThreadState::Exiting)
                    | (ThreadState::Blocked, ThreadState::Ready)
                    | (ThreadState::Blocked, ThreadState::Exiting)
                    | (ThreadState::Exiting, ThreadState::Reaped)
                    | (ThreadState::Reaped, ThreadState::Empty)
            );
            if !allowed {
                return false;
            }
            thread.state = state;
            true
        } else {
            false
        }
    }

    pub fn process_exists(&self, id: usize) -> bool {
        self.processes
            .lock()
            .iter()
            .any(|process| process.id == id && process.state != ProcessState::Empty)
    }

    pub fn address_space(&self, process_id: usize) -> Option<usize> {
        self.processes
            .lock()
            .iter()
            .find(|process| process.id == process_id && process.state != ProcessState::Empty)
            .map(|process| process.address_space_id)
    }

    /// Replace a process address space without allowing orphaned page tables.
    pub fn replace_address_space(&self, process_id: usize, address_space_id: usize) -> bool {
        let mut processes = self.processes.lock();
        if let Some(process) = processes
            .iter_mut()
            .find(|process| process.id == process_id && process.state != ProcessState::Empty)
        {
            let old = process.address_space_id;
            process.address_space_id = address_space_id;
            drop(processes);
            if old != 0 && old != address_space_id {
                let _ = crate::memory::destroy_address_space(old);
            }
            true
        } else {
            false
        }
    }

    pub fn process_snapshot(&self, id: usize) -> Option<Process> {
        self.processes
            .lock()
            .iter()
            .find(|process| process.id == id)
            .copied()
    }

    pub fn thread_snapshot(&self, id: usize) -> Option<Thread> {
        self.threads
            .lock()
            .iter()
            .find(|thread| thread.id == id)
            .copied()
    }

    pub fn destroy_thread(&self, id: usize) -> bool {
        let mut threads = self.threads.lock();
        if let Some(thread) = threads.iter_mut().find(|thread| thread.id == id) {
            let process_id = thread.process_id;
            let address_space_id = self.address_space(process_id).unwrap_or(0);
            let _ = crate::memory::deallocate_frame(thread.kernel_stack_base);
            let _ = crate::memory::deallocate_frame(thread.kernel_stack_guard);
            if thread.user_stack_physical != 0 {
                if address_space_id != 0 {
                    let _ = crate::memory::unmap_page(address_space_id, thread.user_stack_base);
                }
                let _ = crate::memory::deallocate_frame(thread.user_stack_physical);
            }
            *thread = Thread {
                id: 0,
                process_id: 0,
                state: ThreadState::Empty,
                kernel_stack_base: 0,
                kernel_stack_top: 0,
                kernel_stack_guard: 0,
                user_stack_base: 0,
                user_stack_physical: 0,
            };
            let mut processes = self.processes.lock();
            if let Some(process) = processes
                .iter_mut()
                .find(|process| process.id == process_id)
            {
                process.thread_count = process.thread_count.saturating_sub(1);
            }
            true
        } else {
            false
        }
    }

    pub fn destroy_process(&self, id: usize) -> bool {
        let mut threads = self.threads.lock();
        for thread in threads.iter_mut() {
            if thread.process_id == id {
                let _ = crate::memory::deallocate_frame(thread.kernel_stack_base);
                let _ = crate::memory::deallocate_frame(thread.kernel_stack_guard);
                if thread.user_stack_physical != 0 {
                    let address_space_id = self.address_space(id).unwrap_or(0);
                    if address_space_id != 0 {
                        let _ = crate::memory::unmap_page(address_space_id, thread.user_stack_base);
                    }
                    let _ = crate::memory::deallocate_frame(thread.user_stack_physical);
                }
                *thread = Thread {
                    id: 0,
                    process_id: 0,
                    state: ThreadState::Empty,
                    kernel_stack_base: 0,
                    kernel_stack_top: 0,
                    kernel_stack_guard: 0,
                    user_stack_base: 0,
                    user_stack_physical: 0,
                };
            }
        }
        drop(threads);

        let mut processes = self.processes.lock();
        if let Some(process) = processes.iter_mut().find(|process| process.id == id) {
            let address_space_id = process.address_space_id;
            let _ = crate::memory::destroy_address_space(address_space_id);
            *process = Process {
                id: 0,
                parent_id: 0,
                state: ProcessState::Empty,
                address_space_id: 0,
                uid: 0,
                thread_count: 0,
            };
            true
        } else {
            false
        }
    }

    pub fn process_count(&self) -> usize {
        self.processes
            .lock()
            .iter()
            .filter(|process| process.state != ProcessState::Empty)
            .count()
    }

    pub fn thread_count(&self) -> usize {
        self.threads
            .lock()
            .iter()
            .filter(|thread| thread.state != ThreadState::Empty)
            .count()
    }
}

pub static PROCESS_TABLE: ProcessTable = ProcessTable::new();
