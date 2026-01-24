#[cfg(target_os = "linux")]
use libc::{
    EOWNERDEAD, PTHREAD_MUTEX_ROBUST, pthread_mutex_consistent, pthread_mutexattr_setrobust,
};

use libc::{
    EBUSY, PTHREAD_PROCESS_SHARED, pthread_cond_broadcast, pthread_cond_init, pthread_cond_signal,
    pthread_cond_t, pthread_cond_timedwait, pthread_cond_wait, pthread_condattr_destroy,
    pthread_condattr_init, pthread_condattr_setpshared, pthread_condattr_t, pthread_mutex_init,
    pthread_mutex_lock, pthread_mutex_t, pthread_mutex_trylock, pthread_mutex_unlock,
    pthread_mutexattr_destroy, pthread_mutexattr_init, pthread_mutexattr_setpshared,
    pthread_mutexattr_t, timespec,
};
use std::cell::UnsafeCell;
use std::mem::MaybeUninit;
use std::time::Duration;
use thiserror::Error;

#[derive(Error, Debug)]
pub enum MutexError {
    #[error("Pthread error: {0}")]
    Pthread(i32),
    #[error("Lock recovered from dead owner")]
    #[cfg_attr(not(target_os = "linux"), allow(dead_code))]
    Recovered,
    #[error("Lock is busy")]
    Busy,
}

#[derive(Error, Debug)]
pub enum CondError {
    #[error("Pthread error: {0}")]
    Pthread(i32),
    #[error("Mutex error: {0}")]
    Mutex(#[from] MutexError),
}

#[repr(transparent)]
pub struct ShmCondVar {
    inner: UnsafeCell<pthread_cond_t>,
}

unsafe impl Send for ShmCondVar {}
unsafe impl Sync for ShmCondVar {}

impl ShmCondVar {
    pub unsafe fn initialize_at(ptr: *mut u8) -> Result<&'static Self, CondError> {
        let cond_ptr = ptr as *mut pthread_cond_t;
        let mut attr = MaybeUninit::<pthread_condattr_t>::uninit();

        let ret = unsafe { pthread_condattr_init(attr.as_mut_ptr()) };
        if ret != 0 {
            return Err(CondError::Pthread(ret));
        }
        let mut attr = unsafe { attr.assume_init() };

        let ret = unsafe { pthread_condattr_setpshared(&mut attr, PTHREAD_PROCESS_SHARED) };
        if ret != 0 {
            unsafe { pthread_condattr_destroy(&mut attr) };
            return Err(CondError::Pthread(ret));
        }

        let ret = unsafe { pthread_cond_init(cond_ptr, &attr) };
        unsafe { pthread_condattr_destroy(&mut attr) };

        if ret != 0 {
            return Err(CondError::Pthread(ret));
        }

        unsafe { Ok(&*(ptr as *const ShmCondVar)) }
    }

    pub unsafe fn from_ptr(ptr: *mut u8) -> &'static Self {
        unsafe { &*(ptr as *const ShmCondVar) }
    }

    #[allow(dead_code)]
    pub fn wait(&self, mutex: &RobustMutex) -> Result<(), CondError> {
        let ret = unsafe { pthread_cond_wait(self.inner.get(), mutex.inner.get()) };
        if ret != 0 {
            // If EOWNERDEAD happens during wait, we might need to be careful.
            // But standard pthread_cond_wait re-acquires the mutex.
            // If it returns successfully, we hold the mutex.
            // If it returns an error, we might not.
            return Err(CondError::Pthread(ret));
        }
        Ok(())
    }

    pub fn wait_timeout(&self, mutex: &RobustMutex, timeout: Duration) -> Result<bool, CondError> {
        let ts = timespec {
            tv_sec: timeout.as_secs() as _,
            tv_nsec: timeout.subsec_nanos() as _,
        };

        unsafe {
            let mut now = timespec {
                tv_sec: 0,
                tv_nsec: 0,
            };
            libc::clock_gettime(libc::CLOCK_REALTIME, &mut now);

            let mut abs_ts = timespec {
                tv_sec: now.tv_sec + ts.tv_sec,
                tv_nsec: now.tv_nsec + ts.tv_nsec,
            };

            if abs_ts.tv_nsec >= 1_000_000_000 {
                abs_ts.tv_sec += 1;
                abs_ts.tv_nsec -= 1_000_000_000;
            }

            let ret = pthread_cond_timedwait(self.inner.get(), mutex.inner.get(), &abs_ts);
            if ret == 0 {
                Ok(true)
            } else if ret == libc::ETIMEDOUT {
                Ok(false)
            } else if ret == libc::EINVAL {
                // FALLBACK: If time is invalid (e.g. slightly in the past), just do a tiny sleep
                // release lock, sleep, re-lock
                mutex.unlock()?;
                std::thread::sleep(Duration::from_millis(10));
                mutex.lock()?;
                Ok(false)
            } else {
                Err(CondError::Pthread(ret))
            }
        }
    }

    #[allow(dead_code)]
    pub fn notify_one(&self) -> Result<(), CondError> {
        let ret = unsafe { pthread_cond_signal(self.inner.get()) };
        if ret != 0 {
            Err(CondError::Pthread(ret))
        } else {
            Ok(())
        }
    }

    pub fn notify_all(&self) -> Result<(), CondError> {
        let ret = unsafe { pthread_cond_broadcast(self.inner.get()) };
        if ret != 0 {
            Err(CondError::Pthread(ret))
        } else {
            Ok(())
        }
    }
}

