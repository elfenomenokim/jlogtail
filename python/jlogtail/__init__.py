"""Bounded JSON Lines tailing backed by Rust."""
import json
import os
import warnings
from ._native import DEFAULT_MAX_BYTES
from ._native import read_forward_raw as _read_forward_raw
from ._native import select_native as _select_native_raw
from ._native import tail_raw as _tail_raw
from ._native import tail_window_raw as _tail_window_raw


def tail(
    path,
    *,
    lines=10,
    byte_window=None,
    max_bytes=DEFAULT_MAX_BYTES,
    keep_partial_first=False,
    errors="strict",
):
    """Return raw, complete JSONL rows from the last line or byte window.

    ``errors`` is ``"strict"`` (default, invalid UTF-8 raises ValueError), ``"replace"`` or ``"ignore"``.
    """
    meta_json, rows = _tail_window_raw(
        os.fspath(path), lines, byte_window, max_bytes, keep_partial_first, errors
    )
    meta = json.loads(meta_json)
    if meta["skipped_oversized"]:
        warnings.warn(
            f"jlogtail: skipped {meta['skipped_oversized']} oversized record(s) exceeding max_bytes={meta['max_bytes']}",
            RuntimeWarning,
            stacklevel=2,
        )
    elif meta["ceiling_reached"]:
        warnings.warn(
            f"jlogtail: tail window capped at max_bytes={meta['max_bytes']}",
            RuntimeWarning,
            stacklevel=2,
        )
    return rows


def tail_window(
    path,
    *,
    lines=10,
    byte_window=None,
    max_bytes=DEFAULT_MAX_BYTES,
    keep_partial_first=False,
    errors="strict",
):
    """Return bounded tail metadata and raw, complete JSONL rows. ``errors`` as in ``tail``."""
    meta_json, rows = _tail_window_raw(
        os.fspath(path), lines, byte_window, max_bytes, keep_partial_first, errors
    )
    return json.loads(meta_json), rows


def read_forward(path, *, offset=0, max_bytes=DEFAULT_MAX_BYTES, errors="strict"):
    """Read complete rows forward from a saved byte offset under snapshot identity.

    ``offset`` is a cursor from a previous call's ``meta["new_offset"]``; 0 reads
    from the beginning. Returns ``(meta, rows)``; ``meta["new_offset"]`` is the
    cursor to save. When ``meta["identity_ok"]`` is False the file was replaced,
    truncated below the offset, or changed during the read: ``rows`` is empty and
    ``new_offset`` is the unchanged start offset (0 when the file no longer
    reaches the saved offset), so the caller rescans instead of guessing. An
    unterminated trailing row is held: the cursor stops before it, and a later
    call returns it once the writer finishes it. ``errors`` is as in ``tail``.
    """
    meta_json, rows = _read_forward_raw(os.fspath(path), offset, max_bytes, errors)
    return json.loads(meta_json), rows


def select_native(path):
    """Return metadata and decoded recent Codex message objects."""
    metadata, rows = _select_native_raw(os.fspath(path))
    return json.loads(metadata), [json.loads(row) for row in rows]


__all__ = ["DEFAULT_MAX_BYTES", "tail", "tail_window", "read_forward", "select_native"]
