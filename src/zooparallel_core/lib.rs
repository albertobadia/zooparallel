use pyo3::create_exception;
use pyo3::exceptions::PyRuntimeError;
use pyo3::prelude::*;

mod pool;
mod queue;
mod shm;
mod sync;

use queue::RingBuffer;
use shm::ShmSegment;
use sync::{MutexError, RobustMutex};

create_exception!(zooparallel_core, LockRecovered, PyRuntimeError);
create_exception!(zooparallel_core, TimeoutError, PyRuntimeError);

use std::sync::atomic::{AtomicBool, Ordering};

/// A safe wrapper around a view of the queue memory.
/// Holds a reference to the queue to prevent use-after-free.
#[pyclass]
struct ZooView {
    queue: Py<ZooQueue>,
    ptr: usize,
    len: usize,
    next_pos: u64,
    committed: AtomicBool,
}

#[pymethods]
impl ZooView {
    fn __enter__<'py>(slf: Bound<'py, Self>, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
        let view = unsafe { pyo3::ffi::PyMemoryView_FromObject(slf.as_ptr()) };
        if view.is_null() {
            return Err(PyRuntimeError::new_err(
                "Failed to create memoryview from ZooView",
            ));
        }
        unsafe { Ok(Bound::from_owned_ptr(py, view)) }
    }

    fn __exit__(
        &self,
        py: Python,
        _exc_type: PyObject,
        _exc_value: PyObject,
        _traceback: PyObject,
    ) -> PyResult<()> {
        if !self.committed.swap(true, Ordering::SeqCst) {
            let queue = self.queue.borrow(py);
            queue.commit_read(self.next_pos)?;
        }
        Ok(())
    }

    unsafe fn __getbuffer__(
        &self,
        view: *mut pyo3::ffi::Py_buffer,
        flags: std::os::raw::c_int,
    ) -> PyResult<()> {
        if view.is_null() {
            return Err(PyRuntimeError::new_err("View is null"));
        }

        unsafe {
            let ptr = self.ptr as *mut std::ffi::c_void;

            (*view).buf = ptr;
            (*view).len = self.len as isize;
            (*view).itemsize = 1;
            (*view).readonly = 1;
            (*view).ndim = 1;
            (*view).format = std::ptr::null_mut();

            let ret = pyo3::ffi::PyBuffer_FillInfo(
                view,
                std::ptr::null_mut(),
                ptr,
                self.len as isize,
                1, // readonly
                flags,
            );
            if ret != 0 {
                return Err(PyRuntimeError::new_err("PyBuffer_FillInfo failed"));
            }
        }
        Ok(())
    }

    unsafe fn __releasebuffer__(&self, _view: *mut pyo3::ffi::Py_buffer) {
        // No op
    }
}

#[pyclass]
struct ZooLock {
    _shm: ShmSegment,            // Keep shm alive
    mutex: &'static RobustMutex, // Reference into shm
}

#[pymethods]
impl ZooLock {
    #[new]
    fn new(name: String) -> PyResult<Self> {
        // Size of pthread_mutex_t is 64 bytes on 64-bit systems normally, but let's alloc a page to be safe/lazy
        let size = 4096;

        let shm = match ShmSegment::open(&name, size) {
            Ok(s) => s,
            Err(_) => {
                // Try create
                let s = ShmSegment::create(&name, size)
                    .map_err(|e| PyRuntimeError::new_err(format!("Failed to create shm: {}", e)))?;

                // Initialize mutex in the first bytes
                unsafe {
                    RobustMutex::initialize_at(s.ptr.as_ptr()).map_err(|e| {
                        PyRuntimeError::new_err(format!("Failed into init mutex: {}", e))
                    })?;
                }
                s
            }
        };

        // Get reference
        let mutex = unsafe { RobustMutex::from_ptr(shm.ptr.as_ptr()) };

        Ok(ZooLock { _shm: shm, mutex })
    }

    fn acquire(&self, py: Python) -> PyResult<()> {
        // Optimistic: Try to get lock without releasing GIL (avoid context switch)
        match self.mutex.try_lock() {
            Ok(_) => return Ok(()),
            Err(MutexError::Recovered) => {
                return Err(LockRecovered::new_err("Lock recovered from dead process"));
            }
            Err(MutexError::Busy) => { /* Continue to blocking wait */ }
            Err(e) => return Err(PyRuntimeError::new_err(format!("Lock failure: {}", e))),
        }

        py.allow_threads(|| {
            match self.mutex.lock() {
                Ok(_) => Ok(()),
                Err(MutexError::Recovered) => {
                    // We need to signal this back.
                    // Since allow_threads expects Send, and PyErr isn't always Send easily,
                    // we return a specific status.
                    Err(MutexError::Recovered)
                }
                Err(e) => Err(e),
            }
        })
        .map_err(|e| match e {
            MutexError::Recovered => LockRecovered::new_err("Lock recovered from dead process"),
            _ => PyRuntimeError::new_err(format!("Lock failure: {}", e)),
        })
    }

