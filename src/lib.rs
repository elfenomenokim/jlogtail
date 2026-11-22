#[cfg(feature = "extension-module")]
use pyo3::prelude::*;
mod cli;
use serde_json::{json, Value};
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

pub const SCAN_CAP: u64 = 64 * 1024 * 1024;
pub const DEFAULT_MAX_BYTES: usize = 64 * 1024 * 1024;
pub const RECORD_CAP: usize = 256 * 1024;
pub(crate) const CHUNK: usize = 64 * 1024;

/// How Python-facing row decoding treats invalid UTF-8 (`errors=` in the Python API).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Decode {
    /// Fail the whole read (default, Python `errors="strict"`).
    Strict,
    /// Replace each maximal invalid subpart with U+FFFD (Python `errors="replace"`).
    Replace,
    /// Drop invalid bytes (Python `errors="ignore"`).
    Ignore,
}

impl Decode {
    pub fn parse(name: &str) -> Option<Decode> {
        match name {
            "strict" => Some(Decode::Strict),
            "replace" => Some(Decode::Replace),
            "ignore" => Some(Decode::Ignore),
            _ => None,
        }
    }
}

/// Split a window on `\n`, drop empty lines and decode each row.
pub fn decode_rows(bytes: &[u8], mode: Decode) -> Result<Vec<String>, String> {
    bytes
        .split(|b| *b == b'\n')
        .filter(|line| !line.is_empty())
        .map(|line| decode_row(line, mode))
        .collect()
}

/// Decode one row. `Err` only in `Strict` mode, carrying the standard UTF-8 error text.
pub fn decode_row(line: &[u8], mode: Decode) -> Result<String, String> {
    match mode {
        Decode::Strict => std::str::from_utf8(line)
            .map(str::to_owned)
            .map_err(|e| e.to_string()),
        Decode::Replace => Ok(String::from_utf8_lossy(line).into_owned()),
        Decode::Ignore => {
            let mut out = String::with_capacity(line.len());
            for chunk in line.utf8_chunks() {
                out.push_str(chunk.valid());
            }
            Ok(out)
        }
    }
}

