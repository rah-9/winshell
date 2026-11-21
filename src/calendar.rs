//! Month grid maths for the calendar flyout (pure, Gregorian).

pub fn is_leap(year: i32) -> bool {
    (year % 4 == 0 && year % 100 != 0) || year % 400 == 0
}

pub fn days_in_month(year: i32, month: u32) -> u32 {
    match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if is_leap(year) => 29,
        2 => 28,
        _ => 0,
    }
}

/// 0 = Sunday .. 6 = Saturday (Sakamoto's method).
pub fn weekday(year: i32, month: u32, day: u32) -> u32 {
    const T: [i32; 12] = [0, 3, 2, 5, 0, 3, 5, 1, 4, 6, 2, 4];
    let y = if month < 3 { year - 1 } else { year };
    ((y + y / 4 - y / 100 + y / 400 + T[(month - 1) as usize] + day as i32).rem_euclid(7)) as u32
}

pub fn add_months(year: i32, month: u32, delta: i32) -> (i32, u32) {
    let idx = year * 12 + (month as i32 - 1) + delta;
    (idx.div_euclid(12), (idx.rem_euclid(12) + 1) as u32)
}

/// 6 x 7 grid of (year, month, day, in_this_month), weeks starting on
/// `first_weekday` (0 = Sunday, 1 = Monday, ...).
pub fn month_grid(year: i32, month: u32, first_weekday: u32) -> Vec<(i32, u32, u32, bool)> {
    let lead = (weekday(year, month, 1) + 7 - first_weekday % 7) % 7;
    let (py, pm) = add_months(year, month, -1);
    let (ny, nm) = add_months(year, month, 1);
    let prev_days = days_in_month(py, pm);
    let days = days_in_month(year, month);
    (0..42)
        .map(|i| {
            let i = i as i64 - lead as i64;
            if i < 0 {
                (py, pm, (prev_days as i64 + i + 1) as u32, false)
            } else if i < days as i64 {
                (year, month, (i + 1) as u32, true)
            } else {
                (ny, nm, (i - days as i64 + 1) as u32, false)
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_weekdays() {
        assert_eq!(weekday(2026, 11, 20), 5); // Friday
        assert_eq!(weekday(2000, 1, 1), 6); // Saturday
        assert_eq!(weekday(2024, 2, 29), 4); // Thursday
    }

    #[test]
    fn month_lengths() {
        assert_eq!(days_in_month(2024, 2), 29);
        assert_eq!(days_in_month(1900, 2), 28);
        assert_eq!(days_in_month(2000, 2), 29);
        assert_eq!(days_in_month(2026, 11), 30);
    }

    #[test]
    fn month_arithmetic_wraps_years() {
        assert_eq!(add_months(2026, 1, -1), (2025, 12));
        assert_eq!(add_months(2026, 12, 1), (2027, 1));
        assert_eq!(add_months(2026, 11, -23), (2024, 12));
    }

    #[test]
    fn grid_starts_on_requested_weekday() {
        // November 2026 starts on a Sunday.
        let g = month_grid(2026, 11, 0);
        assert_eq!(g[0], (2026, 11, 1, true));
        let g = month_grid(2026, 11, 1); // Monday-first: leading 31 Oct
        assert_eq!(g[0], (2026, 10, 26, false));
        assert_eq!(g[6], (2026, 11, 1, true));
        assert_eq!(g.len(), 42);
        assert_eq!(g.iter().filter(|c| c.3).count(), 30);
    }
}
