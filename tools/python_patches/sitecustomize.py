"""
sitecustomize.py — Auto-loaded by every Python interpreter that has
`tools/python_patches` on its `PYTHONPATH`.

This file installs an import hook that monkey-patches the
`caspar_client.CasparClient` class with fault-tolerant wrappers around its
network methods. The benchmark's deploy.py / workflow_tests.py do not own
their own retry policy, so a single transient stall (container teardown
mid-deploy, a babble round briefly missing quorum, an idle socket reaped
by a stale NAT entry) immediately fails the entire run with the opaque
"Connection refused" / "socket closed while reading" cascade the
benchmark report keeps showing.

The hook is intentionally non-invasive:
  * We do not touch the caspar_client source on disk — the upstream
    repository is checked out fresh on every CI run.
  * We only wrap methods that already exist; an upstream rename does not
    break the hook, it just no-ops.
  * Wrapped behaviour is observable from logs (each retry prints to
    stderr) so a run that depends on retries does not silently look
    healthy.

Activate by prepending this directory to PYTHONPATH:
    PYTHONPATH=/path/to/tools/python_patches:$PYTHONPATH python3 deploy.py

run-nodes.sh and bench-all.sh both do this when they invoke the bench
scripts.
"""

from __future__ import annotations

import functools
import os
import socket as _socket
import sys
import time
import importlib.abc
import importlib.util

# How aggressively to retry. The defaults are tuned for the benchmark's
# typical failure window: container restart ≈ 5–15 s, a stalled babble
# round resolves in < 30 s, deploy_to_node's own timeout is 120 s, so a
# 6-attempt back-off totalling ~63 s fits comfortably under it.
_MAX_ATTEMPTS = int(os.environ.get("CASPAR_CLIENT_RETRY_ATTEMPTS", "6"))
_BASE_BACKOFF_SECS = float(os.environ.get("CASPAR_CLIENT_RETRY_BACKOFF", "1.0"))
_MAX_BACKOFF_SECS = float(os.environ.get("CASPAR_CLIENT_RETRY_MAX_BACKOFF", "8.0"))

# Errors we treat as transient — anything else (auth failures, malformed
# frames, protocol errors) propagates immediately.
_TRANSIENT_EXCEPTIONS = (
    ConnectionRefusedError,
    ConnectionResetError,
    ConnectionAbortedError,
    BrokenPipeError,
    TimeoutError,
    _socket.timeout,
    OSError,  # generic "Errno 111", "Errno 104", etc.
)


def _log(msg: str) -> None:
    """Stderr only — stdout belongs to deploy.py's report writer."""
    try:
        sys.stderr.write("[caspar_client_patches] " + msg + "\n")
        sys.stderr.flush()
    except Exception:
        pass


def _backoff(attempt: int) -> float:
    """Exponential back-off with a cap, plus a small jitter so concurrent
    clients don't synchronise their retries."""
    base = min(_MAX_BACKOFF_SECS, _BASE_BACKOFF_SECS * (2 ** attempt))
    # Deterministic-ish jitter without importing random (avoid surprising
    # the deploy report which seeds it for repeatability).
    jitter = (time.monotonic_ns() % 250) / 1000.0
    return base + jitter


def _retry(method_name: str, fn):
    """Wraps `fn` with bounded exponential-back-off retries on transient
    network errors. The wrapped function preserves the underlying
    signature so introspection-based callers still work."""

    @functools.wraps(fn)
    def wrapper(*args, **kwargs):
        last_err = None
        for attempt in range(_MAX_ATTEMPTS):
            try:
                return fn(*args, **kwargs)
            except _TRANSIENT_EXCEPTIONS as e:
                last_err = e
                if attempt + 1 >= _MAX_ATTEMPTS:
                    break
                delay = _backoff(attempt)
                _log(
                    f"{method_name} failed ({type(e).__name__}: {e}); "
                    f"retry {attempt + 1}/{_MAX_ATTEMPTS} in {delay:.2f}s"
                )
                time.sleep(delay)
        # Out of retries — bubble up so the caller sees a real error.
        raise last_err  # type: ignore[misc]

    return wrapper


