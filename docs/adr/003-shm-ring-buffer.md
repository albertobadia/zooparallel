# ADR 003: Shared Memory Ring Buffer (Zero-Copy)

## Status
Accepted

## Context
Passing data between processes usually involves serialization (Pickle), writing to a pipe (Simulated file I/O), reading, and deserializing. This involves multiple memory copies and syscalls.

## Decision
We implement a **Mirrored Ring Buffer** in Shared Memory.
1.  **Shared Memory**: Data resides in a file usually mapped to `/dev/shm`.
2.  **Ring Buffer**: A circular queue structure with atomic `read` and `write` pointers.
3.  **Mirroring**: We use the OS `mmap` trick (double mapping) where the buffer is mapped twice back-to-back in virtual memory. This makes the ring buffer appear contiguous, eliminating the need to handle wrapping logic for `memcpy`.
4.  **Zero-Copy Views**: Readers can access data directly from shared memory via `memoryview` without copying it into Python's heap.

## Consequences
*   **Pros**:
    *   **Throughput**: 100x+ faster for large payloads confirmed by benchmarks.
    *   **Latency**: Reduced synchronization cost.
*   **Cons**:
    *   **Fixed Size**: The buffer must be pre-allocated. It cannot grow dynamically beyond its initialization size easily without re-mapping all processes.
    *   **Safety**: If a user writes to the raw memory view dangerously (bypassing our API), they can corrupt the queue.
