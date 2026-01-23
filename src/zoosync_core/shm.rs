use nix::fcntl::OFlag;
use nix::sys::mman::{MapFlags, ProtFlags, mmap, munmap, shm_open, shm_unlink};
use nix::sys::stat::Mode;
use nix::unistd::ftruncate;
use std::ffi::CString;
// use std::os::fd::AsRawFd;
use std::ptr::NonNull;
use thiserror::Error;

#[derive(Error, Debug)]
pub enum ShmError {
    #[error("Nix error: {0}")]
    Nix(#[from] nix::Error),
    #[error("CString error: {0}")]
    CString(#[from] std::ffi::NulError),
}

pub struct ShmSegment {
    pub ptr: NonNull<u8>,
    pub size: usize,
    #[allow(dead_code)]
    pub name: String,
}

impl ShmSegment {
    pub fn create(name: &str, size: usize) -> Result<Self, ShmError> {
        let name_slash = if name.starts_with('/') {
            name.to_string()
        } else {
            format!("/{}", name)
        };
        let shm_name = CString::new(name_slash)?;
        let fd = shm_open(
            shm_name.as_c_str(),
            OFlag::O_CREAT | OFlag::O_RDWR | OFlag::O_EXCL,
            Mode::S_IRUSR | Mode::S_IWUSR,
        )?;

        ftruncate(&fd, size as i64)?;

        let ptr = unsafe {
            mmap(
                None,
                std::num::NonZeroUsize::new(size).unwrap(),
                ProtFlags::PROT_READ | ProtFlags::PROT_WRITE,
                MapFlags::MAP_SHARED,
                &fd,
                0,
            )?
        };

        Ok(Self {
            ptr: NonNull::new(ptr.as_ptr() as *mut u8).unwrap(),
            size,
            name: name.to_string(),
        })
    }

    pub fn open(name: &str, size: usize) -> Result<Self, ShmError> {
        let name_slash = if name.starts_with('/') {
            name.to_string()
        } else {
            format!("/{}", name)
        };
        let shm_name = CString::new(name_slash)?;
        let fd = shm_open(
            shm_name.as_c_str(),
            OFlag::O_RDWR,
            Mode::S_IRUSR | Mode::S_IWUSR,
        )?;

        let ptr = unsafe {
            mmap(
                None,
                std::num::NonZeroUsize::new(size).unwrap(),
                ProtFlags::PROT_READ | ProtFlags::PROT_WRITE,
                MapFlags::MAP_SHARED,
                &fd,
                0,
            )?
        };

        Ok(Self {
            ptr: NonNull::new(ptr.as_ptr() as *mut u8).unwrap(),
            size,
            name: name.to_string(),
        })
    }

    #[allow(dead_code)]
    pub fn unlink(name: &str) -> Result<(), ShmError> {
        let name_slash = if name.starts_with('/') {
            name.to_string()
        } else {
            format!("/{}", name)
        };
        let shm_name = CString::new(name_slash)?;
        shm_unlink(shm_name.as_c_str())?;
        Ok(())
    }
}

impl Drop for ShmSegment {
    fn drop(&mut self) {
        unsafe {
            let _ = munmap(
                // SAFETY: We stored the pointer as NonNull, so we cast back.
                // Note: This logic assumes mapped region started at self.ptr.
                // In a real impl we might want to store the RawFd or be careful about Drop semantics if cloned.
                // For now, this struct owns the mapping for this process.
                NonNull::new(self.ptr.as_ptr() as *mut std::ffi::c_void).unwrap(),
                self.size,
            );
        }
    }
}

unsafe impl Send for ShmSegment {}
unsafe impl Sync for ShmSegment {}
