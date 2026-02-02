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
#[pyclass(weakref)]
struct ZooView {
    queue: Py<ZooQueue>,
    ptr: usize,
    len: usize,
    next_pos: u64,
    committed: AtomicBool,
}

#[pymethods]
impl ZooView {
    fn __enter__<'py>(slf: Bound<'py, Self>, _py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
        // Create memoryview directly from self
        slf.py()
            .import("builtins")?
            .call_method1("memoryview", (slf,))
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

            // Fill buffer info manually
            // Note: PyBuffer_FillInfo is safer than manual assignment for basic cases
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

    unsafe fn __releasebuffer__(&self, _view: *mut pyo3::ffi::Py_buffer) {}

    fn __traverse__(&self, visit: pyo3::gc::PyVisit<'_>) -> Result<(), pyo3::gc::PyTraverseError> {
        visit.call(&self.queue)
    }

    fn __clear__(&mut self) {}
}

impl Drop for ZooView {
    fn drop(&mut self) {}
}

#[pyclass]
struct ZooLock {
    _shm: ShmSegment,
    mutex: &'static RobustMutex,
}

#[pymethods]
impl ZooLock {
    #[new]
    fn new(name: String) -> PyResult<Self> {
        let size = 4096;

        let shm = match ShmSegment::open(&name, size) {
            Ok(s) => s,
            Err(_) => {
                let s = ShmSegment::create(&name, size)
                    .map_err(|e| PyRuntimeError::new_err(format!("Failed to create shm: {}", e)))?;

                unsafe {
                    RobustMutex::initialize_at(s.ptr.as_ptr()).map_err(|e| {
                        PyRuntimeError::new_err(format!("Failed to init mutex: {}", e))
                    })?;
                }
                s
            }
        };

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

        py.allow_threads(|| self.mutex.lock()).map_err(|e| match e {
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
                unsafe {
                    RingBuffer::initialize_at(s.ptr.as_ptr(), s.size).map_err(|e| {
                        PyRuntimeError::new_err(format!("Failed init buffer: {}", e))
                    })?;
                }
                s
            }
        };

        let buffer = unsafe {
            RingBuffer::from_ptr(shm.ptr.as_ptr(), shm.size)
                .map_err(|e| PyRuntimeError::new_err(format!("Failed to open buffer: {}", e)))?
        };
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
    m.add_class::<ZooView>()?;
    m.add_class::<pool::ZooPoolCore>()?;
    m.add("LockRecovered", py.get_type::<LockRecovered>())?;
    m.add("TimeoutError", py.get_type::<TimeoutError>())?;

    if cfg!(not(target_os = "linux")) {
        let warning_msg = "Running on non-Linux platform. Crash recovery (Robust Mutex) is NOT supported. Deadlocked processes may require manual cleanup.";
        let warnings = py.import("warnings")?;
        warnings.call_method1(
            "warn",
            (
                warning_msg,
                py.get_type::<pyo3::exceptions::PyUserWarning>(),
            ),
        )?;
    }

    Ok(())
}
