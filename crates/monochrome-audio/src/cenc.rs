use aes::Aes128;
use aes::cipher::{KeyIvInit, StreamCipher};
use std::collections::VecDeque;
use std::io::{Read, Result as IoResult, Seek, SeekFrom};
use symphonia::core::io::MediaSource;

type Cipher = ctr::Ctr64BE<Aes128>;

const FLAC_MAGIC: &[u8; 4] = b"fLaC";
const MAX_BOX: usize = 8 * 1024 * 1024;
const IV_SIZE: usize = 8;

pub fn parse_key(hex: &str) -> Option<[u8; 16]> {
    let cleaned = hex.trim();
    if cleaned.len() != 32 {
        return None;
    }
    let mut key = [0u8; 16];
    for (index, slot) in key.iter_mut().enumerate() {
        *slot = u8::from_str_radix(cleaned.get(index * 2..index * 2 + 2)?, 16).ok()?;
    }
    Some(key)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Sample {
    size: usize,
    iv: [u8; 16],
}

pub struct FlacFromCenc<R: Read> {
    inner: R,
    key: [u8; 16],
    out: VecDeque<u8>,
    samples: VecDeque<Sample>,
    mdat_remaining: u64,
    finished: bool,
    emitted_header: bool,
}

impl<R: Read> FlacFromCenc<R> {
    pub fn new(inner: R, key: [u8; 16]) -> Self {
        Self {
            inner,
            key,
            out: VecDeque::new(),
            samples: VecDeque::new(),
            mdat_remaining: 0,
            finished: false,
            emitted_header: false,
        }
    }

    fn fill(&mut self, size: usize) -> IoResult<Option<Vec<u8>>> {
        let mut buffer = vec![0u8; size];
        let mut filled = 0;
        while filled < size {
            match self.inner.read(&mut buffer[filled..])? {
                0 => return Ok(None),
                read => filled += read,
            }
        }
        Ok(Some(buffer))
    }

    fn skip(&mut self, mut amount: u64) -> IoResult<()> {
        let mut scratch = [0u8; 8192];
        while amount > 0 {
            let want = amount.min(scratch.len() as u64) as usize;
            match self.inner.read(&mut scratch[..want])? {
                0 => return Ok(()),
                read => amount -= read as u64,
            }
        }
        Ok(())
    }

    fn pump(&mut self) -> IoResult<()> {
        if self.mdat_remaining > 0 {
            return self.pump_media();
        }

        let Some(header) = self.fill(8)? else {
            self.finished = true;
            return Ok(());
        };
        let declared = u32::from_be_bytes([header[0], header[1], header[2], header[3]]) as u64;
        let kind = [header[4], header[5], header[6], header[7]];

        let (size, header_len) = match declared {
            0 => (u64::MAX, 8u64),
            1 => match self.fill(8)? {
                Some(extended) => (
                    u64::from_be_bytes(extended.try_into().unwrap_or_default()),
                    16,
                ),
                None => {
                    self.finished = true;
                    return Ok(());
                }
            },
            other => (other, 8),
        };

        if size < header_len {
            self.finished = true;
            return Ok(());
        }
        let payload = size - header_len;

        match &kind {
            b"moov" => {
                if payload as usize > MAX_BOX {
                    self.skip(payload)?;
                    return Ok(());
                }
                match self.fill(payload as usize)? {
                    Some(body) => self.emit_header(&body),
                    None => self.finished = true,
                }
            }
            b"moof" => {
                if payload as usize > MAX_BOX {
                    self.skip(payload)?;
                    return Ok(());
                }
                match self.fill(payload as usize)? {
                    Some(body) => self.plan_fragment(&body),
                    None => self.finished = true,
                }
            }
            b"mdat" => self.mdat_remaining = payload,
            _ => {
                if size == u64::MAX {
                    self.finished = true;
                } else {
                    self.skip(payload)?;
                }
            }
        }
        Ok(())
    }

    fn pump_media(&mut self) -> IoResult<()> {
        let Some(sample) = self.samples.pop_front() else {
            let remaining = self.mdat_remaining;
            self.mdat_remaining = 0;
            self.skip(remaining)?;
            return Ok(());
        };

        let size = sample.size.min(self.mdat_remaining as usize);
        if size == 0 {
            self.mdat_remaining = 0;
            return Ok(());
        }

        match self.fill(size)? {
            Some(mut data) => {
                let mut cipher = Cipher::new(&self.key.into(), &sample.iv.into());
                cipher.apply_keystream(&mut data);
                self.out.extend(data);
                self.mdat_remaining -= size as u64;
            }
            None => {
                self.finished = true;
                self.mdat_remaining = 0;
            }
        }
        Ok(())
    }

    fn emit_header(&mut self, moov: &[u8]) {
        if self.emitted_header {
            return;
        }
        if let Some(blocks) = find_dfla(moov) {
            self.out.extend(FLAC_MAGIC);
            self.out.extend(blocks);
            self.emitted_header = true;
        }
    }

    fn plan_fragment(&mut self, moof: &[u8]) {
        let sizes = find_box(moof, b"trun")
            .map(|body| parse_trun(body, find_box(moof, b"tfhd").and_then(parse_tfhd_default)))
            .unwrap_or_default();
        let ivs = find_box(moof, b"senc").map(parse_senc).unwrap_or_default();

        self.samples.clear();
        for (index, size) in sizes.into_iter().enumerate() {
            self.samples.push_back(Sample {
                size,
                iv: ivs.get(index).copied().unwrap_or([0u8; 16]),
            });
        }
    }
}

impl<R: Read> Read for FlacFromCenc<R> {
    fn read(&mut self, buffer: &mut [u8]) -> IoResult<usize> {
        while self.out.is_empty() && !self.finished {
            self.pump()?;
        }
        let take = buffer.len().min(self.out.len());
        for slot in buffer.iter_mut().take(take) {
            *slot = self.out.pop_front().unwrap_or(0);
        }
        Ok(take)
    }
}

impl<R: Read> Seek for FlacFromCenc<R> {
    fn seek(&mut self, _target: SeekFrom) -> IoResult<u64> {
        Err(std::io::Error::new(
            std::io::ErrorKind::Unsupported,
            "a decrypted stream cannot be rewound",
        ))
    }
}

impl<R: Read + Send + Sync> MediaSource for FlacFromCenc<R> {
    fn is_seekable(&self) -> bool {
        false
    }

    fn byte_len(&self) -> Option<u64> {
        None
    }
}

pub fn find_box<'a>(data: &'a [u8], wanted: &[u8; 4]) -> Option<&'a [u8]> {
    let mut offset = 0usize;
    while offset + 8 <= data.len() {
        let size = u32::from_be_bytes([
            data[offset],
            data[offset + 1],
            data[offset + 2],
            data[offset + 3],
        ]) as usize;
        let kind = [
            data[offset + 4],
            data[offset + 5],
            data[offset + 6],
            data[offset + 7],
        ];
        if size < 8 || offset + size > data.len() {
            return None;
        }
        let body = &data[offset + 8..offset + size];
        if &kind == wanted {
            return Some(body);
        }
        if is_container(&kind)
            && let Some(inner) = find_box(body, wanted)
        {
            return Some(inner);
        }
        offset += size;
    }
    None
}