#[repr(transparent)]
pub struct RobustMutex {
    pub(crate) inner: UnsafeCell<pthread_mutex_t>,
}

unsafe impl Send for RobustMutex {}
unsafe impl Sync for RobustMutex {}

impl RobustMutex {
    /// Initialize a robust mutex at the given memory location.
    /// # Safety
    /// The memory pointed to by `ptr` must be valid and large enough for `pthread_mutex_t`.
    pub unsafe fn initialize_at(ptr: *mut u8) -> Result<&'static Self, MutexError> {
        let mutex_ptr = ptr as *mut pthread_mutex_t;
        let mut attr = MaybeUninit::<pthread_mutexattr_t>::uninit();

        let ret = unsafe { pthread_mutexattr_init(attr.as_mut_ptr()) };
        if ret != 0 {
            return Err(MutexError::Pthread(ret));
        }
        let mut attr = unsafe { attr.assume_init() };

        // Process Shared
        let ret = unsafe { pthread_mutexattr_setpshared(&mut attr, PTHREAD_PROCESS_SHARED) };
        if ret != 0 {
            unsafe { pthread_mutexattr_destroy(&mut attr) };
            return Err(MutexError::Pthread(ret));
        }

        // Robust (Linux Only)
        #[cfg(target_os = "linux")]
        {
            let ret = unsafe { pthread_mutexattr_setrobust(&mut attr, PTHREAD_MUTEX_ROBUST) };
            if ret != 0 {
                unsafe { pthread_mutexattr_destroy(&mut attr) };
                return Err(MutexError::Pthread(ret));
            }
        }

        let ret = unsafe { pthread_mutex_init(mutex_ptr, &attr) };
        unsafe { pthread_mutexattr_destroy(&mut attr) };

        if ret != 0 {
            return Err(MutexError::Pthread(ret));
        }

        // Cast to our wrapper wrapper.
        // Note: This relies on RobustMutex being repr(transparent) or standard layout compatible
        // if it wraps UnsafeCell<pthread_mutex_t>, which is the only member.
        // For safety, let's just interpret the pointer as a reference to our struct.
        unsafe { Ok(&*(ptr as *const RobustMutex)) }
    }

    /// Get reference to existing mutex
    pub unsafe fn from_ptr(ptr: *mut u8) -> &'static Self {
        unsafe { &*(ptr as *const RobustMutex) }
    }

    pub fn lock(&self) -> Result<(), MutexError> {
        let ret = unsafe { pthread_mutex_lock(self.inner.get()) };
        if ret == 0 {
            return Ok(());
        }

        #[cfg(target_os = "linux")]
        if ret == EOWNERDEAD {
            // We acquired the lock, but the previous owner died.
            // We must mark it consistent.
            unsafe {
                pthread_mutex_consistent(self.inner.get());
            }
            return Err(MutexError::Recovered);
        }

        Err(MutexError::Pthread(ret))
    }

    pub fn try_lock(&self) -> Result<(), MutexError> {
        let ret = unsafe { pthread_mutex_trylock(self.inner.get()) };
        if ret == 0 {
            return Ok(());
        }
        if ret == EBUSY {
            return Err(MutexError::Busy);
        }

        #[cfg(target_os = "linux")]
        if ret == EOWNERDEAD {
            unsafe {
                pthread_mutex_consistent(self.inner.get());
            }
            return Err(MutexError::Recovered);
        }

        Err(MutexError::Pthread(ret))
    }

    pub fn unlock(&self) -> Result<(), MutexError> {
        let ret = unsafe { pthread_mutex_unlock(self.inner.get()) };
        if ret == 0 {
            Ok(())
        } else {
            Err(MutexError::Pthread(ret))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Barrier};
    use std::thread;

    #[test]
    fn test_lock_unlock() {
        let mut buffer = vec![0u8; 128];
        let mutex = unsafe { RobustMutex::initialize_at(buffer.as_mut_ptr()).unwrap() };

        mutex.lock().expect("Lock failed");
        mutex.unlock().expect("Unlock failed");
    }

    #[test]
    fn test_thread_contention() {
        let mut buffer = vec![0u8; 128];
        let mutex = unsafe { RobustMutex::initialize_at(buffer.as_mut_ptr()).unwrap() };
        let mutex_ptr = mutex as *const RobustMutex as usize;

        let barrier = Arc::new(Barrier::new(2));
        let b2 = barrier.clone();

        let handle = thread::spawn(move || {
            let m = unsafe { &*(mutex_ptr as *const RobustMutex) };
            b2.wait(); // Wait for main thread to lock
            m.lock().unwrap(); // Should block until main unlocks
            m.unlock().unwrap();
        });

        mutex.lock().unwrap();
        barrier.wait(); // Release thread
        thread::sleep(Duration::from_millis(50)); // Hold it a bit
        mutex.unlock().unwrap();

        handle.join().unwrap();
    }
}
