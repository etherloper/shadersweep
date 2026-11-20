use std::time::{Duration, SystemTime};

pub fn bytes(n: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut value = n as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 { format!("{n} B") } else { format!("{value:.1} {}", UNITS[unit]) }
}

/// Coarse "last used" wording, measured from `now`.
pub fn age(time: Option<SystemTime>, now: SystemTime) -> String {
    let Some(time) = time else { return "Unknown".into() };
    let elapsed = now.duration_since(time).unwrap_or(Duration::ZERO);
    let days = elapsed.as_secs() / 86_400;
    match days {
        0 => "Today".into(),
        1 => "Yesterday".into(),
        2..=13 => format!("{days} days ago"),
        14..=59 => format!("{} weeks ago", days / 7),
        60..=729 => format!("{} months ago", days / 30),
        _ => format!("{} years ago", days / 365),
    }
}

pub fn plural(count: usize, one: &str, many: &str) -> String {
    format!("{count} {}", if count == 1 { one } else { many })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_bytes() {
        assert_eq!(bytes(0), "0 B");
        assert_eq!(bytes(1023), "1023 B");
        assert_eq!(bytes(1536), "1.5 KB");
        assert_eq!(bytes(10 * 1024 * 1024 * 1024), "10.0 GB");
    }

    #[test]
    fn formats_age() {
        let now = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000 * 86_400);
        let ago = |days: u64| Some(now - Duration::from_secs(days * 86_400));
        assert_eq!(age(None, now), "Unknown");
        assert_eq!(age(ago(0), now), "Today");
        assert_eq!(age(ago(1), now), "Yesterday");
        assert_eq!(age(ago(5), now), "5 days ago");
        assert_eq!(age(ago(21), now), "3 weeks ago");
        assert_eq!(age(ago(90), now), "3 months ago");
        assert_eq!(age(Some(now + Duration::from_secs(60)), now), "Today");
    }
}