def _retry_with_reconnect(method_name: str, fn, reconnect_method_name: str = "connect"):
    """Like `_retry`, but between attempts it tries to re-establish the
    underlying connection via `self.<reconnect_method_name>()`. Used for
    request/response calls that need a live socket — a stale FD or a
    mid-flight reset is the most common cause of the deploy script's
    'socket closed while reading' errors."""

    @functools.wraps(fn)
    def wrapper(self, *args, **kwargs):
        last_err = None
        for attempt in range(_MAX_ATTEMPTS):
            try:
                return fn(self, *args, **kwargs)
            except _TRANSIENT_EXCEPTIONS as e:
                last_err = e
                if attempt + 1 >= _MAX_ATTEMPTS:
                    break
                delay = _backoff(attempt)
                _log(
                    f"{method_name} failed ({type(e).__name__}: {e}); "
                    f"reconnect + retry {attempt + 1}/{_MAX_ATTEMPTS} in {delay:.2f}s"
                )
                time.sleep(delay)
                reconnect = getattr(self, reconnect_method_name, None)
                if callable(reconnect):
                    try:
                        # Close stale socket if present so the reconnect
                        # actually creates a fresh FD.
                        sock = getattr(self, "sock", None)
                        if sock is not None:
                            try:
                                sock.close()
                            except Exception:
                                pass
                        reconnect()
                    except _TRANSIENT_EXCEPTIONS as re:
                        _log(f"  reconnect failed: {type(re).__name__}: {re}")
                        # Keep going — next attempt may succeed.
        raise last_err  # type: ignore[misc]

    return wrapper


def _apply_patches(caspar_client_module) -> None:
    """Wrap the network-bearing methods on the CasparClient class."""
    cls = getattr(caspar_client_module, "CasparClient", None)
    if cls is None:
        _log("CasparClient class not found — leaving module untouched")
        return

    # `connect` — open the TCP socket. Retry on refused/reset.
    if hasattr(cls, "connect") and not getattr(cls.connect, "_caspar_patched", False):
        wrapped = _retry("CasparClient.connect", cls.connect)
        wrapped._caspar_patched = True  # type: ignore[attr-defined]
        cls.connect = wrapped

    # Request/response methods — name varies between upstream versions;
    # wrap any that exist. The deploy.log traceback shows
    # `_reconnect_and_login` calls `c.connect()`, so retrying connect is
    # already enough to recover the most common failure path, but
    # send_request / call / recv add another layer for mid-flight stalls.
    for name in (
        "send_request",
        "send",
        "call",
        "request",
        "rpc",
        "recv_response",
        "recv",
    ):
        attr = getattr(cls, name, None)
        if not callable(attr) or getattr(attr, "_caspar_patched", False):
            continue
        # `send` / `recv` cannot meaningfully reconnect mid-frame; only
        # higher-level call/request methods can. Heuristic: if the name
        # is a low-level primitive, just retry; otherwise wrap with
        # reconnect-aware retry.
        if name in ("send", "recv"):
            wrapped = _retry(f"CasparClient.{name}", attr)
        else:
            wrapped = _retry_with_reconnect(f"CasparClient.{name}", attr)
        wrapped._caspar_patched = True  # type: ignore[attr-defined]
        setattr(cls, name, wrapped)

    _log("CasparClient methods wrapped with retry policy "
         f"(attempts={_MAX_ATTEMPTS}, base_backoff={_BASE_BACKOFF_SECS}s)")


class _PostImportPatcher(importlib.abc.MetaPathFinder):
    """Sits at the front of sys.meta_path. When `caspar_client` is being
    loaded, we let the normal import machinery do its work, then run the
    patch over the freshly imported module. The hook removes itself once
    it has fired so a re-import doesn't re-wrap (which would compound
    retry counts)."""

    _fired = False

    def find_spec(self, fullname, path, target=None):
        if self._fired or fullname != "caspar_client":
            return None
        # Step out of meta_path while we resolve the real spec, so this
        # hook doesn't recurse on itself.
        sys.meta_path.remove(self)
        try:
            spec = importlib.util.find_spec(fullname)
        finally:
            sys.meta_path.insert(0, self)
        if spec is None or spec.loader is None:
            return None

        original_exec = spec.loader.exec_module

        def exec_module_with_patch(module):
            original_exec(module)
            try:
                _apply_patches(module)
            except Exception as e:
                _log(f"patch failed: {type(e).__name__}: {e}")
            finally:
                self._fired = True

        spec.loader.exec_module = exec_module_with_patch  # type: ignore[assignment]
        return spec


# Install the hook at the very front so we beat any other meta-path
# finder that might find a stale cached `caspar_client`.
sys.meta_path.insert(0, _PostImportPatcher())
