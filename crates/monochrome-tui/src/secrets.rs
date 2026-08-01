use anyhow::Result;
use std::path::PathBuf;

const SERVICE: &str = "monochrome-tui";
pub const SESSION_TOKEN: &str = "session-token";
pub const PLAYBACK_SESSION: &str = "playback-session";
pub const LEGACY_PLAYBACK_SESSION: &str = "amazon-jwt";

enum Request {
    Get(String, std::sync::mpsc::Sender<Option<String>>),
    Set(String, String, std::sync::mpsc::Sender<bool>),
    Clear(String, std::sync::mpsc::Sender<bool>),
}

pub struct Secrets {
    fallback: PathBuf,
    worker: std::sync::Mutex<Option<std::sync::mpsc::Sender<Request>>>,
}

impl Secrets {
    pub fn new(fallback: PathBuf) -> Self {
        let (sender, receiver) = std::sync::mpsc::channel::<Request>();
        let spawned = std::thread::Builder::new()
            .name("monochrome-keyring".into())
            .spawn(move || keyring_worker(receiver));

        Self {
            fallback,
            worker: std::sync::Mutex::new(spawned.ok().map(|_| sender)),
        }
    }

    fn dispatch(&self, request: Request) -> bool {
        let guard = self.worker.lock().expect("worker");
        match guard.as_ref() {
            Some(sender) => sender.send(request).is_ok(),
            None => false,
        }
    }

    pub fn get(&self, key: &str) -> Option<String> {
        let (reply, answer) = std::sync::mpsc::channel();
        if self.dispatch(Request::Get(key.to_string(), reply))
            && let Ok(Some(secret)) = answer.recv()
        {
            return Some(secret);
        }
        self.read_fallback(key)
    }

    pub fn set(&self, key: &str, value: &str) -> Result<()> {
        let (reply, answer) = std::sync::mpsc::channel();
        if self.dispatch(Request::Set(key.to_string(), value.to_string(), reply))
            && answer.recv().unwrap_or(false)
        {
            let _ = self.remove_fallback(key);
            return Ok(());
        }
        self.write_fallback(key, Some(value))
    }

    pub fn clear(&self, key: &str) -> bool {
        let (reply, answer) = std::sync::mpsc::channel();
        if self.dispatch(Request::Clear(key.to_string(), reply)) {
            let _ = answer.recv();
        }
        let _ = self.remove_fallback(key);
        self.get(key).is_none()
    }

    fn entries(&self) -> Vec<(String, String)> {
        let Ok(raw) = std::fs::read_to_string(&self.fallback) else {
            return Vec::new();
        };
        raw.lines()
            .filter_map(|line| {
                let line = line.trim();
                if line.is_empty() || line.starts_with('#') {
                    return None;
                }
                let (key, value) = line.split_once('=')?;
                Some((key.trim().to_string(), value.trim().to_string()))
            })
            .collect()
    }

    fn read_fallback(&self, key: &str) -> Option<String> {
        self.entries()
            .into_iter()
            .find(|(name, _)| name == key)
            .map(|(_, value)| value)
            .filter(|value| !value.is_empty())
    }

    fn remove_fallback(&self, key: &str) -> Result<()> {
        self.write_fallback(key, None)
    }

    fn write_fallback(&self, key: &str, value: Option<&str>) -> Result<()> {
        let mut entries = self.entries();
        entries.retain(|(name, _)| name != key);
        if let Some(value) = value {
            entries.push((key.to_string(), value.to_string()));
        }
        if entries.is_empty() {
            let _ = std::fs::remove_file(&self.fallback);
            return Ok(());
        }
        let body = entries
            .iter()
            .map(|(name, value)| format!("{name}={value}"))
            .collect::<Vec<_>>()
            .join("\n");
        crate::paths::write_private(&self.fallback, body.as_bytes())
    }
}

