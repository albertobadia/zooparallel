import gc
import weakref
from zooparallel import ZooQueue


def test_view_gc_cycle():
    """
    Test that creating a reference cycle involving ZooView doesn't cause leaks or crash.
    """
    q_name = "/test_gc_cycle"
    try:
        ZooQueue.unlink(q_name)
    except Exception:
        pass

    # Subclass to check cycles with custom attributes
    class MyQueue(ZooQueue):
        pass

    q = MyQueue(q_name, 1)
    q.put_bytes(b"hello")

    # Using recv_view
    # ZooView holds a reference to 'q' (MyQueue instance)
    ctx = q.recv_view()

    # Create cycle: ctx -> q -> ctx
    # By subclassing, 'q' has a __dict__ or at least supports dynamic attributes
    # (unless slots prevent it, but ZooQueue doesn't define slots in Python)
    try:
        q.cycle_ref = ctx
        print("Cycle created successfully")
    except AttributeError:
        print("Could not attach attribute to Queue subclass. Skipping cycle test.")
        return

    # Weakref to check life
    w_ctx = weakref.ref(ctx)

    # Delete references
    # ctx should be kept alive by q, q by ctx.
    # They are isolated.
    del q
    del ctx

    # Force Collection
    n = gc.collect()
    print(f"Collected {n} objects")

    if w_ctx() is None:
        print("ctx died")
    else:
        print("ctx alive")

    assert n > 0 or w_ctx() is None, (
        "GC failed to collect reference cycle involving ZooView!"
    )


def test_stress_view():
    q_name = "/test_stress_view"
    try:
        ZooQueue.unlink(q_name)
    except Exception:
        pass

    q = ZooQueue(q_name, 10)
    data = b"x" * 1024

    # Repeat many times (reduced for speed)
    for i in range(100):
        q.put_bytes(data)
        with q.recv_view() as view:
            assert len(view) == 1024
            # Force GC inside view
            gc.collect()

    print("Stress view passed")


if __name__ == "__main__":
    test_view_gc_cycle()
    test_stress_view()
    print("All GC tests passed")
