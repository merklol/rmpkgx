//! Codex-inspired dark palette. Text labels remain meaningful without color.
use ratatui::{
    prelude::*,
    widgets::{Block, Borders},
};

pub const BACKGROUND: Color = Color::Rgb(24, 24, 27);
pub const PANEL: Color = Color::Rgb(39, 39, 43);
pub const TEXT: Color = Color::Rgb(237, 237, 239);
pub const MUTED: Color = Color::Rgb(161, 161, 170);
pub const BORDER: Color = Color::Rgb(82, 82, 91);
pub const ACCENT: Color = Color::Rgb(167, 139, 250);
pub const GREEN: Color = Color::Rgb(134, 219, 172);
pub const AMBER: Color = Color::Rgb(240, 199, 125);
pub const RED: Color = Color::Rgb(248, 145, 145);
pub const BLUE: Color = Color::Rgb(147, 197, 253);

pub fn section(title: String) -> Block<'static> {
    Block::default()
        .borders(Borders::TOP)
        .border_style(Style::default().fg(BORDER))
        .title(Span::styled(title, Style::default().fg(ACCENT).bold()))
}
