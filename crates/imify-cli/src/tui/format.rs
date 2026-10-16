//! Text forms of sizes, rates and durations, and middle truncation of paths.

use std::time::Duration;

/// `n` bytes in binary units with one decimal: `0 B`, `1023 B`, `1.0 KiB`, `1.4 GiB`.
pub fn bytes(n: u64) -> String {
    #[expect(
        clippy::cast_precision_loss,
        reason = "one decimal is shown; sizes above 2^53 bytes lose nothing visible"
    )]
    let n = n as f64;
    scaled(n, "")
}

/// A transfer rate: `18.3 MiB/s`.
pub fn rate(bytes_per_second: f64) -> String {
    scaled(bytes_per_second.max(0.0), "/s")
}

/// `value` bytes in the largest binary unit that keeps it at or above 1, then `suffix`.
fn scaled(value: f64, suffix: &str) -> String {
    const UNITS: [&str; 5] = ["KiB", "MiB", "GiB", "TiB", "PiB"];
    if value < 1024.0 {
        return format!("{value:.0} B{suffix}");
    }
    let mut value = value / 1024.0;
    let mut unit = UNITS[0];
    for next in &UNITS[1..] {
        if value < 1024.0 {
            break;
        }
        value /= 1024.0;
        unit = next;
    }
    format!("{value:.1} {unit}{suffix}")
}

/// A duration in its two largest units, rounded down to whole seconds: `45s`, `1m 12s`,
/// `2h 03m`.
pub fn duration(d: Duration) -> String {
    let seconds = d.as_secs();
    match (seconds / 3600, seconds / 60 % 60, seconds % 60) {
        (0, 0, s) => format!("{s}s"),
        (0, m, s) => format!("{m}m {s:02}s"),
        (h, m, _) => format!("{h}h {m:02}m"),
    }
}

/// `text` shortened to at most `width` characters by replacing its middle with `…`. Text that
/// fits is returned unchanged.
pub fn truncate_middle(text: &str, width: usize) -> String {
    let length = text.chars().count();
    if length <= width {
        return text.to_owned();
    }
    let Some(kept) = width.checked_sub(1) else {
        return String::new();
    };
    let head = kept / 2;
    let tail = kept - head;
    let mut truncated: String = text.chars().take(head).collect();
    truncated.push('…');
    truncated.extend(text.chars().skip(length - tail));
    truncated
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bytes_below_one_kib_are_whole_bytes() {
        assert_eq!([bytes(0), bytes(1023)], ["0 B", "1023 B"]);
    }

    #[test]
    fn bytes_use_binary_units_with_one_decimal() {
        assert_eq!(
            [
                bytes(1024),
                bytes(1_503_238_553),
                bytes(300 * 1024 * 1024),
                bytes(3 * 1024_u64.pow(4))
            ],
            ["1.0 KiB", "1.4 GiB", "300.0 MiB", "3.0 TiB"]
        );
    }

    #[test]
    fn rate_is_a_size_per_second() {
        assert_eq!(
            [rate(0.0), rate(18.3 * 1024.0 * 1024.0)],
            ["0 B/s", "18.3 MiB/s"]
        );
    }

    #[test]
    fn duration_shows_the_two_largest_units() {
        assert_eq!(
            [
                duration(Duration::from_millis(45_900)),
                duration(Duration::from_secs(72)),
                duration(Duration::from_secs(2 * 3600 + 3 * 60 + 59)),
            ],
            ["45s", "1m 12s", "2h 03m"]
        );
    }

    #[test]
    fn truncate_middle_returns_text_that_fits_unchanged() {
        assert_eq!(truncate_middle("meta/info.json", 14), "meta/info.json");
    }

    #[test]
    fn truncate_middle_keeps_the_start_and_the_end() {
        assert_eq!(
            truncate_middle("videos/observation.images.front/episode_000002.mp4", 27),
            "videos/observ…de_000002.mp4"
        );
    }

    #[test]
    fn truncate_middle_never_returns_more_than_width_characters() {
        let text = "data/chunk-000/file-001.parquet";

        for width in 0..40 {
            assert!(
                truncate_middle(text, width).chars().count() <= width,
                "{width}"
            );
        }
    }
}
