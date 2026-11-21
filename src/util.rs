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

/// Normalise 32-bit BGRA from a shell/GDI bitmap to premultiplied alpha:
/// * no alpha channel at all (every alpha 0) -> fully opaque;
/// * straight alpha (some colour channel exceeds its alpha) -> premultiply;
/// * otherwise it is already premultiplied and left alone.
pub fn normalize_to_premultiplied(px: &mut [u8]) {
    if all_alpha_zero(px) {
        for p in px.chunks_exact_mut(4) {
            p[3] = 255;
        }
        return;
    }
    let straight = px.chunks_exact(4).any(|p| p[0] > p[3] || p[1] > p[3] || p[2] > p[3]);
    if straight {
        premultiply_bgra(px);
    }
}

/// Dominant tint of premultiplied BGRA pixels for the media card background:
/// a saturation-weighted average that ignores near-transparent pixels, so a
/// colourful cover gives its main colour rather than muddy grey.
/// Returns 0xRRGGBB, or `None` if nothing is opaque enough.
pub fn dominant_tint(px: &[u8]) -> Option<u32> {
    let (mut r, mut g, mut b, mut wsum) = (0f64, 0f64, 0f64, 0f64);
    for p in px.chunks_exact(4) {
        let a = p[3] as f64;
        if a < 128.0 {
            continue;
        }
        let (pb, pg, pr) = (p[0] as f64 * 255.0 / a, p[1] as f64 * 255.0 / a, p[2] as f64 * 255.0 / a);
        let max = pr.max(pg).max(pb);
        let min = pr.min(pg).min(pb);
        let sat = if max > 0.0 { (max - min) / max } else { 0.0 };
        let w = 0.05 + sat * sat;
        r += pr * w;
        g += pg * w;
        b += pb * w;
        wsum += w;
    }
    if wsum == 0.0 {
        return None;
    }
    let c = |v: f64| ((v / wsum).round().clamp(0.0, 255.0)) as u32;
    Some((c(r) << 16) | (c(g) << 8) | c(b))
}

/// Scale an 0xRRGGBB colour's brightness (for readable dark card backgrounds).
pub fn darken(rgb: u32, factor: f32) -> u32 {
    let f = |shift: u32| ((((rgb >> shift) & 0xFF) as f32 * factor).round().clamp(0.0, 255.0)) as u32;
    (f(16) << 16) | (f(8) << 8) | f(0)
}

/// Is `name` (path relative to the watched folder) one we reload on?
pub fn is_relevant(name: &str) -> bool {
    let n = name.to_ascii_lowercase().replace('/', "\\");
    if n == "config.toml" {
        return true;
    }
    match n.strip_prefix("themes\\") {
        Some(rest) => rest.ends_with(".toml") && !rest.contains('\\'),
        None => false,
    }
}

/// Walk a FILE_NOTIFY_INFORMATION chain in `buf[..len]`. Returns true if any
/// record names a relevant file, or if the chain is malformed.
pub fn buffer_has_relevant(buf: &[u8], len: usize) -> bool {
    let len = len.min(buf.len());
    if len == 0 {
        return true; // overflow: the system dropped details, so reload to be safe
    }
    let mut off = 0usize;
    loop {
        // Header: NextEntryOffset, Action, FileNameLength (3 x u32).
        let Some(hdr) = buf.get(off..off + 12) else { return true };
        if off + 12 > len {
            return true;
        }
        let next = u32::from_le_bytes(hdr[0..4].try_into().unwrap()) as usize;
        let name_len = u32::from_le_bytes(hdr[8..12].try_into().unwrap()) as usize;
        let start = off + 12;
        let Some(end) = start.checked_add(name_len) else { return true };
        if end > len || !name_len.is_multiple_of(2) {
            return true;
        }
        let units: Vec<u16> = buf[start..end].chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect();
        if is_relevant(&String::from_utf16_lossy(&units)) {
            return true;
        }
        if next == 0 {
            return false;
        }
        match off.checked_add(next) {
            Some(n) if n > off && n < len => off = n,
            _ => return true,
        }
    }
}

