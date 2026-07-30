use std::fs::File;
use std::io::{Read, Result as IoResult, Seek, SeekFrom, Write};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::Duration;
use symphonia::core::io::MediaSource;

const CHUNK: usize = 64 * 1024;
const WAIT: Duration = Duration::from_millis(2);

struct Progress {
    written: AtomicU64,
    drained: AtomicBool,
    failed: AtomicBool,
}

pub struct Spill {
    reader: File,
    progress: Arc<Progress>,
    position: u64,
}

impl Spill {
    pub fn new<R: Read + Send + 'static>(mut inner: R) -> IoResult<Self> {
        let reader = scratch_file()?;
        let mut writer = reader.try_clone()?;
        let progress = Arc::new(Progress {
            written: AtomicU64::new(0),
            drained: AtomicBool::new(false),
            failed: AtomicBool::new(false),
        });

        let filling = Arc::clone(&progress);
        std::thread::Builder::new()
            .name("monochrome-prefetch".into())
            .spawn(move || {
                let mut scratch = vec![0u8; CHUNK];
                let mut offset = 0u64;
                loop {
                    match inner.read(&mut scratch) {
                        Ok(0) => break,
                        Ok(read) => {
                            if writer.seek(SeekFrom::Start(offset)).is_err()
                                || writer.write_all(&scratch[..read]).is_err()
                            {
                                filling.failed.store(true, Ordering::Release);
                                break;
                            }
                            offset += read as u64;
                            filling.written.store(offset, Ordering::Release);
                        }
                        Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
                        Err(_) => {
                            filling.failed.store(true, Ordering::Release);
                            break;
                        }
                    }
                }
                filling.drained.store(true, Ordering::Release);
            })?;

        Ok(Self {
            reader,
            progress,
            position: 0,
        })
    }

    pub fn buffered(&self) -> u64 {
        self.progress.written.load(Ordering::Acquire)
    }

    pub fn is_complete(&self) -> bool {
        self.progress.drained.load(Ordering::Acquire)
    }

    fn wait_for(&self, offset: u64) -> u64 {
        loop {
            let written = self.buffered();
            if written > offset || self.is_complete() {
                return written;
            }
            std::thread::sleep(WAIT);
        }
    }
}

impl Read for Spill {
    fn read(&mut self, buffer: &mut [u8]) -> IoResult<usize> {
        if buffer.is_empty() {
            return Ok(0);
        }
        let written = self.wait_for(self.position);
        if self.position >= written {
            return Ok(0);
        }
        let available = (written - self.position).min(buffer.len() as u64) as usize;
        self.reader.seek(SeekFrom::Start(self.position))?;
        let read = self.reader.read(&mut buffer[..available])?;
        self.position += read as u64;
        Ok(read)
    }
}

impl Seek for Spill {
    fn seek(&mut self, target: SeekFrom) -> IoResult<u64> {
        let requested = match target {
            SeekFrom::Start(offset) => offset as i128,
            SeekFrom::Current(delta) => self.position as i128 + delta as i128,
            SeekFrom::End(delta) => {
                while !self.is_complete() {
                    std::thread::sleep(WAIT);
                }
                self.buffered() as i128 + delta as i128
            }
        };
        if requested < 0 {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "cannot seek before the start of the stream",
            ));
        }
        self.position = requested as u64;
        Ok(self.position)
    }
}

impl MediaSource for Spill {
    fn is_seekable(&self) -> bool {
        self.is_complete()
    }

    fn byte_len(&self) -> Option<u64> {
        self.is_complete().then(|| self.buffered())
    }
}

