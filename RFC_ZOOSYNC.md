# RFC 001: ZooSync - High Performance Multiprocessing Primitives

**Date:** 2026-01-22  
**Status:** Draft  
**Target Audience:** Systems Engineers, Data Platform Developers  

## 1. Abstract

ZooSync is a proposed Python extension library written in Rust designed to provide high-performance, crash-safe, and zero-copy synchronization primitives for multiprocessing. It aims to replace the standard `multiprocessing` synchronization tools (`Lock`, `Queue`, `Semaphore`) with robust implementations backed by Shared Memory and Linux Futexes, specifically addressing the GIL bottleneck and process crash recovery.

## 2. Motivation

The current state of concurrent programming in Python involves significant trade-offs:

*   `threading`: Limited by the Global Interpreter Lock (GIL). CPU-bound tasks do not scale.
*   `multiprocessing`: Bypasses the GIL but introduces high overhead due to pickle serialization and pipe/socket IPC.
*   **Safety Issues**: Standard `multiprocessing.Lock` is not robust. If a process crashes while holding a lock, the lock remains held indefinitely, causing deadlocks.
*   **Performance**: Passing large data between processes involves multiple alloc/copy steps.

ZooSync aims to solve these by leveraging **Rust**, **Shared Memory (shm)**, and **OS-native primitives** (Pthread Mutexes with Robustness, Futexes).

### 2.1. Relationship with ZooLink (Data vs. Control)

**Does this replace `zoolink`? NO.** They are complementary layers of the same stack.

*   **`zoolink` is the DATA Layer (The "Payload")**: Use it to store the 1GB DataFrame, the shared dictionary, or the raw ring buffer. It handles memory mapping and serialization.
*   **`zoosync` is the CONTROL Layer (The "Traffic Light")**: Use it to coordinate *access* to that data.
    *   *Example*: Use `zoolink` to share a video frame. Use `zoosync.Event` to signal the consumer that the frame is ready.

## 3. Proposed Architecture


### 3.1. Core Technology
ZooSync will be built on top of the shared memory foundations established in `zoolink` but will specialize in synchronization logic.

*   **Backend**: POSIX Shared Memory (`shm_open`, `mmap`).
*   **Language**: Rust (with `pyo3` for bindings).
*   **Crash Safety**: Use of `PTHREAD_MUTEX_ROBUST` to detect and recover from dead processes.

### 3.2. Primitives

#### A. ZooLock (Robust Mutex)
A drop-in replacement for `multiprocessing.Lock`.
*   **Implementation**: Uses `pthread_mutex_t` initialized with `PTHREAD_MUTEX_ROBUST`.
*   **Behavior**:
    1.  Process A acquires lock. Code runs in Rust, releasing GIL (`py.allow_threads`).
    2.  If Process A crashes (SIGKILL, Segfault):
    3.  Process B attempts to acquire. The OS returns `EOWNERDEAD`.
    4.  ZooSync automatically marks the lock state as consistent (`pthread_mutex_consistent`) and grants ownership to Process B.
    5.  Process B raises a `LockRecoveredWarning` (optional) but continues execution safely.

#### B. ZooQueue (Zero-Copy MPSC)
A drop-in replacement for `multiprocessing.Queue`.
*   **Implementation**: A circular buffer in shared memory.
*   **Zero-Copy**: Supports writing primitive types (bytes, int, float) directly to the buffer without Pickle overhead.
*   **Smart Serde**: For complex objects, uses `rkyv` (archived zero-copy serialization) or raw bytes, avoiding full Pickle deserialization until access.

#### C. ZooEvent & ZooSemaphore
Backed by **Linux Futexes** (Fast Userspace Mutexes).
*   Waiting on an event consumes **0% CPU** (thread sleeps in kernel) vs `time.sleep` polling loops.
*   Wakeups are instantaneous and broadcast to all processes mapped to the same shared memory region.

### 3.3. GIL Management
All synchronization operations that might block (waiting for a lock, waiting for queue data) will release the python GIL.

```rust
// Rust Pseudo-code
fn acquire(&self, py: Python) -> PyResult<()> {
    py.allow_threads(|| {
        // This blocks the OS thread, but lets other Python threads run
        self.inner_mutex.lock() 
    })
}
```

### 3.4. The Execution Loop (Rust Orchestrator)

You correctly identified a new *pattern* of multiprocessing: **Rust-Orchestrated Python Workers**.

