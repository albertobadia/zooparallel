import time
import multiprocessing
from zoosync import ZooQueue

# Payload sizes to test
SIZES = [1024, 64 * 1024, 1024 * 1024]  # 1KB, 64KB, 1MB
ops_count = 10_000
BUFFER_SIZE_MB = 200


def consumer_mp(q, count):
    for _ in range(count):
        _ = q.get()


def consumer_zoo(name, count, size_mb):
    # Consumer waits a bit to ensure producer created it?
    # No, ZooQueue.new handles synchronization via init_done.
    try:
        q = ZooQueue(name, size_mb)
        for _ in range(count):
            _ = q.get_bytes()
    except Exception as e:
        print(f"Consumer error: {e}")


def run_bench(payload_size):
    print(f"\n--- Payload: {payload_size} bytes, {ops_count} ops ---")
    payload = b"x" * payload_size

    # --- Multiprocessing Queue ---
    q = multiprocessing.Queue()
    p = multiprocessing.Process(target=consumer_mp, args=(q, ops_count))
    p.start()

    start = time.perf_counter()
    for _ in range(ops_count):
        q.put(payload)
    p.join(timeout=10)  # 10s timeout for stability
    if p.is_alive():
        print("MP Queue HANGED!")
        p.terminate()
        return

    duration = time.perf_counter() - start
    mb_processed = (ops_count * payload_size) / (1024 * 1024)
    print(
        f"MP Queue:   {duration:.4f}s  ({ops_count / duration:.0f} ops/s, {mb_processed / duration:.2f} MB/s)"
    )

    # --- ZooQueue ---
    name = f"bench_q_{payload_size}"

    # Cleanup previous run if any
    try:
        ZooQueue.unlink(name)
    except Exception as e:
        print(f"Unlink error: {e}")

    p = multiprocessing.Process(
        target=consumer_zoo, args=(name, ops_count, BUFFER_SIZE_MB)
    )
    p.start()

    # Small delay to let consumer start waiting or producer start creating
    time.sleep(0.1)

    try:
        q = ZooQueue(name, BUFFER_SIZE_MB)
        start = time.perf_counter()
        for i in range(ops_count):
            q.put_bytes(payload)

        p.join(timeout=20)  # 20s timeout for ZooQueue
        if p.is_alive():
            print("ZooQueue HANGED!")
            # Diagnostic: print if possible?
            p.terminate()
        else:
            duration = time.perf_counter() - start
            print(
                f"ZooQueue:   {duration:.4f}s  ({ops_count / duration:.0f} ops/s, {mb_processed / duration:.2f} MB/s)"
            )
    except Exception as e:
        print(f"Producer error: {e}")
    finally:
        try:
            ZooQueue.unlink(name)
        except Exception as e:
            print(f"Unlink error: {e}")


if __name__ == "__main__":
    for s in SIZES:
        run_bench(s)
