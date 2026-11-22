# Changelog

All notable changes to this project are documented here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and the project uses `MAJOR.MINOR.PATCH`.

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
