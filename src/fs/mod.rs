//! Storage-independent virtual filesystem primitives.
//!
//! The VFS uses fixed-size records so kernel paths and file reads never require
//! heap allocation.  The in-memory filesystem is useful as an initial root
//! filesystem and boot-readable storage can later be implemented behind the
//! same mount and file-operation interfaces.

use crate::drivers::AtaPio;
use crate::interrupts::IrqLock;

pub const NAME_LIMIT: usize = 32;
pub const FILE_LIMIT: usize = 512;
pub const MAX_NODES: usize = 16;
pub const MAX_MOUNT_POINTS: usize = 4;

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum FsError {
    NotFound,
    NotDirectory,
    NotFile,
    Exists,
    NameTooLong,
    TableFull,
    NotMounted,
    TooLarge,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum NodeKind {
    Directory,
    File,
}

#[derive(Clone, Copy)]
pub struct DirEntry {
    pub name: [u8; NAME_LIMIT],
    pub name_len: usize,
    pub kind: NodeKind,
    pub size: usize,
}

impl DirEntry {
    pub fn name(&self) -> &str {
        unsafe { core::str::from_utf8_unchecked(&self.name[..self.name_len]) }
    }
}

#[derive(Clone, Copy)]
pub struct Node {
    pub path: [u8; NAME_LIMIT],
    pub path_len: usize,
    pub kind: NodeKind,
    pub data: [u8; FILE_LIMIT],
    pub size: usize,
}

impl Node {
    const fn empty() -> Self {
        Self {
            path: [0; NAME_LIMIT],
            path_len: 0,
            kind: NodeKind::Directory,
            data: [0; FILE_LIMIT],
            size: 0,
        }
    }

    fn set_path(&mut self, path: &str) -> bool {
        let bytes = path.as_bytes();
        if bytes.len() > NAME_LIMIT {
            return false;
        }
        self.path_len = bytes.len();
        self.path[..bytes.len()].copy_from_slice(bytes);
        true
    }

    fn path(&self) -> &str {
        unsafe { core::str::from_utf8_unchecked(&self.path[..self.path_len]) }
    }
}

/// A small initial filesystem containing read-only documentation and an empty
/// mount directory.  Its fixed backing arrays keep it usable before a heap is
/// available; it can later grow through reclaimable page-backed storage.
pub struct RamFs {
    nodes: [Node; MAX_NODES],
    count: usize,
}

impl RamFs {
    pub const fn new() -> Self {
        let filesystem = Self {
            nodes: [const { Node::empty() }; MAX_NODES],
            count: 0,
        };
        // Initialization is performed at runtime because path strings and byte
        // slices cannot safely be copied into a public const array.
        filesystem
    }

    pub fn initialize(&mut self) {
        self.count = 0;
        let mut root = Node::empty();
        root.set_path("/");
        root.kind = NodeKind::Directory;
        self.nodes[0] = root;

        let mut mnt = Node::empty();
        mnt.set_path("/mnt");
        mnt.kind = NodeKind::Directory;
        self.nodes[1] = mnt;

        let mut readme = Node::empty();
        readme.set_path("/README");
        readme.kind = NodeKind::File;
        let data = b"PDC-OS in-memory initial filesystem\n";
        readme.data[..data.len()].copy_from_slice(data);
        readme.size = data.len();
        self.nodes[2] = readme;

        let mut hello = Node::empty();
        hello.set_path("/hello.txt");
        hello.kind = NodeKind::File;
        let data = b"Hello from PDC-OS!\n";
        hello.data[..data.len()].copy_from_slice(data);
        hello.size = data.len();
        self.nodes[3] = hello;

        // Standard userspace directories and configuration are installed into
        // the initial RAM filesystem before the startup service is launched.
        let directories = ["/bin", "/sbin", "/etc", "/lib"];
        for (index, path) in directories.iter().enumerate() {
            let mut directory = Node::empty();
            directory.set_path(path);
            directory.kind = NodeKind::Directory;
            self.nodes[4 + index] = directory;
        }

        // Configuration and startup text live in the VFS, not in kernel code.
        // The inittab is parsed by the userspace startup service.
        if self.count == 0 {
            let mut os_release = Node::empty();
            os_release.set_path("/etc/os-release");
            os_release.kind = NodeKind::File;
            // Build the release record from the same identity constants used by
            // the shell and boot diagnostics, preventing version-string drift.
            let data = crate::kernel::core::OS_RELEASE.as_bytes();
            os_release.data[..data.len()].copy_from_slice(data);
            os_release.size = data.len();
            self.nodes[8] = os_release;

            let mut inittab = Node::empty();
            inittab.set_path("/etc/inittab");
            inittab.kind = NodeKind::File;
            let data = b"::once:/sbin/init\n::respawn:/bin/pdc-sh\n";
            inittab.data[..data.len()].copy_from_slice(data);
            inittab.size = data.len();
            self.nodes[9] = inittab;

            let mut shell = Node::empty();
            shell.set_path("/bin/pdc-sh");
            shell.kind = NodeKind::File;
            let data = b"# PDC-OS userspace shell\n";
            shell.data[..data.len()].copy_from_slice(data);
            shell.size = data.len();
            self.nodes[10] = shell;

            let mut init = Node::empty();
            init.set_path("/sbin/init");
            init.kind = NodeKind::File;
            let data = b"# PDC-OS startup service\n";
            init.data[..data.len()].copy_from_slice(data);
            init.size = data.len();
            self.nodes[11] = init;

            self.count = 12;
        }
    }

    fn find(&self, path: &str) -> Option<usize> {
        (0..self.count).find(|&index| self.nodes[index].path() == path)
    }

    pub fn list(&self, path: &str, entries: &mut [DirEntry]) -> Result<usize, FsError> {
        let directory = self.find(path).ok_or(FsError::NotFound)?;
        if self.nodes[directory].kind != NodeKind::Directory {
            return Err(FsError::NotDirectory);
        }

        let prefix = if path == "/" { "/" } else { path };
        let mut found = 0;
        for index in 0..self.count {
            if index == directory {
                continue;
            }
            let node_path = self.nodes[index].path();
            let relative = if prefix == "/" {
                node_path.strip_prefix('/')
            } else {
                node_path.strip_prefix(prefix).and_then(|rest| rest.strip_prefix('/'))
            };
            let Some(relative) = relative else { continue };

            // Only include direct children: the VFS does not recurse implicitly.
            if relative.is_empty() || relative.contains('/') {
                continue;
            }
            if found >= entries.len() {
                break;
            }

            let bytes = relative.as_bytes();
            if bytes.len() > NAME_LIMIT {
                continue;
            }
            let mut name = [0u8; NAME_LIMIT];
            name[..bytes.len()].copy_from_slice(bytes);
            entries[found] = DirEntry {
                name,
                name_len: bytes.len(),
                kind: self.nodes[index].kind,
                size: self.nodes[index].size,
            };
            found += 1;
        }
        Ok(found)
    }

    pub fn read(&self, path: &str, buffer: &mut [u8]) -> Result<usize, FsError> {
        let node = self.find(path).ok_or(FsError::NotFound)?;
        if self.nodes[node].kind != NodeKind::File {
            return Err(FsError::NotFile);
        }
        let length = buffer.len().min(self.nodes[node].size);
        buffer[..length].copy_from_slice(&self.nodes[node].data[..length]);
        Ok(length)
    }

    pub fn create_file(&mut self, path: &str) -> Result<(), FsError> {
        if self.find(path).is_some() {
            return Err(FsError::Exists);
        }
        if path.as_bytes().len() > NAME_LIMIT {
            return Err(FsError::NameTooLong);
        }
        if self.count >= MAX_NODES {
            return Err(FsError::TableFull);
        }
        let mut node = Node::empty();
        if !node.set_path(path) {
            return Err(FsError::NameTooLong);
        }
        node.kind = NodeKind::File;
        self.nodes[self.count] = node;
        self.count += 1;
        Ok(())
    }

    pub fn write(&mut self, path: &str, data: &[u8]) -> Result<usize, FsError> {
        let node = self.find(path).ok_or(FsError::NotFound)?;
        if self.nodes[node].kind != NodeKind::File {
            return Err(FsError::NotFile);
        }
        if data.len() > FILE_LIMIT {
            return Err(FsError::TooLarge);
        }
        let length = data.len();
        self.nodes[node].data[..length].copy_from_slice(&data[..length]);
        self.nodes[node].size = length;
        Ok(length)
    }
}

#[derive(Clone, Copy)]
pub struct Mount {
    pub target: [u8; NAME_LIMIT],
    pub target_len: usize,
    pub source: [u8; NAME_LIMIT],
    pub source_len: usize,
}

impl Mount {
    pub const fn empty() -> Self {
        Self {
            target: [0; NAME_LIMIT],
            target_len: 0,
            source: [0; NAME_LIMIT],
            source_len: 0,
        }
    }

    pub fn target(&self) -> &str {
        unsafe { core::str::from_utf8_unchecked(&self.target[..self.target_len]) }
    }

    pub fn source(&self) -> &str {
        unsafe { core::str::from_utf8_unchecked(&self.source[..self.source_len]) }
    }
}

pub struct Vfs {
    ramfs: RamFs,
    mounts: [Mount; MAX_MOUNT_POINTS],
    mount_count: usize,
    ata_ready: bool,
    cache: [DiskCacheEntry; 4],
    cache_next: usize,
}

#[derive(Clone, Copy)]
pub struct DiskCacheEntry {
    path: [u8; NAME_LIMIT],
    path_len: usize,
    data: [u8; FILE_LIMIT],
    data_len: usize,
    valid: bool,
}

impl DiskCacheEntry {
    pub const fn empty() -> Self {
        Self {
            path: [0; NAME_LIMIT],
            path_len: 0,
            data: [0; FILE_LIMIT],
            data_len: 0,
            valid: false,
        }
    }
}

impl Vfs {
    pub const fn new() -> Self {
        Self {
            ramfs: RamFs::new(),
            mounts: [const { Mount::empty() }; MAX_MOUNT_POINTS],
            mount_count: 0,
            ata_ready: false,
            cache: [const { DiskCacheEntry::empty() }; 4],
            cache_next: 0,
        }
    }

    pub fn initialize(&mut self) {
        self.ramfs.initialize();
        self.mount_count = 0;
        self.cache = [const { DiskCacheEntry::empty() }; 4];
        self.cache_next = 0;
        self.mount("ramfs", "/").unwrap();

        // Detect storage at boot, but do not assume that it contains a
        // filesystem.  It is mounted as a raw block source for future format
        // drivers and is always represented explicitly by `mount`.
        let mut ata_descriptor = crate::drivers::DeviceDescriptor::new(
            "ATA PIO storage",
            crate::drivers::DeviceClass::Storage,
        );
        ata_descriptor.resources[0] =
            crate::drivers::HardwareResource::PortRange { base: 0x1F0, length: 8 };
        ata_descriptor.resources[1] =
            crate::drivers::HardwareResource::Irq { line: 14 };
        ata_descriptor.resource_count = 2;
        let _ = crate::drivers::DEVICE_REGISTRY.register_descriptor(ata_descriptor);
        let mut ata = AtaPio::primary_master();
        match ata.initialize_result() {
            Ok(_) => {
                self.ata_ready = true;
                crate::drivers::DEVICE_REGISTRY.initialize("ATA PIO storage");
                crate::drivers::DEVICE_REGISTRY.activate("ATA PIO storage");
                crate::serial::println(format_args!(
                    "Storage initialization complete: ATA primary-master"
                ));
            }
            Err(error) => {
                self.ata_ready = false;
                crate::recovery::report_init_failure("ATA primary-master", error);
            }
        }
        if self.ata_ready {
            let _ = self.mount("ata0", "/mnt/ata");
        }
    }

    fn add_mount(&mut self, source: &str, target: &str) -> Result<(), FsError> {
        if source.as_bytes().len() > NAME_LIMIT || target.as_bytes().len() > NAME_LIMIT {
            return Err(FsError::NameTooLong);
        }
        if self.mount_count >= MAX_MOUNT_POINTS {
            return Err(FsError::TableFull);
        }
        for index in 0..self.mount_count {
            if self.mounts[index].target() == target {
                return Err(FsError::Exists);
            }
        }
        let mut mount = Mount::empty();
        mount.source[..source.len()].copy_from_slice(source.as_bytes());
        mount.source_len = source.len();
        mount.target[..target.len()].copy_from_slice(target.as_bytes());
        mount.target_len = target.len();
        self.mounts[self.mount_count] = mount;
        self.mount_count += 1;
        Ok(())
    }

    pub fn mount(&mut self, source: &str, target: &str) -> Result<(), FsError> {
        if target != "/" && target.as_bytes().first() != Some(&b'/') {
            return Err(FsError::NotFound);
        }
        match source {
            "ramfs" => self.add_mount(source, target),
            "ata0" if self.ata_ready => self.add_mount(source, target),
            "ata0" => Err(FsError::NotMounted),
            _ => Err(FsError::NotFound),
        }
    }

    pub fn mounts_snapshot(&self) -> ([Mount; MAX_MOUNT_POINTS], usize) {
        (self.mounts, self.mount_count)
    }

    pub fn normalize(path: &str) -> Option<[u8; NAME_LIMIT]> {
        if path.is_empty() || path.as_bytes().first() != Some(&b'/') || path.len() > NAME_LIMIT {
            return None;
        }
        // Dot-dot traversal and embedded NULs are rejected rather than
        // silently rewritten, preserving stable VFS path semantics.
        if path.as_bytes().contains(&0) {
            return None;
        }
        let mut normalized = [0u8; NAME_LIMIT];
        let mut length = 0;
        for component in path.split('/').filter(|component| !component.is_empty()) {
            if component == "." {
                continue;
            }
            if component == ".." {
                return None;
            }
            if length == 0 {
                normalized[length] = b'/';
                length += 1;
            }
            if length + component.len() > NAME_LIMIT {
                return None;
            }
            normalized[length..length + component.len()].copy_from_slice(component.as_bytes());
            length += component.len();
            // Components are currently limited to top-level/simple paths.
            // A separator is retained only in the mount lookup below.
        }
        if length == 0 {
            normalized[0] = b'/';
        }
        Some(normalized)
    }

    fn resolve<'a, 'b>(&'a self, path: &'b str) -> (&'a str, &'b str) {
        let mut best = "/";
        for index in 0..self.mount_count {
            let target = self.mounts[index].target();
            if path == target || (target == "/" && path.starts_with('/'))
                || path.strip_prefix(target).is_some_and(|rest| rest.starts_with('/'))
            {
                if target.len() >= best.len() {
                    best = target;
                }
            }
        }
        (best, path)
    }

    pub fn list(&self, path: &str, entries: &mut [DirEntry]) -> Result<usize, FsError> {
        let (mount, resolved) = self.resolve(path);
        if mount == "/" {
            let relative = if resolved == "/" { "/" } else { resolved };
            self.ramfs.list(relative, entries)
        } else {
            Err(FsError::NotMounted)
        }
    }

    pub fn read(&mut self, path: &str, buffer: &mut [u8]) -> Result<usize, FsError> {
        let (mount, resolved) = self.resolve(path);
        if mount == "/" {
            // A fixed four-entry cache avoids repeated node-table searches.
            for cached in &self.cache {
                if cached.valid
                    && cached.path_len == resolved.len()
                    && &cached.path[..cached.path_len] == resolved.as_bytes()
                {
                    let length = buffer.len().min(cached.data_len);
                    buffer[..length].copy_from_slice(&cached.data[..length]);
                    crate::performance::record_disk_cache_hit();
                    return Ok(length);
                }
            }

            let result = self.ramfs.read(resolved, buffer);
            crate::performance::record_disk_cache_miss();
            if let Ok(length) = result {
                let slot = self.cache_next;
                self.cache_next = (self.cache_next + 1) % self.cache.len();
                let entry = &mut self.cache[slot];
                entry.valid = true;
                entry.path_len = resolved.len().min(NAME_LIMIT);
                entry.path[..entry.path_len].copy_from_slice(&resolved.as_bytes()[..entry.path_len]);
                entry.data_len = length.min(FILE_LIMIT);
                entry.data[..entry.data_len].copy_from_slice(&buffer[..entry.data_len]);
            }
            result
        } else {
            Err(FsError::NotMounted)
        }
    }

    pub fn create_file(&mut self, path: &str) -> Result<(), FsError> {
        let (mount, resolved) = self.resolve(path);
        if mount == "/" {
            self.ramfs.create_file(resolved)
        } else {
            Err(FsError::NotMounted)
        }
    }

    pub fn write(&mut self, path: &str, data: &[u8]) -> Result<usize, FsError> {
        let (mount, resolved) = self.resolve(path);
        if mount == "/" {
            let result = self.ramfs.write(resolved, data);
            // A successful write invalidates fixed disk-cache entries.  Leaving
            // stale data would make repeated reads violate VFS consistency.
            if result.is_ok() {
                for entry in &mut self.cache {
                    if entry.valid
                        && entry.path_len == resolved.len()
                        && &entry.path[..entry.path_len] == resolved.as_bytes()
                    {
                        *entry = DiskCacheEntry::empty();
                    }
                }
            }
            result
        } else {
            Err(FsError::NotMounted)
        }
    }

    pub fn ata_ready(&self) -> bool {
        self.ata_ready
    }
}

static VFS: IrqLock<Vfs> = IrqLock::new(Vfs::new());

pub fn init() {
    {
        let mut vfs = VFS.lock();
        vfs.initialize();
        crate::serial::println(format_args!(
            "Storage: ATA primary-master={} mount={}",
            if vfs.ata_ready { "ready" } else { "unsupported" },
            if vfs.ata_ready { "/mnt/ata" } else { "none" }
        ));
    }
}

pub fn with<R>(action: impl FnOnce(&mut Vfs) -> R) -> R {
    let mut vfs = VFS.lock();
    action(&mut *vfs)
}
