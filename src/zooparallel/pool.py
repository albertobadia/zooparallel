import os
import pickle
import threading
import multiprocessing
import uuid
import time

from concurrent.futures import Future
from .zooparallel_core import ZooPoolCore
from .queue import ZooQueue


class ZooPool:
    def __init__(self, num_workers=None, buffer_size_mb=10):
        if num_workers is None:
            num_workers = os.cpu_count() or 1

        self.num_workers = num_workers
        self.buffer_size_mb = buffer_size_mb
        self.core = ZooPoolCore(buffer_size_mb)

        self.futures = {}
        self.workers = []
        self._shutdown = False

        for _ in range(num_workers):
            p = multiprocessing.Process(
                target=self._worker_loop,
                args=(self.core.task_q_name, self.core.result_q_name, buffer_size_mb),
            )
            p.start()
            self.workers.append(p)

        self.result_thread = threading.Thread(target=self._result_listener, daemon=True)
        self.result_thread.start()

        self.monitor_thread = threading.Thread(
            target=self._monitor_workers, daemon=True
        )
        self.monitor_thread.start()

    def _monitor_workers(self):
        """
        Monitor worker processes and restart them if they exit unexpectedly.
        """
        while not self._shutdown:
            for i, p in enumerate(self.workers):
                if not p.is_alive():
                    if self._shutdown:
                        break

                    exitcode = p.exitcode
                    print(f"Worker {p.pid} exited (code {exitcode}). Restarting...")

                    new_p = multiprocessing.Process(
                        target=self._worker_loop,
                        args=(
                            self.core.task_q_name,
                            self.core.result_q_name,
                            self.buffer_size_mb,
                        ),
                    )
                    new_p.start()
                    self.workers[i] = new_p

            time.sleep(1.0)

    @staticmethod
    def _worker_loop(task_q_name, result_q_name, buffer_size_mb):
        task_q = ZooQueue(task_q_name, buffer_size_mb)
        result_q = ZooQueue(result_q_name, buffer_size_mb)

        while True:
            try:
                with task_q.recv_view() as view:
                    task_id, func, args, kwargs = pickle.loads(view)

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

                if task_id is None:  # Sentinel
                    break

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

    def shutdown(self, wait=True, unlink=True):
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

        # Shutdown result listener
        try:
            # Connect to result queue to send sentinel
            result_q = ZooQueue(self.core.result_q_name, self.buffer_size_mb)
            sentinel = pickle.dumps((None, None, None))
            result_q.put_bytes(sentinel)
        except Exception as e:
            print(f"Failed to stop result listener: {e}")

        if self.result_thread.is_alive():
            self.result_thread.join(timeout=1.0)

        if self.monitor_thread.is_alive():
            self.monitor_thread.join(timeout=1.0)

        if unlink:
            self.core.unlink()

    def __enter__(self):
        return self

    def __exit__(self, exc_type, exc_val, exc_tb):
        self.shutdown()
