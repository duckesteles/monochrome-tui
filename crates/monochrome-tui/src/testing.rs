use std::path::{Path, PathBuf};

pub struct Scratch {
    directory: PathBuf,
}

impl Scratch {
    pub fn new(name: &str) -> Self {
        let directory = std::env::temp_dir().join(format!(
            "monochrome-test-{}-{}-{name}",
            std::process::id(),
            next_id()
        ));
        let _ = std::fs::remove_dir_all(&directory);
        Self { directory }
    }

    pub fn dir(&self) -> &Path {
        &self.directory
    }

    pub fn file(&self, name: &str) -> PathBuf {
        self.directory.join(name)
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.directory);
    }
}

fn next_id() -> u64 {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    COUNTER.fetch_add(1, Ordering::Relaxed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_directory_disappears_when_the_guard_does() {
        let path = {
            let scratch = Scratch::new("dropped");
            let file = scratch.file("thing");
            std::fs::create_dir_all(scratch.dir()).expect("create");
            std::fs::write(&file, b"x").expect("write");
            assert!(file.exists());
            scratch.dir().to_path_buf()
        };
        assert!(
            !path.exists(),
            "a test must not leave anything in the temporary directory"
        );
    }

    #[test]
    fn it_still_cleans_up_when_a_test_panics() {
        let path = {
            let scratch = Scratch::new("panicking");
            std::fs::create_dir_all(scratch.dir()).expect("create");
            let directory = scratch.dir().to_path_buf();
            let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                let _held = &scratch;
                panic!("the test failed");
            }));
            assert!(outcome.is_err());
            directory
        };
        assert!(
            !path.exists(),
            "a failing test must clean up after itself too"
        );
    }

    #[test]
    fn two_guards_never_share_a_directory() {
        let first = Scratch::new("same");
        let second = Scratch::new("same");
        assert_ne!(first.dir(), second.dir());
    }
}
