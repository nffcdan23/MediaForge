//! Display formatting used by the backend itself (notifications). The UI has its own copy in
//! `src/lib/format.ts`, kept to the same rules.

/// File sizes in decimal units, as Finder and Explorer's "size on disk" columns show them: "203 KB", "4.4 MB".
pub fn bytes(count: u64) -> String {
    const UNITS: [&str; 4] = ["KB", "MB", "GB", "TB"];
    if count < 1000 {
        return if count == 1 { "1 byte".into() } else { format!("{count} bytes") };
    }
    let mut value = count as f64 / 1000.0;
    let mut unit = 0;
    while value >= 999.95 && unit < UNITS.len() - 1 {
        value /= 1000.0;
        unit += 1;
    }
    let decimals = match unit {
        0 => 0,
        1 => 1,
        _ => 2,
    };
    let mut text = format!("{value:.decimals$}");
    if text.contains('.') {
        text = text.trim_end_matches('0').trim_end_matches('.').to_string();
    }
    format!("{text} {}", UNITS[unit])
}

/// `3:07` or `1:02:45`.
pub fn duration(seconds: f64) -> String {
    if !seconds.is_finite() || seconds < 0.0 {
        return "–:––".into();
    }
    let total = seconds.min(359_999.0).round() as u64;
    let (hours, minutes, secs) = (total / 3600, (total % 3600) / 60, total % 60);
    if hours > 0 { format!("{hours}:{minutes:02}:{secs:02}") } else { format!("{minutes}:{secs:02}") }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sizes() {
        assert_eq!(bytes(0), "0 bytes");
        assert_eq!(bytes(1), "1 byte");
        assert_eq!(bytes(203_400), "203 KB");
        assert_eq!(bytes(1_000_000), "1 MB");
        assert_eq!(bytes(4_400_000), "4.4 MB");
        assert_eq!(bytes(216_049_000), "216 MB");
        assert_eq!(bytes(2_345_000_000), "2.35 GB");
        assert_eq!(bytes(999_999), "1 MB");
    }

    #[test]
    fn durations() {
        assert_eq!(duration(0.0), "0:00");
        assert_eq!(duration(187.0), "3:07");
        assert_eq!(duration(3765.0), "1:02:45");
        assert_eq!(duration(f64::NAN), "–:––");
    }
}