fn keyring_worker(requests: std::sync::mpsc::Receiver<Request>) {
    while let Ok(request) = requests.recv() {
        match request {
            Request::Get(key, reply) => {
                let secret = keyring::Entry::new(SERVICE, &key)
                    .ok()
                    .and_then(|entry| entry.get_password().ok());
                let _ = reply.send(secret);
            }
            Request::Set(key, value, reply) => {
                let stored = keyring::Entry::new(SERVICE, &key)
                    .ok()
                    .map(|entry| entry.set_password(&value).is_ok())
                    .unwrap_or(false);
                let _ = reply.send(stored);
            }
            Request::Clear(key, reply) => {
                let gone = match keyring::Entry::new(SERVICE, &key) {
                    Ok(entry) => match entry.delete_credential() {
                        Ok(()) => true,
                        Err(keyring::Error::NoEntry) => true,
                        Err(_) => false,
                    },
                    Err(_) => false,
                };
                let _ = reply.send(gone);
            }
        }
    }
}

pub fn redact(text: &str) -> String {
    let mut cleaned = String::with_capacity(text.len());
    for word in text.split_inclusive(char::is_whitespace) {
        let trimmed = word.trim_end();
        if looks_secret(trimmed) {
            cleaned.push_str("[redacted]");
            cleaned.push_str(&word[trimmed.len()..]);
        } else {
            cleaned.push_str(word);
        }
    }
    cleaned
}

fn looks_secret(word: &str) -> bool {
    let candidate =
        word.trim_matches(|c: char| !c.is_ascii_alphanumeric() && c != '.' && c != '_' && c != '-');
    if candidate.len() < 24 {
        return false;
    }
    let allowed = candidate
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '_' || c == '-');
    let has_digit = candidate.chars().any(|c| c.is_ascii_digit());
    let has_upper = candidate.chars().any(|c| c.is_ascii_uppercase());
    allowed && (has_digit || has_upper)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::Scratch;

    #[test]
    fn a_value_written_to_the_fallback_can_be_read_back() {
        let scratch = Scratch::new("roundtrip");
        let store = Secrets::new(scratch.file("credentials"));
        store.write_fallback("a", Some("value")).expect("write");
        assert_eq!(store.read_fallback("a").as_deref(), Some("value"));
    }

    #[test]
    fn removing_a_key_leaves_the_others_alone() {
        let scratch = Scratch::new("remove");
        let store = Secrets::new(scratch.file("credentials"));
        store.write_fallback("a", Some("1")).expect("write");
        store.write_fallback("b", Some("2")).expect("write");
        store.remove_fallback("a").expect("remove");
        assert_eq!(store.read_fallback("a"), None);
        assert_eq!(store.read_fallback("b").as_deref(), Some("2"));
    }

    #[test]
    fn the_fallback_file_disappears_when_it_empties() {
        let scratch = Scratch::new("empty");
        let path = scratch.file("credentials");
        let store = Secrets::new(path.clone());
        store.write_fallback("a", Some("1")).expect("write");
        assert!(path.exists());
        store.remove_fallback("a").expect("remove");
        assert!(!path.exists());
    }

    #[cfg(unix)]
    #[test]
    fn the_fallback_file_is_private_to_the_user() {
        use std::os::unix::fs::PermissionsExt;
        let scratch = Scratch::new("mode");
        let path = scratch.file("credentials");
        let store = Secrets::new(path.clone());
        store.write_fallback("a", Some("1")).expect("write");
        let mode = std::fs::metadata(&path)
            .expect("metadata")
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o600);
    }

    #[test]
    fn a_blank_stored_value_counts_as_missing() {
        let scratch = Scratch::new("blank");
        let store = Secrets::new(scratch.file("credentials"));
        store.write_fallback("a", Some("")).expect("write");
        assert_eq!(store.read_fallback("a"), None);
    }

    #[test]
    fn long_opaque_values_are_redacted_from_messages() {
        let message = "request failed with token AbC123dEf456GhI789jKl012MnO attached";
        let cleaned = redact(message);
        assert!(!cleaned.contains("AbC123dEf456GhI789jKl012MnO"));
        assert!(cleaned.contains("[redacted]"));
        assert!(cleaned.starts_with("request failed with token"));
    }

    #[test]
    fn a_long_token_without_digits_is_still_redacted() {
        let cleaned = redact("header X-Turnstile-JWT abcdefGHIJKLmnopqrstuvwxyzABCD here");
        assert!(cleaned.contains("[redacted]"));
        assert!(!cleaned.contains("abcdefGHIJKLmnopqrstuvwxyzABCD"));
    }

    #[test]
    fn ordinary_words_survive_redaction() {
        let message = "every catalog instance failed (https://eu-central.monochrome.tf: HTTP 503)";
        assert_eq!(redact(message), message);
    }

    #[test]
    fn short_values_are_not_treated_as_secrets() {
        assert_eq!(redact("code 401 returned"), "code 401 returned");
    }

    #[test]
    fn a_bearer_token_in_a_header_dump_is_redacted() {
        let cleaned = redact("authorization: Bearer aGVsbG93b3JsZDEyMzQ1Njc4OTAxMjM0");
        assert!(cleaned.contains("[redacted]"));
        assert!(cleaned.contains("Bearer"));
    }
}

