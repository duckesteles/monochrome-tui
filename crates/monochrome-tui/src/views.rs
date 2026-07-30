use crate::app::{App, Focus, LibrarySection, Row, Tab};
use crate::theme::Theme;
use monochrome_core::Repeat;
use ratatui::Frame;
use ratatui::layout::{Alignment, Constraint, Direction, Layout, Rect};
use ratatui::text::{Line, Span};
use ratatui::widgets::{List, ListItem, ListState, Paragraph};
use unicode_width::UnicodeWidthStr;

pub fn render(frame: &mut Frame, app: &App, theme: &Theme, list: &mut ListState) {
    let area = frame.area();
    if area.height < 8 || area.width < 24 {
        frame.render_widget(Paragraph::new("too small").style(theme.dim()), area);
        return;
    }

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Min(1),
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Length(1),
        ])
        .split(area);

    frame.render_widget(tabs(app, theme), chunks[0]);
    frame.render_widget(context_line(app, theme), chunks[1]);
    frame.render_widget(rule(theme, area.width), chunks[2]);

    match app.focus {
        _ if app.show_help => frame.render_widget(help(app, theme), chunks[3]),
        Focus::Login => frame.render_widget(login(app, theme), chunks[3]),
        Focus::Verification => frame.render_widget(verification(app, theme), chunks[3]),
        _ => render_list(frame, app, theme, list, chunks[3]),
    }

    frame.render_widget(rule(theme, area.width), chunks[4]);
    frame.render_widget(now_playing(app, theme, chunks[5].width), chunks[5]);
    frame.render_widget(progress(app, theme, chunks[6].width), chunks[6]);
    frame.render_widget(status(app, theme), chunks[7]);
}

fn tabs<'a>(app: &App, theme: &Theme) -> Paragraph<'a> {
    let mut spans = vec![Span::raw("  ")];
    for tab in Tab::ALL {
        let style = if tab == app.tab {
            theme.active()
        } else {
            theme.dim()
        };
        spans.push(Span::styled(tab.label().to_string(), style));
        spans.push(Span::raw("  "));
    }
    if app.tab == Tab::Library && app.stack.is_empty() {
        spans.push(Span::styled("   ", theme.dim()));
        for section in LibrarySection::ALL {
            let style = if section == app.section {
                theme.strong()
            } else {
                theme.dim()
            };
            spans.push(Span::styled(section.label().to_string(), style));
            spans.push(Span::raw(" "));
        }
    }
    Paragraph::new(Line::from(spans))
}

fn context_line<'a>(app: &App, theme: &Theme) -> Paragraph<'a> {
    if app.focus == Focus::SearchInput {
        return Paragraph::new(Line::from(vec![
            Span::raw("  "),
            Span::styled("search: ", theme.dim()),
            Span::styled(app.search_input.clone(), theme.base()),
            Span::styled("_", theme.strong()),
        ]));
    }
    let mut spans = vec![Span::raw("  "), Span::styled(app.breadcrumb(), theme.dim())];
    if let Some(user) = &app.user {
        spans.push(Span::styled(
            format!("   [{}]", user.display_name()),
            theme.dim(),
        ));
    }
    Paragraph::new(Line::from(spans))
}

fn rule<'a>(theme: &Theme, width: u16) -> Paragraph<'a> {
    let inner = width.saturating_sub(4).max(1) as usize;
    Paragraph::new(Line::from(vec![
        Span::raw("  "),
        Span::styled("\u{2500}".repeat(inner), theme.dim()),
    ]))
}

fn render_list(frame: &mut Frame, app: &App, theme: &Theme, state: &mut ListState, area: Rect) {
    let rows = app.rows();
    let width = area.width.saturating_sub(4) as usize;
    let playing = app.now.track.as_ref().map(|track| track.id);

    let items: Vec<ListItem> = rows
        .iter()
        .map(|row| {
            let line = line_for(row, theme, width, playing);
            if app.roomy_rows {
                ListItem::new(vec![line, Line::from("")])
            } else {
                ListItem::new(line)
            }
        })
        .collect();

    state.select(Some(app.cursor().min(rows.len().saturating_sub(1))));
    let list = List::new(items).highlight_style(theme.selected());
    frame.render_stateful_widget(list, area, state);
}