fn scratch_file() -> IoResult<File> {
    let directory = std::env::var_os("XDG_RUNTIME_DIR")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);

    for attempt in 0..64u32 {
        let path = directory.join(format!("monochrome-{}-{attempt}.audio", std::process::id()));
        let mut options = std::fs::OpenOptions::new();
        options.read(true).write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        match options.open(&path) {
            Ok(file) => {
                let _ = std::fs::remove_file(&path);
                return Ok(file);
            }
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error),
        }
    }
    Err(std::io::Error::other(
        "cannot open a scratch file for audio",
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn data(len: usize) -> Vec<u8> {
        (0..len).map(|i| (i % 251) as u8).collect()
    }

    fn spill(bytes: Vec<u8>) -> Spill {
        Spill::new(std::io::Cursor::new(bytes)).expect("scratch file")
    }

    fn settled(bytes: Vec<u8>) -> Spill {
        let spill = spill(bytes);
        while !spill.is_complete() {
            std::thread::sleep(Duration::from_millis(1));
        }
        spill
    }

    #[test]
    fn reading_returns_the_source_bytes_in_order() {
        let source = data(200_000);
        let mut spill = spill(source.clone());
        let mut out = Vec::new();
        spill.read_to_end(&mut out).expect("read");
        assert_eq!(out, source);
    }

    #[test]
    fn the_whole_stream_is_fetched_without_being_read() {
        let spill = settled(data(150_000));
        assert_eq!(spill.buffered(), 150_000);
    }

    #[test]
    fn seeking_backwards_returns_bytes_already_seen() {
        let source = data(50_000);
        let mut spill = settled(source.clone());
        spill.seek(SeekFrom::Start(40_000)).expect("seek");
        spill.seek(SeekFrom::Start(100)).expect("seek");
        let mut out = vec![0u8; 32];
        spill.read_exact(&mut out).expect("read");
        assert_eq!(&out[..], &source[100..132]);
    }

    #[test]
    fn seeking_forwards_lands_on_the_right_bytes() {
        let source = data(120_000);
        let mut spill = settled(source.clone());
        spill.seek(SeekFrom::Start(90_000)).expect("seek");
        let mut out = vec![0u8; 16];
        spill.read_exact(&mut out).expect("read");
        assert_eq!(&out[..], &source[90_000..90_016]);
    }

    #[test]
    fn seeking_from_the_end_resolves_against_the_full_length() {
        let source = data(9000);
        let mut spill = spill(source.clone());
        spill.seek(SeekFrom::End(-4)).expect("seek");
        let mut out = vec![0u8; 4];
        spill.read_exact(&mut out).expect("read");
        assert_eq!(&out[..], &source[8996..]);
    }

    #[test]
    fn relative_seeks_move_from_the_current_position() {
        let source = data(1000);
        let mut spill = settled(source.clone());
        spill.seek(SeekFrom::Start(500)).expect("seek");
        spill.seek(SeekFrom::Current(-200)).expect("seek");
        let mut out = vec![0u8; 4];
        spill.read_exact(&mut out).expect("read");
        assert_eq!(&out[..], &source[300..304]);
    }

    #[test]
    fn a_finished_stream_reports_its_length_and_allows_seeking() {
        let spill = settled(data(4096));
        assert!(spill.is_seekable());
        assert_eq!(spill.byte_len(), Some(4096));
    }

    #[test]
    fn an_unfinished_stream_reports_no_length() {
        let spill = Spill::new(SlowSource::new(1_000_000)).expect("spill");
        assert!(!spill.is_seekable());
        assert_eq!(spill.byte_len(), None);
    }

    #[test]
    fn reading_past_the_end_reports_zero() {
        let mut spill = settled(data(64));
        let mut out = Vec::new();
        spill.read_to_end(&mut out).expect("drain");
        assert_eq!(spill.read(&mut [0u8; 8]).expect("eof"), 0);
    }

    #[test]
    fn seeking_before_the_start_is_refused() {
        let mut spill = settled(data(64));
        assert!(spill.seek(SeekFrom::Current(-1)).is_err());
    }

    #[test]
    fn a_reader_waits_for_bytes_that_have_not_arrived_yet() {
        let mut spill = Spill::new(SlowSource::new(4096)).expect("spill");
        let mut out = Vec::new();
        spill.read_to_end(&mut out).expect("read");
        assert_eq!(out.len(), 4096);
    }

    #[test]
    fn the_scratch_file_is_not_left_on_disk() {
        let spill = settled(data(16));
        let directory = std::env::var_os("XDG_RUNTIME_DIR")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(std::env::temp_dir);
        let leftovers: Vec<_> = std::fs::read_dir(&directory)
            .expect("listing")
            .filter_map(Result::ok)
            .filter(|entry| {
                entry
                    .file_name()
                    .to_string_lossy()
                    .starts_with(&format!("monochrome-{}-", std::process::id()))
            })
            .collect();
        assert!(leftovers.is_empty(), "a scratch file was left behind");
        drop(spill);
    }

    struct SlowSource {
        remaining: usize,
    }

    impl SlowSource {
        fn new(remaining: usize) -> Self {
            Self { remaining }
        }
    }

    impl Read for SlowSource {
        fn read(&mut self, buffer: &mut [u8]) -> IoResult<usize> {
            if self.remaining == 0 {
                return Ok(0);
            }
            std::thread::sleep(Duration::from_millis(4));
            let take = buffer.len().min(self.remaining).min(512);
            buffer[..take].fill(7);
            self.remaining -= take;
            Ok(take)
        }
    }
}
