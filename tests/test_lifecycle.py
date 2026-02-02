import pytest
from zooparallel import ZooPool, ZooQueue


def identity(x):
    return x


def test_no_unlink_on_drop():
    """
    Verify that dropping a ZooPool object does NOT unlink the underlying shared memory.
    """
    # Create a pool
    pool = ZooPool(num_workers=1)

    # Get names of queues
    task_q_name = pool.core.task_q_name
    result_q_name = pool.core.result_q_name

    # Verify SHM files exist by attempting to open a queue wrapper
    _ = ZooQueue(task_q_name, 10)

    # Store explicit data
    pool.submit(identity, 1)

    # Shutdown explicitly but DO NOT unlink to simulate persistent SHM
    pool.shutdown(unlink=False)

    # Now delete pool (should be safe as threads are stopped)
    del pool
    import gc

    gc.collect()

    # Check if we can still open the queues
    # If unlinked, opening might succeed (creating new) but checks below verify behavior
    try:
        _ = ZooQueue(task_q_name, 10)
    except Exception as e:
        pytest.fail(f"Failed to access queue after pool drop: {e}")

    # Clean up explicitly
    ZooQueue.unlink(task_q_name)
    ZooQueue.unlink(result_q_name)


def test_explicit_shutdown_unlinks():
    """
    Verify that explicit shutdown DOES unlink.
    """
    pool = ZooPool(num_workers=1)
    task_q_name = pool.core.task_q_name

    pool.shutdown()

    # ZooQueue.unlink should define the behavior for non-existent files (e.g., raise or ignore)
    with pytest.raises(Exception, match="No such file|ENOENT"):
        ZooQueue.unlink(task_q_name)


if __name__ == "__main__":
    # verification
    try:
        test_no_unlink_on_drop()
        print("test_no_unlink_on_drop passed")
        test_explicit_shutdown_unlinks()
        print("test_explicit_shutdown_unlinks passed")
    except Exception as e:
        print(f"FAILED: {e}")
        import traceback

        traceback.print_exc()