pub fn run_cli(args: Vec<String>) -> Result<(), String> {
    cli::run(args)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TailWindow {
    pub bytes: Vec<u8>,
    pub end_offset: u64,
    pub skipped_oversized: usize,
    pub ceiling_reached: bool,
    pub trailing_oversized: bool,
}

/// Read a bounded suffix using `DEFAULT_MAX_BYTES`, dropping a partial first byte-window row and any incomplete final row.
pub fn read_tail(
    path: &Path,
    lines: Option<usize>,
    byte_window: Option<u64>,
    keep_partial_first: bool,
) -> std::io::Result<TailWindow> {
    read_tail_bounded(path, lines, byte_window, keep_partial_first, DEFAULT_MAX_BYTES)
}

/// Read a suffix bounded by `max_bytes`, dropping a partial first row and any incomplete final row.
///
/// Records larger than `max_bytes` are skipped instead of growing an unbounded buffer.
pub fn read_tail_bounded(
    path: &Path,
    lines: Option<usize>,
    byte_window: Option<u64>,
    keep_partial_first: bool,
    max_bytes: usize,
) -> std::io::Result<TailWindow> {
    if max_bytes == 0 {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "max_bytes must be greater than 0",
        ));
    }
    let cap = max_bytes as u64;
    let mut file = File::open(path)?;
    let size = file.metadata()?.len();
    if size == 0 {
        return Ok(TailWindow {
            bytes: Vec::new(),
            end_offset: 0,
            skipped_oversized: 0,
            ceiling_reached: false,
            trailing_oversized: false,
        });
    }

    if let Some(bytes) = byte_window {
        let effective_bytes = bytes.min(cap);
        let window_capped = bytes > cap && size > cap;
        let mut start = size.saturating_sub(effective_bytes);
        let mut starts_on_boundary = start == 0;
        if start > 0 && !keep_partial_first {
            file.seek(SeekFrom::Start(start - 1))?;
            let mut previous = [0u8; 1];
            file.read_exact(&mut previous)?;
            starts_on_boundary = previous[0] == b'\n';
        }
        file.seek(SeekFrom::Start(start))?;
        let mut data = Vec::new();
        file.by_ref().take(effective_bytes).read_to_end(&mut data)?;
        let read_len = data.len() as u64;
        let had_newline = data.iter().any(|b| *b == b'\n');

        if !had_newline && start > 0 && bytes >= cap {
            return Ok(TailWindow {
                bytes: Vec::new(),
                end_offset: start + read_len,
                skipped_oversized: 1,
                ceiling_reached: true,
                trailing_oversized: true,
            });
        }

        if start > 0 && !keep_partial_first && !starts_on_boundary {
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
        let skipped_oversized = usize::from(
            window_capped && had_newline && data.is_empty() && !keep_partial_first,
        );
        return Ok(TailWindow {
            bytes: data,
            end_offset: start + complete as u64,
            skipped_oversized,
            ceiling_reached: window_capped,
            trailing_oversized: false,
        });
    }

    let n = lines.unwrap_or(10);
    let mut logical_end = size;
    let mut trailing_start = size;

    file.seek(SeekFrom::Start(size - 1))?;
    let mut last = [0u8; 1];
    file.read_exact(&mut last)?;
    let has_final_newline = last[0] == b'\n';

    if !has_final_newline {
        let min_scan = size.saturating_sub(cap.saturating_add(1));
        let mut scan = size;
        let mut found = None;
        let mut block = [0u8; CHUNK];
        while scan > min_scan && found.is_none() {
            let amount = (scan - min_scan).min(block.len() as u64) as usize;
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
                if min_scan > 0 || size > cap {
                    return Ok(TailWindow {
                        bytes: Vec::new(),
                        end_offset: size,
                        skipped_oversized: 1,
                        ceiling_reached: true,
                        trailing_oversized: true,
                    });
                }
                logical_end = 0;
                trailing_start = 0;
            }
        }
    }

    if n == 0 || logical_end == 0 {
        return Ok(TailWindow {
            bytes: Vec::new(),
            end_offset: trailing_start,
            skipped_oversized: 0,
            ceiling_reached: false,
            trailing_oversized: false,
        });
    }

    let complete_end = if has_final_newline {
        size
    } else {
        logical_end + 1
    };
    let min_start = complete_end.saturating_sub(cap);
    let mut pos = logical_end;
    let mut count = 0usize;
    let mut block = [0u8; CHUNK];
    if has_final_newline && pos > 0 {
        pos -= 1;
    }
    let mut start = min_start;
    while pos > min_start && count < n {
        let amount = (pos - min_start).min(block.len() as u64) as usize;
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

    let mut starts_on_boundary = start == 0;
    if start > 0 && count < n {
        file.seek(SeekFrom::Start(start - 1))?;
        let mut previous = [0u8; 1];
        file.read_exact(&mut previous)?;
        starts_on_boundary = previous[0] == b'\n';
    } else if count == n {
        starts_on_boundary = true;
    }

    let read_len = complete_end - start;
    file.seek(SeekFrom::Start(start))?;
    let mut data = Vec::new();
    file.by_ref().take(read_len).read_to_end(&mut data)?;

    let mut ceiling_reached = false;
    let mut skipped_oversized = 0usize;
    if count < n && min_start > 0 {
        if !(starts_on_boundary && count + 1 == n) {
            ceiling_reached = true;
        }
        if !starts_on_boundary {
            if let Some(nl) = data.iter().position(|b| *b == b'\n') {
                data.drain(..=nl);
                start += (nl + 1) as u64;
            } else {
                start += data.len() as u64;
                data.clear();
            }
            if data.is_empty() {
                skipped_oversized = 1;
            }
        }
    }

    let complete = data.iter().rposition(|b| *b == b'\n').map_or(0, |i| i + 1);
    data.truncate(complete);

    Ok(TailWindow {
        bytes: data,
        end_offset: trailing_start.max(start + complete as u64),
        skipped_oversized,
        ceiling_reached,
        trailing_oversized: false,
    })
}

/// Snapshot identity of a log file: the platform file ID (dev/ino, or its
/// Windows equivalent) plus size and mtime in nanoseconds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SnapshotIdentity {
    pub file_id: file_id::FileId,
    pub size: u64,
    pub mtime_ns: i64,
}

