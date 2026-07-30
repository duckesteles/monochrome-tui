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
            ("/", "search"),
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
    ("library", &[("f", "save or unsave"), ("a", "add to queue")]),
    ("leave", &[("?", "close this"), ("Q ctrl+c", "quit")]),
];
