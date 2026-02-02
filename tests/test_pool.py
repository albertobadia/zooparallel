from zooparallel import ZooPool
import time


def square(x):
    return x * x


def slow_square(x):
    time.sleep(0.1)
    return x * x


def test_pool_basic():
    with ZooPool(num_workers=4) as pool:
        f = pool.submit(square, 10)
        assert f.result() == 100, f"Expected 100, got {f.result()}"

        results = pool.map(square, range(10))
        assert results == [x * x for x in range(10)], f"Map results mismatch: {results}"

        start = time.perf_counter()
        pool.map(slow_square, range(8))
        duration = time.perf_counter() - start

        assert duration < 0.8, f"Parallel map took too long: {duration:.2f}s"


if __name__ == "__main__":
    test_pool_basic()
