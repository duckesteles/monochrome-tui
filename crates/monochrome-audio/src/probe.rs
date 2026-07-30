#[derive(Debug, PartialEq, Eq)]
pub struct BoxInfo {
    pub kind: String,
    pub size: u64,
    pub truncated: bool,
}

pub fn top_level_boxes(bytes: &[u8]) -> Vec<BoxInfo> {
    let mut boxes = Vec::new();
    let mut offset = 0usize;
    while offset + 8 <= bytes.len() && boxes.len() < 32 {
        let size = u32::from_be_bytes([
            bytes[offset],
            bytes[offset + 1],
            bytes[offset + 2],
            bytes[offset + 3],
        ]) as u64;
        let kind: String = bytes[offset + 4..offset + 8]
            .iter()
            .map(|byte| {
                if byte.is_ascii_graphic() {
                    *byte as char
                } else {
                    '.'
                }
            })
            .collect();
        if !kind.chars().all(|c| c.is_ascii_alphanumeric() || c == ' ') {
            break;
        }
        let (size, header) = match size {
            0 => ((bytes.len() - offset) as u64, 8usize),
            1 if offset + 16 <= bytes.len() => (
                u64::from_be_bytes(
                    bytes[offset + 8..offset + 16]
                        .try_into()
                        .unwrap_or_default(),
                ),
                16,
            ),
            1 => break,
            other => (other, 8),
        };
        if size < header as u64 {
            break;
        }
        let truncated = offset as u64 + size > bytes.len() as u64;
        boxes.push(BoxInfo {
            kind,
            size,
            truncated,
        });
        offset += size as usize;
    }
    boxes
}

pub fn describe(bytes: &[u8]) -> String {
    if bytes.len() >= 4 && &bytes[..4] == b"fLaC" {
        return "flac".into();
    }
    if bytes.len() >= 4 && &bytes[..4] == b"OggS" {
        return "ogg".into();
    }
    if bytes.len() >= 3 && &bytes[..3] == b"ID3" {
        return "mp3 with an id3 tag".into();
    }
    if bytes.len() >= 2 && bytes[0] == 0xff && bytes[1] & 0xe0 == 0xe0 {
        return "raw mpeg or adts audio".into();
    }
    if bytes.len() >= 4 && &bytes[..4] == b"RIFF" {
        return "wav".into();
    }

    let boxes = top_level_boxes(bytes);
    if boxes.is_empty() {
        return "no recognisable container".into();
    }

    let kinds: Vec<&str> = boxes.iter().map(|entry| entry.kind.as_str()).collect();
    let has_ftyp = kinds.contains(&"ftyp");
    let has_moov = kinds.contains(&"moov");
    let has_moof = kinds.contains(&"moof");

    if !has_ftyp && !has_moov && !has_moof {
        return format!("unknown container, starts with {}", kinds.join(" "));
    }
    if has_moof && !has_moov {
        return "fragmented mp4 without its init segment".into();
    }
    if has_moov {
        return "mp4 with an init segment".into();
    }
    "mp4 without an init segment".into()
}

pub const ENCRYPTION_MARKERS: [&str; 7] = ["pssh", "sinf", "schm", "senc", "tenc", "enca", "encv"];

pub fn encryption_markers(bytes: &[u8]) -> Vec<&'static str> {
    ENCRYPTION_MARKERS
        .into_iter()
        .filter(|marker| {
            bytes
                .windows(marker.len())
                .any(|window| window == marker.as_bytes())
        })
        .collect()
}

pub fn hex_preview(bytes: &[u8], limit: usize) -> String {
    bytes
        .iter()
        .take(limit)
        .map(|byte| format!("{byte:02x}"))
        .collect::<Vec<_>>()
        .join(" ")
}

