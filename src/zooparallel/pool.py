import os
import pickle
import threading
import multiprocessing
import uuid
import time
import signal
import weakref
import atexit

from concurrent.futures import Future
from .zooparallel_core import ZooPoolCore
from .queue import ZooQueue

_active_pools = weakref.WeakSet()
_signal_handlers_registered = False
_handler_lock = threading.Lock()


def _cleanup_all_pools(signum=None, frame=None):
    """Cleanup helper for signals and exit"""
    for pool in list(_active_pools):
        try:
            pool.shutdown(wait=False, unlink=True)
        except Exception:
            pass
    if signum is not None and signum != signal.SIGINT:
        os._exit(1)


def _register_signal_handlers():
    global _signal_handlers_registered
    with _handler_lock:
        if _signal_handlers_registered:
            return

        for sig in (signal.SIGINT, signal.SIGTERM):
            try:
                original = signal.getsignal(sig)

                def handler(s, f):
                    _cleanup_all_pools(s, f)
                    if callable(original):
                        original(s, f)
                    elif s == signal.SIGINT:
                        raise KeyboardInterrupt
                    else:
                        os._exit(1)

                signal.signal(sig, handler)
            except (ValueError, RuntimeError):
                pass

        atexit.register(_cleanup_all_pools)
        _signal_handlers_registered = True


class ZooPool:
    def __init__(self, num_workers=None, buffer_size_mb=10):
        if num_workers is None:
            num_workers = os.cpu_count() or 1

        self.num_workers = num_workers
        self.buffer_size_mb = buffer_size_mb
        self.workers = []
        self.futures = {}
        self.task_payloads = {}
        self.active_tasks = {}
        self._shutdown = False

        try:
            self.core = ZooPoolCore(buffer_size_mb)

            for _ in range(num_workers):
                p = multiprocessing.Process(
                    target=self._worker_loop,
                    args=(
                        self.core.task_q_name,
                        self.core.result_q_name,
                        buffer_size_mb,
                    ),
                )
                p.start()
                self.workers.append(p)

            self.result_thread = threading.Thread(
                target=self._result_listener, daemon=True
            )
            self.result_thread.start()

            self.monitor_thread = threading.Thread(
                target=self._monitor_workers, daemon=True
            )
            self.monitor_thread.start()

            _register_signal_handlers()
            _active_pools.add(self)
            self._finalizer = weakref.finalize(
                self, self.shutdown, wait=False, unlink=True
            )
        except Exception:
            self._shutdown = True
            for p in self.workers:
                p.terminate()
            if hasattr(self, "core"):
                self.core.unlink()
            raise

    def _monitor_workers(self):
        while not self._shutdown:
            for i, p in enumerate(self.workers):
                if not p.is_alive():
                    if self._shutdown:
                        break

                    exitcode = p.exitcode
                    dead_pid = p.pid
                    print(f"Worker {dead_pid} exited (code {exitcode}). Restarting...")

                    # Check for lost task
                    lost_task_id = self.active_tasks.pop(dead_pid, None)
                    if lost_task_id:
                        print(
                            f"Monitor: Detected lost task {lost_task_id} on worker {dead_pid}"
                        )
                        payload = self.task_payloads.get(lost_task_id)
                        if payload:
                            print(f"Monitor: Re-submitting task {lost_task_id}")
                            try:
                                self.core.put_task(payload)
                            except Exception as e:
                                print(f"Monitor: Failed to re-submit task: {e}")
                        else:
                            print(
                                f"Monitor: CRITICAL - Payload for {lost_task_id} not found!"
                            )

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

                if task_id is None:
                    break

                ack_payload = (task_id, os.getpid(), "ACK")
                result_q.put_bytes(pickle.dumps(ack_payload))

                try:
                    result = func(*args, **kwargs)
                    res_payload = (task_id, result, None, "DONE")
                except Exception as e:
                    res_payload = (task_id, None, e, "DONE")

                result_q.put_bytes(pickle.dumps(res_payload))

            except Exception:
                break

    def _result_listener(self):
        while not self._shutdown:
            try:
                res_bytes = self.core.get_result()
                data = pickle.loads(res_bytes)

                # Check generic tuple structure
                if len(data) >= 1 and data[0] is None:
                    break

                if len(data) == 3 and data[2] == "ACK":
                    task_id, pid, _ = data
                    self.active_tasks[pid] = task_id
                    continue

                if len(data) == 4 and data[3] == "DONE":
                    task_id, result, exc, _ = data

                    # Cleanup tracking
                    self.task_payloads.pop(task_id, None)

                    # Remove task mapping for any worker that had this task
                    # (This is a bit inefficient (O(N)), but N=num_workers is small)
                    # Ideally we would track which PID sent the result, but DONE msg doesn't have it yet.
                    # We can assume if we received DONE, the task is no longer active on anyone.
                    pids_to_remove = [
                        p for p, t in self.active_tasks.items() if t == task_id
                    ]
                    for pid in pids_to_remove:
                        self.active_tasks.pop(pid, None)

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
        payload_bytes = pickle.dumps(payload)

        self.task_payloads[task_id] = payload_bytes

        self.core.put_task(payload_bytes)

        return future

    def map(self, func, iterable, timeout=None):
        futures = [self.submit(func, item) for item in iterable]
        return [f.result(timeout=timeout) for f in futures]

    def shutdown(self, wait=True, unlink=True):
        if self._shutdown:
            return
        self._shutdown = True

        if hasattr(self, "_finalizer"):
            self._finalizer.detach()

        for _ in range(self.num_workers):
            try:
                self.core.put_task(pickle.dumps((None, None, None, None)))
            except Exception as e:
                print(f"Failed to send shutdown signal: {e}")

        if wait:
            for p in self.workers:
                p.join()

        try:
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
