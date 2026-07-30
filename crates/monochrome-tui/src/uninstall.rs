use crate::config::Paths;
use std::path::PathBuf;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Target {
    Directory {
        what: &'static str,
        path: PathBuf,
    },
    File {
        what: &'static str,
        path: PathBuf,
    },
    Secret {
        what: &'static str,
        key: &'static str,
    },
}

impl Target {
    pub fn describe(&self) -> String {
        match self {
            Target::Directory { what, path } | Target::File { what, path } => {
                format!("{what}  {}", path.display())
            }
            Target::Secret { what, .. } => format!("{what}  system keyring"),
        }
    }

    pub fn exists(&self) -> bool {
        match self {
            Target::Directory { path, .. } | Target::File { path, .. } => path.exists(),
            Target::Secret { .. } => true,
        }
    }
}

pub fn plan(paths: &Paths, binary: Option<PathBuf>) -> Vec<Target> {
    let state = paths.log_dir.clone();
    let config = paths
        .config
        .parent()
        .map(PathBuf::from)
        .unwrap_or_else(|| paths.config.clone());

    let mut targets = vec![
        Target::Secret {
            what: "account token   ",
            key: crate::secrets::SESSION_TOKEN,
        },
        Target::Secret {
            what: "gateway token   ",
            key: crate::secrets::AMAZON_JWT,
        },
        Target::Directory {
            what: "settings        ",
            path: config,
        },
        Target::Directory {
            what: "library and logs",
            path: state,
        },
    ];

    if let Some(cache) = cache_dir() {
        targets.push(Target::Directory {
            what: "cache           ",
            path: cache,
        });
    }
    if let Some(binary) = binary {
        targets.push(Target::File {
            what: "the program     ",
            path: binary,
        });
    }
    targets
}

pub fn cache_dir() -> Option<PathBuf> {
    dirs::cache_dir().map(|base| base.join("monochrome-tui"))
}

pub fn execute(targets: &[Target], secrets: &crate::secrets::Secrets) -> Vec<(String, bool)> {
    targets
        .iter()
        .map(|target| {
            let done = match target {
                Target::Directory { path, .. } => std::fs::remove_dir_all(path)
                    .map(|_| true)
                    .unwrap_or_else(|error| error.kind() == std::io::ErrorKind::NotFound),
                Target::File { path, .. } => std::fs::remove_file(path)
                    .map(|_| true)
                    .unwrap_or_else(|error| error.kind() == std::io::ErrorKind::NotFound),
                Target::Secret { key, .. } => {
                    secrets.clear(key);
                    true
                }
            };
            (target.describe(), done)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::Scratch;

    fn paths_in(scratch: &Scratch) -> Paths {
        Paths {
            config: scratch.file("config/config.toml"),
            snapshot: scratch.file("state/snapshot.json"),
            log_dir: scratch.file("state"),
        }
    }

    #[test]
    fn the_plan_covers_every_place_the_client_writes() {
        let scratch = Scratch::new("plan");
        let targets = plan(
            &paths_in(&scratch),
            Some(PathBuf::from("/somewhere/monochrome")),
        );
        let described: Vec<String> = targets.iter().map(Target::describe).collect();
        let all = described.join("\n");

        assert!(all.contains("account token"));
        assert!(all.contains("gateway token"));
        assert!(all.contains("settings"));
        assert!(all.contains("library and logs"));
        assert!(all.contains("the program"));
    }

    #[test]
    fn the_settings_directory_is_removed_not_just_the_file() {
        let scratch = Scratch::new("settings");
        let targets = plan(&paths_in(&scratch), None);
        let settings = targets
            .iter()
            .find_map(|target| match target {
                Target::Directory { what, path } if what.trim() == "settings" => Some(path.clone()),
                _ => None,
            })
            .expect("settings directory is planned");
        assert!(settings.ends_with("config"));
    }

    #[test]
    fn a_plan_without_a_known_binary_leaves_it_out() {
        let scratch = Scratch::new("nobinary");
        let targets = plan(&paths_in(&scratch), None);
        assert!(
            !targets
                .iter()
                .any(|target| matches!(target, Target::File { .. })),
            "nothing should be removed when the program's own path is unknown"
        );
    }

    #[test]
    fn removing_something_that_is_already_gone_still_counts_as_done() {
        let scratch = Scratch::new("absent");
        let secrets = crate::secrets::Secrets::new(scratch.file("credentials"));
        let target = Target::Directory {
            what: "settings        ",
            path: scratch.file("never-created"),
        };
        let outcome = execute(&[target], &secrets);
        assert_eq!(outcome.len(), 1);
        assert!(outcome[0].1, "an absent directory is not a failure");
    }

    #[test]
    fn a_directory_and_its_contents_are_removed() {
        let scratch = Scratch::new("contents");
        let secrets = crate::secrets::Secrets::new(scratch.file("credentials"));
        let directory = scratch.file("state");
        std::fs::create_dir_all(&directory).expect("create");
        std::fs::write(directory.join("snapshot.json"), b"{}").expect("write");
        std::fs::write(directory.join("log"), b"lines").expect("write");

        let target = Target::Directory {
            what: "library and logs",
            path: directory.clone(),
        };
        let outcome = execute(&[target], &secrets);
        assert!(outcome[0].1);
        assert!(!directory.exists());
    }

    #[test]
    fn a_target_reports_whether_it_is_there() {
        let scratch = Scratch::new("exists");
        let missing = Target::File {
            what: "the program     ",
            path: scratch.file("nothing"),
        };
        assert!(!missing.exists());

        let present = scratch.file("something");
        std::fs::create_dir_all(scratch.dir()).expect("create");
        std::fs::write(&present, b"x").expect("write");
        assert!(
            Target::File {
                what: "the program     ",
                path: present
            }
            .exists()
        );

        assert!(
            Target::Secret {
                what: "account token   ",
                key: "session-token"
            }
            .exists(),
            "a keyring entry cannot be checked without reading it, so it is always attempted"
        );
    }
}
