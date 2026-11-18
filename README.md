<div align="center">

<img src="https://raw.githubusercontent.com/elfenomenokim/jlogtail/main/docs/logo.svg" width="96" height="96" alt="jlogtail: the last lines of a file, measured from the end">

# jlogtail
[한국어](README.ko.md)

A Rust command-line tool and Python module that read the end of large JSON Lines transcripts without loading the whole file.

Rust · Python 3.9+

[![pypi](https://img.shields.io/pypi/v/jlogtail?label=pypi)](https://pypi.org/project/jlogtail/)
[![license](https://img.shields.io/github/license/elfenomenokim/jlogtail)](https://github.com/elfenomenokim/jlogtail/blob/main/LICENSE)

5.0 ms for the tail of a 2.18 GB transcript, against 66.5 ms for `tail | jq` and 1395.6 ms for a full Python read, from the saved 10-run file.

[Install](#install) · [Quick start](#quick-start) · [How it reads](#how-it-reads)

</div>

<p align="center">
<picture>
  <source media="(prefers-color-scheme: dark)" srcset="https://raw.githubusercontent.com/elfenomenokim/jlogtail/main/docs/demo/dark.gif">
  <img src="https://raw.githubusercontent.com/elfenomenokim/jlogtail/main/docs/demo/light.gif" alt="The CLI selects recent transcript fields from a small JSONL fixture" width="1200">
</picture>
</p>

[Social preview](https://raw.githubusercontent.com/elfenomenokim/jlogtail/main/docs/social.png)

The demo uses synthetic transcript rows. It shows field selection and filtering, not benchmark data.

## Install

```sh
cargo install --path .
pipx install jlogtail
# or: uv tool install jlogtail
```

The wheel installs the `jlogtail` command and the importable `jlogtail` PyO3 module. Use Cargo to install the standalone Rust CLI from this directory. Python 3.9+ is required; source builds also require Rust.

## Quick start

```sh
jlogtail ~/.claude/projects/PROJECT/session.jsonl --lines 20
jlogtail ~/.codex/sessions/SESSION.jsonl --bytes 65536
jlogtail transcript.jsonl --filter payload.role=assistant --fields timestamp,payload.role
jlogtail transcript.jsonl --follow
```

Claude Code, Codex, and Pi transcripts use the same JSONL commands. The CLI outputs original rows by default. `--fields` emits a compact object with selected dotted paths. `--filter PATH=VALUE` compares JSON values; unquoted non-JSON values are strings. A byte window skips a partial first record but keeps it when the window starts at a line boundary. Follow reads each appended byte once, keeps an incomplete row in memory, and emits it when the newline arrives. A row that is already unfinished when follow starts is kept from its first byte, so the first poll emits the whole row rather than a fragment.

```console
$ jlogtail docs/demo/agents.jsonl --lines 2 --fields role,text
{"role":"assistant","text":"Checkpoint 42 finished."}
{"role":"assistant","text":"The next run is queued."}
```

## How it reads

<picture>
  <source media="(prefers-color-scheme: dark)" srcset="https://raw.githubusercontent.com/elfenomenokim/jlogtail/main/docs/diagrams/tail-flow.en.dark.svg">
  <img src="https://raw.githubusercontent.com/elfenomenokim/jlogtail/main/docs/diagrams/tail-flow.en.light.svg" alt="The reader seeks to the requested boundary, filters complete rows, then follows appended data and resets after rotation or truncation" width="1200">
</picture>

## Why

`tail -n 20 file.jsonl | jq .` is useful for full-file pipelines and richer JSON queries, but it reads from the start to find the last rows. This tool seeks backward to the requested records, then reads only that suffix. It is for large transcripts and repeated polling.

## Measurements

On M5 Pro, macOS arm64, hyperfine ran 3 warmups and 10 measured runs per command against one 2.18 GB temporary copy. The read-only 272.9 MB source transcript was repeated eight times. Mean wall times were 5.0 ms for jlogtail, 66.5 ms for `tail | jq`, and 1395.6 ms for Python's full read. See the [summary](docs/bench/m5-bench.txt), [raw output](docs/bench/m5-hyperfine.txt), and [raw measurements](docs/bench/m5-hyperfine.json). Results describe this copy and host only.

<picture>
  <source media="(prefers-color-scheme: dark)" srcset="https://raw.githubusercontent.com/elfenomenokim/jlogtail/main/docs/diagrams/bench-results.en.dark.svg">
  <img src="https://raw.githubusercontent.com/elfenomenokim/jlogtail/main/docs/diagrams/bench-results.en.light.svg" alt="Warm-cache mean wall time across 10 runs: Rust reverse tail 5.0 milliseconds, tail plus jq 66.5 milliseconds, Python full read 1395.6 milliseconds" width="1200">
</picture>

## Reference

| Option | Meaning |
|---|---|
| `--lines N`, `-n N` | Read the last N complete records; default 10 |
| `--bytes N`, `-c N` | Read N trailing bytes and drop a partial first record unless the window starts at a line boundary |
| `--follow`, `-f` | Emit complete appended records; restart from byte 0 after replacement or truncation |
| `--filter PATH=VALUE` | Keep rows whose dotted key path equals the JSON value |
| `--fields PATH,...` | Emit only the selected dotted paths as a compact object |
| `--poll-ms N` | Follow poll interval; default 250 ms |
| `--keep-partial-first` | Keep the first partial row in a byte window |

The Python module provides `jlogtail.tail(path, lines=20)` for raw complete rows; use `byte_window=N` to choose a suffix by bytes. `jlogtail.select_native(path)` returns a metadata dictionary and oldest-to-newest Codex message objects. `lib/native-history-tail.py` preserves feno-handoff's existing one-path command and metadata output.

## Limits

The benchmark and demo ran on macOS arm64. Linux and Windows builds have not been exercised. Follow compares platform file IDs and resets when it observes a shorter file. The benchmark ran on macOS arm64; Linux and Windows follow behavior has not been exercised. A filter requires valid JSON. A follow poll waits for the newline that completes each row.

## Contributing

Issues and pull requests are welcome. See [CONTRIBUTING.md](CONTRIBUTING.md) for the build, the fixture check and the review process.

## License

MIT. See [LICENSE](LICENSE).
