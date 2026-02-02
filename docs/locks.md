# Features: Robust Locks (`ZooLock`)

## Overview

`ZooLock` is a high-performance, crash-safe replacement for `multiprocessing.Lock`. It is built on top of POSIX Shared Memory and Pthread Mutexes.

## Key Capabilities

### 1. Crash Safety
The most critical feature of `ZooLock` is **Robustness**.
*   **Problem**: In standard `multiprocessing`, if a worker process is killed (`kill -9`) while it is inside a `with lock:` block, the lock remains held forever. All other processes waiting for it will hang indefinitely (Deadlock).
*   **Solution**: `ZooLock` leverages `PTHREAD_MUTEX_ROBUST`. If the kernel detects the owner of a lock has died, the next waiter receives a special return code (`EOWNERDEAD`). ZooParallel converts this into a successful acquisition but raises a `LockRecovered` warning/exception.

### 2. Performance
*   **Futex Implementation**: Unlike some file-lock based implementations, `ZooLock` uses Linux Futexes (Fast Userspace Mutexes).
*   **User Space**: In the uncontended case (no one else holds the lock), acquiring is a purely user-space atomic operation (Compare-and-Swap). No syscalls are made.
*   **Kernel Sleep**: Only when contention occurs does the process sleep in the kernel.

## Usage

```python
from zooparallel import ZooLock, LockRecovered
import time

lock = ZooLock("my_db_lock")

def safe_update():
    try:
        with lock:
            # Atomic update
            update_shared_resource()
    except LockRecovered:
        print("Previous worker died! repairing state...")
        repair_shared_resource()
        # Retry update
        safe_update()
```