pub fn ascii_preview(bytes: &[u8], limit: usize) -> String {
    bytes
        .iter()
        .take(limit)
        .map(|byte| {
            if byte.is_ascii_graphic() {
                *byte as char
            } else {
                '.'
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mp4_box(kind: &[u8; 4], payload: usize) -> Vec<u8> {
        let size = (8 + payload) as u32;
        let mut out = size.to_be_bytes().to_vec();
        out.extend_from_slice(kind);
        out.extend(std::iter::repeat_n(0u8, payload));
        out
    }

    #[test]
    fn flac_is_recognised_by_its_marker() {
        assert_eq!(describe(b"fLaC\x00\x00\x00\x22"), "flac");
    }

    #[test]
    fn ogg_and_wav_are_recognised() {
        assert_eq!(describe(b"OggS----"), "ogg");
        assert_eq!(describe(b"RIFF....WAVE"), "wav");
    }

    #[test]
    fn an_id3_tagged_mp3_is_recognised() {
        assert_eq!(describe(b"ID3\x04\x00"), "mp3 with an id3 tag");
    }

    #[test]
    fn a_raw_mpeg_frame_is_recognised() {
        assert_eq!(
            describe(&[0xff, 0xfb, 0x90, 0x00]),
            "raw mpeg or adts audio"
        );
    }

    #[test]
    fn a_complete_mp4_lists_its_boxes() {
        let mut data = mp4_box(b"ftyp", 16);
        data.extend(mp4_box(b"moov", 32));
        data.extend(mp4_box(b"mdat", 64));
        let boxes = top_level_boxes(&data);
        let kinds: Vec<&str> = boxes.iter().map(|entry| entry.kind.as_str()).collect();
        assert_eq!(kinds, vec!["ftyp", "moov", "mdat"]);
        assert_eq!(boxes[0].size, 24);
        assert_eq!(describe(&data), "mp4 with an init segment");
    }

    #[test]
    fn a_fragment_without_an_init_segment_is_called_out() {
        let mut data = mp4_box(b"styp", 8);
        data.extend(mp4_box(b"moof", 40));
        data.extend(mp4_box(b"mdat", 100));
        assert_eq!(describe(&data), "fragmented mp4 without its init segment");
    }

    #[test]
    fn a_truncated_box_is_marked_rather_than_read_past() {
        let mut data = 4096u32.to_be_bytes().to_vec();
        data.extend_from_slice(b"mdat");
        data.extend_from_slice(&[0u8; 16]);
        let boxes = top_level_boxes(&data);
        assert_eq!(boxes.len(), 1);
        assert!(boxes[0].truncated);
    }

    #[test]
    fn random_bytes_are_not_mistaken_for_a_container() {
        let noise: Vec<u8> = (0..64).map(|i| (i * 7 + 3) as u8).collect();
        assert_eq!(describe(&noise), "no recognisable container");
    }

    #[test]
    fn an_empty_body_is_handled() {
        assert_eq!(describe(&[]), "no recognisable container");
        assert!(top_level_boxes(&[]).is_empty());
    }

    #[test]
    fn a_box_smaller_than_its_header_stops_the_walk() {
        let mut data = 4u32.to_be_bytes().to_vec();
        data.extend_from_slice(b"ftyp");
        assert!(top_level_boxes(&data).is_empty());
    }

    #[test]
    fn encryption_boxes_are_detected_when_present() {
        let mut data = mp4_box(b"ftyp", 8);
        data.extend_from_slice(b"....sinf....tenc....");
        let markers = encryption_markers(&data);
        assert!(markers.contains(&"sinf"));
        assert!(markers.contains(&"tenc"));
    }

    #[test]
    fn a_clear_file_reports_no_encryption() {
        let mut data = mp4_box(b"ftyp", 8);
        data.extend(mp4_box(b"moov", 32));
        assert!(encryption_markers(&data).is_empty());
    }

    #[test]
    fn previews_are_bounded() {
        let data: Vec<u8> = (0..255).map(|i| i as u8).collect();
        assert_eq!(hex_preview(&data, 4), "00 01 02 03");
        assert_eq!(ascii_preview(b"ftyp\x00\x01", 6), "ftyp..");
    }
}
