import os
import signal
import time
from zooparallel import ZooPool


def suicide():
    """Kill self with SIGKILL to simulate crash"""
    os.kill(os.getpid(), signal.SIGKILL)
    time.sleep(1)  # Should not reach here


def identity(x):
    return x


def test_worker_restart():
    # Start pool with 1 worker
    with ZooPool(num_workers=1) as pool:
        # Submit task that kills the worker to trigger restart
        pool.submit(suicide)

        # Wait for monitor (interval 1s)
        time.sleep(2.0)

        # Verify worker restarted by submitting a new task
        f = pool.submit(identity, 42)
        assert f.result(timeout=5.0) == 42


if __name__ == "__main__":
    test_worker_restart()
    print("test_worker_restart passed")
