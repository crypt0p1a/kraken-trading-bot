//! Wall clock

/// UTC time, in milliseconds since the epoch
pub fn now_millis() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|it| it.as_millis() as i64)
        .unwrap_or_default()
}

/// UTC time, in seconds since the epoch
pub fn now_seconds() -> f64 {
    now_millis() as f64 / 1000.0
}