fn is_container(kind: &[u8; 4]) -> bool {
    matches!(
        kind,
        b"moov"
            | b"trak"
            | b"mdia"
            | b"minf"
            | b"stbl"
            | b"stsd"
            | b"moof"
            | b"traf"
            | b"edts"
            | b"mvex"
            | b"sinf"
            | b"schi"
            | b"enca"
            | b"udta"
    )
}

fn find_dfla(moov: &[u8]) -> Option<&[u8]> {
    let mut offset = 0usize;
    while offset + 8 <= moov.len() {
        if &moov[offset + 4..offset + 8] == b"dfLa" {
            let size = u32::from_be_bytes([
                moov[offset],
                moov[offset + 1],
                moov[offset + 2],
                moov[offset + 3],
            ]) as usize;
            if size >= 16 && offset + size <= moov.len() {
                return Some(&moov[offset + 12..offset + size]);
            }
        }
        offset += 1;
    }
    None
}

fn parse_tfhd_default(body: &[u8]) -> Option<usize> {
    if body.len() < 8 {
        return None;
    }
    let flags = u32::from_be_bytes([0, body[1], body[2], body[3]]);
    let mut offset = 8;
    if flags & 0x000001 != 0 {
        offset += 8;
    }
    if flags & 0x000002 != 0 {
        offset += 4;
    }
    if flags & 0x000008 != 0 {
        offset += 4;
    }
    if flags & 0x000010 == 0 || body.len() < offset + 4 {
        return None;
    }
    Some(u32::from_be_bytes([
        body[offset],
        body[offset + 1],
        body[offset + 2],
        body[offset + 3],
    ]) as usize)
}

