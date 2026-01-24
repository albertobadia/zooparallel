# Features: Zero-Copy Queues (`ZooQueue`)

## Overview

`ZooQueue` is a high-throughput MPSC (Multi-Producer, Single-Consumer) queue designed for passing large data blobs between processes.

## Theory of Operation

Standard `multiprocessing.Queue` uses:
1.  **Pickle**: Serializes Python objects to bytes (CPU intensive).
2.  **Pipe/Socket**: Writes bytes to a file descriptor (Syscalls, Kernel buffer copy).
3.  **Unpickle**: Recreates objects in the receiver.

**`ZooQueue` optimizations**:
1.  **Shared Memory**: A fixed-size Ring Buffer in `/dev/shm`.
2.  **Raw Bytes**: You can push/pop raw `bytes`, skipping Pickle if you handle serialization (e.g. using Arrow/Protobuf/Flatbuffers).
3.  **Zero-Copy**: The memory is mapped into both processes. The "Send" is a `memcpy` to shared memory. The "Receive" can be a `memoryview` pointing to that same memory, avoiding the final copy into Python's heap.

## Usage Patterns

### Standard Mode (Copy)
Simpler API, safer. Copies data from shm to a Python `bytes` object.

```python
q.put_bytes(b"hello")
data = q.get_bytes() # -> b"hello"
```

### Zero-Copy Mode (Advanced)
For maximum speed with large data (images, tensors).

```python
# Receiver
view, cursor = q.recv_view()
# 'view' is a memoryview. It is valid ONLY while the lock is held 
# (which is implicitly released by commit_read, but you must be careful not to
# block the queue for too long).
# Use immediately (e.g. np.frombuffer)
arr = np.frombuffer(view, dtype='uint8')
q.commit_read(cursor) # Advance the queue and release lock
```

## Benchmarks
*   **Throughput**: ~32 GB/s for 1MB payloads.
*   **Latency**: ~2µs for small messages.
