import time
from concurrent.futures import ProcessPoolExecutor
from zooparallel import ZooPool


def fast_task(x):
    return x + 1


def large_payload_task(data):
    return len(data)


def run_bench():
    num_workers = 4
    num_tasks = 2000

    print(f"--- Small Task Overhead ({num_tasks} tasks, {num_workers} workers) ---")

    # ProcessPoolExecutor
    with ProcessPoolExecutor(max_workers=num_workers) as executor:
        # Warmup
        list(executor.map(fast_task, range(10)))

        start = time.perf_counter()
        list(executor.map(fast_task, range(num_tasks)))
        duration = time.perf_counter() - start
        print(
            f"ProcessPoolExecutor: {duration:.4f}s ({num_tasks / duration:.0f} tasks/s)"
        )

    # ZooPool
    with ZooPool(num_workers=num_workers) as pool:
        # Warmup
        pool.map(fast_task, range(10))

        start = time.perf_counter()
        pool.map(fast_task, range(num_tasks))
        duration = time.perf_counter() - start
        print(
            f"ZooPool:            {duration:.4f}s ({num_tasks / duration:.0f} tasks/s)"
        )

    print("\n--- Large Payload Overhead (100 tasks, 1MB payload) ---")
    payload = b"x" * (1024 * 1024)
    num_large = 100

    # ProcessPoolExecutor
    with ProcessPoolExecutor(max_workers=num_workers) as executor:
        start = time.perf_counter()
        list(executor.map(large_payload_task, [payload] * num_large))
        duration = time.perf_counter() - start
        print(
            f"ProcessPoolExecutor: {duration:.4f}s ({num_large / duration:.1f} tasks/s)"
        )

    # ZooPool
    with ZooPool(num_workers=num_workers, buffer_size_mb=150) as pool:
        start = time.perf_counter()
        pool.map(large_payload_task, [payload] * num_large)
        duration = time.perf_counter() - start
        print(
            f"ZooPool:            {duration:.4f}s ({num_large / duration:.1f} tasks/s)"
        )


if __name__ == "__main__":
    run_bench()
