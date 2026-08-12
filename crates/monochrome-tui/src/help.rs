pub const SHORTCUTS: &[(&str, &[(&str, &str)])] = &[
    (
        "move",
        &[
            ("j k / arrows", "up and down"),
            ("g G", "first and last"),
            ("ctrl+d ctrl+u", "half a page"),
            ("enter", "play or open"),
            ("esc", "back"),
        ],
    ),
    (
        "go to",
        &[
            ("tab / 1-5", "switch tab"),
            ("h l", "library section"),
            ("/", "search the catalogue, or filter the list you are on"),
            ("q", "queue"),
        ],
    ),
    (
        "play",
        &[
            ("space", "pause"),
            ("left right", "seek 10s"),
            ("shift+left/right", "previous, next"),
            ("+ -", "volume"),
            ("m s r", "mute, shuffle, repeat"),
        ],
    ),
    (
        "library",
        &[
            ("f", "save or unsave"),
            ("a", "add to queue"),
            ("R", "queue more tracks like this one"),
        ],
    ),
    (
        "the bottom line",
        &[
            ("flac amazon", "how this track arrived"),
            ("syncing", "sending your library to your account"),
            (
                "changes waiting",
                "saved here, not sent yet, it keeps trying",
            ),
        ],
    ),
    (
        "leave",
        &[
            ("?", "close this"),
            ("X", "sign out, press twice"),
            ("Q ctrl+c", "quit"),
        ],
    ),
];

pub fn line_count() -> usize {
    SHORTCUTS
        .iter()
        .map(|(_, entries)| entries.len() + 2)
        .sum::<usize>()
        + 1
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_group_documents_at_least_one_key() {
        assert!(!SHORTCUTS.is_empty());
        for (group, entries) in SHORTCUTS {
            assert!(!group.is_empty());
            assert!(!entries.is_empty(), "{group} documents nothing");
        }
    }

    #[test]
    fn every_mark_the_bottom_line_can_show_is_explained_here() {
        let explained: Vec<&str> = SHORTCUTS
            .iter()
            .flat_map(|(_, entries)| entries.iter().map(|(keys, _)| *keys))
            .collect();
        for mark in ["syncing", "changes waiting"] {
            assert!(
                explained.contains(&mark),
                "the line can show {mark:?} with nothing to tell the reader what it means"
            );
        }
    }

    #[test]
    fn no_shortcut_is_listed_without_an_explanation() {
        for (_, entries) in SHORTCUTS {
            for (keys, what) in *entries {
                assert!(!keys.is_empty());
                assert!(!what.is_empty(), "{keys} has no description");
            }
        }
    }

    #[test]
    fn the_line_count_matches_what_is_rendered() {
        let rendered: usize = SHORTCUTS
            .iter()
            .map(|(_, entries)| entries.len() + 2)
            .sum::<usize>()
            + 1;
        assert_eq!(line_count(), rendered);
    }

    #[test]
    fn shuffle_is_documented_because_people_look_for_it() {
        let all: String = SHORTCUTS
            .iter()
            .flat_map(|(_, entries)| entries.iter().map(|(_, what)| *what))
            .collect::<Vec<_>>()
            .join(" ");
        assert!(all.contains("shuffle"));
        assert!(all.contains("repeat"));
    }
}
