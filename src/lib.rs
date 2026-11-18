#[cfg(feature = "extension-module")]
use pyo3::prelude::*;
mod cli;
use serde_json::{json, Value};
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

pub const SCAN_CAP: u64 = 64 * 1024 * 1024;
pub const RECORD_CAP: usize = 256 * 1024;
const CHUNK: usize = 64 * 1024;

pub fn run_cli(args: Vec<String>) -> Result<(), String> {
    cli::run(args)
}

pub struct TailWindow {
    pub bytes: Vec<u8>,
    pub end_offset: u64,
}

/// Read a bounded suffix, dropping a partial first byte-window row and any incomplete final row.
pub fn read_tail(
    path: &Path,
    lines: Option<usize>,
    byte_window: Option<u64>,
    keep_partial_first: bool,
) -> std::io::Result<TailWindow> {
    let mut file = File::open(path)?;
    let size = file.metadata()?.len();
    let mut start = if let Some(bytes) = byte_window {
        size.saturating_sub(bytes)
    } else {
        0
    };
    if byte_window.is_none() {
        let n = lines.unwrap_or(10);
        let mut logical_end = size;
        let mut has_final_newline = false;
        // First byte of the record still being written when the file ends mid-line.
        let mut trailing_start = size;
        if size > 0 {
            file.seek(SeekFrom::Start(size - 1))?;
            let mut last = [0u8; 1];
            file.read_exact(&mut last)?;
            has_final_newline = last[0] == b'\n';
            if !has_final_newline {
                let mut scan = size;
                let mut found = None;
                let mut block = [0u8; CHUNK];
                while scan > 0 && found.is_none() {
                    let amount = scan.min(block.len() as u64) as usize;
                    scan -= amount as u64;
                    file.seek(SeekFrom::Start(scan))?;
                    file.read_exact(&mut block[..amount])?;
                    if let Some(i) = block[..amount].iter().rposition(|b| *b == b'\n') {
                        found = Some(scan + i as u64);
                    }
                }
                match found {
                    Some(i) => {
                        logical_end = i;
                        trailing_start = i + 1;
                    }
                    None => {
                        logical_end = 0;
                        trailing_start = 0;
                    }
                }
            }
        }
        let mut pos = logical_end;
        let mut count = 0usize;
        let mut block = [0u8; CHUNK];
        if has_final_newline && pos > 0 {
            pos -= 1;
        }
        while pos > 0 && count < n {
            let amount = pos.min(block.len() as u64) as usize;
            pos -= amount as u64;
            file.seek(SeekFrom::Start(pos))?;
            file.read_exact(&mut block[..amount])?;
            for i in (0..amount).rev() {
                if block[i] == b'\n' {
                    count += 1;
                    if count == n {
                        start = pos + i as u64 + 1;
                        break;
                    }
                }
            }
            if count == n {
                break;
            }
        }
        if n == 0 || logical_end == 0 {
            // Resume at the first byte of an unterminated final record, not at
            // EOF: follow must emit that record whole once its newline arrives.
            start = trailing_start;
        }
    }
    let mut starts_on_boundary = start == 0;
    if start > 0 && byte_window.is_some() && !keep_partial_first {
        file.seek(SeekFrom::Start(start - 1))?;
        let mut previous = [0u8; 1];
        file.read_exact(&mut previous)?;
        starts_on_boundary = previous[0] == b'\n';
    }
    file.seek(SeekFrom::Start(start))?;
    let mut data = Vec::new();
    file.read_to_end(&mut data)?;
    if start > 0 && byte_window.is_some() && !keep_partial_first && !starts_on_boundary {
        if let Some(nl) = data.iter().position(|b| *b == b'\n') {
            data.drain(..=nl);
            start += (nl + 1) as u64;
        } else {
            start += data.len() as u64;
            data.clear();
        }
    }
    let complete = data.iter().rposition(|b| *b == b'\n').map_or(0, |i| i + 1);
    data.truncate(complete);
    Ok(TailWindow {
        bytes: data,
        end_offset: start + complete as u64,
    })
}

fn text_field<'a>(row: &'a Value, key: &str) -> Option<&'a str> {
    row.get(key).and_then(Value::as_str)
}