fn line_for<'a>(row: &Row, theme: &Theme, width: usize, playing: Option<u64>) -> Line<'a> {
    match row {
        Row::Section(label) => Line::from(vec![
            Span::raw("  "),
            Span::styled(label.clone(), theme.dim()),
        ]),
        Row::Empty(label) => Line::from(vec![
            Span::raw("  "),
            Span::styled(label.clone(), theme.dim()),
        ]),
        Row::Track(track) => {
            let marker = if playing == Some(track.id) {
                "\u{25b6} "
            } else {
                "  "
            };
            let right = if track.duration == 0 {
                String::new()
            } else {
                format_duration(track.duration as f64)
            };
            let title = track.display_title();
            let artist = track.artist_name().to_string();
            two_column(
                marker,
                &title,
                &artist,
                &right,
                width,
                theme,
                playing == Some(track.id),
            )
        }
        Row::Album(album) => {
            let right = album.year().unwrap_or("").to_string();
            two_column(
                "  ",
                &album.title,
                album.artist_name(),
                &right,
                width,
                theme,
                false,
            )
        }
        Row::Artist(artist) => two_column("  ", &artist.name, "", "", width, theme, false),
        Row::Playlist(playlist) => {
            let right = playlist
                .number_of_tracks
                .map(|count| format!("{count}"))
                .unwrap_or_default();
            two_column(
                "  ",
                &playlist.title,
                playlist.creator_name.as_deref().unwrap_or(""),
                &right,
                width,
                theme,
                false,
            )
        }
    }
}

fn two_column<'a>(
    marker: &str,
    primary: &str,
    secondary: &str,
    right: &str,
    width: usize,
    theme: &Theme,
    highlight: bool,
) -> Line<'a> {
    let right_width = right.width();
    let available = width
        .saturating_sub(marker.width())
        .saturating_sub(right_width)
        .saturating_sub(2);
    let secondary_budget = if secondary.is_empty() {
        0
    } else {
        (available / 3).min(secondary.width() + 3)
    };
    let primary_budget = available.saturating_sub(secondary_budget);

    let primary_text = fit(primary, primary_budget);
    let secondary_text = if secondary_budget == 0 {
        String::new()
    } else {
        format!(
            " \u{00b7} {}",
            fit(secondary, secondary_budget.saturating_sub(3))
        )
    };

    let used = marker.width() + primary_text.width() + secondary_text.width();
    let padding = width.saturating_sub(used).saturating_sub(right_width);

    Line::from(vec![
        Span::styled(marker.to_string(), theme.dim()),
        Span::styled(
            primary_text,
            if highlight {
                theme.playing()
            } else {
                theme.base()
            },
        ),
        Span::styled(secondary_text, theme.dim()),
        Span::raw(" ".repeat(padding)),
        Span::styled(right.to_string(), theme.dim()),
    ])
}

fn now_playing<'a>(app: &App, theme: &Theme, width: u16) -> Paragraph<'a> {
    let Some(track) = &app.now.track else {
        return Paragraph::new(Line::from(vec![
            Span::raw("   "),
            Span::styled("nothing playing", theme.dim()),
        ]));
    };

    let symbol = if app.now.loading {
        "\u{2026}"
    } else if app.now.paused {
        "\u{2016}"
    } else {
        "\u{25b6}"
    };
    let length = app
        .now
        .duration
        .filter(|duration| *duration > 0.0)
        .or_else(|| (track.duration > 0).then_some(track.duration as f64));
    let timing = match length {
        Some(length) => format!(
            "{} / {}",
            format_duration(app.now.position),
            format_duration(length)
        ),
        None => format_duration(app.now.position),
    };
    let title = format!("{} \u{00b7} {}", track.display_title(), track.artist_name());
    let inner = width.saturating_sub(6) as usize;
    let title = fit(&title, inner.saturating_sub(timing.width() + 2));
    let padding = inner.saturating_sub(title.width() + timing.width());

    Paragraph::new(Line::from(vec![
        Span::raw("   "),
        Span::styled(format!("{symbol}  "), theme.strong()),
        Span::styled(title, theme.base()),
        Span::raw(" ".repeat(padding)),
        Span::styled(timing, theme.dim()),
    ]))
}

fn progress<'a>(app: &App, theme: &Theme, width: u16) -> Paragraph<'a> {
    let inner = width.saturating_sub(6).max(1) as usize;
    let ratio = match (app.now.track.as_ref(), app.now.duration) {
        (Some(_), Some(duration)) if duration > 0.0 => {
            (app.now.position / duration).clamp(0.0, 1.0)
        }
        _ => 0.0,
    };
    let filled = (inner as f64 * ratio).round() as usize;
    Paragraph::new(Line::from(vec![
        Span::raw("      "),
        Span::styled("\u{2501}".repeat(filled), theme.strong()),
        Span::styled("\u{2500}".repeat(inner.saturating_sub(filled)), theme.dim()),
    ]))
}

