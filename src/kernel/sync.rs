//! Kernel synchronization contracts and fixed-size wait queues.
//!
//! Locks are held only over bounded critical sections.  Sleeping tasks first
//! enter the scheduler's Blocked state, then enqueue on a wait queue.  A
//! wakeup removes the queue entry and transitions the task to Ready.

use crate::interrupts::IrqLock;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WaitReason {
    None,
    Mutex,
    Sleep,
    Io,
    Device,
}

#[derive(Clone, Copy)]
struct Waiter {
    task_id: usize,
    reason: WaitReason,
}

impl Waiter {
    const fn empty() -> Self {
        Self {
            task_id: 0,
            reason: WaitReason::None,
        }
    }
}

/// Allocation-free wait queue shared by scheduler consumers.
pub struct WaitQueue {
    waiters: IrqLock<[Waiter; 16]>,
}

impl WaitQueue {
    pub const fn new() -> Self {
        const EMPTY: Waiter = Waiter {
            task_id: 0,
            reason: WaitReason::None,
        };
        Self {
            waiters: IrqLock::new([EMPTY; 16]),
        }
    }

    pub fn wait(&self, task_id: usize, reason: WaitReason) -> bool {
        if task_id == 0 {
            return false;
        }
        let mut waiters = self.waiters.lock();
        // A task has at most one entry in a wait queue.
        if waiters.iter().any(|waiter| waiter.task_id == task_id) {
            return false;
        }
        if let Some(slot) = waiters.iter_mut().find(|waiter| waiter.task_id == 0) {
            *slot = Waiter { task_id, reason };
            true
        } else {
            false
        }
    }

    pub fn remove(&self, task_id: usize) -> bool {
        let mut waiters = self.waiters.lock();
        if let Some(slot) = waiters.iter_mut().find(|waiter| waiter.task_id == task_id) {
            *slot = Waiter::empty();
            true
        } else {
            false
        }
    }

    pub fn wake_one(&self) -> Option<usize> {
        let mut waiters = self.waiters.lock();
        if let Some(slot) = waiters.iter_mut().find(|waiter| waiter.task_id != 0) {
            let task_id = slot.task_id;
            *slot = Waiter::empty();
            Some(task_id)
        } else {
            None
        }
    }

    pub fn wake_all(&self) -> usize {
        let mut waiters = self.waiters.lock();
        let mut count = 0;
        for slot in waiters.iter_mut() {
            if slot.task_id != 0 {
                count += 1;
                *slot = Waiter::empty();
            }
        }
        count
    }

    pub fn contains(&self, task_id: usize) -> bool {
        self.waiters
            .lock()
            .iter()
            .any(|waiter| waiter.task_id == task_id)
    }

    pub fn reason(&self, task_id: usize) -> Option<WaitReason> {
        self.waiters
            .lock()
            .iter()
            .find(|waiter| waiter.task_id == task_id)
            .map(|waiter| waiter.reason)
    }

    pub fn len(&self) -> usize {
        self.waiters
            .lock()
            .iter()
            .filter(|waiter| waiter.task_id != 0)
            .count()
    }
}
