use anyhow::{Context, Result};
use std::path::Path;

#[cfg(unix)]
const PRIVATE_MODE: u32 = 0o600;

pub fn create_private_dir(path: &Path) -> Result<()> {
    std::fs::create_dir_all(path).with_context(|| format!("cannot create {}", path.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700));
    }
    Ok(())
}

pub fn create_private_file(path: &Path) -> Result<()> {
    if let Some(parent) = path.parent() {
        create_private_dir(parent)?;
    }

    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        use std::os::unix::fs::PermissionsExt;
        std::fs::OpenOptions::new()
            .append(true)
            .create(true)
            .mode(PRIVATE_MODE)
            .open(path)
            .with_context(|| format!("cannot open {}", path.display()))?;
        let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(PRIVATE_MODE));
    }
    #[cfg(not(unix))]
    {
        std::fs::OpenOptions::new()
            .append(true)
            .create(true)
            .open(path)
            .with_context(|| format!("cannot open {}", path.display()))?;
    }
    Ok(())
}

pub fn discard(path: &Path) -> Result<()> {
    match std::fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error).with_context(|| format!("cannot remove {}", path.display())),
    }
}

pub fn write_private(path: &Path, contents: &[u8]) -> Result<()> {
    if let Some(parent) = path.parent() {
        create_private_dir(parent)?;
    }

    let temporary = path.with_extension("tmp");
    write_file(&temporary, contents)?;
    std::fs::rename(&temporary, path)
        .with_context(|| format!("cannot replace {}", path.display()))?;
    Ok(())
}

#[cfg(unix)]
fn write_file(path: &Path, contents: &[u8]) -> Result<()> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;

    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(PRIVATE_MODE)
        .open(path)
        .with_context(|| format!("cannot write {}", path.display()))?;
    file.write_all(contents)?;
    file.sync_all()?;
    Ok(())
}

#[cfg(not(unix))]
fn write_file(path: &Path, contents: &[u8]) -> Result<()> {
    std::fs::write(path, contents).with_context(|| format!("cannot write {}", path.display()))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::Scratch;

    #[test]
    fn writing_creates_missing_directories() {
        let scratch = Scratch::new("nested");
        let path = scratch.file("file");
        write_private(&path, b"hello").expect("write");
        assert_eq!(std::fs::read(&path).expect("read"), b"hello");
    }

    #[cfg(unix)]
    #[test]
    fn files_are_created_private_to_the_user() {
        use std::os::unix::fs::PermissionsExt;
        let scratch = Scratch::new("mode");
        let path = scratch.file("file");
        write_private(&path, b"secret").expect("write");
        let mode = std::fs::metadata(&path)
            .expect("metadata")
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o600, "mode was {:o}", mode & 0o777);
    }

    #[cfg(unix)]
    #[test]
    fn a_log_file_is_created_private_to_the_user() {
        use std::os::unix::fs::PermissionsExt;
        let scratch = Scratch::new("log");
        let path = scratch.file("file");
        create_private_file(&path).expect("create");
        let mode = std::fs::metadata(&path)
            .expect("metadata")
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o600, "mode was {:o}", mode & 0o777);
    }

    #[cfg(unix)]
    #[test]
    fn an_existing_world_readable_log_is_tightened() {
        use std::os::unix::fs::PermissionsExt;
        let scratch = Scratch::new("loose");
        let path = scratch.file("file");
        std::fs::create_dir_all(path.parent().unwrap()).expect("dir");
        std::fs::write(&path, b"old").expect("write");
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).expect("chmod");

        create_private_file(&path).expect("tighten");
        let mode = std::fs::metadata(&path)
            .expect("metadata")
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o600);
    }

    #[cfg(unix)]
    #[test]
    fn state_directories_are_private_too() {
        use std::os::unix::fs::PermissionsExt;
        let scratch = Scratch::new("dir");
        let path = scratch.file("file");
        let directory = path.parent().unwrap();
        create_private_dir(directory).expect("create");
        let mode = std::fs::metadata(directory)
            .expect("metadata")
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o700, "mode was {:o}", mode & 0o777);
    }

    #[test]
    fn discarding_removes_the_file() {
        let scratch = Scratch::new("discard");
        let path = scratch.file("file");
        write_private(&path, b"a library and a listening history").expect("write");
        assert!(path.exists());
        discard(&path).expect("discard");
        assert!(!path.exists(), "signing out must leave nothing behind");
    }

    #[test]
    fn discarding_something_that_is_already_gone_is_not_an_error() {
        let scratch = Scratch::new("absent");
        let path = scratch.file("file");
        assert!(discard(&path).is_ok());
    }

    #[test]
    fn rewriting_replaces_the_previous_contents() {
        let scratch = Scratch::new("replace");
        let path = scratch.file("file");
        write_private(&path, b"first").expect("write");
        write_private(&path, b"second").expect("rewrite");
        assert_eq!(std::fs::read(&path).expect("read"), b"second");
    }

    #[test]
    fn no_temporary_file_is_left_behind() {
        let scratch = Scratch::new("temp");
        let path = scratch.file("file");
        write_private(&path, b"data").expect("write");
        assert!(!path.with_extension("tmp").exists());
    }
}
