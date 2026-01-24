use crate::sync::{CondError, MutexError, RobustMutex, ShmCondVar};
use std::ptr;
use std::sync::atomic::{AtomicU64, Ordering};
use thiserror::Error;

#[derive(Error, Debug)]
pub enum QueueError {
    #[error("Mutex error: {0}")]
    Mutex(#[from] MutexError),
    #[error("Cond error: {0}")]
    Cond(#[from] CondError),
    #[error("Buffer too small")]
    BufferTooSmall,
}

pub const HEADER_SIZE: usize = 16384;

#[repr(C, align(16384))]
struct RingBufferHeader {
    // 128 bytes for mutex
    _mutex_padding: [u8; 128],
    // 128 bytes for not_empty cond
    _cond_not_empty_padding: [u8; 128],
    // 128 bytes for not_full cond
    _cond_not_full_padding: [u8; 128],

    read_pos: AtomicU64,
    write_pos: AtomicU64,
    capacity: AtomicU64,
    init_done: AtomicU64,
    // Ensure the struct itself is 16KB
    _padding: [u8; 16384 - 128 * 3 - 8 * 4],
}

pub struct RingBuffer {
    mutex: &'static RobustMutex,
    not_empty: &'static ShmCondVar,
    not_full: &'static ShmCondVar,
    header: *mut RingBufferHeader,
    buffer: *mut u8,
}

unsafe impl Send for RingBuffer {}
unsafe impl Sync for RingBuffer {}

impl RingBuffer {
    /// Initialize a ring buffer in the given memory region.
    /// Layout: [Header (4KB)] [Data ...] [Data Mirror ...]
    pub unsafe fn initialize_at(ptr: *mut u8, total_vm_size: usize) -> Result<Self, QueueError> {
        if total_vm_size <= HEADER_SIZE {
            return Err(QueueError::BufferTooSmall);
        }

        // Data size is (total_vm_size - HEADER_SIZE) / 2 because of double mapping
        let data_size = (total_vm_size - HEADER_SIZE) / 2;
        let header = ptr as *mut RingBufferHeader;

        let mutex_ptr = ptr;
        let not_empty_ptr = unsafe { ptr.add(128) };
        let not_full_ptr = unsafe { ptr.add(256) };

        let mutex = unsafe { RobustMutex::initialize_at(mutex_ptr)? };
        let not_empty = unsafe { ShmCondVar::initialize_at(not_empty_ptr)? };
        let not_full = unsafe { ShmCondVar::initialize_at(not_full_ptr)? };

        // Initialize header fields
        unsafe {
            (*header).read_pos = AtomicU64::new(0);
            (*header).write_pos = AtomicU64::new(0);
            (*header).capacity = AtomicU64::new(data_size as u64);
            (*header).init_done = AtomicU64::new(1);
        }

        let buffer = unsafe { ptr.add(HEADER_SIZE) };

        Ok(Self {
            mutex,
            not_empty,
            not_full,
            header,
            buffer,
        })
    }

    pub unsafe fn from_ptr(ptr: *mut u8, _total_vm_size: usize) -> Self {
        let header = ptr as *mut RingBufferHeader;

        let mutex = unsafe { RobustMutex::from_ptr(ptr) };
        let not_empty = unsafe { ShmCondVar::from_ptr(ptr.add(128)) };
        let not_full = unsafe { ShmCondVar::from_ptr(ptr.add(256)) };

        // Wait for init_done
        while unsafe { (*header).init_done.load(Ordering::SeqCst) } == 0 {
            std::thread::yield_now();
        }

        Self {
            mutex,
            not_empty,
            not_full,
            header,
            buffer: unsafe { ptr.add(HEADER_SIZE) },
        }
    }

    // Helper to wrap lock
    fn lock(&self) -> Result<(), QueueError> {
        loop {
            match self.mutex.lock() {
                Ok(_) => return Ok(()),
                Err(MutexError::Recovered) => continue, // Retry
                Err(e) => return Err(QueueError::from(e)),
            }
        }
    }

    pub fn put_bytes(&self, data: &[u8]) -> Result<(), QueueError> {
        let len = data.len() as u64;
        let required = len + 4; // 4 bytes for length header

        self.lock()?;

        let header = unsafe { &*self.header };
        let capacity = header.capacity.load(Ordering::SeqCst);

        loop {
            let write_pos = header.write_pos.load(Ordering::SeqCst);
            let read_pos = header.read_pos.load(Ordering::SeqCst);
            let used = write_pos - read_pos;
            let free = capacity - used;

            if free >= required {
                break;
            }

            self.not_full
                .wait_timeout(self.mutex, std::time::Duration::from_millis(100))?;
        }

        let write_pos = header.write_pos.load(Ordering::SeqCst);

        // Write length (4 bytes)
        let mut wp = write_pos;
        let len_bytes = (len as u32).to_le_bytes();
        self.write_block_at(wp, capacity, &len_bytes);
        wp += 4;

        self.write_block_at(wp, capacity, data);
        wp += len;

        header.write_pos.store(wp, Ordering::SeqCst);

        self.not_empty.notify_all()?;
        self.mutex.unlock()?;

        Ok(())
    }

    pub fn get_bytes(&self) -> Result<Vec<u8>, QueueError> {
        self.lock()?;

        let header = unsafe { &*self.header };
        let capacity = header.capacity.load(Ordering::SeqCst);

        loop {
            let write_pos = header.write_pos.load(Ordering::SeqCst);
            let read_pos = header.read_pos.load(Ordering::SeqCst);
            if write_pos > read_pos {
                break;
            }
            self.not_empty
                .wait_timeout(self.mutex, std::time::Duration::from_millis(100))?;
        }

        let read_pos = header.read_pos.load(Ordering::SeqCst);

        let mut rp = read_pos;
        let mut len_bytes = [0u8; 4];
        self.read_block_at(rp, capacity, &mut len_bytes);
        let len = u32::from_le_bytes(len_bytes) as u64;
        rp += 4;

        let mut data = vec![0u8; len as usize];
        self.read_block_at(rp, capacity, &mut data);
        rp += len;

        // Commit read_pos
        header.read_pos.store(rp, Ordering::SeqCst);

        self.not_full.notify_all()?;
        self.mutex.unlock()?;

        Ok(data)
    }

    /// Optimized: contiguous view of data in shared memory (zero-copy)
    /// Returns (pointer, length, total_advanced_index).
    /// Advance index is used for commit_read later.
    pub fn get_view(&self) -> Result<(*const u8, usize, u64), QueueError> {
        self.lock()?;

        let header = unsafe { &*self.header };
        let capacity = header.capacity.load(Ordering::SeqCst);

        loop {
            let write_pos = header.write_pos.load(Ordering::SeqCst);
            let read_pos = header.read_pos.load(Ordering::SeqCst);
            if write_pos > read_pos {
                break;
            }
            self.not_empty
                .wait_timeout(self.mutex, std::time::Duration::from_millis(100))?;
        }

        let read_pos = header.read_pos.load(Ordering::SeqCst);

        let mut rp = read_pos;
        let mut len_bytes = [0u8; 4];
        self.read_block_at(rp, capacity, &mut len_bytes);
        let len = u32::from_le_bytes(len_bytes) as usize;
        rp += 4;

        // Because of mirroring, we can just return a pointer to the start of data.
        // The data is guaranteed to be contiguous for up to 'capacity' bytes.
        let offset = (rp % capacity) as usize;
        let ptr = unsafe { self.buffer.add(offset) as *const u8 };

        // We don't advance read_pos here, commit_read will do it.
        // We return the new rp (including length advances) so user can commit.
        Ok((ptr, len, rp + (len as u64)))
    }

    pub fn commit_read(&self, new_read_pos: u64) -> Result<(), QueueError> {
        // We assume we still hold the lock if this is called immediately after get_view?
        // Actually, for Python API, we might release the lock and then re-acquire it.
        // If we release the lock, someone else might try to read.
        // So Zero-Copy with PyMemoryView needs careful lock management.
        // Let's assume for now the user calls commit_read which re-locks.

        self.lock()?;
        let header = unsafe { &*self.header };
        header.read_pos.store(new_read_pos, Ordering::SeqCst);
        self.not_full.notify_all()?;
        self.mutex.unlock()?;
        Ok(())
    }

    pub fn release_lock(&self) -> Result<(), QueueError> {
        self.mutex.unlock().map_err(QueueError::from)
    }

    // Internal helpers (must hold lock)
    fn write_block_at(&self, pos: u64, capacity: u64, src: &[u8]) {
        let len = src.len();
        if len == 0 {
            return;
        }

        unsafe {
            // Because of Mirroring, we always have at least 'capacity' bytes contiguous
            // starting from any 'pos % capacity'.
            let offset = (pos % capacity) as usize;
            ptr::copy_nonoverlapping(src.as_ptr(), self.buffer.add(offset), len);
        }
    }

    fn read_block_at(&self, pos: u64, capacity: u64, dst: &mut [u8]) {
        let len = dst.len();
        if len == 0 {
            return;
        }

        unsafe {
            let offset = (pos % capacity) as usize;
            ptr::copy_nonoverlapping(self.buffer.add(offset), dst.as_mut_ptr(), len);
        }
    }
}
