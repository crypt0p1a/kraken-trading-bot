//! The numbers and the times, as a trading desk shows them

/// 12345.678 → "12,345.68"
pub fn money(value: f64) -> String {
    grouped(value, 2)
}

/// 1234.5 → "+1,234.50", the sign always shown
pub fn signed(value: f64) -> String {
    let sign = if value > 0.0 {
        "+"
    } else if value < 0.0 {
        "-"
    } else {
        ""
    };
    format!("{sign}{}", grouped(value.abs(), 2))
}

/// 1.234 → "+1.23%"
pub fn percent(value: f64) -> String {
    format!("{value:+.2}%")
}

/// A price, with the decimals its size calls for
pub fn price(value: f64) -> String {
    let decimals = match value.abs() {
        it if it >= 1000.0 => 1,
        it if it >= 10.0 => 2,
        it if it >= 1.0 => 4,
        _ => 6,
    };
    grouped(value, decimals)
}

/// A volume of base: up to 8 decimals, the trailing zeros removed
pub fn volume(value: f64) -> String {
    let text = format!("{value:.8}");
    let text = text.trim_end_matches('0').trim_end_matches('.');
    if text.is_empty() || text == "-" {
        "0".into()
    } else {
        text.into()
    }
}

fn grouped(value: f64, decimals: usize) -> String {
    let text = format!("{:.*}", decimals, value.abs());
    let (integer, fraction) = match text.split_once('.') {
        Some((integer, fraction)) => (integer, Some(fraction)),
        None => (text.as_str(), None),
    };
    let mut out = String::new();
    for (index, digit) in integer.chars().enumerate() {
        if index > 0 && (integer.len() - index) % 3 == 0 {
            out.push(',');
        }
        out.push(digit);
    }
    if let Some(fraction) = fraction {
        out.push('.');
        out.push_str(fraction);
    }
    // -0.00 is 0.00
    if value < 0.0 && out.chars().any(|it| it.is_ascii_digit() && it != '0') {
        out.insert(0, '-');
    }
    out
}

/// Year, month, day of the days since 1970-01-01 (Howard Hinnant's algorithm)
fn civil(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let month = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let year = yoe + era * 400 + i64::from(month <= 2);
    (year, month, day)
}

struct Utc {
    year: i64,
    month: u32,
    day: u32,
    hour: i64,
    minute: i64,
    second: i64,
}

fn utc(millis: i64) -> Utc {
    let seconds = millis.div_euclid(1000);
    let (year, month, day) = civil(seconds.div_euclid(86_400));
    let time = seconds.rem_euclid(86_400);
    Utc {
        year,
        month,
        day,
        hour: time / 3600,
        minute: time % 3600 / 60,
        second: time % 60,
    }
}

/// "14:32:07"
pub fn clock(millis: i64) -> String {
    let it = utc(millis);
    format!("{:02}:{:02}:{:02}", it.hour, it.minute, it.second)
}

/// "2026-11-03"
pub fn date(millis: i64) -> String {
    let it = utc(millis);
    format!("{}-{:02}-{:02}", it.year, it.month, it.day)
}

/// "11-03 14:32"
pub fn stamp(millis: i64) -> String {
    let it = utc(millis);
    format!(
        "{:02}-{:02} {:02}:{:02}",
        it.month, it.day, it.hour, it.minute
    )
}

/// How long ago: "42s", "5m", "3h", "2d"
pub fn ago(millis: i64) -> String {
    let seconds = (millis / 1000).max(0);
    match seconds {
        it if it < 60 => format!("{it}s"),
        it if it < 3600 => format!("{}m", it / 60),
        it if it < 86_400 => format!("{}h", it / 3600),
        it => format!("{}d", it / 86_400),
    }
}

/// The values as a line of blocks, from ▁ to █
pub fn sparkline(values: &[f64], width: usize) -> String {
    const BLOCKS: [char; 8] = ['▁', '▂', '▃', '▄', '▅', '▆', '▇', '█'];
    if values.is_empty() || width == 0 {
        return " ".repeat(width);
    }
    // the last `width` buckets of the values
    let chunk = values.len().div_ceil(width).max(1);
    let points: Vec<f64> = values.chunks(chunk).map(|it| it[it.len() - 1]).collect();
    let points = &points[points.len().saturating_sub(width)..];

    let (low, high) = points.iter().fold((f64::MAX, f64::MIN), |(low, high), it| {
        (low.min(*it), high.max(*it))
    });
    let line: String = points
        .iter()
        .map(|it| {
            if (high - low).abs() < f64::EPSILON {
                BLOCKS[3]
            } else {
                BLOCKS[(((it - low) / (high - low)) * 7.0).round() as usize]
            }
        })
        .collect();
    format!("{line:<width$}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn groups_the_thousands() {
        assert_eq!(money(1_234_567.891), "1,234,567.89");
        assert_eq!(money(0.0), "0.00");
        assert_eq!(money(-12.5), "-12.50");
        assert_eq!(money(-0.001), "0.00");
        assert_eq!(signed(1234.5), "+1,234.50");
        assert_eq!(signed(-3.0), "-3.00");
    }

    #[test]
    fn shows_the_decimals_of_the_size() {
        assert_eq!(price(76_567.14), "76,567.1");
        assert_eq!(price(2233.675), "2,233.7");
        assert_eq!(price(181.234), "181.23");
        assert_eq!(price(0.5123), "0.512300");
        assert_eq!(volume(0.00040000), "0.0004");
        assert_eq!(volume(12.0), "12");
        assert_eq!(volume(0.0), "0");
    }

    #[test]
    fn shows_the_utc_times() {
        // 2026-11-03T14:32:07.250Z
        let millis = 1_793_716_327_250;
        assert_eq!(clock(millis), "14:32:07");
        assert_eq!(date(millis), "2026-11-03");
        assert_eq!(stamp(millis), "11-03 14:32");
        assert_eq!(date(0), "1970-01-01");
        assert_eq!(date(951_782_400_000), "2000-02-29");
    }

    #[test]
    fn says_how_long_ago() {
        assert_eq!(ago(42_000), "42s");
        assert_eq!(ago(300_000), "5m");
        assert_eq!(ago(3 * 3_600_000), "3h");
        assert_eq!(ago(-5), "0s");
    }

    #[test]
    fn draws_the_values_with_blocks() {
        assert_eq!(sparkline(&[1.0, 2.0, 3.0], 3), "▁▅█");
        assert_eq!(sparkline(&[5.0, 5.0], 4), "▄▄  ");
        assert_eq!(sparkline(&[], 2), "  ");
        assert_eq!(sparkline(&[1.0, 9.0, 2.0, 8.0], 2).chars().count(), 2);
    }
}
