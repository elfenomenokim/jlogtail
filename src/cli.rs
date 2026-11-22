use crate::{read_tail_bounded, select_native, CHUNK, DEFAULT_MAX_BYTES};
use serde_json::Value;
use std::fs::File;
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::thread;
use std::time::Duration;

#[derive(Clone)]
struct Options {
    path: PathBuf,
    lines: Option<usize>,
    bytes: Option<u64>,
    from_offset: Option<u64>,
    follow: bool,
    filter: Option<(String, Value)>,
    fields: Vec<String>,
    poll_ms: u64,
    keep_partial: bool,
    max_bytes: usize,
}

fn usage() -> &'static str {
    "Usage: jlogtail FILE [--lines N | --bytes N | --from-offset N] [--follow] [--filter PATH=VALUE] [--fields PATH,...] [--keep-partial-first] [--poll-ms N] [--max-bytes N]\n\nReads complete JSONL records from the end of a file, or forward from a saved byte offset with --from-offset. Values use JSON syntax when valid, otherwise strings."
}

enum Parsed {
    Run(Options),
    ExitOk,
}

fn parse(args: Vec<String>) -> Result<Parsed, String> {
    let mut args = args.into_iter();
    let first = args.next().ok_or_else(|| usage().to_string())?;
    if first == "--help" || first == "-h" {
        return Err(usage().to_string());
    }
    if first == "--version" || first == "-V" {
        println!("jlogtail {}", env!("CARGO_PKG_VERSION"));
        return Ok(Parsed::ExitOk);
    }
    let mut o = Options {
        path: PathBuf::from(first),
        lines: Some(10),
        bytes: None,
        from_offset: None,
        follow: false,
        filter: None,
        fields: Vec::new(),
        poll_ms: 250,
        keep_partial: false,
        max_bytes: DEFAULT_MAX_BYTES,
    };
    let mut it = args;
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--lines" | "-n" => {
                let n = it
                    .next()
                    .ok_or("--lines requires a number")?
                    .parse()
                    .map_err(|_| "invalid line count")?;
                o.lines = Some(n);
                o.bytes = None;
                o.from_offset = None;
            }
            "--bytes" | "-c" => {
                let n = it
                    .next()
                    .ok_or("--bytes requires a number")?
                    .parse()
                    .map_err(|_| "invalid byte count")?;
                o.bytes = Some(n);
                o.lines = None;
                o.from_offset = None;
            }
            "--from-offset" => {
                let n = it
                    .next()
                    .ok_or("--from-offset requires a byte offset")?
                    .parse()
                    .map_err(|_| "invalid byte offset")?;
                o.from_offset = Some(n);
                o.lines = None;
                o.bytes = None;
            }
            "--follow" | "-f" => o.follow = true,
            "--keep-partial-first" => o.keep_partial = true,
            "--poll-ms" => {
                o.poll_ms = it
                    .next()
                    .ok_or("--poll-ms requires a number")?
                    .parse()
                    .map_err(|_| "invalid poll interval")?;
            }
            "--max-bytes" => {
                let n: usize = it
                    .next()
                    .ok_or("--max-bytes requires a number")?
                    .parse()
                    .map_err(|_| "invalid max-bytes count")?;
                if n == 0 {
                    return Err("--max-bytes must be greater than 0".to_string());
                }
                o.max_bytes = n;
            }
            "--filter" => {
                let v = it.next().ok_or("--filter requires PATH=VALUE")?;
                let (path, value) = v.split_once('=').ok_or("--filter requires PATH=VALUE")?;
                let value = serde_json::from_str(value)
                    .unwrap_or_else(|_| Value::String(value.to_string()));
                o.filter = Some((path.into(), value));
            }
            "--fields" => {
                o.fields = it
                    .next()
                    .ok_or("--fields requires comma-separated paths")?
                    .split(',')
                    .map(str::to_string)
                    .collect();
            }
            "--version" | "-V" => {
                println!("jlogtail {}", env!("CARGO_PKG_VERSION"));
                return Ok(Parsed::ExitOk);
            }
            _ => return Err(format!("unknown argument: {arg}\n{}", usage())),
        }
    }
    Ok(Parsed::Run(o))
}
fn value_at<'a>(root: &'a Value, path: &str) -> Option<&'a Value> {
    let mut value = root;
    for part in path.split('.') {
        value = if let Ok(index) = part.parse::<usize>() {
            value.get(index)?
        } else {
            value.get(part)?
        };
    }
    Some(value)
}
fn matches(raw: &[u8], filter: &Option<(String, Value)>) -> bool {
    let Some((path, expected)) = filter else {
        return true;
    };
    let Ok(row) = serde_json::from_slice::<Value>(raw) else {
        return false;
    };
    value_at(&row, path) == Some(expected)
}
fn render(raw: &[u8], fields: &[String]) -> Vec<u8> {
    if fields.is_empty() {
        let mut out = raw.to_vec();
        out.push(b'\n');
        return out;
    }
    let Ok(row) = serde_json::from_slice::<Value>(raw) else {
        let mut out = raw.to_vec();
        out.push(b'\n');
        return out;
    };
    let mut projected = serde_json::Map::new();
    for key in fields {
        if let Some(value) = value_at(&row, key) {
            projected.insert(key.clone(), value.clone());
        }
    }
    let mut out = serde_json::to_vec(&Value::Object(projected)).unwrap_or_else(|_| raw.to_vec());
    out.push(b'\n');
    out
}
fn identity(path: &Path) -> io::Result<file_id::FileId> {
    file_id::get_file_id(path)
}
fn emit_line(line: &[u8], o: &Options, out: &mut impl Write) -> io::Result<()> {
    if line.is_empty() {
        return Ok(());
    }
    if matches(line, &o.filter) {
        out.write_all(&render(line, &o.fields))?;
    }
    Ok(())
}

