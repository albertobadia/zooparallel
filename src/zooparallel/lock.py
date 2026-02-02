from .zooparallel_core import ZooLock as ZooLockCore, LockRecovered


class ZooLock:
    """
    ZooLock: A high-performance inter-process mutex in shared memory.
    Supports robust recovery if a process dies while holding the lock.
    """

    def __init__(self, name: str):
        self._core = ZooLockCore(name)

    def acquire(self):
        """Acquire the lock. Blocks until available."""
        self._core.acquire()

    def release(self):
        """Release the lock."""
        self._core.release()

    def __enter__(self):
        self.acquire()
        return self

    def __exit__(self, exc_type, exc_val, exc_tb):
        self.release()

    @staticmethod
    def unlink(name: str):
        """Remove the shared memory segment associated with the lock name."""
        ZooLockCore.unlink(name)


__all__ = ["ZooLock", "LockRecovered"]
