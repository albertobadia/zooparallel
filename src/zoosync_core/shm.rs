use nix::fcntl::OFlag;
use nix::sys::mman::{MapFlags, ProtFlags, mmap, munmap, shm_open, shm_unlink};
use nix::sys::stat::Mode;
use nix::unistd::ftruncate;
use std::ffi::CString;
use std::os::unix::io::AsRawFd;
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
}

impl ShmSegment {
    pub fn create(name: &str, size: usize) -> Result<Self, ShmError> {
        Self::create_impl(name, size, false)
    }

    pub fn create_mirrored(
        name: &str,
        header_size: usize,
        data_size: usize,
    ) -> Result<Self, ShmError> {
        Self::create_mirrored_impl(name, header_size, data_size, true)
    }

    pub fn open(name: &str, size: usize) -> Result<Self, ShmError> {
        Self::create_impl(name, size, false)
    }

    pub fn open_mirrored(
        name: &str,
        header_size: usize,
        data_size: usize,
    ) -> Result<Self, ShmError> {
        Self::create_mirrored_impl(name, header_size, data_size, false)
    }

    fn create_impl(name: &str, size: usize, create: bool) -> Result<Self, ShmError> {
        let name_slash = if name.starts_with('/') {
            name.to_string()
        } else {
            format!("/{}", name)
        };
        let shm_name = CString::new(name_slash)?;

        let flags = if create {
            OFlag::O_CREAT | OFlag::O_RDWR | OFlag::O_EXCL
        } else {
            OFlag::O_RDWR
        };

        let fd = shm_open(shm_name.as_c_str(), flags, Mode::S_IRUSR | Mode::S_IWUSR)?;

        if create {
            ftruncate(&fd, size as i64)?;
        }

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
        })
    }

    fn create_mirrored_impl(
        name: &str,
        header_size: usize,
        data_size: usize,
        create: bool,
    ) -> Result<Self, ShmError> {
        let name_slash = if name.starts_with('/') {
            name.to_string()
        } else {
            format!("/{}", name)
        };
        let shm_name = CString::new(name_slash)?;

        let flags = if create {
            OFlag::O_CREAT | OFlag::O_RDWR | OFlag::O_EXCL
        } else {
            OFlag::O_RDWR
        };

        let fd = shm_open(shm_name.as_c_str(), flags, Mode::S_IRUSR | Mode::S_IWUSR)?;

        let total_shm_size = header_size + data_size;
        if create {
            ftruncate(&fd, total_shm_size as i64)?;
        }

        // The "Double Mapping Trick":
        // 1. Reserve a contiguous virtual memory block of Header + 2*Data size
        // 2. Map header into the first part
        // 3. Map data into the second part
        // 4. Map the SAME data into the third part (the mirror)

        let total_vm_size = header_size + 2 * data_size;

        unsafe {
            // Step 1: Reserve address space
            let reserved_ptr = libc::mmap(
                std::ptr::null_mut(),
                total_vm_size,
                libc::PROT_NONE,
                libc::MAP_ANON | libc::MAP_PRIVATE,
                -1,
                0,
            );
            if reserved_ptr == libc::MAP_FAILED {
                return Err(ShmError::Nix(nix::Error::last()));
            }

            let base = reserved_ptr as *mut u8;

            // Step 2: Map Header
            let r2 = libc::mmap(
                base as *mut libc::c_void,
                header_size,
                libc::PROT_READ | libc::PROT_WRITE,
                libc::MAP_SHARED | libc::MAP_FIXED,
                fd.as_raw_fd(),
                0,
            );
            if r2 == libc::MAP_FAILED {
                libc::munmap(reserved_ptr, total_vm_size);
                return Err(ShmError::Nix(nix::Error::last()));
            }

            // Step 3: Map Data (first time)
            let r3 = libc::mmap(
                base.add(header_size) as *mut libc::c_void,
                data_size,
                libc::PROT_READ | libc::PROT_WRITE,
                libc::MAP_SHARED | libc::MAP_FIXED,
                fd.as_raw_fd(),
                header_size as libc::off_t,
            );
            if r3 == libc::MAP_FAILED {
                libc::munmap(reserved_ptr, total_vm_size);
                return Err(ShmError::Nix(nix::Error::last()));
            }

            // Step 4: Map Data (second time - the mirror)
            let r4 = libc::mmap(
                base.add(header_size + data_size) as *mut libc::c_void,
                data_size,
                libc::PROT_READ | libc::PROT_WRITE,
                libc::MAP_SHARED | libc::MAP_FIXED,
                fd.as_raw_fd(),
                header_size as libc::off_t,
            );
            if r4 == libc::MAP_FAILED {
                libc::munmap(reserved_ptr, total_vm_size);
                return Err(ShmError::Nix(nix::Error::last()));
            }

            Ok(Self {
                ptr: NonNull::new(base).unwrap(),
                size: total_vm_size,
            })
        }
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
