# Changelog

All notable changes to this project are documented here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and the project uses `MAJOR.MINOR.PATCH`.

## [0.3.2] - 2026-11-23

### Added

- `blob=True` option on `jlogtail.tail_window(...)` and `jlogtail.read_forward(...)`:
  returns `(meta, data)` where `data` is ONE `bytes` object holding exactly the
  selected complete rows, each with its trailing `\n` — `b"".join(row + b"\n" for
  row in raw_rows)` — instead of a list of per-row `bytes` objects. Nothing is
  decoded, so `errors` and `raw` are ignored on the blob path; row selection,
  metadata offsets, snapshot identity, oversized and ceiling answers are identical
  to the 0.3.1 paths.

### Performance

- The byte-window tail and the forward read now fetch the one boundary probe byte
  in the same seek+read as the window instead of a separate one-byte read, and the
  byte-window trim drains the front of the read buffer instead of copying the whole
  window a second time. The `blob=True` path goes one pass further: a small head
  read locates the first row boundary, then the trimmed window is read straight
  into the returned bytes object's buffer — no intermediate `Vec`, no second copy
  (an oversized first row or a short read falls back to the buffered path).
  Measured through the Orbit host's `transcript_window` callers on a 380 MB
  transcript (M5, 50-run mean, `end=size`, hot page cache): `_read_window_before`
  256 KiB 0.024 -> 0.018 ms and 2 MiB 0.113 -> 0.059 ms, both well below the
  pre-jlogtail pure-Python read (0.024 / 0.113 ms) that jlogtail 0.3.1 had
  regressed past (0.051 / 0.224 ms), and at the plain `os.pread` floor; the 2 MiB
  forward range read halves too, 0.227 -> 0.063 ms. Blob output is byte-identical
  to the joined 0.3.1 raw rows and the metadata matches decision for decision.

## [0.3.1] - 2026-11-23

### Added

- `raw=True` option on `jlogtail.tail(...)`, `jlogtail.tail_window(...)` and
  `jlogtail.read_forward(...)`: each row comes back as a `bytes` object holding the row
  exactly as stored in the file, without its trailing `\n`. No UTF-8 decode happens, so
  invalid bytes pass through unchanged and `errors` is ignored; row selection (partial
  first row, incomplete final row, `end=`, `max_bytes`, oversized skip, metadata offsets)
  is identical to the default `str` rows. Defaults are unchanged: `raw=False` keeps the
  0.3.0 behavior.

### Performance

- Hot read paths (`read_tail_bounded` byte_window and lines modes, `read_forward`,
  `decode_rows`, `row_bytes`) preallocate their read buffer and find newlines with the
  `memchr` crate instead of per-byte loops, and window trimming slices instead of
  draining, so the partial first row no longer memmoves the window. Measured on a 380 MB
  transcript (M5, 50-run mean, `end=size`): `tail_window_raw(raw=True)` 2 MiB window
  0.90 ms -> 0.15 ms, 256 KiB 0.13 ms -> 0.04 ms; `lines=200` 1.15 ms -> 0.21 ms.
  Selection rules and metadata are byte-identical.

## [0.3.0] - 2026-11-23

### Added

- `end=` snapshot bound on `jlogtail.tail(..., end=None)`, `jlogtail.tail_window(..., end=None)`
  and `jlogtail FILE --end N`: the read treats the file as if it ended at that byte offset,
  so a caller can page backwards row by row. The final row must end with `\n` at `end - 1`,
  otherwise it is dropped exactly like today's trailing partial row; `end` beyond the file size
  raises `ValueError` (Python) or exits 2 (CLI), and `end=0` returns no rows. `tail_window` meta
  gains `start_offset`, the byte offset of the first returned row (equal to `end_offset` when no
  rows), which lets a reverse scan continue with `end=meta["start_offset"]` and stop when the
  cursor no longer moves. Defaults are unchanged: `end=None` keeps the 0.2.2 behavior.

## [0.2.2] - 2026-11-22

### Added

- Forward read from a saved byte offset: `read_forward(path, start, max_bytes)` in Rust,
  `jlogtail.read_forward(path, offset=..., max_bytes=..., errors=...)` in Python, and
  `jlogtail FILE --from-offset N` in the CLI. One call reads at most `max_bytes` forward from
  the cursor and answers `(rows, new_offset, identity_ok)`: `new_offset` is the cursor to save
  (it stops before an unterminated trailing row, which a later call returns once complete), and
  `identity_ok` is false — with no rows, never guessed rows — when the dev/ino/size/mtime snapshot
  taken before and after the read disagrees or the file shrank below the offset. A partial first
  row is dropped when the offset is not at a line boundary, and a record that can never complete
  inside the window parks the cursor with `skipped_oversized = 1`. The Python call accepts the
  same `errors` option as `tail`; the CLI prints rows on stdout and the forward metadata JSON on
  stderr (`jlogtail: forward: {...}`), and `--from-offset` composes with `--follow`.

## [0.2.1] - 2026-11-22

### Added

- `errors` option (`"strict"` default, `"replace"`, `"ignore"`) on `jlogtail.tail(...)` and
  `jlogtail.tail_window(...)`. `"strict"` keeps the old behavior (`ValueError` on invalid UTF-8);
  `"replace"` substitutes U+FFFD for each invalid sequence and `"ignore"` drops invalid bytes, so one
  damaged byte no longer fails the whole window. Rows are split on `\n` before decoding, so damage in
  one row never touches its neighbors. An unknown value raises `ValueError`.

## [0.2.0] - 2026-11-19

### Added

- Caller-set memory ceiling across `read_tail_bounded`, `--max-bytes` (default 64 MiB), follow-mode
  appended reads, and incomplete trailing-record buffering.
- `jlogtail.tail(..., max_bytes=DEFAULT_MAX_BYTES, keep_partial_first=False)` and
  `jlogtail.tail_window(...)` returning `(meta, rows)` with `end_offset`, `max_bytes`,
  `skipped_oversized`, `ceiling_reached`, and `trailing_oversized`.
- Clear markers when a record exceeds the ceiling or caps the window: `stderr` diagnostics in the
  CLI, `RuntimeWarning` in Python `tail()`, and explicit fields on `TailWindow`.
- `--version` / `-V` CLI flag matching `SECURITY.md`.

### Changed

- `read_tail` and Python `tail()` enforce the safe 64 MiB default ceiling (`DEFAULT_MAX_BYTES`)
  instead of unbounded `read_to_end` and backward scans.
- Follow mode reads appended bytes in bounded chunks and discards oversized records until the next
  newline instead of growing `pending` without limit.

## [0.1.2] - 2026-11-18

### Fixed

- Follow mode keeps an unfinished record that is already in the file when the follow starts, so
  the first poll emits the whole row instead of a fragment. `--lines 0` had the same problem.
- An oversized record that begins near the end of a scan block is classified from its own prefix
  instead of from the partial block, so older messages stay eligible.

## [0.1.1] - 2026-11-18

### Fixed

- The Codex selector matches the reference `native-history-tail` behaviour on user barriers and a
  byte-order mark.

### Changed

- Wheels are built as abi3-py39: one wheel per platform for Python 3.9 and newer.

## [0.1.0] - 2026-11-17

### Added

- Rust CLI and PyO3 module that read the end of a JSON Lines file without loading it.
- `--lines`, `--bytes`, `--filter`, `--fields`, `--follow` and the Codex message selector.
