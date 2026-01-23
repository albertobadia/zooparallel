use pyo3::create_exception;
use pyo3::exceptions::PyRuntimeError;
use pyo3::prelude::*;

mod shm;
mod sync;

use shm::ShmSegment;
use sync::{MutexError, RobustMutex};

create_exception!(zoosync_core, LockRecovered, PyRuntimeError);

#[pyclass]
struct ZooLock {
    #[allow(dead_code)]
    shm: ShmSegment, // Keep shm alive
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

        Ok(ZooLock { shm, mutex })
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
}

#[pymodule]
fn zoosync_core(py: Python, m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_class::<ZooLock>()?;
    m.add("LockRecovered", py.get_type::<LockRecovered>())?;
    Ok(())
}
