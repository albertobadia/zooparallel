from zoosync import ZooPool
import time


def square(x):
    return x * x


def slow_square(x):
    time.sleep(0.1)
    return x * x


def test_pool_basic():
    print("Testing ZooPool basic...")
    with ZooPool(num_workers=4) as pool:
        # Single submit
        f = pool.submit(square, 10)
        print(f"10 * 10 = {f.result()}")
        assert f.result() == 100

        # Map
        results = pool.map(square, range(10))
        print(f"Map results: {results}")
        assert results == [x * x for x in range(10)]

        # Parallel map
        start = time.perf_counter()
        pool.map(slow_square, range(8))
        duration = time.perf_counter() - start
        print(f"Parallel map (8 items, 4 workers, 0.1s each) took {duration:.2f}s")
        assert duration < 0.3  # Should take ~0.2s

    print("ZooPool basic tests passed!")


if __name__ == "__main__":
    test_pool_basic()