fn emit(data: &[u8], o: &Options, out: &mut impl Write) -> io::Result<()> {
    for line in data.split(|b| *b == b'\n').filter(|line| !line.is_empty()) {
        emit_line(line, o, out)?;
    }
    out.flush()
}

struct FollowState {
    offset: u64,
    pending: Vec<u8>,
    skip_oversized: bool,
}

fn process_chunk(
    mut rest: &[u8],
    o: &Options,
    state: &mut FollowState,
    out: &mut impl Write,
    err: &mut impl Write,
) -> io::Result<()> {
    while !rest.is_empty() {
        if state.skip_oversized {
            if let Some(nl) = rest.iter().position(|b| *b == b'\n') {
                state.skip_oversized = false;
                rest = &rest[nl + 1..];
            } else {
                rest = &[];
            }
            continue;
        }
        if let Some(nl) = rest.iter().position(|b| *b == b'\n') {
            let part = &rest[..nl];
            if state.pending.len().saturating_add(part.len()) > o.max_bytes {
                state.pending.clear();
                writeln!(
                    err,
                    "jlogtail: skipped oversized record exceeding max-bytes ({})",
                    o.max_bytes
                )?;
            } else if state.pending.is_empty() {
                emit_line(part, o, out)?;
            } else {
                state.pending.extend_from_slice(part);
                emit_line(&state.pending, o, out)?;
                state.pending.clear();
            }
            rest = &rest[nl + 1..];
        } else {
            if state.pending.len().saturating_add(rest.len()) > o.max_bytes {
                state.pending.clear();
                state.skip_oversized = true;
                writeln!(
                    err,
                    "jlogtail: skipped oversized record exceeding max-bytes ({})",
                    o.max_bytes
                )?;
            } else {
                state.pending.extend_from_slice(rest);
            }
            rest = &[];
        }
    }
    Ok(())
}

fn read_follow_appended(
    file: &mut File,
    file_len: u64,
    o: &Options,
    state: &mut FollowState,
    buf: &mut [u8],
    out: &mut impl Write,
    err: &mut impl Write,
) -> io::Result<()> {
    file.seek(SeekFrom::Start(state.offset))?;
    while state.offset < file_len {
        let to_read = (file_len - state.offset).min(buf.len() as u64) as usize;
        let n = file.read(&mut buf[..to_read])?;
        if n == 0 {
            break;
        }
        state.offset += n as u64;
        process_chunk(&buf[..n], o, state, out, err)?;
    }
    out.flush()?;
    err.flush()
}

