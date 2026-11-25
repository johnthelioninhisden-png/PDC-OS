//! Preemptive round-robin task scheduler.
//!
//! Tasks with an executable entry point receive a private 4 KiB kernel stack.
//! Timer interrupts copy the interrupted register frame into the active task
//! and restore the next runnable task's frame.

use crate::interrupts::IrqLock;

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum TaskState {
    Empty,
    Ready,
    Running,
    Blocked,
}

#[derive(Clone, Copy)]
pub struct Task {
    pub id: usize,
    pub state: TaskState,
    pub name: &'static str,
}

// This table-based prototype tracks task metadata but does not yet save CPU
// contexts or switch stacks. Its fixed capacity keeps scheduler operations
// allocation-free.
pub struct Scheduler {
    tasks: [Task; 16],
    contexts: [[u64; 22]; 16],
    executable: [bool; 16],
    initialized: [bool; 16],
    stack_bases: [usize; 16],
    bootstrap_context: [u64; 22],
    bootstrap_initialized: bool,
    current: usize,
}

impl Scheduler {
    const fn new() -> Self {
        const EMPTY_TASK: Task = Task {
            id: 0,
            state: TaskState::Empty,
            name: "",
        };
        Self {
            tasks: [EMPTY_TASK; 16],
            contexts: [[0; 22]; 16],
            executable: [false; 16],
            initialized: [false; 16],
            stack_bases: [0; 16],
            bootstrap_context: [0; 22],
            bootstrap_initialized: false,
            current: 0,
        }
    }

    /// Diagnostic snapshot of the active task slot.
    pub fn diagnostic_task(&self) -> (usize, &'static str, TaskState) {
        if self.current < self.tasks.len() {
            let task = self.tasks[self.current];
            (task.id, task.name, task.state)
        } else {
            (0, "kernel/bootstrap", TaskState::Running)
        }
    }

    pub fn spawn(&mut self, id: usize, name: &'static str) -> bool {
        // Reuse the first vacant slot. Return false when the fixed table is full;
        // a future scheduler can replace this with an owned task queue.
        for task in &mut self.tasks {
            if task.state == TaskState::Empty {
                *task = Task {
                    id,
                    state: TaskState::Ready,
                    name,
                };
                return true;
            }
        }
        false
    }

    /// Spawn a task with a real kernel entry point and a private 4 KiB stack.
    pub fn spawn_with_entry(
        &mut self,
        id: usize,
        name: &'static str,
        entry: extern "C" fn() -> !,
    ) -> bool {
        for index in 0..self.tasks.len() {
            if self.tasks[index].state == TaskState::Empty {
                // Frames are identity-mapped by the bootstrap, so task stack
                // addresses can be used directly by x86-64 interrupt frames.
                let Some(stack_base) = crate::memory::allocate_frame() else {
                    return false;
                };
                let stack_top = stack_base + crate::memory::frame_size();
                let mut context = [0u64; 22];
                context[17] = entry as usize as u64; // Synthetic IRET instruction pointer.
                context[18] = 0x08;                  // Kernel code segment.
                context[19] = 0x202;                 // Interrupts enabled, reserved flag set.
                context[20] = stack_top as u64;      // Synthetic interrupt stack pointer.
                context[21] = 0x10;                  // Kernel data segment.

                self.tasks[index] = Task {
                    id,
                    state: TaskState::Ready,
                    name,
                };
                self.contexts[index] = context;
                self.executable[index] = true;
                self.initialized[index] = true;
                self.stack_bases[index] = stack_base;
                return true;
            }
        }
        false
    }

    pub fn tick(&mut self) {
        // Round-robin search begins after the previous slot and wraps once.
        // Entry-free tasks retain metadata-only cooperative behavior.
        let old = self.current;
        for offset in 1..=self.tasks.len() {
            let candidate = (old + offset) % self.tasks.len();
            if self.tasks[candidate].state == TaskState::Ready
                && self.executable[candidate]
            {
                if old != 0 {
                    self.tasks[old].state = TaskState::Ready;
                }
                self.tasks[candidate].state = TaskState::Running;
                self.current = candidate;
                return;
            }
        }
    }

