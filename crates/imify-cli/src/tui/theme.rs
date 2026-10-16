//! Every style the live views use.

use ratatui::style::{Color, Modifier, Style};

/// The styles of the live views. Colors are the 16 named ANSI colors, so the terminal's palette
/// applies.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Theme {
    /// The phase of an operation and headings.
    pub accent: Style,
    /// Secondary detail.
    pub muted: Style,
    pub success: Style,
    pub warning: Style,
    pub error: Style,
    /// The completed part of a progress bar.
    pub bar_filled: Style,
    /// The remaining part of a progress bar.
    pub bar_empty: Style,
}

impl Theme {
    /// The theme with color, or with `color == false` the same modifiers without any foreground
    /// or background color.
    pub fn new(color: bool) -> Self {
        let theme = Self {
            accent: Style::new().fg(Color::Cyan).add_modifier(Modifier::BOLD),
            muted: Style::new().add_modifier(Modifier::DIM),
            success: Style::new().fg(Color::Green),
            warning: Style::new().fg(Color::Yellow),
            error: Style::new().fg(Color::Red).add_modifier(Modifier::BOLD),
            bar_filled: Style::new().fg(Color::Cyan),
            bar_empty: Style::new().add_modifier(Modifier::DIM),
        };
        if color {
            theme
        } else {
            theme.map(|style| Style {
                fg: None,
                bg: None,
                ..style
            })
        }
    }

    /// Every style of the theme.
    #[cfg(test)]
    pub fn styles(&self) -> [Style; 7] {
        [
            self.accent,
            self.muted,
            self.success,
            self.warning,
            self.error,
            self.bar_filled,
            self.bar_empty,
        ]
    }

    fn map(self, f: impl Fn(Style) -> Style) -> Self {
        Self {
            accent: f(self.accent),
            muted: f(self.muted),
            success: f(self.success),
            warning: f(self.warning),
            error: f(self.error),
            bar_filled: f(self.bar_filled),
            bar_empty: f(self.bar_empty),
        }
    }
}