fn classify(row: &Value) -> &'static str {
    if text_field(row, "type") != Some("response_item") {
        return "skip";
    }
    let Some(payload) = row.get("payload") else {
        return "skip";
    };
    if text_field(payload, "type") != Some("message") {
        return "skip";
    }
    let role = text_field(payload, "role");
    if role != Some("user") && role != Some("assistant") {
        return "skip";
    }
    if role == Some("assistant")
        && !matches!(
            text_field(payload, "phase"),
            None | Some("") | Some("final_answer")
        )
    {
        return "skip";
    }
    let Some(content) = payload.get("content").and_then(Value::as_array) else {
        // The reference tail treats a user message it cannot represent as a
        // barrier: the scan stops instead of stepping over a gap.
        return if role == Some("user") { "barrier" } else { "skip" };
    };
    if role == Some("user")
        && (content.is_empty()
            || content.iter().any(|part| {
                part.get("text")
                    .and_then(Value::as_str)
                    .map_or(true, str::is_empty)
            }))
    {
        return "barrier";
    }
    if content.iter().any(|part| {
        part.get("text")
            .and_then(Value::as_str)
            .map_or(false, |s| !s.is_empty())
    }) {
        "candidate"
    } else {
        "skip"
    }
}

fn trim_ws(mut input: &[u8]) -> &[u8] {
    while input.first().is_some_and(u8::is_ascii_whitespace) {
        input = &input[1..];
    }
    input
}

fn take_string(input: &[u8]) -> Option<(&[u8], &[u8])> {
    let input = trim_ws(input);
    if !input.starts_with(b"\"") {
        return None;
    }
    let body = &input[1..];
    for (index, byte) in body.iter().enumerate() {
        match byte {
            b'\\' => return None,
            b'"' => return Some((&body[..index], &body[index + 1..])),
            _ => {}
        }
    }
    None
}

fn take_string_property<'a>(input: &'a [u8], key: &[u8]) -> Option<(&'a [u8], &'a [u8])> {
    let input = trim_ws(input);
    if !input.starts_with(b"\"") || input.get(1..1 + key.len())? != key {
        return None;
    }
    let rest = input.get(key.len() + 2..)?;
    if !trim_ws(rest).starts_with(b":") {
        return None;
    }
    let colon = rest.iter().position(|byte| *byte == b':')?;
    take_string(&rest[colon + 1..])
}

fn consume_comma(input: &[u8]) -> Option<&[u8]> {
    let input = trim_ws(input);
    input.strip_prefix(b",")
}

fn prefix_proves_nonmessage(prefix: &[u8]) -> bool {
    let head = &prefix[..prefix.len().min(4096)];
    let Some(mut rest) = trim_ws(head).strip_prefix(b"{") else {
        return false;
    };
    rest = trim_ws(rest);
    if rest.starts_with(b"\"timestamp\"") {
        let Some((timestamp, after)) = take_string_property(rest, b"timestamp") else {
            return false;
        };
        if timestamp.len() > 128 {
            return false;
        }
        let Some(after) = consume_comma(after) else {
            return false;
        };
        rest = after;
    }
    rest = trim_ws(rest);
    if rest.starts_with(b"\"ordinal\"") {
        let Some(key_end) = rest.iter().position(|byte| *byte == b':') else {
            return false;
        };
        if !trim_ws(&rest[..key_end]).ends_with(b"\"ordinal\"") {
            return false;
        }
        let digits = trim_ws(&rest[key_end + 1..]);
        let count = digits
            .iter()
            .take_while(|byte| byte.is_ascii_digit())
            .count();
        if count == 0 || count > 20 {
            return false;
        }
        let Some(after) = consume_comma(&digits[count..]) else {
            return false;
        };
        rest = after;
    }
    let Some((outer, after_type)) = take_string_property(rest, b"type") else {
        return false;
    };
    const OUTER: &[&[u8]] = &[
        b"session_meta",
        b"turn_context",
        b"event_msg",
        b"token_count",
        b"token_usage_record",
        b"reasoning",
        b"compacted",
    ];
    if OUTER.contains(&outer) {
        return true;
    }
    if outer != b"response_item" {
        return false;
    }
    let Some(payload) = consume_comma(after_type) else {
        return false;
    };
    let payload = trim_ws(payload);
    if !payload.starts_with(b"\"payload\"") {
        return false;
    }
    let Some(colon) = payload.iter().position(|byte| *byte == b':') else {
        return false;
    };
    let inner = trim_ws(&payload[colon + 1..]);
    let Some(inner) = inner.strip_prefix(b"{") else {
        return false;
    };
    let Some((kind, after_inner_type)) = take_string_property(inner, b"type") else {
        return false;
    };
    const INNER: &[&[u8]] = &[
        b"function_call",
        b"function_call_output",
        b"custom_tool_call",
        b"custom_tool_call_output",
        b"reasoning",
    ];
    if INNER.contains(&kind) {
        return true;
    }
    if kind != b"message" {
        return false;
    }
    let Some(role_properties) = consume_comma(after_inner_type) else {
        return false;
    };
    let Some((role, after_role)) = take_string_property(role_properties, b"role") else {
        return false;
    };
    if role != b"assistant" {
        return false;
    }
    let Some(phase_properties) = consume_comma(after_role) else {
        return false;
    };
    let Some((phase, _)) = take_string_property(phase_properties, b"phase") else {
        return false;
    };
    phase == b"commentary" || phase == b"analysis"
}

