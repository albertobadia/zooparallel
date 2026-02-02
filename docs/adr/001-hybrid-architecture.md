# ADR 001: Hybrid Architecture (Rust + Python)

## Status
Accepted

## Context
Python applications require high performance for certain tasks (like IPC) but flexibility for user code. The Global Interpreter Lock (GIL) is a major bottleneck for CPU-bound concurrency. We needed a way to provide performant synchronization primitives without the overhead of Python's object model and GIL.

## Decision
We chose a hybrid architecture:
1.  **Rust (zooparallel_core)**: Handles the "Control Plane". It implements lower-level synchronization logic, interacts with OS primitives (shm, futexes, robust mutexes), and manages memory safety.
2.  **Python (zooparallel)**: Handles the "User API". It wraps the Rust extension in pythonic classes (`ZooLock`, `ZooQueue`), manages process lifecycle (`ZooPool`), and handles serialization (pickle/rkyv).

## Consequences
*   **Pros**:
    *   **GIL Release**: Rust code releases the GIL during blocking operations (waiting for lock/queue), allowing true parallelism for Python threads managing these waits.
    *   **Safety**: Rust's borrow checker prevents many classes of memory errors common in C extensions.
    *   **Performance**: Zero-cost abstractions and direct memory access in Rust.
*   **Cons**:
    *   **Build Complexity**: Requires `cargo` and `maturin` to build.
    *   **Binary Size**: The extension is a shared library (`.so`/`.dylib`).
