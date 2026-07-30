use ratatui::style::{Color, Modifier, Style};

#[derive(Debug, Clone, Copy, Default)]
pub struct Theme {
    accent: Option<Color>,
}

impl Theme {
    pub fn new(accent: &str) -> Self {
        Self {
            accent: parse_color(accent),
        }
    }

    pub fn base(&self) -> Style {
        Style::default()
    }

    pub fn dim(&self) -> Style {
        Style::default().add_modifier(Modifier::DIM)
    }

    pub fn strong(&self) -> Style {
        Style::default().add_modifier(Modifier::BOLD)
    }

    pub fn selected(&self) -> Style {
        Style::default().add_modifier(Modifier::REVERSED)
    }

    pub fn active(&self) -> Style {
        match self.accent {
            Some(color) => Style::default().fg(color).add_modifier(Modifier::BOLD),
            None => Style::default().add_modifier(Modifier::BOLD),
        }
    }

    pub fn playing(&self) -> Style {
        self.active()
    }
}

fn parse_color(value: &str) -> Option<Color> {
    let value = value.trim();
    if value.is_empty() {
        return None;
    }
    if let Some(hex) = value.strip_prefix('#') {
        if hex.len() == 6
            && let Ok(rgb) = u32::from_str_radix(hex, 16)
        {
            return Some(Color::Rgb(
                ((rgb >> 16) & 0xff) as u8,
                ((rgb >> 8) & 0xff) as u8,
                (rgb & 0xff) as u8,
            ));
        }
        return None;
    }
    match value.to_ascii_lowercase().as_str() {
        "red" => Some(Color::Red),
        "green" => Some(Color::Green),
        "yellow" => Some(Color::Yellow),
        "blue" => Some(Color::Blue),
        "magenta" => Some(Color::Magenta),
        "cyan" => Some(Color::Cyan),
        "white" => Some(Color::White),
        "gray" | "grey" => Some(Color::Gray),
        other => other.parse::<u8>().ok().map(Color::Indexed),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_style_ever_sets_a_background() {
        let theme = Theme::new("#ff0000");
        for style in [
            theme.base(),
            theme.dim(),
            theme.strong(),
            theme.selected(),
            theme.active(),
            theme.playing(),
        ] {
            assert_eq!(
                style.bg, None,
                "a background would break terminal transparency"
            );
        }
    }

    #[test]
    fn the_default_theme_sets_no_foreground_either() {
        let theme = Theme::default();
        assert_eq!(theme.base().fg, None);
        assert_eq!(theme.active().fg, None);
        assert_eq!(theme.selected().fg, None);
    }

    #[test]
    fn selection_uses_reverse_video_so_it_works_on_any_theme() {
        assert!(
            Theme::default()
                .selected()
                .add_modifier
                .contains(Modifier::REVERSED)
        );
    }

    #[test]
    fn an_accent_colour_is_applied_only_to_the_active_style() {
        let theme = Theme::new("cyan");
        assert_eq!(theme.active().fg, Some(Color::Cyan));
        assert_eq!(theme.base().fg, None);
        assert_eq!(theme.dim().fg, None);
    }

    #[test]
    fn hex_accents_are_understood() {
        assert_eq!(parse_color("#1e90ff"), Some(Color::Rgb(0x1e, 0x90, 0xff)));
    }

    #[test]
    fn indexed_accents_are_understood() {
        assert_eq!(parse_color("208"), Some(Color::Indexed(208)));
    }

    #[test]
    fn nonsense_accents_are_ignored_rather_than_failing() {
        assert_eq!(parse_color("not-a-colour"), None);
        assert_eq!(parse_color("#12345"), None);
        assert_eq!(parse_color(""), None);
    }
}
