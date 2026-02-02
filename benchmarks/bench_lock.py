import time
import multiprocessing
from zooparallel import ZooLock

ITERATIONS = 100_000


def bench_mplock(lock, iterations):
    start = time.perf_counter()
    for _ in range(iterations):
        with lock:
            pass
    return time.perf_counter() - start


def bench_zoolock(lock_name, iterations):
    # Re-open lock in child/process
    lock = ZooLock(lock_name)
    start = time.perf_counter()
    for _ in range(iterations):
        with lock:
            pass
    return time.perf_counter() - start


def run_single_process():
    print(f"\n--- Single Process Latency ({ITERATIONS} ops) ---")

    # Multiprocessing Lock
    lock = multiprocessing.Lock()
    duration = bench_mplock(lock, ITERATIONS)
    print(f"MP Lock:  {duration:.4f}s  ({ITERATIONS / duration:.0f} ops/s)")

    # ZooParallel Lock
    lock = ZooLock("bench_single")
    duration = bench_zoolock("bench_single", ITERATIONS)
    print(f"ZooParallel Lock:  {duration:.4f}s  ({ITERATIONS / duration:.0f} ops/s)")


def worker_contention(lock_type, lock_arg, iterations):
    if lock_type == "mp":
        with lock_arg:
            pass  # Warmup?
        start = time.perf_counter()
        for _ in range(iterations):
            with lock_arg:
                pass
        return time.perf_counter() - start
    else:
        # lock_arg is name
        lock = ZooLock(lock_arg)
        start = time.perf_counter()
        for _ in range(iterations):
            with lock:
                pass
        return time.perf_counter() - start


def run_multi_process(procs=4):
    print(f"\n--- Multi Process Contention ({procs} procs, {ITERATIONS} ops each) ---")

    # MP Lock
    lock = multiprocessing.Lock()
    ctx = multiprocessing.get_context("spawn")
    processes = []

    start_global = time.perf_counter()
    for _ in range(procs):
        p = ctx.Process(target=worker_contention, args=("mp", lock, ITERATIONS))
        p.start()
        processes.append(p)

    for p in processes:
        p.join()
    duration = time.perf_counter() - start_global
    total_ops = ITERATIONS * procs
    print(f"MP Lock:  {duration:.4f}s  ({total_ops / duration:.0f} ops/s combined)")

    # ZooLock
    lock_name = "bench_multi"
    # Ensure it exists
    _ = ZooLock(lock_name)

    processes = []
    start_global = time.perf_counter()
    for _ in range(procs):
        p = ctx.Process(target=worker_contention, args=("zoo", lock_name, ITERATIONS))
        p.start()
        processes.append(p)

    for p in processes:
        p.join()
    duration = time.perf_counter() - start_global
    print(f"ZooLock:  {duration:.4f}s  ({total_ops / duration:.0f} ops/s combined)")


if __name__ == "__main__":
    run_single_process()
    run_multi_process(2)
    run_multi_process(4)