fn parse_line(line: &[u8]) -> Result<Value, ()> {
    // `json.loads` in the reference reads bytes and skips a UTF-8 BOM; serde_json
    // rejects one. Strip it for classification only: the emitted record keeps it.
    let line = line.strip_prefix(b"\xef\xbb\xbf").unwrap_or(line);
    serde_json::from_slice(line).map_err(|_| ())
}

/// Read one record's leading bytes so an oversized record can be classified even
/// when it starts in the last bytes of a scan block.
fn read_prefix(file: &mut File, offset: u64) -> std::io::Result<Vec<u8>> {
    const PROBE: usize = 4096;
    let mut probe = vec![0u8; PROBE];
    file.seek(SeekFrom::Start(offset))?;
    let mut filled = 0;
    while filled < probe.len() {
        match file.read(&mut probe[filled..]) {
            Ok(0) => break,
            Ok(n) => filled += n,
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(e),
        }
    }
    probe.truncate(filled);
    Ok(probe)
}

/// Read the bounded recent Codex message window used by feno-handoff.
pub fn select_native(path: &Path) -> std::io::Result<(Value, Vec<Vec<u8>>)> {
    let mut file = File::open(path)?;
    let size = file.metadata()?.len();
    let mut position = size;
    let mut scanned = 0u64;
    let mut pending: Vec<u8> = Vec::new();
    let mut overlong = false;
    let mut newest_first: Vec<Vec<u8>> = Vec::new();
    let mut selected_bytes = 0usize;
    let mut skipped_overlong_nonmessage = 0usize;
    let mut source_gap_reason: Option<&str> = None;
    let mut candidate_cap_reached = false;

    while position > 0
        && scanned < SCAN_CAP
        && !candidate_cap_reached
        && source_gap_reason.is_none()
    {
        let amount = (position.min(CHUNK as u64).min(SCAN_CAP - scanned)) as usize;
        position -= amount as u64;
        file.seek(SeekFrom::Start(position))?;
        let mut block = vec![0; amount];
        file.read_exact(&mut block)?;
        scanned += amount as u64;
        let pieces: Vec<&[u8]> = block.split(|b| *b == b'\n').collect();
        // Each piece's byte offset inside the block; the last piece has no
        // trailing newline, so this only overruns for unused entries.
        let mut piece_offsets = Vec::with_capacity(pieces.len());
        let mut cursor = 0usize;
        for piece in &pieces {
            piece_offsets.push(cursor);
            cursor += piece.len() + 1;
        }
        for (index, segment) in pieces.iter().enumerate().skip(1).rev() {
            if overlong || segment.len() + pending.len() > RECORD_CAP {
                let probe = read_prefix(&mut file, position + piece_offsets[index] as u64)?;
                if prefix_proves_nonmessage(&probe) {
                    skipped_overlong_nonmessage += 1;
                } else {
                    source_gap_reason = Some("oversized_unclassified_record");
                }
                overlong = false;
                pending.clear();
                if source_gap_reason.is_some() {
                    break;
                }
                continue;
            }
            let mut line = segment.to_vec();
            line.extend_from_slice(&pending);
            pending.clear();
            overlong = false;
            if line.is_empty() {
                continue;
            }
            let row: Value = match parse_line(&line) {
                Ok(v) => v,
                Err(_) => {
                    source_gap_reason = Some("invalid_native_record");
                    break;
                }
            };
            let kind = classify(&row);
            if kind == "barrier" {
                source_gap_reason = Some("unrepresentable_user_message");
                break;
            }
            if kind != "candidate" {
                continue;
            }
            if selected_bytes + line.len() + 1 > RECORD_CAP {
                candidate_cap_reached = true;
                break;
            }
            selected_bytes += line.len() + 1;
            newest_first.push(line);
        }
        if candidate_cap_reached || source_gap_reason.is_some() {
            break;
        }
        let first = pieces[0];
        if overlong || first.len() + pending.len() > RECORD_CAP {
            overlong = true;
            pending.clear();
        } else {
            let mut next = first.to_vec();
            next.extend_from_slice(&pending);
            pending = next;
        }
    }
    if position == 0
        && !overlong
        && !pending.is_empty()
        && !candidate_cap_reached
        && source_gap_reason.is_none()
    {
        match parse_line(&pending) {
            Err(_) => source_gap_reason = Some("invalid_native_record"),
            Ok(row) => match classify(&row) {
                "barrier" => source_gap_reason = Some("unrepresentable_user_message"),
                "candidate" if selected_bytes + pending.len() + 1 > RECORD_CAP => {
                    candidate_cap_reached = true
                }
                "candidate" => {
                    selected_bytes += pending.len() + 1;
                    newest_first.push(pending.clone());
                }
                _ => (),
            },
        }
    }
    if position == 0 && overlong && source_gap_reason.is_none() {
        if prefix_proves_nonmessage(&read_prefix(&mut file, 0)?) {
            skipped_overlong_nonmessage += 1;
        } else {
            source_gap_reason = Some("oversized_unclassified_record");
        }
    }
    newest_first.reverse();
    let meta = json!({
        "max_scanned_bytes": SCAN_CAP,
        "scanned_bytes": scanned,
        "source_bytes_at_start": size,
        "selected_record_cap": RECORD_CAP,
        "selected_record_bytes": selected_bytes,
        "scan_cap_reached": position > 0 && scanned >= SCAN_CAP,
        "selected_cap_reached": candidate_cap_reached,
        "source_gap_stop": source_gap_reason.is_some(),
        "source_gap_reason": source_gap_reason,
        "unrepresentable_user_stop": source_gap_reason == Some("unrepresentable_user_message"),
        "skipped_overlong_nonmessage": skipped_overlong_nonmessage,
    });
    Ok((meta, newest_first))
}