impl SnapshotIdentity {
    pub fn stat(path: &Path) -> std::io::Result<SnapshotIdentity> {
        let metadata = std::fs::metadata(path)?;
        Ok(SnapshotIdentity {
            file_id: file_id::get_file_id(path)?,
            size: metadata.len(),
            mtime_ns: system_time_ns(metadata.modified()?),
        })
    }
}

fn system_time_ns(time: std::time::SystemTime) -> i64 {
    match time.duration_since(std::time::UNIX_EPOCH) {
        Ok(d) => d.as_nanos().min(i64::MAX as u128) as i64,
        Err(e) => -(e.duration().as_nanos().min(i64::MAX as u128) as i64),
    }
}

/// Result of one bounded forward read from a saved byte offset.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ForwardRead {
    /// Complete rows read forward from the offset, oldest first; empty when
    /// `identity_ok` is false.
    pub bytes: Vec<u8>,
    /// Cursor to save for the next call: the byte after the last complete row.
    pub new_offset: u64,
    /// False when the file was replaced, shrank below the offset, or changed
    /// during the read; `bytes` is then empty and `new_offset` is the unchanged
    /// start offset (0 when the file no longer reaches the saved offset).
    pub identity_ok: bool,
    /// The platform file ID differed around the read (replacement or rotation).
    pub rotated: bool,
    /// The file shrank below the saved offset or while it was being read.
    pub truncated: bool,
    /// Size or mtime differed between the snapshots taken around the read.
    pub changed: bool,
    /// File size observed after the read.
    pub size: u64,
    /// File mtime in nanoseconds observed after the read.
    pub mtime_ns: i64,
    /// `max_bytes` cut the read before the end of the file.
    pub ceiling_reached: bool,
    /// A record that cannot complete inside the window was skipped; the cursor
    /// stays before it.
    pub skipped_oversized: usize,
}

/// Combine the snapshots taken around a read with the bytes read into a
/// [`ForwardRead`]. Pure so rotation and truncation answers are testable.
fn forward_read_result(
    before: &SnapshotIdentity,
    opened: &SnapshotIdentity,
    after: &SnapshotIdentity,
    start: u64,
    data: &[u8],
    starts_on_boundary: bool,
    max_bytes: usize,
) -> ForwardRead {
    let rotated = before.file_id != opened.file_id || opened.file_id != after.file_id;
    let truncated = after.size < start || after.size < before.size;
    let changed = before.size != after.size
        || before.mtime_ns != after.mtime_ns
        || opened.size != before.size
        || opened.mtime_ns != before.mtime_ns;
    let identity_ok = !rotated && !truncated && !changed;

    let capped = data.len() == max_bytes && start + (data.len() as u64) < before.size;
    let mut consumed = 0u64;
    let mut complete_rows: &[u8] = data;
    if !starts_on_boundary {
        match data.iter().position(|b| *b == b'\n') {
            Some(nl) => {
                consumed = nl as u64 + 1;
                complete_rows = &data[consumed as usize..];
            }
            None => complete_rows = &[],
        }
    }
    let complete_len = complete_rows
        .iter()
        .rposition(|b| *b == b'\n')
        .map_or(0, |i| i + 1);
    let trailing = &complete_rows[complete_len..];
    consumed += complete_len as u64;

    let (bytes, new_offset, skipped_oversized) = if identity_ok {
        (
            complete_rows[..complete_len].to_vec(),
            start + consumed,
            usize::from(capped && !trailing.is_empty() && !trailing.contains(&b'\n')),
        )
    } else {
        (Vec::new(), start, 0)
    };
    ForwardRead {
        bytes,
        new_offset,
        identity_ok,
        rotated,
        truncated,
        changed,
        size: after.size,
        mtime_ns: after.mtime_ns,
        ceiling_reached: capped,
        skipped_oversized,
    }
}