    fn release(&self) -> PyResult<()> {
        self.mutex
            .unlock()
            .map_err(|e| PyRuntimeError::new_err(format!("Unlock failed: {}", e)))
    }

    fn __enter__(&self, py: Python) -> PyResult<()> {
        self.acquire(py)
    }

    fn __exit__(
        &self,
        _exc_type: PyObject,
        _exc_value: PyObject,
        _traceback: PyObject,
    ) -> PyResult<()> {
        self.release()
    }

    #[staticmethod]
    fn unlink(name: String) -> PyResult<()> {
        ShmSegment::unlink(&name)
            .map_err(|e| PyRuntimeError::new_err(format!("Unlink failed: {}", e)))
    }
}

#[pyclass]
struct ZooQueue {
    _shm: ShmSegment,
    buffer: RingBuffer,
}

#[pymethods]
impl ZooQueue {
    #[new]
    fn new(name: String, size_mb: usize) -> PyResult<Self> {
        let data_size = size_mb * 1024 * 1024;
        let header_size = crate::queue::HEADER_SIZE;

        let shm = match ShmSegment::open_mirrored(&name, header_size, data_size) {
            Ok(s) => s,
            Err(_) => {
                let s =
                    ShmSegment::create_mirrored(&name, header_size, data_size).map_err(|e| {
                        PyRuntimeError::new_err(format!("Failed to create mirrored shm: {}", e))
                    })?;
                // Init
                unsafe {
                    RingBuffer::initialize_at(s.ptr.as_ptr(), s.size).map_err(|e| {
                        PyRuntimeError::new_err(format!("Failed init buffer: {}", e))
                    })?;
                }
                s
            }
        };

        // Load existing
        let buffer = unsafe { RingBuffer::from_ptr(shm.ptr.as_ptr(), shm.size) };
        Ok(ZooQueue { _shm: shm, buffer })
    }

    fn put_bytes(&self, py: Python, data: &[u8]) -> PyResult<()> {
        py.allow_threads(|| self.buffer.put_bytes(data))
            .map_err(|e| PyRuntimeError::new_err(format!("Queue put error: {}", e)))
    }

    fn get_bytes<'py>(&self, py: Python<'py>) -> PyResult<Vec<u8>> {
        py.allow_threads(|| self.buffer.get_bytes())
            .map_err(|e| PyRuntimeError::new_err(format!("Queue get error: {}", e)))
    }

    /// Zero-copy receive. Returns a ZooView context manager.
    /// Usage: with queue.recv_view() as view: ...
    fn recv_view(self_: Py<ZooQueue>, py: Python) -> PyResult<ZooView> {
        let queue = self_.borrow(py);

        let (ptr, len, next_pos) = queue
            .buffer
            .get_view()
            .map_err(|e| PyRuntimeError::new_err(format!("Queue get_view error: {}", e)))?;

        queue
            .buffer
            .release_lock()
            .map_err(|e| PyRuntimeError::new_err(format!("Failed to release lock: {}", e)))?;

        Ok(ZooView {
            queue: self_.clone_ref(py),
            ptr: ptr as usize,
            len,
            next_pos,
            committed: AtomicBool::new(false),
        })
    }

    fn commit_read(&self, next_pos: u64) -> PyResult<()> {
        self.buffer
            .commit_read(next_pos)
            .map_err(|e| PyRuntimeError::new_err(format!("Queue commit_read error: {}", e)))
    }

    #[staticmethod]
    fn unlink(name: String) -> PyResult<()> {
        ShmSegment::unlink(&name)
            .map_err(|e| PyRuntimeError::new_err(format!("Unlink failed: {}", e)))
    }
}

#[pymodule]
fn zooparallel_core(py: Python, m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_class::<ZooLock>()?;
    m.add_class::<ZooQueue>()?;
    m.add_class::<pool::ZooPoolCore>()?;
    m.add_class::<ZooLock>()?;
    m.add_class::<ZooQueue>()?;
    m.add_class::<ZooView>()?;
    m.add_class::<pool::ZooPoolCore>()?;
    m.add("LockRecovered", py.get_type::<LockRecovered>())?;
    m.add("TimeoutError", py.get_type::<TimeoutError>())?;
    Ok(())
}
