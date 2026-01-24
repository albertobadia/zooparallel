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
        Self::create_impl(name, size, true)
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

#[cfg(test)]
mod tests {
    use super::*;
    use uuid::Uuid;

    fn temp_name() -> String {
        format!("/test_{}", &Uuid::new_v4().simple().to_string()[..8])
    }

    #[test]
    fn test_create_and_open() {
        let name = temp_name();
        let size = 16384; // Use 16KB for safety on M1

        {
            let shm = ShmSegment::create(&name, size).expect("Failed to create shm");
            assert_eq!(shm.size, size);
            // Write something
            unsafe {
                std::ptr::write(shm.ptr.as_ptr() as *mut u64, 0xDEADBEEF);
            }
        } // Drop closes mapping, but shm stays

        {
            let shm = ShmSegment::open(&name, size).expect("Failed to open shm");
            let val = unsafe { std::ptr::read(shm.ptr.as_ptr() as *mut u64) };
            assert_eq!(val, 0xDEADBEEF);
        }

        ShmSegment::unlink(&name).expect("Failed to unlink");
    }

    #[test]
    fn test_mirrored_mapping() {
        let name = temp_name();
        let header_size = 16384; // 16KB aligned
        let data_size = 16384; // 16KB aligned

        let shm = ShmSegment::create_mirrored(&name, header_size, data_size)
            .expect("Failed to create mirrored shm");

        let base = shm.ptr.as_ptr();

        unsafe {
            // Write to first data byte
            let data_start = base.add(header_size);
            *data_start = 42;

            // Check its mirror
            let mirror_start = base.add(header_size + data_size);
            assert_eq!(
                *mirror_start, 42,
                "Mirror did not reflect write to original"
            );

            // Write to mirror
            *mirror_start = 100;
            // Check original
            assert_eq!(*data_start, 100, "Original did not reflect write to mirror");
        }

        ShmSegment::unlink(&name).expect("Failed to unlink");
    }
}
