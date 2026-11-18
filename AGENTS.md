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
- Emit only complete records in follow mode. Keep an unterminated suffix for the next poll and compare file IDs to detect rotation.
- Keep Python `tail()` on the shared Rust suffix reader. Preserve `native-history-tail.py` metadata keys and oldest-to-newest output order.
- Demo records must be synthetic. Benchmarks may read a temporary copy of a real transcript, never its live source.
- Do not publish to GitHub, PyPI or crates.io from this checkout.
- Keep generated `target/` and `.venv/` files out of `public.json`.
- The Orbit receipts note is a future integration pointer; do not edit Orbit host files here.
- This source folder is not a system-wide installation.
- Keep package exports listed in `public.json` in sync with the files that ship.

## Checks

Build the Rust CLI with `cargo build --release`. Inspect the actual output with a small synthetic JSONL fixture. The public package has not been released.
