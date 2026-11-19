"""Bounded JSON Lines tailing backed by Rust."""
import json
import os
import warnings
from ._native import DEFAULT_MAX_BYTES
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
):
    """Return raw, complete JSONL rows from the last line or byte window."""
    meta_json, rows = _tail_window_raw(
        os.fspath(path), lines, byte_window, max_bytes, keep_partial_first
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
):
    """Return bounded tail metadata and raw, complete JSONL rows."""
    meta_json, rows = _tail_window_raw(
        os.fspath(path), lines, byte_window, max_bytes, keep_partial_first
    )
    return json.loads(meta_json), rows


def select_native(path):
    """Return metadata and decoded recent Codex message objects."""
    metadata, rows = _select_native_raw(os.fspath(path))
    return json.loads(metadata), [json.loads(row) for row in rows]


__all__ = ["DEFAULT_MAX_BYTES", "tail", "tail_window", "select_native"]