1.  **Orchestration**: The `ZooPool` worker is essentially a `while True` loop in Python that calls into Rust.
2.  **No-GIL Waiting**: The worker calls `zoosync_core.wait_for_task()`. This Rust function:
    *   Releases the GIL immediately.
    *   Sleeps on a **Futex** (OS kernel wait) consuming 0% CPU.
    *   Only wakes up when new data is pushed to the RingBuffer.
3.  **Execution**: Once awake, Rust re-acquires the GIL, deserializes the raw bytes (zero-copy view), and returns control to Python to run the user function.

### 3.5. Security Model

**Do we need a Security Wall?**
*   **OS Permissions**: We rely on standard POSIX Shared Memory permissions (`0o600`). Only the user who spawned the pool can read/write the memory.
*   **Memory Safety**: Rust guarantees (via `unsafe` blocks carefully audited) that a rogue python process cannot corrupt the *structure* of the RingBuffer (pointers, headers), though it could technically write garbage data payload if malicious.
*   **Isolation**: Since workers are separate processes, if one Segfaults (e.g. C-extension bug), it does *not* crash the Orchestrator or other workers. `ZooSync` detects the death and cleans up.

### 3.6. Thread Safety & Reentrancy

**Question**: *Can I call `pool.map` from a background thread?*

**Answer**: Yes, `ZooPool` is internally thread-safe.

*   **The Problem**: If two threads in the *same* parent process try to write to the RingBuffer (shared memory) simultaneously, they could race on the `write_head` pointer.
*   **The Solution**: The `ZooPool` Python object maintains a lightweight internal `threading.Lock`.
    *   This ensures only one local thread serializes/sends a task at a time.
    *   **No Security Wall Needed**: We don't need to block the user. We just serialize their access.
*   **Fork Safety**: We strongly recommend (and on macOS enforce) `spawn`. Calling `fork()` from a multithreaded parent is effectively broken in POSIX. We will warn if `fork` is attempted in a threaded context.

## 4. Implementation Plan



## 4. Implementation Plan

**Phase 1: The Foundation (Robust Locks)**
*   Create `zoosync` crate.
*   Implement `ShmMutex` wrapping `pthread_mutex_t` with `ROBUST` attribute.
*   Expose `ZooLock` to Python.
*   **Goal**: Verify crash recovery with a test script killing processes.

**Phase 2: The Queue (Zero-Copy)**
*   Implement a RingBuffer in Rust (reusing `zoolink` logic).
*   Implement atomic Head/Tail pointers.
*   Expose `ZooQueue` with `put_bytes` / `get_bytes`.

**Phase 3: High-Level API**
*   Add `ZooPool` (Worker pool using ZooQueues).
*   Add decorators for easy usage: `@unsync` (runs function in separate process with transparent ZooSync coordination).

## 5. Comparison

| Feature | `multiprocessing.Lock` | `posix_ipc` | `ZooSync` |
| :--- | :--- | :--- | :--- |
| **GIL Free** | YES (Process based) | YES | **YES (+ Thread-friendly)** |
| **Crash Safe** | NO (Deadlocks) | NO | **YES (Auto-recover)** |
| **Performance** | Low (Pickle/Pipe) | High | **Ultra-High (Shm/Futex)** |
| **UX** | Pythonic | C-like | **Pythonic** |

| **UX** | Pythonic | C-like | **Pythonic** |

## 6. Performance Estimates

Estimated overhead comparison (latency per message):

| Scenario | MP Native + ZooLink | ZooSync | Gain |
| :--- | :--- | :--- | :--- |
| **Streaming Video (60 FPS)** | ~5% CPU overhead | ~0.1% CPU overhead | Marginal (Already fast) |
| **High Frequency Trading (10k msgs/s)** | **CPU Saturation** (Pickle/Pipes) | ~5% CPU overhead | **10x - 50x** |
| **Ping-Pong Latency** | ~40 µs | ~2 µs | **20x Lower** |

### Why?
*   **Zero Syscalls**: `ZooSync` uses memory pointers for data.
*   **Futex Efficiency**: Waiting for a lock sleeps in the Kernel. `multiprocessing` often spins or uses slower Semaphores.
*   **No Pickle**: We bypass the entire Python object serialization machinery for raw data.

## 7. Developer Experience (DX)


The goal is to make `ZooSync` feel like standard Python, but "indestructible".

### 6.1. Robust Locking (The "Tank" Lock)

Standard `multiprocessing.Lock` hangs if a process dies. `ZooSync` recovers.

