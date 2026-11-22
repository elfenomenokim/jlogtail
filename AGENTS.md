# jlogtail

This folder contains the Rust CLI and PyO3 extension for reading JSON Lines tails.

## Set up for a user

Goal: `jlogtail` prints the last records of a large JSON Lines file (agent transcripts) without reading the whole file. Nothing here needs the person except which file.

1. Check what is present.
   - `uv --version` prints a version. If not: `curl -LsSf https://astral.sh/uv/install.sh | sh`, then `export PATH="$HOME/.local/bin:$PATH"` (or open a new shell).
   - No Rust or compiler is needed for this route.
2. Install: `uv tool install jlogtail`. It ends with `Installed 1 executable: jlogtail`. `uv tool update-shell` puts `~/.local/bin` on PATH for later shells. (Rust CLI from source instead: clone the repository and run `cargo install --path .` there; there is no crates.io package.)
3. Prove it on a synthetic file:
   ```sh
   d=$(mktemp -d); printf '{"role":"user","text":"hi"}\n{"role":"assistant","text":"one"}\n{"role":"assistant","text":"two"}\n' > $d/t.jsonl
   jlogtail $d/t.jsonl --lines 2 --fields role,text; rm -rf $d
   ```
   It prints `{"role":"assistant","text":"one"}` and `{"role":"assistant","text":"two"}` and exits 0.
4. Real use: ask "어느 로그나 대화 기록(.jsonl) 파일을 볼까요?", then `jlogtail FILE --lines 20`. Claude Code transcripts live under `~/.claude/projects/`, Codex under `~/.codex/sessions/`; read them only for the user's request, and never paste their contents into a public place.
5. If it fails, in this order:
   - `jlogtail: command not found`: `~/.local/bin` is not on PATH (step 2).
   - `FILE: No such file or directory (os error 2)`, exit 2: the path is wrong.
   - Empty output: the file has no complete record in that window (the last line may still be unfinished); raise `--lines` or `--bytes`.
   - `skipped oversized record` on stderr: a record is bigger than `--max-bytes` (default 64 MiB); raise it only if the user needs that record.

## Layout

- `src/lib.rs`: shared bounded suffix reader, Codex history selector, and PyO3 module.
- `src/main.rs` and `src/cli.rs`: Rust CLI, suffix reads, filters, projection and follow mode.
- `python/jlogtail/`: PyO3-backed Python package and command entry point.
- `docs/demo/`: synthetic JSONL fixture, VHS tapes and recordings.
- `Cargo.toml`, `pyproject.toml`: Rust and Python package metadata.

## Build

Run `cargo build --release` in this directory. Install the Rust command with `cargo install --path .`, or install the Python wheel and command with `uv tool install jlogtail` / `pipx install jlogtail`.

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
