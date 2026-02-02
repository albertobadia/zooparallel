import os
import pytest
from zooparallel import ZooPool


def large_task(size_mb):
    # Process large data and return it
    data = os.urandom(size_mb * 1024 * 1024)
    return len(data)


def error_task():
    raise ValueError("Intentional error for testing")


def test_large_payloads():
    """Test that the ring buffer handles large data transfer correctly."""
    with ZooPool(num_workers=2, buffer_size_mb=20) as pool:
        # Submit task that uses significant chunk of buffer
        future = pool.submit(large_task, 5)
        assert future.result() == 5 * 1024 * 1024

        # Multiple large tasks
        futures = [pool.submit(large_task, 2) for _ in range(5)]
        results = [f.result() for f in futures]
        assert all(r == 2 * 1024 * 1024 for r in results)


def test_error_propagation():
    """Verify that exceptions in workers are correctly propagated to futures."""
    with ZooPool(num_workers=1) as pool:
        future = pool.submit(error_task)
        with pytest.raises(ValueError, match="Intentional error"):
            future.result()


def test_concurrent_pools():
    """Verify that multiple pools can co-exist without SHM name collisions."""
    with ZooPool(num_workers=1) as p1:
        with ZooPool(num_workers=1) as p2:
            f1 = p1.submit(os.getpid)
            f2 = p2.submit(os.getpid)
            # They should work independently
            assert f1.result() != f2.result()


if __name__ == "__main__":
    pytest.main([__file__])
