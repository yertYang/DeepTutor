//! Incremental SSE framing. Bytes may split at any UTF-8 or CRLF boundary.

use serde_json::Value;

use crate::Error;

pub const MAX_EVENT_BYTES: usize = 8 * 1024 * 1024;

/// Protocol-independent framing shared by Codex and the other provider streams.
#[derive(Default)]
pub struct Framer {
    line: Vec<u8>,
    data: Vec<u8>,
    after_cr: bool,
}

impl Framer {
    pub fn push(&mut self, bytes: &[u8]) -> Result<Vec<Vec<u8>>, Error> {
        let mut events = Vec::new();
        for &byte in bytes {
            if self.after_cr {
                self.after_cr = false;
                if byte == b'\n' {
                    continue;
                }
            }
            if byte == b'\r' || byte == b'\n' {
                self.finish_line(&mut events)?;
                self.after_cr = byte == b'\r';
            } else {
                self.line.push(byte);
                if self.line.len() + self.data.len() > MAX_EVENT_BYTES {
                    return Err(Error::StreamTooLarge);
                }
            }
        }
        Ok(events)
    }

    pub fn finish(&mut self) -> Result<Vec<Vec<u8>>, Error> {
        let mut events = Vec::new();
        self.finish_line(&mut events)?;
        self.dispatch(&mut events)?;
        Ok(events)
    }

    fn finish_line(&mut self, events: &mut Vec<Vec<u8>>) -> Result<(), Error> {
        let line = std::mem::take(&mut self.line);
        if line.is_empty() {
            return self.dispatch(events);
        }
        if let Some(value) = line.strip_prefix(b"data:") {
            let value = value.strip_prefix(b" ").unwrap_or(value);
            self.data.extend_from_slice(value);
            self.data.push(b'\n');
            if self.data.len() > MAX_EVENT_BYTES {
                return Err(Error::StreamTooLarge);
            }
        }
        Ok(())
    }

    fn dispatch(&mut self, events: &mut Vec<Vec<u8>>) -> Result<(), Error> {
        if self.data.is_empty() {
            return Ok(());
        }
        events.push(std::mem::take(&mut self.data));
        Ok(())
    }
}

/// Codex requires typed JSON objects and ignores the generic [DONE] marker.
#[derive(Default)]
pub struct Decoder {
    framer: Framer,
}

impl Decoder {
    pub fn push(&mut self, bytes: &[u8]) -> Result<Vec<Value>, Error> {
        Self::decode(self.framer.push(bytes)?)
    }

    pub fn finish(&mut self) -> Result<Vec<Value>, Error> {
        Self::decode(self.framer.finish()?)
    }

    fn decode(frames: Vec<Vec<u8>>) -> Result<Vec<Value>, Error> {
        let mut events = Vec::new();
        for data in frames {
            let text = std::str::from_utf8(&data)
                .map_err(|_| Error::InvalidStream)?
                .trim();
            if text == "[DONE]" || text.is_empty() {
                continue;
            }
            let value: Value = serde_json::from_str(text).map_err(|_| Error::InvalidStream)?;
            if !value.is_object() || value.get("type").and_then(Value::as_str).is_none() {
                return Err(Error::InvalidStream);
            }
            events.push(value);
        }
        Ok(events)
    }
}