fn status<'a>(app: &App, theme: &Theme) -> Paragraph<'a> {
    if let Some(message) = &app.status {
        return Paragraph::new(Line::from(vec![
            Span::raw("   "),
            Span::styled(message.clone(), theme.base()),
        ]));
    }

    let mut parts = Vec::new();
    if let Some(format) = &app.now.format {
        parts.push(format.clone());
    }
    if let Some(source) = &app.now.source {
        parts.push(source.clone());
    }
    parts.push(app.quality.label().to_string());
    if app.queue.shuffle() {
        parts.push("shuffle".into());
    }
    if app.repeat() != Repeat::Off {
        parts.push(app.repeat().label().to_string());
    }
    if app.syncing {
        parts.push("syncing".into());
    }

    Paragraph::new(Line::from(vec![
        Span::raw("   "),
        Span::styled(parts.join("   "), theme.dim()),
        Span::styled(
            format!("   vol {}%", (app.volume * 100.0).round()),
            theme.dim(),
        ),
        Span::styled("   ? keys", theme.dim()),
    ]))
}

fn help<'a>(app: &App, theme: &Theme) -> Paragraph<'a> {
    let mut lines = vec![Line::from("")];
    for (group, entries) in crate::help::SHORTCUTS {
        lines.push(Line::from(Span::styled(
            format!("   {group}"),
            theme.strong(),
        )));
        for (keys, what) in *entries {
            lines.push(Line::from(vec![
                Span::styled(format!("     {keys:<18}"), theme.base()),
                Span::styled((*what).to_string(), theme.dim()),
            ]));
        }
        lines.push(Line::from(""));
    }
    lines.push(Line::from(Span::styled(
        "   arrows scroll, esc or ? closes",
        theme.dim(),
    )));
    Paragraph::new(lines).scroll((app.help_scroll, 0))
}

fn login<'a>(app: &App, theme: &Theme) -> Paragraph<'a> {
    let masked = "*".repeat(app.login.password.chars().count());
    let cursor = |field| {
        if app.login.field == field { "_" } else { "" }
    };
    let lines = vec![
        Line::from(""),
        Line::from(Span::styled("   sign in to monochrome.tf", theme.strong())),
        Line::from(""),
        Line::from(vec![
            Span::styled("   email     ", theme.dim()),
            Span::styled(app.login.email.clone(), theme.base()),
            Span::styled(cursor(crate::app::LoginField::Email), theme.strong()),
        ]),
        Line::from(vec![
            Span::styled("   password  ", theme.dim()),
            Span::styled(masked, theme.base()),
            Span::styled(cursor(crate::app::LoginField::Password), theme.strong()),
        ]),
        Line::from(""),
        Line::from(Span::styled(
            if app.login.submitting {
                "   signing in"
            } else {
                "   tab switches field, enter signs in"
            },
            theme.dim(),
        )),
    ];
    Paragraph::new(lines).alignment(Alignment::Left)
}

fn verification<'a>(app: &App, theme: &Theme) -> Paragraph<'a> {
    let mut lines = vec![
        Line::from(""),
        Line::from(Span::styled(
            "   the amazon gateway needs a browser check",
            theme.strong(),
        )),
        Line::from(""),
    ];

    let url = app.verification_url.clone().unwrap_or_default();
    if let Some(reason) = &app.verification_error {
        lines.push(Line::from(Span::styled(
            format!("   {reason}"),
            theme.base(),
        )));
        lines.push(Line::from(""));
    }
    lines.push(Line::from(Span::styled(
        "   a browser tab is waiting for you at",
        theme.dim(),
    )));
    lines.push(Line::from(Span::styled(format!("   {url}"), theme.base())));

    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        "   nothing else is needed. if you already hold a token, it goes here",
        theme.dim(),
    )));
    let shown = if app.verification_input.chars().count() > 16 {
        format!(
            "{}\u{2026} ({} characters)",
            app.verification_input.chars().take(16).collect::<String>(),
            app.verification_input.chars().count()
        )
    } else {
        app.verification_input.clone()
    };
    lines.push(Line::from(vec![
        Span::styled("   token  ", theme.dim()),
        Span::styled(shown, theme.base()),
        Span::styled("_", theme.strong()),
    ]));
    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        "   esc goes back, the token is stored in your keyring",
        theme.dim(),
    )));

    Paragraph::new(lines)
}

pub fn fit(text: &str, width: usize) -> String {
    if width == 0 {
        return String::new();
    }
    if text.width() <= width {
        return text.to_string();
    }
    let mut out = String::new();
    let mut used = 0;
    for character in text.chars() {
        let size = character.to_string().width();
        if used + size > width.saturating_sub(1) {
            break;
        }
        out.push(character);
        used += size;
    }
    out.push('\u{2026}');
    out
}

pub fn format_duration(seconds: f64) -> String {
    if !seconds.is_finite() || seconds < 0.0 {
        return "0:00".into();
    }
    let total = seconds.round() as u64;
    let hours = total / 3600;
    let minutes = (total % 3600) / 60;
    let secs = total % 60;
    if hours > 0 {
        format!("{hours}:{minutes:02}:{secs:02}")
    } else {
        format!("{minutes}:{secs:02}")
    }
}
