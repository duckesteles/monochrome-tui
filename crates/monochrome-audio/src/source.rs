use std::io::{Read, Result as IoResult, Seek, SeekFrom};
use std::time::Duration;
use symphonia::core::io::MediaSource;

const CONNECT_TIMEOUT: Duration = Duration::from_secs(15);

pub trait ByteRange: Send + Sync {
    fn total_len(&self) -> Option<u64>;
    fn open_at(&self, offset: u64) -> IoResult<Box<dyn Read + Send + Sync>>;
    fn supports_ranges(&self) -> bool;
    fn content_type(&self) -> Option<String> {
        None
    }
}

pub struct RangeSource {
    backend: Box<dyn ByteRange>,
    reader: Option<Box<dyn Read + Send + Sync>>,
    position: u64,
    length: Option<u64>,
}

impl RangeSource {
    pub fn new(backend: Box<dyn ByteRange>) -> Self {
        let length = backend.total_len();
        Self {
            backend,
            reader: None,
            position: 0,
            length,
        }
    }

    fn ensure_reader(&mut self) -> IoResult<()> {
        if self.reader.is_none() {
            self.reader = Some(self.backend.open_at(self.position)?);
        }
        Ok(())
    }
}

impl Read for RangeSource {
    fn read(&mut self, buffer: &mut [u8]) -> IoResult<usize> {
        if buffer.is_empty() {
            return Ok(0);
        }
        self.ensure_reader()?;
        let reader = self.reader.as_mut().expect("reader");
        let read = reader.read(buffer)?;
        self.position += read as u64;
        Ok(read)
    }
}

impl Seek for RangeSource {
    fn seek(&mut self, target: SeekFrom) -> IoResult<u64> {
        let requested = match target {
            SeekFrom::Start(offset) => offset as i128,
            SeekFrom::Current(delta) => self.position as i128 + delta as i128,
            SeekFrom::End(delta) => match self.length {
                Some(length) => length as i128 + delta as i128,
                None => {
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::Unsupported,
                        "the stream length is unknown",
                    ));
                }
            },
        };

        if requested < 0 {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "cannot seek before the start of the stream",
            ));
        }

        let requested = requested as u64;
        if requested != self.position {
            self.position = requested;
            self.reader = None;
        }
        Ok(self.position)
    }
}

impl MediaSource for RangeSource {
    fn is_seekable(&self) -> bool {
        self.backend.supports_ranges() && self.length.is_some()
    }

    fn byte_len(&self) -> Option<u64> {
        self.length
    }
}

pub struct HttpRange {
    client: reqwest::blocking::Client,
    url: String,
    headers: Vec<(String, String)>,
    length: Option<u64>,
    ranges: bool,
    content_type: Option<String>,
}

pub fn extension_for(content_type: &str) -> Option<&'static str> {
    let base = content_type
        .split(';')
        .next()
        .unwrap_or_default()
        .trim()
        .to_ascii_lowercase();
    match base.as_str() {
        "audio/flac" | "audio/x-flac" => Some("flac"),
        "audio/mp4" | "audio/m4a" | "audio/x-m4a" | "video/mp4" | "application/mp4" => Some("mp4"),
        "audio/mpeg" | "audio/mp3" => Some("mp3"),
        "audio/aac" | "audio/aacp" => Some("aac"),
        "audio/ogg" | "application/ogg" => Some("ogg"),
        "audio/wav" | "audio/x-wav" | "audio/wave" => Some("wav"),
        _ => None,
    }
}

pub fn is_textual(content_type: &str) -> bool {
    let base = content_type
        .split(';')
        .next()
        .unwrap_or_default()
        .trim()
        .to_ascii_lowercase();
    base.starts_with("text/")
        || base == "application/json"
        || base == "application/xml"
        || base == "application/problem+json"
}

