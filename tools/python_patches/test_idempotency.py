"""Smoke test for the idempotency guard in sitecustomize.py.

Run: PYTHONPATH=/home/user/caspar/tools/python_patches python3 test_idempotency.py
"""
from __future__ import annotations

import importlib.util
import os
import sys

# Load OUR sitecustomize directly by file path — the interpreter's auto-
# imported `sitecustomize` may be a different file (e.g. python3-minimal's).
_PATCHES_DIR = os.path.dirname(os.path.abspath(__file__))
_SITE_PATH = os.path.join(_PATCHES_DIR, "sitecustomize.py")
_spec = importlib.util.spec_from_file_location("caspar_sitecustomize", _SITE_PATH)
sitecustomize = importlib.util.module_from_spec(_spec)
assert _spec.loader is not None
_spec.loader.exec_module(sitecustomize)

# Inject a fake caspar_client module with deterministic behaviour.
import types

_fake = types.ModuleType("caspar_client")


class CasparClient:
    """Fake client tuned per scenario via flags on the instance.

    The wrapper only patches a known set of method names, so this fake
    must use names from that set (connect, send_request, recv, etc.) to
    actually exercise the retry policy.
    """

    def __init__(self, behaviour: str):
        self.behaviour = behaviour
        self.connect_calls = 0
        self.send_calls = 0
        self.recv_calls = 0

    def connect(self):
        self.connect_calls += 1
        if self.behaviour == "connect_eventually" and self.connect_calls < 3:
            raise ConnectionRefusedError("Errno 111")

    def send_request(self, *_args, **_kwargs):
        self.send_calls += 1
        if self.behaviour == "send_broken_pipe":
            # Non-idempotent + post-write error → must NOT retry.
            raise BrokenPipeError("Errno 32")
        if self.behaviour == "send_refused_eventually":
            # Non-idempotent + pre-socket error → SAFE to retry.
            if self.send_calls < 2:
                raise ConnectionRefusedError("Errno 111")
            return "ok"
        return "default"

    def recv(self):
        # `recv` is in _IDEMPOTENT_METHODS — should retry through
        # transient errors.
        self.recv_calls += 1
        if self.recv_calls < 3:
            raise BrokenPipeError("Errno 32")
        return "data"


_fake.CasparClient = CasparClient
sys.modules["caspar_client"] = _fake

# Apply the patches directly (the meta-path hook normally runs on a
# fresh import; in this test we already have the module assembled).
sitecustomize._apply_patches(_fake)


def _expect_raises(exc_type, fn, *args, **kwargs):
    try:
        fn(*args, **kwargs)
    except exc_type:
        return
    except Exception as e:
        raise AssertionError(f"expected {exc_type.__name__}, got {type(e).__name__}: {e}")
    raise AssertionError(f"expected {exc_type.__name__}, no exception raised")


# 1. connect() retries through 2 ConnectionRefusedError then succeeds.
client = CasparClient("connect_eventually")
client.connect()
assert client.connect_calls == 3, f"expected 3 connect calls, got {client.connect_calls}"
print("✓ connect() retries on ConnectionRefusedError")

# 2. send_request() (non-idempotent) fails with BrokenPipeError — must NOT retry.
client = CasparClient("send_broken_pipe")
_expect_raises(BrokenPipeError, client.send_request, "/chains/registerNode", b"x")
assert client.send_calls == 1, (
    f"expected 1 send call (no retry for non-idempotent BrokenPipe), got {client.send_calls}"
)
print("✓ send_request() refuses retry on BrokenPipeError (non-idempotent)")

# 3. send_request() (non-idempotent) fails with ConnectionRefusedError — MUST retry.
client = CasparClient("send_refused_eventually")
result = client.send_request("/foo", b"y")
assert result == "ok"
assert client.send_calls == 2, (
    f"expected 2 send calls (retry on ConnectionRefused), got {client.send_calls}"
)
print("✓ send_request() retries on ConnectionRefusedError (no bytes flushed)")

# 4. recv() (idempotent — in _IDEMPOTENT_METHODS) retries through BrokenPipeError.
client = CasparClient("default")
result = client.recv()
assert result == "data"
assert client.recv_calls == 3, f"expected 3 recv calls, got {client.recv_calls}"
print("✓ recv() retries on BrokenPipeError (idempotent)")

print("all smoke tests passed")
