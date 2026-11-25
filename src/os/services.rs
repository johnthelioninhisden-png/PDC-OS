//! Kernel services shared by processes: IPC, signals, shared memory, and identity.
//!
//! The structures deliberately use fixed-size storage.  This keeps the
//! implementation allocation-free and makes all kernel service operations
//! interrupt-safe and bounded.

use crate::interrupts::IrqLock;

pub const MESSAGE_LIMIT: usize = 256;
pub const SHARED_RESOURCE_LIMIT: usize = 512;
pub const MAX_USERS: usize = 16;
pub const MAX_IPC_MESSAGES: usize = 64;
pub const MAX_SHARED_RESOURCES: usize = 16;

pub const EPERM: isize = -1;
pub const ESRCH: isize = -3;
pub const EINVAL: isize = -22;
pub const EACCES: isize = -13;
pub const ENOMEM: isize = -12;
pub const ENOENT: isize = -2;

pub type UserId = u16;

/// Lightweight permission bits for shared resources.
#[derive(Clone, Copy, PartialEq)]
pub struct Permissions {
    pub owner: UserId,
    pub mode: u16,
}

impl Permissions {
    pub const fn new(owner: UserId, mode: u16) -> Self {
        Self { owner, mode }
    }

    pub fn permits_read(&self, uid: UserId) -> bool {
        uid == 0 || uid == self.owner || self.mode & 0o044 != 0
    }

    pub fn permits_write(&self, uid: UserId) -> bool {
        uid == 0 || uid == self.owner || self.mode & 0o022 != 0
    }
}

#[derive(Clone, Copy)]
pub struct User {
    pub uid: UserId,
    pub name: [u8; 16],
    pub name_len: usize,
    pub active: bool,
}

impl User {
    const fn empty() -> Self {
        Self {
            uid: 0,
            name: [0; 16],
            name_len: 0,
            active: false,
        }
    }

    pub fn name(&self) -> &str {
        unsafe { core::str::from_utf8_unchecked(&self.name[..self.name_len]) }
    }
}

pub struct UserTable {
    users: [User; MAX_USERS],
}

impl UserTable {
    const fn new() -> Self {
        Self { users: [const { User::empty() }; MAX_USERS] }
    }

    pub fn initialize(&mut self) {
        self.users = [const { User::empty() }; MAX_USERS];
        let mut root = User::empty();
        root.uid = 0;
        root.name[..4].copy_from_slice(b"root");
        root.name_len = 4;
        root.active = true;
        self.users[0] = root;
    }

    pub fn add(&mut self, uid: UserId, name: &str) -> bool {
        if uid == 0 || name.len() == 0 || name.len() > 16 {
            return false;
        }
        if self.users.iter().any(|user| user.active && user.uid == uid) {
            return false;
        }
        if let Some(slot) = self.users.iter_mut().find(|user| !user.active) {
            slot.uid = uid;
            slot.name[..name.len()].copy_from_slice(name.as_bytes());
            slot.name_len = name.len();
            slot.active = true;
            return true;
        }
        false
    }

    pub fn exists(&self, uid: UserId) -> bool {
        self.users.iter().any(|user| user.active && user.uid == uid)
    }

    pub fn snapshot(&self) -> [User; MAX_USERS] {
        self.users
    }
}

#[derive(Clone, Copy)]
pub struct IpcMessage {
    pub sender: usize,
    pub receiver: usize,
    pub length: usize,
    pub data: [u8; MESSAGE_LIMIT],
}

impl IpcMessage {
    const fn empty() -> Self {
        Self {
            sender: 0,
            receiver: 0,
            length: 0,
            data: [0; MESSAGE_LIMIT],
        }
    }
}

pub struct IpcTable {
    messages: [IpcMessage; MAX_IPC_MESSAGES],
}

impl IpcTable {
    const fn new() -> Self {
        Self { messages: [const { IpcMessage::empty() }; MAX_IPC_MESSAGES] }
    }

    pub fn send(&mut self, sender: usize, sender_uid: UserId, receiver: usize,
               receiver_uid: UserId, data: &[u8]) -> isize {
        if data.len() > MESSAGE_LIMIT {
            return EINVAL;
        }
        // Non-root processes may only communicate with their own UID.
        if sender_uid != 0 && sender_uid != receiver_uid {
            return EACCES;
        }
        if let Some(slot) = self.messages.iter_mut().find(|message| message.length == 0) {
            slot.sender = sender;
            slot.receiver = receiver;
            slot.length = data.len();
            slot.data[..data.len()].copy_from_slice(data);
            return data.len() as isize;
        }
        ENOMEM
    }

    pub fn receive(&mut self, receiver: usize, buffer: &mut [u8]) -> isize {
        if let Some(index) = (0..MAX_IPC_MESSAGES)
            .find(|&index| self.messages[index].receiver == receiver
                && self.messages[index].length != 0)
        {
            let message = self.messages[index];
            let length = message.length.min(buffer.len());
            buffer[..length].copy_from_slice(&message.data[..length]);
            self.messages[index] = IpcMessage::empty();
            return length as isize;
        }
        0
    }