#[cfg(test)]
mod runtime_tests {
    use super::*;
    use crate::testing::Scratch;

    fn unique(name: &str) -> String {
        format!("monochrome-test-{}-{name}", std::process::id())
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn secret_access_from_inside_the_async_runtime_does_not_panic() {
        let key = unique("inside");
        let scratch = Scratch::new("inside-runtime");
        let store = Secrets::new(scratch.file("credentials"));
        store.set(&key, "value-from-a-task").expect("set");
        assert_eq!(store.get(&key).as_deref(), Some("value-from-a-task"));
        store.clear(&key);
        assert_eq!(store.get(&key), None);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn secret_access_from_a_spawned_task_does_not_panic() {
        let key = unique("spawned");
        let scratch = Scratch::new("spawned");
        let store = std::sync::Arc::new(Secrets::new(scratch.file("credentials")));
        let writer = std::sync::Arc::clone(&store);
        let written = key.clone();
        tokio::spawn(async move {
            writer.set(&written, "jwt-from-a-task").expect("set");
        })
        .await
        .expect("task finished without panicking");
        assert_eq!(store.get(&key).as_deref(), Some("jwt-from-a-task"));
        store.clear(&key);
    }

    #[test]
    fn secret_access_outside_any_runtime_does_not_panic() {
        let key = unique("outside");
        let scratch = Scratch::new("outside");
        let store = Secrets::new(scratch.file("credentials"));
        store.set(&key, "plain").expect("set");
        assert_eq!(store.get(&key).as_deref(), Some("plain"));
        store.clear(&key);
        assert_eq!(store.get(&key), None);
    }

    #[test]
    fn clearing_a_secret_waits_for_it_to_be_gone_before_returning() {
        let key = unique("cleared");
        let scratch = Scratch::new("cleared");
        let store = Secrets::new(scratch.file("credentials"));
        store.set(&key, "value-to-remove").expect("set");

        assert!(
            store.clear(&key),
            "clear should report success once the secret is actually gone"
        );
        assert_eq!(
            store.get(&key),
            None,
            "the secret was still readable after clear returned"
        );
    }

    #[test]
    fn clearing_something_that_was_never_stored_still_counts_as_gone() {
        let key = unique("never-stored");
        let scratch = Scratch::new("never-stored");
        let store = Secrets::new(scratch.file("credentials"));
        assert!(store.clear(&key));
    }

    #[test]
    fn a_secret_survives_a_new_handle_to_the_same_store() {
        let key = unique("persist");
        let scratch = Scratch::new("persist");
        let path = scratch.file("credentials");
        let first = Secrets::new(path.clone());
        first.set(&key, "kept").expect("set");
        drop(first);

        let second = Secrets::new(path);
        assert_eq!(second.get(&key).as_deref(), Some("kept"));
        second.clear(&key);
    }
}