fn execute(o: Options) -> Result<(), String> {
    let mut out = io::stdout().lock();
    let mut err = io::stderr().lock();
    let (follow_offset, follow_skip_oversized) = if let Some(start) = o.from_offset {
        let result = crate::read_forward(&o.path, start, o.max_bytes)
            .map_err(|e| format!("{}: {e}", o.path.display()))?;
        if result.identity_ok {
            emit(&result.bytes, &o, &mut out).map_err(|e| e.to_string())?;
        }
        if result.skipped_oversized > 0 {
            writeln!(
                err,
                "jlogtail: skipped {} oversized record(s) exceeding max-bytes ({})",
                result.skipped_oversized, o.max_bytes
            )
            .map_err(|e| e.to_string())?;
        } else if result.ceiling_reached {
            writeln!(
                err,
                "jlogtail: forward window capped at max-bytes ({})",
                o.max_bytes
            )
            .map_err(|e| e.to_string())?;
        }
        let meta = serde_json::json!({
            "offset": start,
            "new_offset": result.new_offset,
            "identity_ok": result.identity_ok,
            "rotated": result.rotated,
            "truncated": result.truncated,
            "changed": result.changed,
            "size": result.size,
            "mtime_ns": result.mtime_ns,
            "max_bytes": o.max_bytes,
            "ceiling_reached": result.ceiling_reached,
            "skipped_oversized": result.skipped_oversized,
        });
        writeln!(err, "jlogtail: forward: {meta}").map_err(|e| e.to_string())?;
        err.flush().map_err(|e| e.to_string())?;
        if !o.follow {
            return Ok(());
        }
        // A failed identity starts over from byte 0, like a rotation reset.
        (result.new_offset, false)
    } else {
        let window = read_tail_bounded(&o.path, o.lines, o.bytes, o.keep_partial, o.max_bytes)
            .map_err(|e| format!("{}: {e}", o.path.display()))?;
        emit(&window.bytes, &o, &mut out).map_err(|e| e.to_string())?;
        if window.skipped_oversized > 0 {
            writeln!(
                err,
                "jlogtail: skipped {} oversized record(s) exceeding max-bytes ({})",
                window.skipped_oversized, o.max_bytes
            )
            .map_err(|e| e.to_string())?;
            err.flush().map_err(|e| e.to_string())?;
        } else if window.ceiling_reached {
            writeln!(
                err,
                "jlogtail: tail window capped at max-bytes ({})",
                o.max_bytes
            )
            .map_err(|e| e.to_string())?;
            err.flush().map_err(|e| e.to_string())?;
        }
        if !o.follow {
            return Ok(());
        }
        (window.end_offset, window.trailing_oversized)
    };
    let mut current_id = identity(&o.path).map_err(|e| e.to_string())?;
    let mut state = FollowState {
        offset: follow_offset,
        pending: Vec::new(),
        skip_oversized: follow_skip_oversized,
    };
    let mut buf = vec![0u8; o.max_bytes.min(CHUNK).max(1)];
    loop {
        thread::sleep(Duration::from_millis(o.poll_ms.max(10)));
        let id_before = match identity(&o.path) {
            Ok(id) => id,
            Err(_) => continue,
        };
        let mut next = match File::open(&o.path) {
            Ok(file) => file,
            Err(_) => continue,
        };
        let next_id = match identity(&o.path) {
            Ok(id) => id,
            Err(_) => continue,
        };
        if id_before != next_id {
            continue;
        }
        let metadata = next.metadata().map_err(|e| e.to_string())?;
        if next_id != current_id || metadata.len() < state.offset {
            current_id = next_id;
            state.offset = 0;
            state.pending.clear();
            state.skip_oversized = false;
        }
        if metadata.len() <= state.offset {
            continue;
        }
        read_follow_appended(
            &mut next,
            metadata.len(),
            &o,
            &mut state,
            &mut buf,
            &mut out,
            &mut err,
        )
        .map_err(|e| e.to_string())?;
    }
}
pub fn run(args: Vec<String>) -> Result<(), String> {
    if args.first().map(String::as_str) == Some("native-history") {
        let path = args
            .get(1)
            .ok_or_else(|| "one native transcript path required".to_string())?;
        if args.len() != 2 {
            return Err("one native transcript path required".to_string());
        }
        return run_native_history(path.clone());
    }
    match parse(args)? {
        Parsed::Run(options) => execute(options),
        Parsed::ExitOk => Ok(()),
    }
}