/// Read complete rows forward from a saved byte offset, verifying that the file
/// did not rotate, shrink, or change while it was read.
///
/// `start` is a cursor from a previous call's [`ForwardRead::new_offset`] (0
/// reads from the beginning). At most `max_bytes` are read. A partial first row
/// is dropped when `start` is not at a line boundary, and an unterminated
/// trailing row is held: `new_offset` stops before it and a later call returns
/// it once the writer finishes it. When the dev/ino/size/mtime snapshot taken
/// before and after the read disagrees, or the file shrank below `start`, the
/// answer reports `identity_ok = false` with no rows instead of guessing, and
/// the caller decides whether to rescan from 0. A record that can never
/// complete inside `max_bytes` parks the cursor with `skipped_oversized = 1`;
/// it is reported, never partially returned.
pub fn read_forward(path: &Path, start: u64, max_bytes: usize) -> std::io::Result<ForwardRead> {
    if max_bytes == 0 {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "max_bytes must be greater than 0",
        ));
    }
    let before = SnapshotIdentity::stat(path)?;
    if start > before.size {
        // The saved offset no longer exists: only a fresh scan from 0 is safe.
        return Ok(ForwardRead {
            bytes: Vec::new(),
            new_offset: 0,
            identity_ok: false,
            rotated: false,
            truncated: true,
            changed: false,
            size: before.size,
            mtime_ns: before.mtime_ns,
            ceiling_reached: false,
            skipped_oversized: 0,
        });
    }
    let mut file = File::open(path)?;
    let opened = SnapshotIdentity::stat(path)?;
    let starts_on_boundary = if start == 0 {
        true
    } else {
        file.seek(SeekFrom::Start(start - 1))?;
        let mut previous = [0u8; 1];
        file.read_exact(&mut previous)?;
        previous[0] == b'\n'
    };
    file.seek(SeekFrom::Start(start))?;
    let mut data = Vec::new();
    file.by_ref().take(max_bytes as u64).read_to_end(&mut data)?;
    let after = SnapshotIdentity::stat(path)?;
    Ok(forward_read_result(
        &before,
        &opened,
        &after,
        start,
        &data,
        starts_on_boundary,
        max_bytes,
    ))
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
fn parse_errors(errors: &str) -> PyResult<Decode> {
    Decode::parse(errors).ok_or_else(|| {
        pyo3::exceptions::PyValueError::new_err(format!(
            "errors must be 'strict', 'replace' or 'ignore', got {errors:?}"
        ))
    })
}

#[cfg(feature = "extension-module")]
fn split_rows(bytes: &[u8], mode: Decode) -> PyResult<Vec<String>> {
    decode_rows(bytes, mode).map_err(pyo3::exceptions::PyValueError::new_err)
}

#[cfg(feature = "extension-module")]
fn map_tail_io_err(err: std::io::Error) -> PyErr {
    match err.kind() {
        std::io::ErrorKind::InvalidInput => {
            pyo3::exceptions::PyValueError::new_err(err.to_string())
        }
        _ => pyo3::exceptions::PyOSError::new_err(err.to_string()),
    }
}

#[cfg(feature = "extension-module")]
#[pyfunction(signature = (path, lines=None, byte_window=None, max_bytes=None, keep_partial_first=false, errors="strict"))]
fn tail_raw(
    path: &str,
    lines: Option<usize>,
    byte_window: Option<u64>,
    max_bytes: Option<usize>,
    keep_partial_first: bool,
    errors: &str,
) -> PyResult<Vec<String>> {
    let mode = parse_errors(errors)?;
    let limit = max_bytes.unwrap_or(DEFAULT_MAX_BYTES);
    let window = read_tail_bounded(Path::new(path), lines, byte_window, keep_partial_first, limit)
        .map_err(map_tail_io_err)?;
    split_rows(&window.bytes, mode)
}

