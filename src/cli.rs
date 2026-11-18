use crate::{read_tail, select_native};
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
    follow: bool,
    filter: Option<(String, Value)>,
    fields: Vec<String>,
    poll_ms: u64,
    keep_partial: bool,
}

fn usage() -> &'static str {
    "Usage: jlogtail FILE [--lines N | --bytes N] [--follow] [--filter PATH=VALUE] [--fields PATH,...] [--keep-partial-first] [--poll-ms N]\n\nReads complete JSONL records from the end of a file. Values use JSON syntax when valid, otherwise strings."
}
fn parse(args: Vec<String>) -> Result<Options, String> {
    let mut args = args.into_iter();
    let first = args.next().ok_or_else(|| usage().to_string())?;
    if first == "--help" || first == "-h" {
        return Err(usage().to_string());
    }
    let mut o = Options {
        path: PathBuf::from(first),
        lines: Some(10),
        bytes: None,
        follow: false,
        filter: None,
        fields: Vec::new(),
        poll_ms: 250,
        keep_partial: false,
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
            }
            "--bytes" | "-c" => {
                let n = it
                    .next()
                    .ok_or("--bytes requires a number")?
                    .parse()
                    .map_err(|_| "invalid byte count")?;
                o.bytes = Some(n);
                o.lines = None;
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
            _ => return Err(format!("unknown argument: {arg}\n{}", usage())),
        }
    }
    Ok(o)
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
fn emit(data: &[u8], o: &Options, out: &mut impl Write) -> io::Result<()> {
    for line in data.split(|b| *b == b'\n').filter(|line| !line.is_empty()) {
        if matches(line, &o.filter) {
            out.write_all(&render(line, &o.fields))?;
        }
    }
    out.flush()
}
fn execute(o: Options) -> Result<(), String> {
    let window = read_tail(&o.path, o.lines, o.bytes, o.keep_partial)
        .map_err(|e| format!("{}: {e}", o.path.display()))?;
    let initial = window.bytes;
    let mut offset = window.end_offset;
    let mut out = io::stdout().lock();
    emit(&initial, &o, &mut out).map_err(|e| e.to_string())?;
    if !o.follow {
        return Ok(());
    }
    let mut current_id = identity(&o.path).map_err(|e| e.to_string())?;
    let mut pending = Vec::new();
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
        if next_id != current_id || metadata.len() < offset {
            current_id = next_id;
            offset = 0;
            pending.clear();
        }
        if metadata.len() <= offset {
            continue;
        }
        next.seek(SeekFrom::Start(offset))
            .map_err(|e| e.to_string())?;
        let mut appended = Vec::new();
        next.read_to_end(&mut appended).map_err(|e| e.to_string())?;
        offset += appended.len() as u64;
        pending.extend_from_slice(&appended);
        let complete = pending
            .iter()
            .rposition(|b| *b == b'\n')
            .map_or(0, |i| i + 1);
        if complete == 0 {
            continue;
        }
        emit(&pending[..complete], &o, &mut out).map_err(|e| e.to_string())?;
        pending.drain(..complete);
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
    let options = parse(args)?;
    execute(options)
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
