import multiprocessing
from zooparallel import ZooQueue


def test_queue_basic():
    # 1MB buffer
    name = "test_basic_q"
    try:
        ZooQueue.unlink(name)
    except Exception as e:
        print(f"Failed to unlink queue: {e}")
    q = ZooQueue(name, 1)

    data = b"hello world"
    q.put_bytes(data)

    res = q.get_bytes()
    assert bytes(res) == data


def producer(q_name, count):
    q = ZooQueue(q_name, 1)
    for i in range(count):
        msg = f"msg_{i}".encode()
        q.put_bytes(msg)


def test_queue_mp():
    q_name = "test_queue_mp"
    count = 1000

    try:
        ZooQueue.unlink(q_name)
    except Exception as e:
        print(f"Failed to unlink queue: {e}")

    # Initialize queue in parent
    q = ZooQueue(q_name, 1)

    p = multiprocessing.Process(target=producer, args=(q_name, count))
    p.start()

    received = 0
    while received < count:
        msg = q.get_bytes()
        expected = f"msg_{received}".encode()
        assert bytes(msg) == expected
        received += 1

    p.join()