impl HttpRange {
    pub fn open(url: &str, headers: &[(String, String)]) -> IoResult<Self> {
        let client = reqwest::blocking::Client::builder()
            .connect_timeout(CONNECT_TIMEOUT)
            .user_agent(concat!("monochrome-tui/", env!("CARGO_PKG_VERSION")))
            .build()
            .map_err(to_io)?;

        let mut probe = client.head(url);
        for (key, value) in headers {
            probe = probe.header(key.as_str(), value.as_str());
        }
        let response = probe.send().map_err(to_io)?;

        let content_type = response
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .map(str::to_string);

        let (length, ranges) = if response.status().is_success() {
            let length = response.content_length().filter(|len| *len > 0);
            let ranges = response
                .headers()
                .get(reqwest::header::ACCEPT_RANGES)
                .and_then(|value| value.to_str().ok())
                .map(|value| value.contains("bytes"))
                .unwrap_or(false);
            (length, ranges)
        } else {
            (None, false)
        };

        Ok(Self {
            client,
            url: url.to_string(),
            headers: headers.to_vec(),
            length,
            ranges,
            content_type,
        })
    }
}

impl ByteRange for HttpRange {
    fn total_len(&self) -> Option<u64> {
        self.length
    }

    fn content_type(&self) -> Option<String> {
        self.content_type.clone()
    }

    fn supports_ranges(&self) -> bool {
        self.ranges
    }

    fn open_at(&self, offset: u64) -> IoResult<Box<dyn Read + Send + Sync>> {
        let mut request = self.client.get(&self.url);
        for (key, value) in &self.headers {
            request = request.header(key.as_str(), value.as_str());
        }
        if offset > 0 {
            request = request.header(reqwest::header::RANGE, format!("bytes={offset}-"));
        }
        let mut response = request.send().map_err(to_io)?;
        let status = response.status();
        if !status.is_success() {
            let mut body = String::new();
            let _ = std::io::Read::read_to_string(
                &mut std::io::Read::take(&mut response, 400),
                &mut body,
            );
            let detail = summarise(&body);
            return Err(std::io::Error::other(match detail {
                Some(detail) => format!("the audio source answered {status}: {detail}"),
                None => format!("the audio source answered {status}"),
            }));
        }
        Ok(Box::new(response))
    }
}

pub fn summarise(body: &str) -> Option<String> {
    let trimmed = body.trim();
    if trimmed.is_empty() {
        return None;
    }
    if let Ok(value) = serde_json::from_str::<serde_json::Value>(trimmed) {
        for key in ["detail", "message", "error"] {
            if let Some(text) = value.get(key).and_then(serde_json::Value::as_str) {
                return Some(text.to_string());
            }
        }
    }
    let single_line: String = trimmed
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .take(160)
        .collect();
    Some(single_line.split_whitespace().collect::<Vec<_>>().join(" "))
}

fn to_io(error: reqwest::Error) -> std::io::Error {
    std::io::Error::other(error.to_string())
}

#[cfg(test)]
pub struct MemoryRange {
    bytes: Vec<u8>,
    seekable: bool,
}

#[cfg(test)]
impl MemoryRange {
    pub fn new(bytes: Vec<u8>, seekable: bool) -> Self {
        Self { bytes, seekable }
    }
}

#[cfg(test)]
impl ByteRange for MemoryRange {
    fn total_len(&self) -> Option<u64> {
        self.seekable.then_some(self.bytes.len() as u64)
    }

    fn supports_ranges(&self) -> bool {
        self.seekable
    }

