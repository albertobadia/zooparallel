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

#[repr(C)]
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
    /// Layout: [Header (Mutex + Conds + Metadata)] [Data Buffer ...]
    pub unsafe fn initialize_at(ptr: *mut u8, size: usize) -> Result<Self, QueueError> {
        let header_size = std::mem::size_of::<RingBufferHeader>();
        if size <= header_size {
            return Err(QueueError::BufferTooSmall);
        }

        let header = ptr as *mut RingBufferHeader;
        let buffer_capacity = size - header_size;

        // Initialize synchronization primitives in-place within the header padding
        // Adjust offsets based on layout.
        // We assume 64 bytes is enough for pthread_mutex_t and pthread_cond_t (usually 40 and 48 bytes on 64-bit)

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
            (*header).capacity = AtomicU64::new(buffer_capacity as u64);
            (*header).init_done = AtomicU64::new(1);
        }

        let buffer = unsafe { ptr.add(header_size) };

        Ok(Self {
            mutex,
            not_empty,
            not_full,
            header,
            buffer,
        })
    }

    pub unsafe fn from_ptr(ptr: *mut u8, size: usize) -> Self {
        let header_size = std::mem::size_of::<RingBufferHeader>();
        let _buffer_capacity = size - header_size;

        let header = ptr as *mut RingBufferHeader;

        let mutex = unsafe { RobustMutex::from_ptr(ptr) };
        let not_empty = unsafe { ShmCondVar::from_ptr(ptr.add(128)) };
        let not_full = unsafe { ShmCondVar::from_ptr(ptr.add(256)) };

        // Wait for init_done (in case of race where opener starts before creator finishes)
        while unsafe { (*header).init_done.load(Ordering::SeqCst) } == 0 {
            std::thread::yield_now();
        }

        // Note: we trust capacity matches size-header_size, or we read from header if we want dynamic check

        Self {
            mutex,
            not_empty,
            not_full,
            header,
            buffer: unsafe { ptr.add(header_size) },
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
            // Capacity already loaded
            let used = write_pos - read_pos;
            let free = capacity - used;

            if free >= required {
                break;
            }

            // Wait for space with timeout to prevent infinite hang on macOS signal loss
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

    // Internal helpers (must hold lock)
    fn write_block_at(&self, pos: u64, capacity: u64, src: &[u8]) {
        let len = src.len();
        if len == 0 {
            return;
        }

        unsafe {
            let offset = (pos % capacity) as usize;
            let available = (capacity as usize) - offset;

            if available >= len {
                // Single contiguous copy
                ptr::copy_nonoverlapping(src.as_ptr(), self.buffer.add(offset), len);
            } else {
                // Wrap around copy
                ptr::copy_nonoverlapping(src.as_ptr(), self.buffer.add(offset), available);
                ptr::copy_nonoverlapping(src.as_ptr().add(available), self.buffer, len - available);
            }
        }
    }

    fn read_block_at(&self, pos: u64, capacity: u64, dst: &mut [u8]) {
        let len = dst.len();
        if len == 0 {
            return;
        }

        unsafe {
            let offset = (pos % capacity) as usize;
            let available = (capacity as usize) - offset;

            if available >= len {
                // Single contiguous copy
                ptr::copy_nonoverlapping(self.buffer.add(offset), dst.as_mut_ptr(), len);
            } else {
                // Wrap around copy
                ptr::copy_nonoverlapping(self.buffer.add(offset), dst.as_mut_ptr(), available);
                ptr::copy_nonoverlapping(
                    self.buffer,
                    dst.as_mut_ptr().add(available),
                    len - available,
                );
            }
        }
    }
}