#[cfg(feature = "extension-module")]
#[pyfunction(signature = (path, lines=None, byte_window=None, max_bytes=None, keep_partial_first=false, errors="strict"))]
fn tail_window_raw(
    path: &str,
    lines: Option<usize>,
    byte_window: Option<u64>,
    max_bytes: Option<usize>,
    keep_partial_first: bool,
    errors: &str,
) -> PyResult<(String, Vec<String>)> {
    let mode = parse_errors(errors)?;
    let limit = max_bytes.unwrap_or(DEFAULT_MAX_BYTES);
    let window = read_tail_bounded(Path::new(path), lines, byte_window, keep_partial_first, limit)
        .map_err(map_tail_io_err)?;
    let meta = json!({
        "end_offset": window.end_offset,
        "max_bytes": limit,
        "skipped_oversized": window.skipped_oversized,
        "ceiling_reached": window.ceiling_reached,
        "trailing_oversized": window.trailing_oversized,
    });
    let meta_json = serde_json::to_string(&meta)
        .map_err(|e| pyo3::exceptions::PyValueError::new_err(e.to_string()))?;
    let rows = split_rows(&window.bytes, mode)?;
    Ok((meta_json, rows))
}

#[cfg(feature = "extension-module")]
#[pyfunction(signature = (path, offset=0, max_bytes=None, errors="strict"))]
fn read_forward_raw(
    path: &str,
    offset: u64,
    max_bytes: Option<usize>,
    errors: &str,
) -> PyResult<(String, Vec<String>)> {
    let mode = parse_errors(errors)?;
    let limit = max_bytes.unwrap_or(DEFAULT_MAX_BYTES);
    let result = read_forward(Path::new(path), offset, limit).map_err(map_tail_io_err)?;
    let meta = json!({
        "new_offset": result.new_offset,
        "identity_ok": result.identity_ok,
        "rotated": result.rotated,
        "truncated": result.truncated,
        "changed": result.changed,
        "size": result.size,
        "mtime_ns": result.mtime_ns,
        "max_bytes": limit,
        "ceiling_reached": result.ceiling_reached,
        "skipped_oversized": result.skipped_oversized,
    });
    let meta_json = serde_json::to_string(&meta)
        .map_err(|e| pyo3::exceptions::PyValueError::new_err(e.to_string()))?;
    let rows = split_rows(&result.bytes, mode)?;
    Ok((meta_json, rows))
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
    m.add("DEFAULT_MAX_BYTES", DEFAULT_MAX_BYTES)?;
    m.add_function(wrap_pyfunction!(select_native_py, m)?)?;
    m.add_function(wrap_pyfunction!(tail_raw, m)?)?;
    m.add_function(wrap_pyfunction!(tail_window_raw, m)?)?;
    m.add_function(wrap_pyfunction!(read_forward_raw, m)?)?;
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

    #[test]
    fn read_tail_bounded_rejects_zero_ceiling() {
        let path = scratch_file("zero-cap");
        std::fs::write(&path, b"{\"a\":1}\n").unwrap();
        let err = read_tail_bounded(&path, Some(1), None, false, 0).unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::InvalidInput);
    }

    #[test]
    fn read_tail_bounded_caps_lines_window_at_max_bytes() {
        let path = scratch_file("lines-cap");
        let row = "{\"msg\":\"0123456789\"}\n";
        let content = row.repeat(20);
        std::fs::write(&path, content.as_bytes()).unwrap();
        let window = read_tail_bounded(&path, Some(20), None, false, 50).unwrap();
        assert!(window.ceiling_reached);
        assert_eq!(window.skipped_oversized, 0);
        assert!(!window.trailing_oversized);
        assert!(window.bytes.len() <= 50);
        assert_eq!(window.bytes, row.repeat(2).as_bytes());
        assert_eq!(window.end_offset, content.len() as u64);
    }

    #[test]
    fn read_tail_bounded_skips_oversized_complete_record() {
        let path = scratch_file("oversized-complete");
        let small = b"{\"ok\":1}\n";
        let mut content = small.to_vec();
        content.extend_from_slice(format!("{{\"big\":\"{}\"}}\n", "x".repeat(200)).as_bytes());
        std::fs::write(&path, &content).unwrap();
        let window = read_tail_bounded(&path, Some(10), None, false, 64).unwrap();
        assert!(window.bytes.is_empty());
        assert!(window.ceiling_reached);
        assert_eq!(window.skipped_oversized, 1);
        assert!(!window.trailing_oversized);
        assert_eq!(window.end_offset, content.len() as u64);
    }

    #[test]
    fn read_tail_bounded_skips_oversized_incomplete_trailing_record() {
        let path = scratch_file("oversized-trailing");
        let mut content = b"{\"ok\":1}\n".to_vec();
        content.extend_from_slice(format!("{{\"big\":\"{}", "x".repeat(200)).as_bytes());
        std::fs::write(&path, &content).unwrap();
        let window = read_tail_bounded(&path, Some(10), None, false, 64).unwrap();
        assert!(window.bytes.is_empty());
        assert!(window.ceiling_reached);
        assert_eq!(window.skipped_oversized, 1);
        assert!(window.trailing_oversized);
        assert_eq!(window.end_offset, content.len() as u64);
    }

    #[test]
    fn read_tail_bounded_caps_byte_window_and_drops_partial_first() {
        let path = scratch_file("bytes-cap");
        let r1 = b"{\"id\":1,\"pad\":\"aaaaaaaaaa\"}\n";
        let r2 = b"{\"id\":2}\n";
        let mut content = Vec::new();
        content.extend_from_slice(r1);
        content.extend_from_slice(r2);
        std::fs::write(&path, &content).unwrap();
        let window = read_tail_bounded(&path, None, Some(1000), false, 20).unwrap();
        assert!(window.ceiling_reached);
        assert_eq!(window.bytes, r2);
        assert_eq!(window.end_offset, content.len() as u64);
    }

    #[test]
    fn decode_row_modes_handle_invalid_utf8() {
        let bad = b"{\"a\":\"b\xff\xfead\"}";
        assert!(decode_row(bad, Decode::Strict).is_err());
        assert_eq!(decode_row(bad, Decode::Replace).unwrap(), "{\"a\":\"b\u{fffd}\u{fffd}ad\"}");
        assert_eq!(decode_row(bad, Decode::Ignore).unwrap(), "{\"a\":\"bad\"}");
        let good = "{\"a\":\"한글\"}".as_bytes();
        for mode in [Decode::Strict, Decode::Replace, Decode::Ignore] {
            assert_eq!(decode_row(good, mode).unwrap(), "{\"a\":\"한글\"}");
        }
        // a multibyte character cut in half is one invalid subpart, as in Python
        assert_eq!(decode_row(b"x\xea\xb0y", Decode::Replace).unwrap(), "x\u{fffd}y");
        assert_eq!(decode_row(b"x\xea\xb0y", Decode::Ignore).unwrap(), "xy");
        assert!(Decode::parse("bogus").is_none());
    }

    fn identity_with(size: u64, mtime_ns: i64, inode: u64) -> SnapshotIdentity {
        SnapshotIdentity {
            file_id: file_id::FileId::new_inode(1, inode),
            size,
            mtime_ns,
        }
    }

    // Append: a cursor read returns only the new rows and advances to EOF.
    #[test]
    fn read_forward_reads_appends_and_advances_the_cursor() {
        let path = scratch_file("forward-append");
        std::fs::write(&path, b"{\"a\":1}\n{\"b\":2}\n").unwrap();
        let first = read_forward(&path, 0, DEFAULT_MAX_BYTES).unwrap();
        assert!(first.identity_ok, "{first:?}");
        assert_eq!(first.bytes, b"{\"a\":1}\n{\"b\":2}\n");
        assert_eq!(first.new_offset, 16);
        assert!(!first.ceiling_reached);
        use std::io::Write;
        let mut file = std::fs::OpenOptions::new().append(true).open(&path).unwrap();
        file.write_all(b"{\"c\":3}\n").unwrap();
        drop(file);
        let second = read_forward(&path, first.new_offset, DEFAULT_MAX_BYTES).unwrap();
        assert!(second.identity_ok, "{second:?}");
        assert_eq!(second.bytes, b"{\"c\":3}\n");
        assert_eq!(second.new_offset, 24);
        // A cursor at EOF is a cheap no-op poll.
        let idle = read_forward(&path, second.new_offset, DEFAULT_MAX_BYTES).unwrap();
        assert!(idle.identity_ok);
        assert!(idle.bytes.is_empty());
        assert_eq!(idle.new_offset, second.new_offset);
    }

    // Partial final row: the cursor holds before it, and the completed row
    // arrives on the next read from the same cursor.
    #[test]
    fn read_forward_holds_an_unterminated_trailing_row() {
        let path = scratch_file("forward-partial");
        std::fs::write(&path, b"{\"a\":1}\n{\"b\":").unwrap();
        let first = read_forward(&path, 0, DEFAULT_MAX_BYTES).unwrap();
        assert!(first.identity_ok, "{first:?}");
        assert_eq!(first.bytes, b"{\"a\":1}\n");
        assert_eq!(first.new_offset, 8);
        use std::io::Write;
        let mut file = std::fs::OpenOptions::new().append(true).open(&path).unwrap();
        file.write_all(b"2}\n").unwrap();
        drop(file);
        let second = read_forward(&path, first.new_offset, DEFAULT_MAX_BYTES).unwrap();
        assert!(second.identity_ok, "{second:?}");
        assert_eq!(second.bytes, b"{\"b\":2}\n");
        assert_eq!(second.new_offset, 16);
    }

    // A cursor that lands inside a record behaves like a byte window: the
    // partial first row is dropped and the cursor advances past its newline.
    #[test]
    fn read_forward_drops_a_partial_first_row_off_boundary() {
        let path = scratch_file("forward-boundary");
        std::fs::write(&path, b"{\"long\":1}\n{\"b\":2}\n").unwrap();
        let result = read_forward(&path, 5, DEFAULT_MAX_BYTES).unwrap();
        assert!(result.identity_ok, "{result:?}");
        assert_eq!(result.bytes, b"{\"b\":2}\n");
        assert_eq!(result.new_offset, result.size);
    }

    // A record that cannot complete inside the window parks the cursor and is
    // reported instead of being returned in pieces.
    #[test]
    fn read_forward_flags_a_record_spanning_the_window() {
        let path = scratch_file("forward-span");
        let row = format!("{{\"big\":\"{}\"}}\n", "x".repeat(200));
        std::fs::write(&path, row.as_bytes()).unwrap();
        let result = read_forward(&path, 0, 64).unwrap();
        assert!(result.identity_ok, "{result:?}");
        assert!(result.bytes.is_empty());
        assert!(result.ceiling_reached);
        assert_eq!(result.skipped_oversized, 1);
        assert_eq!(result.new_offset, 0);
    }

    // Truncation: a rotated file that is smaller than the saved offset gets a
    // truncation answer with a 0 cursor, never rows guessed from a new file.
    #[test]
    fn read_forward_answers_truncation_when_the_file_shrinks_below_the_cursor() {
        let path = scratch_file("forward-truncate");
        std::fs::write(&path, b"{\"a\":1}\n{\"b\":2}\n").unwrap();
        let first = read_forward(&path, 0, DEFAULT_MAX_BYTES).unwrap();
        assert!(first.identity_ok);
        std::fs::remove_file(&path).unwrap();
        std::fs::write(&path, b"{\"x\":9}\n").unwrap();
        let second = read_forward(&path, first.new_offset, DEFAULT_MAX_BYTES).unwrap();
        assert!(!second.identity_ok, "{second:?}");
        assert!(second.truncated);
        assert!(!second.rotated);
        assert!(second.bytes.is_empty());
        assert_eq!(second.new_offset, 0);
    }

    // Rotation with a new inode: no rows, the cursor stays put for the caller
    // to rescan, and the answer names rotation.
    #[test]
    fn read_forward_rotation_new_inode_reports_no_rows() {
        let before = identity_with(100, 1000, 7);
        let opened = before;
        let mut after = before;
        after.file_id = file_id::FileId::new_inode(1, 8);
        let result = forward_read_result(&before, &opened, &after, 10, b"{\"a\":1}\n", true, DEFAULT_MAX_BYTES);
        assert!(!result.identity_ok, "{result:?}");
        assert!(result.rotated);
        assert!(!result.truncated);
        assert!(!result.changed);
        assert!(result.bytes.is_empty());
        assert_eq!(result.new_offset, 10);
    }

    // An mtime move without a file-ID change is a change, not a rotation.
    #[test]
    fn read_forward_moved_mtime_is_a_change_not_a_rotation() {
        let before = identity_with(100, 1000, 7);
        let opened = before;
        let after = identity_with(100, 2000, 7);
        let result = forward_read_result(&before, &opened, &after, 10, b"{\"a\":1}\n", true, DEFAULT_MAX_BYTES);
        assert!(!result.identity_ok, "{result:?}");
        assert!(!result.rotated);
        assert!(!result.truncated);
        assert!(result.changed);
        assert!(result.bytes.is_empty());
        assert_eq!(result.new_offset, 10);
    }

    // Growth between the before snapshot and the read is reported as a change.
    #[test]
    fn read_forward_growth_between_snapshots_is_a_change() {
        let before = identity_with(100, 1000, 7);
        let opened = before;
        let after = identity_with(120, 1000, 7);
        let result = forward_read_result(&before, &opened, &after, 10, b"{\"a\":1}\n", true, DEFAULT_MAX_BYTES);
        assert!(!result.identity_ok, "{result:?}");
        assert!(result.changed);
        assert!(!result.truncated);
    }

    // A shrink during the read is a truncation answer, not guessed rows.
    #[test]
    fn read_forward_shrink_during_read_is_truncation() {
        let before = identity_with(100, 1000, 7);
        let opened = before;
        let after = identity_with(40, 1000, 7);
        let result = forward_read_result(&before, &opened, &after, 10, b"{\"a\":1}\n", true, DEFAULT_MAX_BYTES);
        assert!(!result.identity_ok, "{result:?}");
        assert!(result.truncated);
        assert_eq!(result.new_offset, 10);
    }

    // Invalid UTF-8 follows the decode mode exactly like the tail reader.
    #[test]
    fn read_forward_decodes_with_the_errors_mode() {
        let path = scratch_file("forward-decode");
        std::fs::write(&path, b"{\"a\":\"x\xff\"}\n").unwrap();
        let result = read_forward(&path, 0, DEFAULT_MAX_BYTES).unwrap();
        assert!(result.identity_ok);
        assert!(decode_rows(&result.bytes, Decode::Strict).is_err());
        assert_eq!(
            decode_rows(&result.bytes, Decode::Replace).unwrap(),
            vec!["{\"a\":\"x\u{fffd}\"}".to_string()]
        );
        assert_eq!(
            decode_rows(&result.bytes, Decode::Ignore).unwrap(),
            vec!["{\"a\":\"x\"}".to_string()]
        );
    }

    // Zero ceiling is rejected like the tail reader.
    #[test]
    fn read_forward_rejects_zero_ceiling() {
        let path = scratch_file("forward-zero-cap");
        std::fs::write(&path, b"{\"a\":1}\n").unwrap();
        let err = read_forward(&path, 0, 0).unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::InvalidInput);
    }

    #[test]
    fn invalid_utf8_in_middle_and_last_row_follows_the_mode() {
        let path = scratch_file("invalid-utf8");
        let mut content = Vec::new();
        content.extend_from_slice(b"{\"id\":1}\n{\"id\":2,\"t\":\"m\xffid\"}\n{\"id\":3,\"t\":\"la\xc3st\"}\n");
        std::fs::write(&path, &content).unwrap();
        let window = read_tail_bounded(&path, Some(10), None, false, DEFAULT_MAX_BYTES).unwrap();
        assert!(decode_rows(&window.bytes, Decode::Strict).is_err());
        let replaced = decode_rows(&window.bytes, Decode::Replace).unwrap();
        assert_eq!(replaced.len(), 3);
        assert_eq!(replaced[0], "{\"id\":1}");
        assert_eq!(replaced[1], "{\"id\":2,\"t\":\"m\u{fffd}id\"}");
        assert_eq!(replaced[2], "{\"id\":3,\"t\":\"la\u{fffd}st\"}");
        let ignored = decode_rows(&window.bytes, Decode::Ignore).unwrap();
        assert_eq!(ignored[1], "{\"id\":2,\"t\":\"mid\"}");
        assert_eq!(ignored[2], "{\"id\":3,\"t\":\"last\"}");
        // damage only in the last row still fails strict and survives replace
        let last_only = b"{\"id\":1}\n{\"id\":2,\"t\":\"x\xff\"}\n";
        assert!(decode_rows(last_only, Decode::Strict).is_err());
        assert_eq!(decode_rows(last_only, Decode::Replace).unwrap().len(), 2);
    }
}
