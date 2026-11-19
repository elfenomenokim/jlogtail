# Contributing to jlogtail

Fixes and reports are welcome. You do not need to ask before opening a pull request for a bug fix.

## Before you start

- **A bug:** open an [issue](https://github.com/elfenomenokim/jlogtail/issues/new/choose) with the
  command, its output, the record shape (make one up if yours is private) and your platform.
- **A change to what counts as a message:** open an issue first. The Codex selector follows
  observed transcript shapes, and a wrong classification silently drops history.
- **A security problem:** do not open a public issue. See [SECURITY.md](SECURITY.md).

## Set up

```sh
git clone https://github.com/elfenomenokim/jlogtail
cd jlogtail
cargo test -j 1
cargo build --release
./target/release/jlogtail docs/demo/agents.jsonl --lines 2 --fields role,text
```

Run `cargo test -j 1` and verify the CLI output against a synthetic file under `docs/demo/`. The Python module is
built by maturin; `python3 -m pip wheel --no-deps --wheel-dir target/wheels .` after the Rust
build passes.

## Rules a patch must keep

- Read only the requested suffix within `max_bytes`. A full-file or unbounded read is not an acceptable implementation of a tail.
- Emit complete records only. A row that starts inside a byte window is skipped, and a follow poll
  keeps an unterminated row up to `max_bytes` until the newline arrives (oversized records are skipped with a clear marker).
- Classify an oversized record from its own first bytes, not from the scan block that happens to
  hold them.
- Keep the Python `tail()` on the shared Rust reader, so both editions agree.

## What a good patch has

- **A fixture that shows the bug**, and the same command passing after the fix.
- **A line under Unreleased** in [CHANGELOG.md](CHANGELOG.md) when a user will notice.
- **The platforms you did not test** written in the pull request. The benchmark and demo ran on
  macOS arm64; Linux and Windows follow behaviour has not been exercised.

## Review

Every pull request runs an automatic DeepSeek review. Ask for another pass with `@codex review` or
`@claude review` in a comment. If a finding looks wrong, say why in the pull request.

## Licence

Contributions are licensed under the MIT licence in [LICENSE](LICENSE).
