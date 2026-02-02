import pytest
import os
import signal
import multiprocessing
from zooparallel import ZooLock, LockRecovered


def test_basic_lock():
    name = "test_basic_lock"
    try:
        ZooLock.unlink(name)
    except Exception as e:
        print(f"Failed to unlink lock: {e}")
    lock = ZooLock(name)
    with lock:
        assert True

    # Re-acquire
    with lock:
        assert True


def crasher_func(lock_name):
    lock = ZooLock(lock_name)
    print(f"Child {os.getpid()} acquiring lock")
    lock.acquire()
    print(f"Child {os.getpid()} acquired lock. Dying.")
    os.kill(os.getpid(), signal.SIGKILL)


def test_crash_recovery():
    import sys

    if sys.platform != "linux":
        pytest.skip("Robust mutex crash recovery only supported on Linux")

    lock_name = "test_crash_recovery"
    try:
        ZooLock.unlink(lock_name)
    except Exception as e:
        print(f"Failed to unlink lock: {e}")
    lock = ZooLock(lock_name)

    # Spawn a process that acquires lock and dies
    p = multiprocessing.Process(target=crasher_func, args=(lock_name,))
    p.start()
    p.join()  # Wait for it to die

    # It should have died with SIGKILL (-9)
    assert p.exitcode == -9

    print("Parent attempting to acquire lock...")

    # Now we try to acquire. It should raise LockRecovered
    # Note: different pyo3 versions might map exceptions differently, but we defined LockRecovered

    with pytest.raises(LockRecovered):
        lock.acquire()

    # Now valid again
    lock.release()

    # Should be normal now
    with lock:
        pass


if __name__ == "__main__":
    # verification manual run
    try:
        test_crash_recovery()
        print("Crash recovery passed!")
    except Exception as e:
        print(f"Failed: {e}")
