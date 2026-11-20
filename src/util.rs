//! Platform-independent helpers (unit-tested on any host).

/// Slack added after the minute boundary so the timer never fires a few
/// milliseconds early and redraws the old minute (timer resolution is ~15.6 ms).
pub const MINUTE_SLACK_MS: u32 = 20;

/// Milliseconds from `second:millisecond` within the current minute to just
/// past the next minute boundary.
pub fn ms_to_next_minute(second: u16, millisecond: u16) -> u32 {
    let into = (second.min(59) as u32) * 1000 + millisecond.min(999) as u32;
    60_000 - into + MINUTE_SLACK_MS
}

/// Truncate to at most `max` characters, appending "…" when cut (PLAN.md
/// section 2: active window title truncated at ~40 chars). Trailing
/// whitespace before the ellipsis is dropped.
pub fn truncate_chars(s: &str, max: usize) -> String {
    let s = s.trim();
    if s.chars().count() <= max {
        return s.to_string();
    }
    let cut: String = s.chars().take(max.saturating_sub(1)).collect();
    format!("{}…", cut.trim_end())
}

/// Straight-alpha BGRA -> premultiplied BGRA, in place.
pub fn premultiply_bgra(px: &mut [u8]) {
    for p in px.chunks_exact_mut(4) {
        let a = p[3] as u32;
        if a == 255 {
            continue;
        }
        for c in &mut p[..3] {
            *c = ((*c as u32 * a + 127) / 255) as u8;
        }
    }
}

/// True if every pixel has alpha 0 (an icon without an alpha channel, whose
/// transparency lives only in its AND mask).
pub fn all_alpha_zero(px: &[u8]) -> bool {
    px.chunks_exact(4).all(|p| p[3] == 0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn next_minute_from_start_of_minute() {
        assert_eq!(ms_to_next_minute(0, 0), 60_000 + MINUTE_SLACK_MS);
    }

    #[test]
    fn next_minute_from_end_of_minute() {
        assert_eq!(ms_to_next_minute(59, 999), 1 + MINUTE_SLACK_MS);
    }

    #[test]
    fn next_minute_clamps_leap_second() {
        assert_eq!(ms_to_next_minute(60, 500), ms_to_next_minute(59, 500));
    }

    #[test]
    fn next_minute_mid_minute() {
        assert_eq!(ms_to_next_minute(30, 250), 29_750 + MINUTE_SLACK_MS);
    }

    #[test]
    fn truncate_short_unchanged() {
        assert_eq!(truncate_chars("  Notepad  ", 40), "Notepad");
    }

    #[test]
    fn truncate_long_adds_ellipsis() {
        let t = truncate_chars("abcdefghij", 5);
        assert_eq!(t, "abcd…");
        assert_eq!(t.chars().count(), 5);
    }

    #[test]
    fn truncate_counts_chars_not_bytes() {
        let t = truncate_chars("ééééééé", 4);
        assert_eq!(t, "ééé…");
    }

    #[test]
    fn truncate_drops_space_before_ellipsis() {
        assert_eq!(truncate_chars("abc defgh", 5), "abc…");
    }

    #[test]
    fn premultiply_halves_at_half_alpha() {
        let mut px = [200, 100, 50, 128];
        premultiply_bgra(&mut px);
        assert_eq!(px, [100, 50, 25, 128]);
    }

    #[test]
    fn premultiply_keeps_opaque() {
        let mut px = [1, 2, 3, 255];
        premultiply_bgra(&mut px);
        assert_eq!(px, [1, 2, 3, 255]);
    }

    #[test]
    fn alpha_zero_detection() {
        assert!(all_alpha_zero(&[9, 9, 9, 0, 1, 1, 1, 0]));
        assert!(!all_alpha_zero(&[9, 9, 9, 0, 1, 1, 1, 1]));
    }
}