fn parse_trun(body: &[u8], default_size: Option<usize>) -> Vec<usize> {
    if body.len() < 8 {
        return Vec::new();
    }
    let flags = u32::from_be_bytes([0, body[1], body[2], body[3]]);
    let count = u32::from_be_bytes([body[4], body[5], body[6], body[7]]) as usize;
    let mut offset = 8;
    if flags & 0x000001 != 0 {
        offset += 4;
    }
    if flags & 0x000004 != 0 {
        offset += 4;
    }

    let mut sizes = Vec::with_capacity(count.min(4096));
    for _ in 0..count {
        if flags & 0x000100 != 0 {
            offset += 4;
        }
        let size = if flags & 0x000200 != 0 {
            if body.len() < offset + 4 {
                break;
            }
            let value = u32::from_be_bytes([
                body[offset],
                body[offset + 1],
                body[offset + 2],
                body[offset + 3],
            ]) as usize;
            offset += 4;
            value
        } else {
            match default_size {
                Some(value) => value,
                None => break,
            }
        };
        if flags & 0x000400 != 0 {
            offset += 4;
        }
        if flags & 0x000800 != 0 {
            offset += 4;
        }
        sizes.push(size);
    }
    sizes
}

fn parse_senc(body: &[u8]) -> Vec<[u8; 16]> {
    if body.len() < 8 {
        return Vec::new();
    }
    let flags = u32::from_be_bytes([0, body[1], body[2], body[3]]);
    let count = u32::from_be_bytes([body[4], body[5], body[6], body[7]]) as usize;
    let mut offset = 8;
    let mut ivs = Vec::with_capacity(count.min(4096));

    for _ in 0..count {
        if body.len() < offset + IV_SIZE {
            break;
        }
        let mut iv = [0u8; 16];
        iv[..IV_SIZE].copy_from_slice(&body[offset..offset + IV_SIZE]);
        offset += IV_SIZE;
        ivs.push(iv);

        if flags & 0x000002 != 0 {
            if body.len() < offset + 2 {
                break;
            }
            let subsamples = u16::from_be_bytes([body[offset], body[offset + 1]]) as usize;
            offset += 2 + subsamples * 6;
        }
    }
    ivs
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mp4_box(kind: &[u8; 4], payload: &[u8]) -> Vec<u8> {
        let mut out = ((payload.len() + 8) as u32).to_be_bytes().to_vec();
        out.extend_from_slice(kind);
        out.extend_from_slice(payload);
        out
    }

    fn flac_blocks() -> Vec<u8> {
        let mut streaminfo = vec![0x80, 0x00, 0x00, 0x22];
        streaminfo.extend_from_slice(&[0u8; 34]);
        streaminfo
    }

    fn moov_with_dfla() -> Vec<u8> {
        let mut dfla_payload = vec![0u8, 0, 0, 0];
        dfla_payload.extend_from_slice(&flac_blocks());
        let dfla = mp4_box(b"dfLa", &dfla_payload);
        let enca = mp4_box(b"enca", &dfla);
        let stsd = mp4_box(b"stsd", &enca);
        let stbl = mp4_box(b"stbl", &stsd);
        let minf = mp4_box(b"minf", &stbl);
        let mdia = mp4_box(b"mdia", &minf);
        let trak = mp4_box(b"trak", &mdia);
        mp4_box(b"moov", &trak)
    }

    fn trun(sizes: &[usize]) -> Vec<u8> {
        let mut payload = vec![0u8, 0x00, 0x02, 0x00];
        payload.extend_from_slice(&(sizes.len() as u32).to_be_bytes());
        for size in sizes {
            payload.extend_from_slice(&(*size as u32).to_be_bytes());
        }
        mp4_box(b"trun", &payload)
    }

    fn senc(ivs: &[[u8; 8]]) -> Vec<u8> {
        let mut payload = vec![0u8, 0, 0, 0];
        payload.extend_from_slice(&(ivs.len() as u32).to_be_bytes());
        for iv in ivs {
            payload.extend_from_slice(iv);
        }
        mp4_box(b"senc", &payload)
    }

    fn encrypt(key: &[u8; 16], iv: &[u8; 8], data: &[u8]) -> Vec<u8> {
        let mut full = [0u8; 16];
        full[..8].copy_from_slice(iv);
        let mut cipher = Cipher::new(&(*key).into(), &full.into());
        let mut out = data.to_vec();
        cipher.apply_keystream(&mut out);
        out
    }

    fn key() -> [u8; 16] {
        [
            0x00, 0x11, 0x22, 0x33, 0x44, 0x55, 0x66, 0x77, 0x88, 0x99, 0xaa, 0xbb, 0xcc, 0xdd,
            0xee, 0xff,
        ]
    }

    fn stream_with(samples: &[Vec<u8>], ivs: &[[u8; 8]]) -> Vec<u8> {
        let mut file = mp4_box(b"ftyp", b"mp41iso8");
        file.extend(moov_with_dfla());
        file.extend(mp4_box(b"sidx", &[0u8; 12]));

        let sizes: Vec<usize> = samples.iter().map(Vec::len).collect();
        let mut traf = mp4_box(b"tfhd", &[0u8; 8]);
        traf.extend(trun(&sizes));
        traf.extend(senc(ivs));
        let traf = mp4_box(b"traf", &traf);
        file.extend(mp4_box(b"moof", &traf));

        let mut mdat = Vec::new();
        for (sample, iv) in samples.iter().zip(ivs) {
            mdat.extend(encrypt(&key(), iv, sample));
        }
        file.extend(mp4_box(b"mdat", &mdat));
        file
    }

    fn run(stream: &[u8]) -> Vec<u8> {
        let mut reader = FlacFromCenc::new(std::io::Cursor::new(stream.to_vec()), key());
        let mut out = Vec::new();
        reader.read_to_end(&mut out).expect("transform");
        out
    }

    #[test]
    fn a_hex_key_of_the_right_length_is_accepted() {
        assert_eq!(parse_key("00112233445566778899aabbccddeeff"), Some(key()));
        assert_eq!(
            parse_key("  00112233445566778899aabbccddeeff  "),
            Some(key())
        );
    }

    #[test]
    fn a_malformed_key_is_refused() {
        assert_eq!(parse_key("short"), None);
        assert_eq!(parse_key(&"z".repeat(32)), None);
        assert_eq!(parse_key(""), None);
    }

    #[test]
    fn the_output_starts_with_the_flac_marker_and_the_stored_metadata() {
        let samples = vec![b"first sample".to_vec()];
        let stream = stream_with(&samples, &[[1, 2, 3, 4, 5, 6, 7, 8]]);
        let out = run(&stream);
        assert_eq!(&out[..4], FLAC_MAGIC);
        assert_eq!(&out[4..4 + flac_blocks().len()], &flac_blocks()[..]);
    }

    #[test]
    fn every_sample_is_decrypted_back_to_its_original_bytes() {
        let samples = vec![
            b"the quick brown fox".to_vec(),
            b"jumps over the lazy dog".to_vec(),
            vec![0xa5; 300],
        ];
        let ivs = [[1u8; 8], [2u8; 8], [3u8; 8]];
        let out = run(&stream_with(&samples, &ivs));

        let mut expected = FLAC_MAGIC.to_vec();
        expected.extend(flac_blocks());
        for sample in &samples {
            expected.extend_from_slice(sample);
        }
        assert_eq!(out, expected);
    }

    #[test]
    fn each_sample_uses_its_own_initialisation_vector() {
        let repeated = vec![b"same bytes".to_vec(), b"same bytes".to_vec()];
        let ivs = [[9u8; 8], [7u8; 8]];
        let stream = stream_with(&repeated, &ivs);

        let mdat_start = stream.len() - repeated[0].len() - repeated[1].len();
        let first = &stream[mdat_start..mdat_start + repeated[0].len()];
        let second = &stream[mdat_start + repeated[0].len()..];
        assert_ne!(first, second, "identical samples must encrypt differently");

        let out = run(&stream);
        let payload = &out[4 + flac_blocks().len()..];
        assert_eq!(&payload[..10], b"same bytes");
        assert_eq!(&payload[10..20], b"same bytes");
    }

    #[test]
    fn several_fragments_are_all_decrypted() {
        let mut stream = mp4_box(b"ftyp", b"mp41");
        stream.extend(moov_with_dfla());
        let mut expected = FLAC_MAGIC.to_vec();
        expected.extend(flac_blocks());

        for round in 0u8..3 {
            let sample = vec![round + 1; 64];
            let iv = [round + 10; 8];
            let mut traf = mp4_box(b"tfhd", &[0u8; 8]);
            traf.extend(trun(&[sample.len()]));
            traf.extend(senc(&[iv]));
            stream.extend(mp4_box(b"moof", &mp4_box(b"traf", &traf)));
            stream.extend(mp4_box(b"mdat", &encrypt(&key(), &iv, &sample)));
            expected.extend(sample);
        }

        assert_eq!(run(&stream), expected);
    }

    #[test]
    fn boxes_that_are_not_needed_are_skipped() {
        let samples = vec![b"payload".to_vec()];
        let mut stream = stream_with(&samples, &[[4u8; 8]]);
        stream.splice(0..0, mp4_box(b"free", &[0u8; 64]));
        let out = run(&stream);
        assert!(out.ends_with(b"payload"));
    }

    #[test]
    fn a_stream_with_no_metadata_produces_nothing_rather_than_garbage() {
        let stream = mp4_box(b"ftyp", b"mp41");
        assert!(run(&stream).is_empty());
    }

    #[test]
    fn a_truncated_stream_stops_cleanly() {
        let samples = vec![vec![7u8; 500]];
        let stream = stream_with(&samples, &[[5u8; 8]]);
        for cut in [10, 40, 120, 200] {
            let partial = &stream[..cut.min(stream.len())];
            let mut reader = FlacFromCenc::new(std::io::Cursor::new(partial.to_vec()), key());
            let mut out = Vec::new();
            assert!(
                reader.read_to_end(&mut out).is_ok(),
                "cut at {cut} panicked"
            );
        }
    }

    #[test]
    fn a_sample_larger_than_the_media_box_does_not_read_past_it() {
        let mut stream = mp4_box(b"ftyp", b"mp41");
        stream.extend(moov_with_dfla());
        let mut traf = mp4_box(b"tfhd", &[0u8; 8]);
        traf.extend(trun(&[9_000]));
        traf.extend(senc(&[[1u8; 8]]));
        stream.extend(mp4_box(b"moof", &mp4_box(b"traf", &traf)));
        stream.extend(mp4_box(b"mdat", &[0u8; 32]));

        let out = run(&stream);
        assert_eq!(out.len(), 4 + flac_blocks().len() + 32);
    }

    #[test]
    fn sample_sizes_fall_back_to_the_fragment_default() {
        let mut tfhd_payload = vec![0u8, 0x00, 0x00, 0x10];
        tfhd_payload.extend_from_slice(&1u32.to_be_bytes());
        tfhd_payload.extend_from_slice(&16u32.to_be_bytes());
        let tfhd = mp4_box(b"tfhd", &tfhd_payload);

        let mut trun_payload = vec![0u8, 0x00, 0x00, 0x00];
        trun_payload.extend_from_slice(&2u32.to_be_bytes());
        let trun = mp4_box(b"trun", &trun_payload);

        let mut traf = tfhd;
        traf.extend(trun);
        traf.extend(senc(&[[1u8; 8], [2u8; 8]]));

        let mut stream = mp4_box(b"ftyp", b"mp41");
        stream.extend(moov_with_dfla());
        stream.extend(mp4_box(b"moof", &mp4_box(b"traf", &traf)));
        let mut mdat = encrypt(&key(), &[1u8; 8], &[0xaa; 16]);
        mdat.extend(encrypt(&key(), &[2u8; 8], &[0xbb; 16]));
        stream.extend(mp4_box(b"mdat", &mdat));

        let out = run(&stream);
        let payload = &out[4 + flac_blocks().len()..];
        assert_eq!(payload.len(), 32);
        assert!(payload[..16].iter().all(|byte| *byte == 0xaa));
        assert!(payload[16..].iter().all(|byte| *byte == 0xbb));
    }

    #[test]
    fn subsample_maps_are_stepped_over_when_present() {
        let mut payload = vec![0u8, 0x00, 0x00, 0x02];
        payload.extend_from_slice(&2u32.to_be_bytes());
        payload.extend_from_slice(&[1u8; 8]);
        payload.extend_from_slice(&1u16.to_be_bytes());
        payload.extend_from_slice(&[0u8; 6]);
        payload.extend_from_slice(&[2u8; 8]);
        payload.extend_from_slice(&0u16.to_be_bytes());
        let ivs = parse_senc(&payload[..]);

        assert_eq!(ivs.len(), 2);
        assert_eq!(ivs[0][..8], [1u8; 8]);
        assert_eq!(ivs[1][..8], [2u8; 8]);
    }

    #[test]
    fn a_nested_box_is_found_through_its_containers() {
        let moov = moov_with_dfla();
        assert!(find_box(&moov[8..], b"stsd").is_some());
    }

    #[test]
    fn the_transformed_stream_reports_itself_as_unseekable() {
        let reader = FlacFromCenc::new(std::io::Cursor::new(Vec::new()), key());
        assert!(!reader.is_seekable());
        assert_eq!(reader.byte_len(), None);
    }
}