#[cfg(feature = "extension-module")]
#[pyfunction(name = "select_native")]
fn select_native_py(path: &str) -> PyResult<(String, Vec<String>)> {
    let (meta, records) = select_native(Path::new(path))
        .map_err(|e| pyo3::exceptions::PyOSError::new_err(e.to_string()))?;
    let meta_json = serde_json::to_string(&meta)
        .map_err(|e| pyo3::exceptions::PyValueError::new_err(e.to_string()))?;
    let text_records = records
        .into_iter()
        .map(String::from_utf8)
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| pyo3::exceptions::PyValueError::new_err(e.to_string()))?;
    Ok((meta_json, text_records))
}

#[cfg(feature = "extension-module")]
#[pyfunction(signature = (path, lines=None, byte_window=None))]
fn tail_raw(path: &str, lines: Option<usize>, byte_window: Option<u64>) -> PyResult<Vec<String>> {
    let window = read_tail(Path::new(path), lines, byte_window, false)
        .map_err(|e| pyo3::exceptions::PyOSError::new_err(e.to_string()))?;
    window
        .bytes
        .split(|b| *b == b'\n')
        .filter(|line| !line.is_empty())
        .map(|line| {
            String::from_utf8(line.to_vec())
                .map_err(|e| pyo3::exceptions::PyValueError::new_err(e.to_string()))
        })
        .collect()
}

#[cfg(feature = "extension-module")]
#[pyfunction]
fn cli_entry(py: Python<'_>) -> PyResult<()> {
    let args = py
        .import("sys")?
        .getattr("argv")?
        .extract::<Vec<String>>()?
        .into_iter()
        .skip(1)
        .collect();
    run_cli(args).map_err(pyo3::exceptions::PyRuntimeError::new_err)
}

