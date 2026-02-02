# ZooParallel User Guide

## Introduction
ZooParallel provides a suite of tools to make Python multiprocessing faster, safer, and easier. This guide goes beyond the basics to help you build production-ready systems.

## 🛠️ Configuration & Tuning

### Choosing Buffer Sizes
`ZooQueue` and `ZooPool` rely on a fixed-size Ring Buffer in Shared Memory.
*   **Default**: 10MB to 100MB is common.
*   **Formula**: `size_mb >= max_item_size * max_items_in_flight * safety_margin`
*   **Sizing Tip**: Better to embrace a larger buffer (e.g., 500MB) than risk blocking. Shared Memory is virtual; unused pages don't steal physical RAM instantly.

### IPC Naming
 IPC primitives use file-system-like names (e.g., `/my_queue`).
*   **Namespacing**: Prefix names with your app name to avoid collisions: `/myapp_video_q`.
*   **Cleanup**: If your app crashes hard, these files (in `/dev/shm` on Linux) might persist. A reboot clears them, or you can manually `rm /dev/shm/myapp_*`.

## 🏗️ Common Patterns

### 1. The "Tank" Lock (Robust Mutex)
Use `ZooLock` when workers are unstable or run risky C-extensions.

```python
from zooparallel import ZooLock, LockRecovered

lock = ZooLock("database_write_lock")

def transactional_write():
    try:
        lock.acquire()
        # 1. Write to Journal
        # 2. Write to DB
        # 3. Clear Journal
    except LockRecovered:
        print("⚠️ Previous worker crashed mid-write!")
        # RECOVERY LOGIC:
        # Check Journal. If pending write exists, roll back or replay.
        recover_from_journal()
    finally:
        lock.release()
```

### 2. The Video Pipeline (Memory Efficient)
Processing high-res frames without copying data unnecessarily.

```python
# Stage 1: Capture (C++) -> Shared Memory -> Stage 2: AI (Python)
q = ZooQueue("camera_frames", size_mb=1024) # 1GB Buffer

while True:
    try:
         # Zero-Copy Read
        view, cursor = q.recv_view()
        
        # Process directly on the view (fast!)
        # (Assuming you wrapped it in numpy/arrow as shown in adapters.md)
        process_frame_in_place(view)
        
        q.commit_read(cursor)
    except Exception as e:
        print(f"Pipeline error: {e}")
```

### 3. The "Fire Hose" (Batching)
If sending millions of tiny items, even ZooParallel has overhead. Batching is key.

```python
# Instead of:
# for item in items: q.put_bytes(pickle.dumps(item))

# Do this:
batch = []
for item in items:
    batch.append(item)
    if len(batch) > 1000:
        q.put_bytes(pickle.dumps(batch)) # One lock acquisition for 1000 items
        batch = []
```

## 🧹 Best Practices

### Resource Cleanup
Shared memory persists until unlinked.
*   **Always use Context Managers**: `with ZooPool(...) as pool:` handles cleanup automatically.
*   **Manual Unlink**: If instantiating `ZooQueue` manually, ensure the **owner** (usually the parent process) calls `.unlink()` at exit.
*   **Signal Handlers**: Register `signal.signal(signal.SIGTERM, cleanup_handler)` in long-running services to ensure unlink happens even on termination.

### Serialization
*   **Pickle is slow**: Standard `ZooPool` uses `pickle`. For maximum speed with complex data, verify if `msgpack` or `pyarrow` serialization (sent as bytes via `ZooQueue`) is faster for your use case.
*   **Raw Bytes**: The fastest path is always `put_bytes` / `get_bytes`.

## ❓ Troubleshooting

### "No such file or directory"
*   **Context**: Calling `ZooQueue("name")` (consumer) before the creator (producer) has initialized it.
*   **Fix**: Ensure the parent process or the designated "creator" starts first and initializes the queue.

### "Bus Error" (SIGBUS)
*   **Context**: Accessing shared memory that has been deleted (unlinked) or truncated by another process.
*   **Fix**: 
    1.  Don't call `unlink()` while other processes are still reading.
    2.  Check if `/dev/shm` is full ( `df -h /dev/shm`).

### "Buffer Full / Deadlock"
*   **Context**: Producer is faster than consumer, filling the ring buffer.
*   **Fix**: Increase `size_mb` or implement "backpressure" (producer slows down if queue is full).