/// The Run command for `exe`: the path quoted (so spaces cannot split it) plus `--autostart`.
/// Returns None for paths containing a quote, which cannot be quoted safely.
pub fn run_command(exe: &str) -> Option<String> {
    if exe.contains('"') || exe.is_empty() {
        return None;
    }
    Some(format!("\"{exe}\" --autostart"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record(next: u32, name: &str) -> Vec<u8> {
        let units: Vec<u16> = name.encode_utf16().collect();
        let mut v = Vec::new();
        v.extend_from_slice(&next.to_le_bytes());
        v.extend_from_slice(&3u32.to_le_bytes()); // FILE_ACTION_MODIFIED
        v.extend_from_slice(&((units.len() * 2) as u32).to_le_bytes());
        for u in units {
            v.extend_from_slice(&u.to_le_bytes());
        }
        while v.len() % 4 != 0 {
            v.push(0);
        }
        v
    }

    #[test]
    fn relevant_names() {
        assert!(is_relevant("config.toml"));
        assert!(is_relevant("CONFIG.TOML"));
        assert!(is_relevant("themes\\dark_glass.toml"));
        assert!(!is_relevant("winshell.log"));
        assert!(!is_relevant("taskbar_state.toml"));
        assert!(!is_relevant("themes\\sub\\x.toml"));
        assert!(!is_relevant("cache\\icons\\a.toml"));
    }

    #[test]
    fn notify_chain_irrelevant_only() {
        let a = record(0, "winshell.log");
        assert!(!buffer_has_relevant(&a, a.len()));
    }

    #[test]
    fn notify_chain_finds_second_record() {
        let mut a = record(0, "winshell.log");
        let first_len = a.len() as u32;
        a[0..4].copy_from_slice(&first_len.to_le_bytes());
        a.extend(record(0, "config.toml"));
        assert!(buffer_has_relevant(&a, a.len()));
    }

    #[test]
    fn notify_overflow_and_malformed_are_treated_as_changes() {
        let a = record(0, "winshell.log");
        assert!(buffer_has_relevant(&a, 0)); // overflow
        assert!(buffer_has_relevant(&a, 8)); // truncated header
        let mut bad_len = a.clone();
        bad_len[8..12].copy_from_slice(&10_000u32.to_le_bytes());
        assert!(buffer_has_relevant(&bad_len, bad_len.len()));
        let mut loop_back = a.clone();
        loop_back[0..4].copy_from_slice(&0u32.wrapping_sub(4).to_le_bytes()); // huge offset
        assert!(buffer_has_relevant(&loop_back, loop_back.len()));
        let mut odd = a.clone();
        odd[8..12].copy_from_slice(&3u32.to_le_bytes());
        assert!(buffer_has_relevant(&odd, odd.len()));
    }

    #[test]
    fn notify_parser_never_reads_past_len() {
        // Exhaustively feed every prefix length of a two-record chain: no panic.
        let mut a = record(0, "winshell.log");
        let n = a.len() as u32;
        a[0..4].copy_from_slice(&n.to_le_bytes());
        a.extend(record(0, "crash.log"));
        for len in 0..=a.len() + 8 {
            let _ = buffer_has_relevant(&a, len);
        }
    }

    #[test]
    fn run_command_quotes_path() {
        assert_eq!(
            run_command("C:\\Users\\A B\\winshell.exe").unwrap(),
            "\"C:\\Users\\A B\\winshell.exe\" --autostart"
        );
        assert_eq!(run_command("C:\\x\"y.exe"), None);
        assert_eq!(run_command(""), None);
    }

    #[test]
    fn tint_prefers_saturated_colour() {
        // 3 grey pixels and 1 strong red pixel (BGRA, opaque).
        let px = [128, 128, 128, 255, 128, 128, 128, 255, 128, 128, 128, 255, 0, 0, 255, 255];
        let t = dominant_tint(&px).unwrap();
        assert!((t >> 16) & 0xFF > 200, "red channel dominates: {t:06x}");
    }

    #[test]
    fn tint_ignores_transparent() {
        assert_eq!(dominant_tint(&[0, 0, 0, 0, 10, 10, 10, 20]), None);
    }

    #[test]
    fn darken_scales_channels() {
        assert_eq!(darken(0x804020, 0.5), 0x402010);
    }

    #[test]
    fn normalize_opaque_when_no_alpha() {
        let mut px = [10, 20, 30, 0, 1, 2, 3, 0];
        normalize_to_premultiplied(&mut px);
        assert_eq!(px, [10, 20, 30, 255, 1, 2, 3, 255]);
    }

    #[test]
    fn normalize_premultiplies_straight_alpha() {
        let mut px = [200, 100, 50, 128, 0, 0, 0, 255];
        normalize_to_premultiplied(&mut px);
        assert_eq!(px, [100, 50, 25, 128, 0, 0, 0, 255]);
    }

    #[test]
    fn normalize_leaves_premultiplied_alone() {
        let mut px = [100, 50, 25, 128];
        normalize_to_premultiplied(&mut px);
        assert_eq!(px, [100, 50, 25, 128]);
    }

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
