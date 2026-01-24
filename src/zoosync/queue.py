import contextlib
from .zoosync_core import ZooQueue as ZooQueueCore


class ZooQueue:
    """
    ZooQueue: A high-performance inter-process queue based on shared memory.
    """

    def __init__(self, name: str, size_mb: int = 10):
        self._core = ZooQueueCore(name, size_mb)

    def put_bytes(self, data: bytes):
        """Put bytes into the queue. Blocks if full."""
        self._core.put_bytes(data)

    def get_bytes(self) -> bytes:
        """Get bytes from the queue. Blocks if empty."""
        return self._core.get_bytes()

    @contextlib.contextmanager
    def recv_view(self):
        """
        Zero-copy context manager for receiving data.

        Usage:
            with queue.recv_view() as view:
                # Use view (memoryview) directly
                process(view)

        The read is committed automatically when exiting the block.
        """
        view, info = self._core.recv_view()
        try:
            yield view
        finally:
            self._core.commit_read(info)

    @staticmethod
    def unlink(name: str):
        """Remove the shared memory segment associated with the queue name."""
        ZooQueueCore.unlink(name)
