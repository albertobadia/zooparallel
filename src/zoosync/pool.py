import os
import pickle
import threading
import multiprocessing
import uuid
import time

from concurrent.futures import Future
from .zoosync_core import ZooPoolCore
from .queue import ZooQueue


class ZooPool:
    def __init__(self, num_workers=None, buffer_size_mb=10):
        if num_workers is None:
            num_workers = os.cpu_count() or 1

        self.num_workers = num_workers
        self.core = ZooPoolCore(buffer_size_mb)

        self.futures = {}
        self.workers = []
        self._shutdown = False

        # Start processes
        for _ in range(num_workers):
            p = multiprocessing.Process(
                target=self._worker_loop,
                args=(self.core.task_q_name, self.core.result_q_name, buffer_size_mb),
            )
            p.start()
            self.workers.append(p)

        # Start result listener thread
        self.result_thread = threading.Thread(target=self._result_listener, daemon=True)
        self.result_thread.start()

    @staticmethod
    def _worker_loop(task_q_name, result_q_name, buffer_size_mb):
        # Workers use standard ZooQueue to talk to the core's buffers
        task_q = ZooQueue(task_q_name, buffer_size_mb)
        result_q = ZooQueue(result_q_name, buffer_size_mb)

        while True:
            try:
                task_bytes = task_q.get_bytes()
                task_id, func, args, kwargs = pickle.loads(task_bytes)

                if task_id is None:  # Shutdown signal
                    break

                try:
                    result = func(*args, **kwargs)
                    res_payload = (task_id, result, None)
                except Exception as e:
                    res_payload = (task_id, None, e)

                result_q.put_bytes(pickle.dumps(res_payload))

            except Exception:
                break

    def _result_listener(self):
        while not self._shutdown:
            try:
                res_bytes = self.core.get_result()
                task_id, result, exc = pickle.loads(res_bytes)

                future = self.futures.pop(task_id, None)
                if future:
                    if exc:
                        future.set_exception(exc)
                    else:
                        future.set_result(result)
            except Exception as e:
                print(f"Result listener error: {e}")
                if self._shutdown:
                    break
                time.sleep(0.01)

    def submit(self, func, *args, **kwargs):
        if self._shutdown:
            raise RuntimeError("Pool is shutdown")

        task_id = uuid.uuid4().hex
        future = Future()
        self.futures[task_id] = future

        payload = (task_id, func, args, kwargs)
        self.core.put_task(pickle.dumps(payload))

        return future

    def map(self, func, iterable, timeout=None):
        futures = [self.submit(func, item) for item in iterable]
        return [f.result(timeout=timeout) for f in futures]

    def shutdown(self, wait=True):
        if self._shutdown:
            return
        self._shutdown = True

        # Send sentinel to each worker
        for _ in range(self.num_workers):
            try:
                self.core.put_task(pickle.dumps((None, None, None, None)))
            except Exception as e:
                print(f"Failed to send shutdown signal: {e}")

        if wait:
            for p in self.workers:
                p.join()

        self.core.unlink()

    def __enter__(self):
        return self

    def __exit__(self, exc_type, exc_val, exc_tb):
        self.shutdown()
