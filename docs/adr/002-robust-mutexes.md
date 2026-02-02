# ADR 002: Robust Mutexes for Crash Safety

## Status
Accepted

## Context
In a multiprocessing environment, if a process crashes (SIGKILL, Segfault) while holding a lock, standard `pthread_mutex` or `multiprocessing.Lock` remains in a locked state. This causes deadlocks for all other processes waiting for that lock.

## Decision
We use `PTHREAD_MUTEX_ROBUST` (available in POSIX/Linux).
*   When a process holding a robust mutex dies, the OS tracks this.
*   The next process attempting to lock it receives `EOWNERDEAD`.
*   ZooParallel catches this error, calls `pthread_mutex_consistent()`, and successfully acquires the lock, raising a `LockRecovered` exception (or warning) to the application.

## Consequences
*   **Pros**:
    *   **Reliability**: Prevents total system freeze on worker crash.
    *   **OS Support**: Handled by the kernel, no complex external health-check daemons needed.
*   **Cons**:
    *   **Platform Support**: `PTHREAD_MUTEX_ROBUST` is widely available on Linux but has varying support on generic POSIX systems (e.g. macOS support is partial or non-standard). We fallback or emulate where necessary (currently mostly robust on Linux).
    *   **Overhead**: Slight overhead for robust mutex checking in the kernel, but negligible compared to Python overhead.
