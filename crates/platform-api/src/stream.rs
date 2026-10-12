//! The progress stream: a live `text/event-stream` response and the incremental parser that turns
//! its bytes into [`ProgressFrame`]s.

use std::str::FromStr as _;

use serde::Deserialize;

use crate::{OperationStatus, PlatformError, ProgressFrame};

/// A live Server-Sent-Events stream of one operation's progress.
#[derive(Debug)]
pub struct EventStream {
    response: Option<reqwest::Response>,
    parser: SseParser,
    terminal_seen: bool,
}

impl EventStream {
    pub(crate) fn new(response: reqwest::Response) -> Self {
        Self {
            response: Some(response),
            parser: SseParser::default(),
            terminal_seen: false,
        }
    }

    /// The next frame, or `None` once the terminal frame has been delivered and nothing
    /// remains.
    ///
    /// # Errors
    ///
    /// [`PlatformError`] if the transport or a frame fails mid-stream.
    pub async fn next_frame(&mut self) -> Result<Option<ProgressFrame>, PlatformError> {
        loop {
            if self.terminal_seen {
                return Ok(None);
            }
            if let Some(frame) = self.parser.next_progress_frame()? {
                self.terminal_seen |= frame.status.is_terminal();
                return Ok(Some(frame));
            }
            let Some(response) = self.response.as_mut() else {
                return Ok(None);
            };
            match response.chunk().await {
                Ok(Some(bytes)) => self.parser.feed(&bytes),
                Ok(None) => {
                    self.response = None;
                    return Ok(None);
                }
                Err(error) => {
                    return Err(if error.is_timeout() {
                        PlatformError::Timeout
                    } else {
                        PlatformError::Network(error)
                    });
                }
            }
        }
    }
}

/// An incremental `text/event-stream` parser over arbitrarily chunked bytes. Comments are
/// skipped, multi-line data joins with newlines, and only `progress` events surface.
#[derive(Default, Debug)]
struct SseParser {
    buffer: String,
}

impl SseParser {
    fn feed(&mut self, bytes: &[u8]) {
        self.buffer.push_str(&String::from_utf8_lossy(bytes));
    }

    /// The next complete `progress` frame, or `None` when more bytes are needed.
    fn next_progress_frame(&mut self) -> Result<Option<ProgressFrame>, PlatformError> {
        loop {
            let Some((event_end, consumed)) = complete_event(&self.buffer) else {
                return Ok(None);
            };
            let raw: String = self.buffer.drain(..consumed).collect();
            let Some(event_text) = raw.get(..event_end) else {
                return Err(PlatformError::MalformedFrame);
            };
            // A comment or another event name parses to None; keep scanning.
            if let Some(frame) = parse_sse_event(event_text)? {
                return Ok(Some(frame));
            }
        }
    }
}

/// Find one complete event in `buffer`: the text before its blank-line terminator and the total
/// number of bytes the terminator consumes. Handles both `\n` and `\r\n` framing in one pass,
/// so whichever terminator comes first wins.
fn complete_event(buffer: &str) -> Option<(usize, usize)> {
    let bytes = buffer.as_bytes();
    for (index, byte) in bytes.iter().enumerate() {
        if *byte != b'\n' {
            continue;
        }
        match (bytes.get(index + 1), bytes.get(index + 2)) {
            (Some(b'\n'), _) => return Some((index, index + 2)),
            (Some(b'\r'), Some(b'\n')) => return Some((index, index + 3)),
            _ => {}
        }
    }
    None
}

/// Parse one event's text into a frame: `None` for comments and non-`progress` names.
fn parse_sse_event(event: &str) -> Result<Option<ProgressFrame>, PlatformError> {
    let mut id: Option<String> = None;
    let mut name: Option<String> = None;
    let mut data_lines: Vec<&str> = Vec::new();
    for line in event.lines() {
        let line = line.strip_suffix('\r').unwrap_or(line);
        if line.is_empty() || line.starts_with(':') {
            continue;
        }
        let (field, value) = match line.split_once(':') {
            Some((field, value)) => (field, value.strip_prefix(' ').unwrap_or(value)),
            None => (line, ""),
        };
        match field {
            "id" => id = Some(value.to_owned()),
            "event" => name = Some(value.to_owned()),
            "data" => data_lines.push(value),
            _ => {}
        }
    }
    if name.as_deref() != Some("progress") || data_lines.is_empty() {
        return Ok(None);
    }
    let wire: FrameWire =
        serde_json::from_str(&data_lines.join("\n")).map_err(|_| PlatformError::MalformedFrame)?;
    let event_id = id.ok_or(PlatformError::MalformedFrame)?;
    let observed = jiff::Timestamp::from_str(&wire.observed_at)
        .map_err(|_| PlatformError::MalformedFrame)?
        .as_second();
    Ok(Some(ProgressFrame {
        event_id: event_id
            .parse()
            .map_err(|_| PlatformError::MalformedFrame)?,
        status: OperationStatus::parse(&wire.status)?,
        stage: wire.stage,
        progress_percent: wire.progress_percent,
        message: wire.message,
        observed_at_secs: observed,
    }))
}

#[derive(Deserialize)]
struct FrameWire {
    status: String,
    #[serde(default)]
    stage: Option<String>,
    #[serde(default)]
    progress_percent: Option<u8>,
    #[serde(default)]
    message: Option<String>,
    observed_at: String,
}
