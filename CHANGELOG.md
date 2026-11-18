# Changelog

All notable changes to this project are documented here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and the project uses `MAJOR.MINOR.PATCH`.

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