```python
import time
import os
import signal
from zoosync import ZooLock, LockRecovered

# Create a named lock (backed by /dev/shm/zoosync_my_resource)
lock = ZooLock("my_resource")

def worker_that_crashes():
    with lock:
        print(f"Process {os.getpid()} got the lock. Dying now...")
        os.kill(os.getpid(), signal.SIGKILL)

def worker_that_survives():
    time.sleep(1) # Wait for the other process to die
    try:
        print(f"Process {os.getpid()} waiting for lock...")
        with lock:
            print(f"Process {os.getpid()} ACQUIRED lock successfully!")
            print("Accessing shared resource safely.")
    except LockRecovered:
        print("WARNING: The previous owner died! State might be inconsistent.")
        # Perform cleanup/repair logic here
        lock.mark_consistent() # Declare that we fixed the mess

# Imagine running these in two separate terminals or processes
```

### 6.2. Zero-Copy Queue

Transmission of binary data (images, numpy arrays, protobufs) without Pickle overhead.

```python
import numpy as np
from zoosync import ZooQueue

# Create a queue with 10MB capacity
queue = ZooQueue("video_frames", size_mb=10)

def producer():
    # 4K Frame (approx 24MB) fits in multiple chunks or simply pass reference if using zoolink
    # Here we show passing raw bytes efficiently
    data = b'\x00' * 1024 * 1024 # 1MB junk
    
    # Blocks if full, releases GIL while waiting
    queue.put_bytes(data) 
    print("Sent frame")

def consumer():
    # Blocks if empty, releases GIL. 
    # Returns 'memoryview' (zero-copy access to shm)
    data_view = queue.get_bytes() 
    
    # Convert to numpy mostly zero-copy
    arr = np.frombuffer(data_view, dtype=np.uint8)
    process(arr)
```

### 6.3. The "Atomic" Block

A higher-level abstraction for convenient critical sections.

```python
from zoosync import Atomic

# Automatically creates/manages the underlying shm mutex
# atomic_db is a ZooLock("db_transaction") under the hood
atomic_db = Atomic("db_transaction")

@atomic_db
def critical_update():
    # Safe multi-process execution
    # If this function crashes mid-way, the next caller gets LockRecovered
    update_shared_state()
```

### 6.4. Process Management

`ZooSync` primitives are picklable and process-safe, so they work with standard `multiprocessing`. However, we also provide `ZooProcess` and `ZooPool` for a smoother experience.

**Option A: Manual (Standard Multiprocessing)**

```python
import multiprocessing
from zoosync import ZooLock

def worker(lock):
    with lock:
        print("Working safely")

if __name__ == "__main__":
    # The lock object can be passed to children. 
    # Under the hood, we pass the "shared memory name", and the child re-opens it.
    lock = ZooLock("my_job") 
    
    p = multiprocessing.Process(target=worker, args=(lock,))
    p.start()
    p.join()
```

**Option B: The "ZooPool" (Recommended)**

A zero-boilerplate pool that handles process spawning and resource cleanup.

```python
from zoosync import ZooPool

def complex_task(dataset_chunk):
    # Do heavy work
    return result

if __name__ == "__main__":
    # Spawns 4 worker processes that are strictly isolated
    # Automatically cleans up shared memory if the parent crashes
    with ZooPool(processes=4) as pool:
        results = pool.map(complex_task, data_chunks)
```

    with ZooPool(processes=4) as pool:
        results = pool.map(complex_task, data_chunks)
```

### 6.5. Does this avoid the GIL?

**YES.**

1.  **Architecture**: `ZooPool` uses **Processes** (not threads). Each process has its own independent GIL. Your 4 CPU cores will run at 100%.
2.  **The ZooSync Difference**:
    *   **Standard `multiprocessing`**: Avoids GIL for computation, but **hitches up** during communication. Sending data involves Pickling (holding GIL) and Pipe I/O (syscalls).
    *   **ZooSync**: Data transfer happens in Shared Memory. The coordination (waiting for the queue) releases the GIL in Rust.
    *   **Result**: You spend nearly 0% time fighting for resources and 100% time computing.

**How it works under the hood:**
1.  **Serialization**: `ZooSync` objects implement `__getstate__` and `__setstate__`. When passed to a child, they serialize their *Shared Memory Name* (string).

2.  **Re-hydration**: The child process receives the name, calls `shm_open`, and maps the *same* physical memory page.
3.  **Atomic Counts**: The shared memory header contains a `ref_count` which is atomically incremented by children, ensuring memory isn't unlinked until everyone is done.



