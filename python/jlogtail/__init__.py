"""Bounded JSON Lines tailing backed by Rust."""
import json
import os
from ._native import select_native as _select_native_raw
from ._native import tail_raw as _tail_raw


def tail(path, *, lines=10, byte_window=None):
    """Return raw, complete JSONL rows from the last line or byte window."""
    return _tail_raw(os.fspath(path), lines, byte_window)


def select_native(path):
    """Return metadata and decoded recent Codex message objects."""
    metadata, rows = _select_native_raw(os.fspath(path))
    return json.loads(metadata), [json.loads(row) for row in rows]


__all__ = ["tail", "select_native"]