    pub fn messages_snapshot(&self) -> [IpcMessage; MAX_IPC_MESSAGES] {
        self.messages
    }
}

pub struct SignalTable {
    pending: [u32; 16],
}

impl SignalTable {
    const fn new() -> Self {
        Self { pending: [0; 16] }
    }

    fn process_slot(pid: usize) -> Option<usize> {
        if pid == 0 || pid > 16 { None } else { Some(pid - 1) }
    }

    pub fn send(&mut self, sender_uid: UserId, target_uid: UserId, pid: usize, signal: u32) -> isize {
        if signal == 0 || signal >= 32 {
            return EINVAL;
        }
        if sender_uid != 0 && sender_uid != target_uid {
            return EACCES;
        }
        let Some(slot) = Self::process_slot(pid) else { return ESRCH };
        self.pending[slot] |= 1u32 << signal;
        0
    }

    pub fn take(&mut self, pid: usize) -> isize {
        let Some(slot) = Self::process_slot(pid) else { return ESRCH };
        if self.pending[slot] == 0 {
            return 0;
        }
        let signal = self.pending[slot].trailing_zeros();
        self.pending[slot] &= !(1u32 << signal);
        signal as isize
    }

    pub fn clear(&mut self, pid: usize) {
        if let Some(slot) = Self::process_slot(pid) {
            self.pending[slot] = 0;
        }
    }
}

#[derive(Clone, Copy)]
pub struct SharedResource {
    pub key: usize,
    pub permissions: Permissions,
    pub data: [u8; SHARED_RESOURCE_LIMIT],
    pub size: usize,
    pub active: bool,
}

impl SharedResource {
    const fn empty() -> Self {
        Self {
            key: 0,
            permissions: Permissions::new(0, 0),
            data: [0; SHARED_RESOURCE_LIMIT],
            size: 0,
            active: false,
        }
    }
}

pub struct SharedResourceTable {
    resources: [SharedResource; MAX_SHARED_RESOURCES],
}

impl SharedResourceTable {
    const fn new() -> Self {
        Self { resources: [const { SharedResource::empty() }; MAX_SHARED_RESOURCES] }
    }

    pub fn create(&mut self, key: usize, uid: UserId, mode: u16) -> isize {
        if key == 0 {
            return EINVAL;
        }
        if let Some(resource) = self.resources.iter_mut().find(|resource| resource.active && resource.key == key) {
            return if resource.permissions.owner == uid || uid == 0 {
                key as isize
            } else {
                EACCES
            };
        }
        let Some(slot) = self.resources.iter_mut().find(|resource| !resource.active) else {
            return ENOMEM;
        };
        *slot = SharedResource::empty();
        slot.key = key;
        slot.permissions = Permissions::new(uid, mode);
        slot.active = true;
        key as isize
    }

    pub fn attach(&self, key: usize, uid: UserId) -> isize {
        let Some(resource) = self.resources.iter().find(|resource| resource.active && resource.key == key) else {
            return ENOENT;
        };
        if resource.permissions.permits_read(uid) || resource.permissions.permits_write(uid) {
            key as isize
        } else {
            EACCES
        }
    }

    pub fn write(&mut self, key: usize, uid: UserId, data: &[u8]) -> isize {
        if data.len() > SHARED_RESOURCE_LIMIT {
            return EINVAL;
        }
        let Some(resource) = self.resources.iter_mut().find(|resource| resource.active && resource.key == key) else {
            return ENOENT;
        };
        if !resource.permissions.permits_write(uid) {
            return EACCES;
        }
        resource.data[..data.len()].copy_from_slice(data);
        resource.size = data.len();
        data.len() as isize
    }

    pub fn read(&self, key: usize, uid: UserId, buffer: &mut [u8]) -> isize {
        let Some(resource) = self.resources.iter().find(|resource| resource.active && resource.key == key) else {
            return ENOENT;
        };
        if !resource.permissions.permits_read(uid) {
            return EACCES;
        }
        let length = resource.size.min(buffer.len());
        buffer[..length].copy_from_slice(&resource.data[..length]);
        length as isize
    }

    pub fn snapshot(&self) -> [SharedResource; MAX_SHARED_RESOURCES] {
        self.resources
    }
}

pub struct ServiceTable {
    pub users: UserTable,
    pub ipc: IpcTable,
    pub signals: SignalTable,
    pub shared: SharedResourceTable,
}

impl ServiceTable {
    const fn new() -> Self {
        Self {
            users: UserTable::new(),
            ipc: IpcTable::new(),
            signals: SignalTable::new(),
            shared: SharedResourceTable::new(),
        }
    }

    pub fn initialize(&mut self) {
        self.users.initialize();
    }
}

static SERVICES: IrqLock<ServiceTable> = IrqLock::new(ServiceTable::new());

pub fn init() {
    SERVICES.lock().initialize();
}

pub fn with<R>(action: impl FnOnce(&mut ServiceTable) -> R) -> R {
    let mut services = SERVICES.lock();
    action(&mut services)
}