    fn open_at(&self, offset: u64) -> IoResult<Box<dyn Read + Send + Sync>> {
        let start = (offset as usize).min(self.bytes.len());
        Ok(Box::new(std::io::Cursor::new(self.bytes[start..].to_vec())))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn source(seekable: bool) -> RangeSource {
        let bytes: Vec<u8> = (0..=255u8).collect();
        RangeSource::new(Box::new(MemoryRange::new(bytes, seekable)))
    }

    #[test]
    fn reading_walks_the_stream_forward() {
        let mut source = source(true);
        let mut buffer = [0u8; 4];
        source.read_exact(&mut buffer).expect("read");
        assert_eq!(buffer, [0, 1, 2, 3]);
        source.read_exact(&mut buffer).expect("read");
        assert_eq!(buffer, [4, 5, 6, 7]);
    }

    #[test]
    fn seeking_from_the_start_reopens_at_the_offset() {
        let mut source = source(true);
        assert_eq!(source.seek(SeekFrom::Start(100)).expect("seek"), 100);
        let mut buffer = [0u8; 2];
        source.read_exact(&mut buffer).expect("read");
        assert_eq!(buffer, [100, 101]);
    }

    #[test]
    fn seeking_from_the_end_resolves_against_the_length() {
        let mut source = source(true);
        assert_eq!(source.seek(SeekFrom::End(-2)).expect("seek"), 254);
        let mut buffer = [0u8; 2];
        source.read_exact(&mut buffer).expect("read");
        assert_eq!(buffer, [254, 255]);
    }

    #[test]
    fn seeking_relative_to_the_cursor_works() {
        let mut source = source(true);
        source.seek(SeekFrom::Start(10)).expect("seek");
        source.seek(SeekFrom::Current(5)).expect("seek");
        let mut buffer = [0u8; 1];
        source.read_exact(&mut buffer).expect("read");
        assert_eq!(buffer, [15]);
    }

    #[test]
    fn seeking_before_the_start_is_rejected() {
        let mut source = source(true);
        assert!(source.seek(SeekFrom::Current(-1)).is_err());
    }

    #[test]
    fn seeking_from_the_end_of_an_unmeasured_stream_is_rejected() {
        let mut source = source(false);
        assert!(source.seek(SeekFrom::End(-1)).is_err());
    }

    #[test]
    fn a_seek_to_the_current_position_does_not_reopen_the_reader() {
        let mut source = source(true);
        let mut buffer = [0u8; 4];
        source.read_exact(&mut buffer).expect("read");
        assert_eq!(source.seek(SeekFrom::Start(4)).expect("seek"), 4);
        source.read_exact(&mut buffer).expect("read");
        assert_eq!(buffer, [4, 5, 6, 7]);
    }

    #[test]
    fn seekability_follows_the_backend() {
        assert!(source(true).is_seekable());
        assert!(!source(false).is_seekable());
        assert_eq!(source(true).byte_len(), Some(256));
        assert_eq!(source(false).byte_len(), None);
    }

    #[test]
    fn audio_content_types_map_to_a_probe_hint() {
        assert_eq!(extension_for("audio/flac"), Some("flac"));
        assert_eq!(
            extension_for("audio/mp4; codecs=\"mp4a.40.2\""),
            Some("mp4")
        );
        assert_eq!(extension_for("audio/mpeg"), Some("mp3"));
        assert_eq!(extension_for("AUDIO/X-M4A"), Some("mp4"));
        assert_eq!(extension_for("application/octet-stream"), None);
    }

    #[test]
    fn textual_responses_are_recognised_as_not_audio() {
        assert!(is_textual("application/json; charset=utf-8"));
        assert!(is_textual("text/html"));
        assert!(is_textual("application/problem+json"));
        assert!(!is_textual("audio/flac"));
        assert!(!is_textual("application/octet-stream"));
    }

    #[test]
    fn a_json_error_body_is_reduced_to_its_message() {
        assert_eq!(
            summarise(r#"{"detail":"Invalid Turnstile JWT."}"#).as_deref(),
            Some("Invalid Turnstile JWT.")
        );
        assert_eq!(
            summarise(r#"{"error":"Forbidden: requests must come from an allowed site"}"#)
                .as_deref(),
            Some("Forbidden: requests must come from an allowed site")
        );
        assert_eq!(
            summarise(r#"{"message":"rate limited"}"#).as_deref(),
            Some("rate limited")
        );
    }

    #[test]
    fn a_plain_text_body_is_collapsed_onto_one_line() {
        let summary = summarise("  something\n   went\twrong  ").expect("summary");
        assert_eq!(summary, "something went wrong");
    }

    #[test]
    fn an_empty_body_yields_nothing_to_report() {
        assert_eq!(summarise("   "), None);
    }

    #[test]
    fn a_long_body_is_truncated() {
        let summary = summarise(&"x".repeat(5000)).expect("summary");
        assert!(summary.len() <= 160);
    }

    #[test]
    fn reading_to_the_end_reports_zero() {
        let mut source = source(true);
        let mut everything = Vec::new();
        source.read_to_end(&mut everything).expect("drain");
        assert_eq!(everything.len(), 256);
        assert_eq!(source.read(&mut [0u8; 4]).expect("eof"), 0);
    }
}
