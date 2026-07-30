use serde_json::Value;

#[derive(Debug, Default, PartialEq)]
pub struct Claims {
    pub expires_at: Option<u64>,
    pub issued_at: Option<u64>,
    pub addresses: Vec<String>,
    pub subject: Option<String>,
}

impl Claims {
    pub fn is_expired(&self, now_secs: u64) -> Option<bool> {
        self.expires_at.map(|expiry| now_secs >= expiry)
    }
}

pub fn inspect(token: &str) -> Option<Claims> {
    let payload = token.split('.').nth(1)?;
    let decoded = base64url(payload)?;
    let value: Value = serde_json::from_slice(&decoded).ok()?;
    let object = value.as_object()?;

    let mut claims = Claims {
        expires_at: object.get("exp").and_then(Value::as_u64),
        issued_at: object.get("iat").and_then(Value::as_u64),
        subject: object
            .get("sub")
            .and_then(Value::as_str)
            .map(str::to_string),
        addresses: Vec::new(),
    };

    for (key, entry) in object {
        if !names_an_address(key) {
            continue;
        }
        if let Some(text) = entry.as_str()
            && looks_like_address(text)
        {
            claims.addresses.push(text.to_string());
        }
    }
    claims.addresses.sort();
    claims.addresses.dedup();
    Some(claims)
}

fn names_an_address(key: &str) -> bool {
    let lowered = key.to_ascii_lowercase();
    if lowered
        .split(|c: char| !c.is_ascii_alphanumeric())
        .any(|part| matches!(part, "ip" | "addr" | "address"))
    {
        return true;
    }
    lowered.starts_with("ip") || lowered.ends_with("addr") || lowered.ends_with("address")
}

fn looks_like_address(value: &str) -> bool {
    value.parse::<std::net::IpAddr>().is_ok()
}

fn base64url(input: &str) -> Option<Vec<u8>> {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
    let mut out = Vec::with_capacity(input.len() * 3 / 4);
    let mut buffer = 0u32;
    let mut bits = 0u32;
    for byte in input.bytes() {
        if byte == b'=' {
            break;
        }
        let index = TABLE.iter().position(|candidate| *candidate == byte)? as u32;
        buffer = (buffer << 6) | index;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((buffer >> bits) as u8);
        }
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn encode(payload: &str) -> String {
        const TABLE: &[u8; 64] =
            b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
        let bytes = payload.as_bytes();
        let mut out = String::new();
        for chunk in bytes.chunks(3) {
            let mut buffer = 0u32;
            for (index, byte) in chunk.iter().enumerate() {
                buffer |= (*byte as u32) << (16 - 8 * index);
            }
            let count = chunk.len() * 8 / 6 + usize::from(chunk.len() * 8 % 6 != 0);
            for index in 0..count {
                out.push(TABLE[((buffer >> (18 - 6 * index)) & 0x3f) as usize] as char);
            }
        }
        format!("header.{out}.signature")
    }

    #[test]
    fn an_expiry_is_read_from_the_payload() {
        let token = encode(r#"{"exp":1700000000,"iat":1699996400}"#);
        let claims = inspect(&token).expect("claims");
        assert_eq!(claims.expires_at, Some(1_700_000_000));
        assert_eq!(claims.issued_at, Some(1_699_996_400));
    }

    #[test]
    fn expiry_is_compared_against_the_current_time() {
        let token = encode(r#"{"exp":1000}"#);
        let claims = inspect(&token).expect("claims");
        assert_eq!(claims.is_expired(999), Some(false));
        assert_eq!(claims.is_expired(1000), Some(true));
        assert_eq!(claims.is_expired(5000), Some(true));
    }

    #[test]
    fn a_token_without_an_expiry_reports_nothing_rather_than_guessing() {
        let claims = inspect(&encode(r#"{"sub":"abc"}"#)).expect("claims");
        assert_eq!(claims.is_expired(0), None);
        assert_eq!(claims.subject.as_deref(), Some("abc"));
    }

    #[test]
    fn an_address_claim_is_found_whatever_it_is_called() {
        let claims = inspect(&encode(r#"{"ip":"192.0.2.10"}"#)).expect("claims");
        assert_eq!(claims.addresses, vec!["192.0.2.10".to_string()]);

        let claims = inspect(&encode(r#"{"client_ip":"1.2.3.4"}"#)).expect("claims");
        assert_eq!(claims.addresses, vec!["1.2.3.4".to_string()]);

        let claims = inspect(&encode(r#"{"remote_addr":"::1"}"#)).expect("claims");
        assert_eq!(claims.addresses, vec!["::1".to_string()]);
    }

    #[test]
    fn a_field_that_merely_mentions_ip_but_holds_no_address_is_ignored() {
        let claims = inspect(&encode(
            r#"{"ip":"not-an-address","description":"1.2.3.4"}"#,
        ))
        .expect("claims");
        assert!(claims.addresses.is_empty());
    }

    #[test]
    fn a_key_that_merely_spells_ip_inside_a_word_is_not_an_address_field() {
        assert!(!names_an_address("description"));
        assert!(!names_an_address("recipient"));
        assert!(!names_an_address("script"));
        assert!(names_an_address("ip"));
        assert!(names_an_address("client_ip"));
        assert!(names_an_address("remote-addr"));
        assert!(names_an_address("ipAddress"));
    }

    #[test]
    fn nonsense_is_rejected_rather_than_panicking() {
        assert!(inspect("").is_none());
        assert!(inspect("not-a-jwt").is_none());
        assert!(inspect("a.!!!!.c").is_none());
        assert!(inspect(&encode("not json")).is_none());
    }

    #[test]
    fn a_real_looking_token_is_parsed() {
        let token =
            encode(r#"{"exp":1900000000,"iat":1899996400,"ip":"192.0.2.10","sub":"turnstile"}"#);
        let claims = inspect(&token).expect("claims");
        assert_eq!(claims.expires_at, Some(1_900_000_000));
        assert_eq!(claims.addresses, vec!["192.0.2.10".to_string()]);
        assert_eq!(claims.subject.as_deref(), Some("turnstile"));
    }
}