#[cfg(feature = "extension-module")]
#[pymodule]
fn _native(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_function(wrap_pyfunction!(select_native_py, m)?)?;
    m.add_function(wrap_pyfunction!(tail_raw, m)?)?;
    m.add_function(wrap_pyfunction!(cli_entry, m)?)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    // The reference tail treats a user message without a representable content
    // array as a barrier; skipping it would let the scan cross a gap.
    #[test]
    fn user_message_without_content_is_a_barrier() {
        for payload in [
            json!({"type": "message", "role": "user"}),
            json!({"type": "message", "role": "user", "content": "plain"}),
            json!({"type": "message", "role": "user", "content": []}),
        ] {
            let row = json!({"type": "response_item", "payload": payload});
            assert_eq!(classify(&row), "barrier", "{row}");
        }
        let assistant = json!({"type": "response_item",
            "payload": {"type": "message", "role": "assistant"}});
        assert_eq!(classify(&assistant), "skip");
    }

    // `json.loads` reads bytes and skips a UTF-8 BOM; serde_json rejects it.
    #[test]
    fn bom_prefixed_record_parses() {
        assert!(parse_line(b"\xef\xbb\xbf{\"a\":1}").is_ok());
        assert!(parse_line(b"{\"a\":1}").is_ok());
        assert!(parse_line(b"{").is_err());
    }

    fn scratch_file(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "jlogtail-test-{}-{name}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir.join("t.jsonl")
    }

    fn read_from(path: &Path, offset: u64) -> Vec<u8> {
        use std::io::{Read, Seek, SeekFrom};
        let mut file = File::open(path).unwrap();
        file.seek(SeekFrom::Start(offset)).unwrap();
        let mut out = Vec::new();
        file.read_to_end(&mut out).unwrap();
        out
    }

    // Follow must keep the record prefix that is already in the file; resuming
    // at EOF would emit only the part appended later.
    #[test]
    fn follow_window_keeps_an_unterminated_record() {
        let path = scratch_file("follow");
        std::fs::write(&path, b"{\"a\":").unwrap();
        let window = read_tail(&path, Some(10), None, false).unwrap();
        assert!(window.bytes.is_empty());
        assert_eq!(window.end_offset, 0);
        use std::io::Write;
        let mut file = std::fs::OpenOptions::new().append(true).open(&path).unwrap();
        file.write_all(b"1}\n").unwrap();
        assert_eq!(read_from(&path, window.end_offset), b"{\"a\":1}\n");
    }

    #[test]
    fn zero_line_window_resumes_at_the_unterminated_record() {
        let path = scratch_file("zero");
        std::fs::write(&path, b"{\"a\":1}\n{\"b\":").unwrap();
        let window = read_tail(&path, Some(0), None, false).unwrap();
        assert!(window.bytes.is_empty());
        assert_eq!(window.end_offset, 8);
        use std::io::Write;
        let mut file = std::fs::OpenOptions::new().append(true).open(&path).unwrap();
        file.write_all(b"2}\n").unwrap();
        assert_eq!(read_from(&path, window.end_offset), b"{\"b\":2}\n");
    }

    // A 65,534-byte user row puts the following oversized function output in the
    // last byte of its 64 KiB scan block, leaving only `{` of its header visible.
    fn aligned_oversized_fixture() -> Vec<u8> {
        fn row(text: &str) -> String {
            json!({"type": "response_item",
                "payload": {"type": "message", "role": "user",
                    "content": [{"text": text}]}})
            .to_string()
        }
        let base = row("OLD").len();
        let text = format!("OLD{}", "p".repeat(65534 - base));
        let mut out = row(&text).into_bytes();
        assert_eq!(out.len(), 65534);
        out.push(b'\n');
        let head = r#"{"type":"response_item","payload":{"type":"function_call_output","output":""#;
        let tail = r#""}}"#;
        out.extend_from_slice(head.as_bytes());
        out.extend(std::iter::repeat(b'x').take(327680 - head.len() - tail.len()));
        out.extend_from_slice(tail.as_bytes());
        out.push(b'\n');
        assert_eq!(out.len(), 393216);
        out
    }

    #[test]
    fn oversized_record_at_a_block_edge_keeps_older_messages() {
        let path = scratch_file("edge");
        std::fs::write(&path, aligned_oversized_fixture()).unwrap();
        let (meta, records) = select_native(&path).unwrap();
        assert_eq!(meta["source_gap_stop"], false);
        assert_eq!(meta["skipped_overlong_nonmessage"], 1);
        assert_eq!(records.len(), 1, "{meta}");
    }

    #[test]
    fn oversized_record_at_the_file_start_is_classified_from_byte_zero() {
        let path = scratch_file("start");
        let fixture = aligned_oversized_fixture();
        std::fs::write(&path, &fixture[65535..]).unwrap();
        let (meta, records) = select_native(&path).unwrap();
        assert_eq!(meta["source_gap_stop"], false);
        assert_eq!(meta["skipped_overlong_nonmessage"], 1);
        assert!(records.is_empty(), "{meta}");
    }
}
