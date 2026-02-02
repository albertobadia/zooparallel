use crate::queue::RingBuffer;
use crate::shm::ShmSegment;
use pyo3::exceptions::PyRuntimeError;
use pyo3::prelude::*;
use uuid::Uuid;

#[pyclass]
#[allow(dead_code)]
pub struct ZooPoolCore {
    task_shm: ShmSegment,
    result_shm: ShmSegment,
    task_buffer: RingBuffer,
    result_buffer: RingBuffer,
    pub task_q_name: String,
    pub result_q_name: String,
    size_bytes: usize,
}

#[pymethods]
impl ZooPoolCore {
    #[new]
    pub fn new(size_mb: usize) -> PyResult<Self> {
        let pool_id = Uuid::new_v4().simple().to_string()[..8].to_string();
        let task_q_name = format!("zp_task_{}", pool_id);
        let result_q_name = format!("zp_res_{}", pool_id);
        let data_size = size_mb * 1024 * 1024;
        let header_size = crate::queue::HEADER_SIZE;

        let task_shm = ShmSegment::create_mirrored(&task_q_name, header_size, data_size)
            .map_err(|e| PyRuntimeError::new_err(format!("Failed to create task shm: {}", e)))?;

        let result_shm = ShmSegment::create_mirrored(&result_q_name, header_size, data_size)
            .map_err(|e| PyRuntimeError::new_err(format!("Failed to create result shm: {}", e)))?;

        let task_buffer = unsafe {
            RingBuffer::initialize_at(task_shm.ptr.as_ptr(), task_shm.size).map_err(|e| {
                PyRuntimeError::new_err(format!("Failed to init task buffer: {}", e))
            })?
        };

        let result_buffer = unsafe {
            RingBuffer::initialize_at(result_shm.ptr.as_ptr(), result_shm.size).map_err(|e| {
                PyRuntimeError::new_err(format!("Failed to init result buffer: {}", e))
            })?
        };

        Ok(Self {
            task_shm,
            result_shm,
            task_buffer,
            result_buffer,
            task_q_name,
            result_q_name,
            size_bytes: data_size,
        })
    }

    #[getter]
    fn get_task_q_name(&self) -> String {
        self.task_q_name.clone()
    }

    #[getter]
    fn get_result_q_name(&self) -> String {
        self.result_q_name.clone()
    }

    pub fn put_task(&self, py: Python, data: &[u8]) -> PyResult<()> {
        py.allow_threads(|| self.task_buffer.put_bytes(data))
            .map_err(|e| PyRuntimeError::new_err(format!("Task put error: {}", e)))
    }

    pub fn get_result(&self, py: Python) -> PyResult<Vec<u8>> {
        py.allow_threads(|| self.result_buffer.get_bytes())
            .map_err(|e| PyRuntimeError::new_err(format!("Result get error: {}", e)))
    }

    pub fn unlink(&self) -> PyResult<()> {
        ShmSegment::unlink(&self.task_q_name).ok();
        ShmSegment::unlink(&self.result_q_name).ok();
        Ok(())
    }
}

impl Drop for ZooPoolCore {
    fn drop(&mut self) {
        let _ = self.unlink();
    }
}