    /// Timer-driven preemption: save the current interrupt frame and install
    /// the next runnable task frame.  The bootstrap frame is slot zero.
    pub fn tick_context(&mut self, interrupt_rsp: *mut u64) -> *mut u64 {
        let switch_start = crate::performance::timestamp();
        let old = self.current;
        let mut candidate = None;
        for offset in 1..=self.tasks.len() {
            let index = (old + offset) % self.tasks.len();
            if (index == 0 && self.bootstrap_initialized)
                || (self.tasks[index].state == TaskState::Ready
                    && self.executable[index])
            {
                candidate = Some(index);
                break;
            }
        }

        let Some(next) = candidate else {
            // No runnable task: keep executing the interrupted context.
            return interrupt_rsp;
        };

        unsafe {
            if old == 0 {
                // Slot zero represents the kernel/bootstrap context, not a task
                // table entry, and is saved separately on the first switch.
                core::ptr::copy_nonoverlapping(
                    interrupt_rsp,
                    self.bootstrap_context.as_mut_ptr(),
                    22,
                );
                self.bootstrap_initialized = true;
            } else if self.executable[old] {
                core::ptr::copy_nonoverlapping(
                    interrupt_rsp,
                    self.contexts[old].as_mut_ptr(),
                    22,
                );
            }

            let next_context = if next == 0 {
                &mut self.bootstrap_context
            } else {
                &mut self.contexts[next]
            };
            core::ptr::copy_nonoverlapping(
                next_context.as_ptr(),
                interrupt_rsp,
                22,
            );
        }

        if old != 0 && self.tasks[old].state == TaskState::Running {
            self.tasks[old].state = TaskState::Ready;
        }
        self.tasks[next].state = TaskState::Running;
        self.current = next;
        crate::performance::record_context_switch(switch_start);
        interrupt_rsp
    }

    pub fn block(&mut self, id: usize) -> bool {
        for task in &mut self.tasks {
            if task.id == id && task.state != TaskState::Empty {
                task.state = TaskState::Blocked;
                return true;
            }
        }
        false
    }

    pub fn block_current(&mut self) -> bool {
        let index = self.current;
        if index != 0 && self.tasks[index].state != TaskState::Empty {
            self.tasks[index].state = TaskState::Blocked;
            return true;
        }
        false
    }

    pub fn wake(&mut self, id: usize) -> bool {
        for task in &mut self.tasks {
            if task.id == id && task.state == TaskState::Blocked {
                task.state = TaskState::Ready;
                return true;
            }
        }
        false
    }

    pub fn cleanup(&mut self, id: usize) -> bool {
        for index in 0..self.tasks.len() {
            if self.tasks[index].id == id && self.tasks[index].state != TaskState::Empty {
                if self.stack_bases[index] != 0 {
                    crate::memory::deallocate_frame(self.stack_bases[index]);
                }
                self.tasks[index].state = TaskState::Empty;
                self.tasks[index].name = "";
                self.contexts[index] = [0; 22];
                self.executable[index] = false;
                self.initialized[index] = false;
                self.stack_bases[index] = 0;
                if self.current == index {
                    self.current = 0;
                }
                return true;
            }
        }
        false
    }

    pub fn cleanup_current(&mut self) -> bool {
        if self.current == 0 {
            return false;
        }
        let id = self.tasks[self.current].id;
        self.cleanup(id)
    }

    pub fn current_task(&self) -> Task {
        self.tasks[self.current]
    }

    pub fn task_count(&self) -> usize {
        self.tasks
            .iter()
            .filter(|task| task.state != TaskState::Empty)
            .count()
    }

    pub fn tasks_snapshot(&self) -> [Task; 16] {
        // Copy the fixed table so readers can format task data without retaining
        // a borrow of the scheduler's internal array.
        self.tasks
    }
}

pub struct SchedulerCell(IrqLock<Scheduler>);

impl SchedulerCell {
    pub const fn new() -> Self {
        Self(IrqLock::new(Scheduler::new()))
    }

    pub fn with<R>(&self, action: impl FnOnce(&mut Scheduler) -> R) -> R {
        let mut scheduler = self.0.lock();
        action(&mut scheduler)
    }
}

unsafe impl Sync for SchedulerCell {}

pub static SCHEDULER: SchedulerCell = SchedulerCell::new();
