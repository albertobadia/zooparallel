# Features: ZooPool

## Overview

`ZooPool` is a process pool executor designed to overcome the overhead of `multiprocessing.Pool` and `ProcessPoolExecutor`.

## The Problem with Standard Pools
Standard pools focus on CPU-bound tasks but often fail to saturate CPUs for:
1.  **Small Tasks**: The overhead of pickling the function+args and sending it via pipe is higher than the task execution time.
2.  **Short Tasks**: Creating/destroying processes or thread-states is expensive.

## The ZooParallel Solution
*   **Persistent Workers**: Workers are long-lived processes.
*   **Shm Communication**: Task payloads and results are exchanged via `ZooQueue`.
*   **Rust Orchestrator**: The worker loop yields the GIL to Rust while waiting for tasks. It sleeps on a Futex (0% CPU) and wakes up instantly when a task arrives.

## Performance
Benchmarks show **8x improvement** in tasks/second for small payloads.

## Code Example

```python
from zooparallel import ZooPool

def compute(x):
    return x * x

# Initialize pool with 4 workers and 100MB shared buffer
with ZooPool(num_workers=4, buffer_size_mb=100) as pool:
    # Blocking map
    results = pool.map(compute, range(1000))

    # Async submit
    future = pool.submit(compute, 99)
    print(future.result())
```
