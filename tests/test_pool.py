from zooparallel import ZooPool
import time


def square(x):
    return x * x


def slow_square(x):
    time.sleep(0.1)
    return x * x


def test_pool_basic():
    # Basic ZooPool test
    with ZooPool(num_workers=4) as pool:
        # Single submit
        f = pool.submit(square, 10)
        # Single submit
        f = pool.submit(square, 10)
        assert f.result() == 100, f"Expected 100, got {f.result()}"

        # Map
        results = pool.map(square, range(10))
        # Map
        results = pool.map(square, range(10))
        assert results == [x * x for x in range(10)], f"Map results mismatch: {results}"

        # Parallel map
        start = time.perf_counter()
        pool.map(slow_square, range(8))
        duration = time.perf_counter() - start
        # Parallel map
        start = time.perf_counter()
        pool.map(slow_square, range(8))
        duration = time.perf_counter() - start

        # Parallel execution (4 workers) should be faster than serial (0.8s)
        # 8 items / 4 workers = 2 rounds * 0.1s = 0.2s ideal + overhead
        assert duration < 0.5, f"Parallel map took too long: {duration:.2f}s"


if __name__ == "__main__":
    test_pool_basic()
