# jlogtail

This folder contains the Rust CLI and PyO3 extension for reading JSON Lines tails.

## Layout

- `src/lib.rs`: shared bounded suffix reader, Codex history selector, and PyO3 module.
- `src/main.rs` and `src/cli.rs`: Rust CLI, suffix reads, filters, projection and follow mode.
- `python/jlogtail/`: PyO3-backed Python package and command entry point.
- `docs/demo/`: synthetic JSONL fixture, VHS tapes and recordings.
- `Cargo.toml`, `pyproject.toml`: Rust and Python package metadata.

## Build

Run `cargo build --release` in this directory. Install the Rust command with `cargo install --path .`, or install the Python wheel and command with `pipx install jlogtail` / `uv tool install jlogtail`.

## Style and behavior

- Read only the requested suffix. Do not replace reverse scanning with a full-file read.
- Emit only complete records in follow mode. Keep an unterminated suffix for the next poll, including the one already present when follow starts, and compare file IDs to detect rotation.
- Classify an oversized record from its own first bytes, not from the scan segment that happens to hold them; a record can start in the last byte of a block.
- Enforce the `max_bytes` ceiling (`DEFAULT_MAX_BYTES`, 64 MiB) across `read_tail` / `read_tail_bounded`, follow-mode chunk reads, and incomplete-record buffering; skip oversized records with a clear marker instead of growing an unbounded buffer.
- Keep Python `tail()` on the shared Rust suffix reader. Preserve `native-history-tail.py` metadata keys and oldest-to-newest output order.
- Demo records must be synthetic. Benchmarks may read a temporary copy of a real transcript, never its live source.
- Do not publish to GitHub, PyPI or crates.io from this checkout.
- Keep generated `target/` and `.venv/` files out of `public.json`.
- The Orbit receipts note is a future integration pointer; do not edit Orbit host files here.
- This source folder is not a system-wide installation.
- Keep package exports listed in `public.json` in sync with the files that ship.

## Checks

Run `cargo test -j 1` and `cargo build --release` in this directory. Inspect the actual output with a small synthetic JSONL fixture under `docs/demo/`.

## Code Review Rules

A change is a real defect when it:

- reads the whole file instead of the requested suffix or byte window.
- drifts a byte window off a line boundary, exposing a partial last line as a record.
- emits an incomplete record in follow mode, drops the unterminated suffix it must keep, or misses a rotation because file IDs are not compared.
- classifies an oversized record from the segment holding it instead of from its own first bytes.
- moves Python `tail()` off the shared Rust suffix reader, or changes the `native-history-tail.py` metadata keys or oldest-to-newest order.
- changes a CLI flag, JSON field or exit code that the README documents.

Not defects: wording, formatting, comment style, requested extra tests, or a renamed helper.