fn run_native_history(path: String) -> Result<(), String> {
    let (meta, records) = select_native(PathBuf::from(path).as_path())
        .map_err(|_| "native history source unavailable".to_string())?;
    let mut output = io::stdout().lock();
    output
        .write_all(
            serde_json::to_string(&meta)
                .map_err(|e| e.to_string())?
                .as_bytes(),
        )
        .map_err(|e| e.to_string())?;
    output.write_all(b"\n").map_err(|e| e.to_string())?;
    for record in records {
        output.write_all(&record).map_err(|e| e.to_string())?;
        output.write_all(b"\n").map_err(|e| e.to_string())?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_options(path: PathBuf, max_bytes: usize) -> Options {
        Options {
            path,
            lines: Some(10),
            bytes: None,
            from_offset: None,
            follow: true,
            filter: None,
            fields: Vec::new(),
            poll_ms: 10,
            keep_partial: false,
            max_bytes,
        }
    }

    #[test]
    fn from_offset_mode_is_exclusive_with_lines_and_bytes() {
        let parsed = parse(vec!["t.jsonl".into(), "--from-offset".into(), "12".into()]).unwrap();
        let Parsed::Run(o) = parsed else { panic!("expected run") };
        assert_eq!(o.from_offset, Some(12));
        assert_eq!(o.lines, None);
        assert_eq!(o.bytes, None);
        let parsed = parse(vec![
            "t.jsonl".into(),
            "--from-offset".into(),
            "12".into(),
            "--lines".into(),
            "3".into(),
        ])
        .unwrap();
        let Parsed::Run(o) = parsed else { panic!("expected run") };
        assert_eq!(o.lines, Some(3));
        assert_eq!(o.from_offset, None);
        let parsed = parse(vec![
            "t.jsonl".into(),
            "--from-offset".into(),
            "12".into(),
            "--bytes".into(),
            "3".into(),
        ])
        .unwrap();
        let Parsed::Run(o) = parsed else { panic!("expected run") };
        assert_eq!(o.bytes, Some(3));
        assert_eq!(o.from_offset, None);
    }

    #[test]
    fn follow_bounds_appended_reads_and_skips_oversized_pending_record() {
        let dir = std::env::temp_dir().join(format!("jlogtail-cli-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("follow.jsonl");
        std::fs::write(&path, b"{\"n\":1}\n").unwrap();

        let o = test_options(path.clone(), 32);
        let window = read_tail_bounded(&path, o.lines, o.bytes, o.keep_partial, o.max_bytes).unwrap();
        assert_eq!(window.bytes, b"{\"n\":1}\n");

        let mut state = FollowState {
            offset: window.end_offset,
            pending: Vec::new(),
            skip_oversized: window.trailing_oversized,
        };
        let mut buf = vec![0u8; o.max_bytes.min(CHUNK).max(1)];
        let mut out = Vec::new();
        let mut err = Vec::new();

        // Append an incomplete oversized record across multiple writes, then a valid record.
        let mut f = std::fs::OpenOptions::new().append(true).open(&path).unwrap();
        f.write_all(b"{\"big\":\"0123456789").unwrap();
        let len1 = f.metadata().unwrap().len();
        read_follow_appended(&mut File::open(&path).unwrap(), len1, &o, &mut state, &mut buf, &mut out, &mut err).unwrap();
        assert!(out.is_empty());
        assert!(err.is_empty());
        assert!(state.pending.len() <= o.max_bytes);

        f.write_all(b"abcdefghijklmnopqrstuvwxyz\"}\n{\"n\":2}\n").unwrap();
        let len2 = f.metadata().unwrap().len();
        read_follow_appended(&mut File::open(&path).unwrap(), len2, &o, &mut state, &mut buf, &mut out, &mut err).unwrap();

        assert_eq!(String::from_utf8_lossy(&out), "{\"n\":2}\n");
        let err_str = String::from_utf8_lossy(&err);
        assert!(err_str.contains("skipped oversized record"), "{err_str}");
        assert!(err_str.contains("32"), "{err_str}");
        assert!(state.pending.is_empty());
        assert!(!state.skip_oversized);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn follow_resumes_skip_when_initial_trailing_record_was_oversized() {
        let dir = std::env::temp_dir().join(format!("jlogtail-cli-init-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("follow.jsonl");
        std::fs::write(&path, format!("{{\"huge\":\"{}", "x".repeat(100))).unwrap();

        let o = test_options(path.clone(), 32);
        let window = read_tail_bounded(&path, o.lines, o.bytes, o.keep_partial, o.max_bytes).unwrap();
        assert!(window.trailing_oversized);
        assert_eq!(window.skipped_oversized, 1);

        let mut state = FollowState {
            offset: window.end_offset,
            pending: Vec::new(),
            skip_oversized: window.trailing_oversized,
        };
        let mut buf = vec![0u8; o.max_bytes.min(CHUNK).max(1)];
        let mut out = Vec::new();
        let mut err = Vec::new();

        let mut f = std::fs::OpenOptions::new().append(true).open(&path).unwrap();
        f.write_all(b"more-tail\"}\n{\"ok\":true}\n").unwrap();
        let len = f.metadata().unwrap().len();
        read_follow_appended(&mut File::open(&path).unwrap(), len, &o, &mut state, &mut buf, &mut out, &mut err).unwrap();

        assert_eq!(String::from_utf8_lossy(&out), "{\"ok\":true}\n");
        assert!(err.is_empty());
        assert!(!state.skip_oversized);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
