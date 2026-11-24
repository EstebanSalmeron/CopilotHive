//! Dark/light palettes and animation helpers.
use ratatui::style::{Color, Modifier, Style};

macro_rules! palette {
    ($($n:ident: $d:expr, $l:expr;)*) => {$(
        pub fn $n() -> Color {
            let (r, g, b): (u8, u8, u8) = if super::settings::light() { $l } else { $d };
            Color::Rgb(r, g, b)
        }
    )*};
}

// Dark: Tokyo Night. Light: Tokyo Night Day.
palette! {
    c_bg: (0x1a, 0x1b, 0x26), (0xe1, 0xe2, 0xe7);
    c_surface: (0x24, 0x28, 0x3b), (0xd0, 0xd5, 0xe3);
    c_border: (0x3b, 0x42, 0x61), (0xa8, 0xae, 0xcb);
    c_fg: (0xc0, 0xca, 0xf5), (0x37, 0x60, 0xbf);
    c_dim: (0x56, 0x5f, 0x89), (0x84, 0x8c, 0xb5);
    c_accent: (0x7a, 0xa2, 0xf7), (0x2e, 0x7d, 0xe9);
    c_running: (0x7d, 0xcf, 0xff), (0x00, 0x71, 0x97);
    c_ok: (0x9e, 0xce, 0x6a), (0x58, 0x75, 0x39);
    c_attention: (0xe0, 0xaf, 0x68), (0x8c, 0x6c, 0x3e);
    c_orange: (0xff, 0x9e, 0x64), (0xb1, 0x5c, 0x00);
    c_error: (0xf7, 0x76, 0x8e), (0xf5, 0x2a, 0x65);
    c_purple: (0xbb, 0x9a, 0xf7), (0x98, 0x54, 0xf1);
}

pub const SPINNER: [&str; 10] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];

/// Ticks are ~100ms: the spinner advances every tick, blinking toggles every 500ms.
pub fn spinner(tick: u64) -> &'static str {
    SPINNER[(tick % 10) as usize]
}

pub fn blink_on(tick: u64) -> bool {
    (tick / 5).is_multiple_of(2)
}

pub fn fg(c: Color) -> Style {
    Style::new().fg(c)
}

pub fn bold(c: Color) -> Style {
    fg(c).add_modifier(Modifier::BOLD)
}

/// Green → amber → red as a fraction fills up.
pub fn gauge_color(pct: u64) -> Color {
    match pct {
        0..=59 => c_ok(),
        60..=84 => c_attention(),
        _ => c_error(),
    }
}

/// `12345` → `12.3k`, `1_500_000` → `1.5M`.
pub fn human(n: u64) -> String {
    match n {
        0..=999 => n.to_string(),
        1_000..=999_999 => format!("{:.1}k", n as f64 / 1e3),
        _ => format!("{:.1}M", n as f64 / 1e6),
    }
}

/// AI credits (1 credit = $0.01 USD): `2.50 cr · $0.025`.
pub fn cost(credits: f64) -> String {
    let usd = credits * 0.01;
    let d = if usd < 1.0 { 3 } else { 2 };
    format!("{credits:.2} cr · ${usd:.d$}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_counts() {
        assert_eq!(cost(2.5), "2.50 cr · $0.025");
        assert_eq!(cost(250.0), "250.00 cr · $2.50");
        assert_eq!(human(999), "999");
        assert_eq!(human(12_345), "12.3k");
        assert_eq!(human(1_500_000), "1.5M");
        assert_eq!(gauge_color(10), c_ok());
        assert_eq!(gauge_color(70), c_attention());
        assert_eq!(gauge_color(95), c_error());
    }
}
