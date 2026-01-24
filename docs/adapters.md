# Data Adapters

ZooSync handles raw bytes (`ZooQueue`) or Shared Memory segments (`recv_view`). To use this efficiently with high-level data libraries, you need to "adapt" these bytes.

## Numpy Adapter (Zero-Copy)

Numpy can wrap existing memory without copying it.

```python
import numpy as np
from zoosync import ZooQueue

q = ZooQueue("camera_feed", size_mb=100)

def receive_image(shape=(1080, 1920, 3)):
    # 1. Get view into shared memory
    view, cursor = q.recv_view()
    
    # 2. Adapt to Numpy (Zero-Copy)
    # Note: 'arr' shares memory with the queue!
    arr = np.frombuffer(view, dtype=np.uint8).reshape(shape)
    
    # 3. Process
    print(arr.mean())
    
    # 4. Release
    q.commit_read(cursor)
```

## PyArrow Adapter (Zero-Copy)

PyArrow handles zero-copy buffers natively.

```python
import pyarrow as pa
import pyarrow.ipc
from zoosync import ZooQueue

q = ZooQueue("arrow_stream", size_mb=200)

def send_batch(batch):
    sink = pa.BufferOutputStream()
    with pa.ipc.new_stream(sink, batch.schema) as writer:
        writer.write_batch(batch)
    q.put_bytes(sink.getvalue().to_pybytes())

def receive_batch():
    data = q.get_bytes()
    reader = pa.ipc.open_stream(data)
    return reader.read_next_batch()
```

## Protobuf Adapter

For structured data, Protobuf is fast, though it typically requires parsing (copying) from the buffer.

```python
from my_proto_pb2 import MyMessage
from zoosync import ZooQueue

q = ZooQueue("proto_q", size_mb=10)

def send_proto(msg):
    q.put_bytes(msg.SerializeToString())

def recv_proto():
    data = q.get_bytes()
    msg = MyMessage()
    msg.ParseFromString(data)
    return msg
```
