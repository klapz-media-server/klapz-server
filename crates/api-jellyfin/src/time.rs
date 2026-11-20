//! Jellyfin's units of time: ticks of 100 ns, and dates with seven decimals.

/// What Jellyfin writes for a date that never happened, `DateTime.MinValue`.
pub const NEVER: &str = "0001-01-01T00:00:00.0000000Z";

/// Ticks per millisecond: .NET counts time in units of 100 ns.
const TICKS_PER_MS: i64 = 10_000;

pub fn ticks(ms: i64) -> i64 {
    ms.saturating_mul(TICKS_PER_MS)
}

pub fn ms(ticks: i64) -> i64 {
    ticks / TICKS_PER_MS
}

/// The official API's `DateTime`, always UTC to the second (`2018-06-06T00:00:00Z`), as .NET
/// writes it: `2018-06-06T00:00:00.0000000Z`.
pub fn date(official: &str) -> String {
    match official.strip_suffix('Z') {
        Some(seconds) if !seconds.contains('.') => format!("{seconds}.0000000Z"),
        _ => official.to_owned(),
    }
}

/// The current time, as Jellyfin writes it.
pub fn now() -> String {
    date(&open_plex_api::date_time::format(
        open_plex_core::now_epoch(),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_millisecond_is_ten_thousand_ticks() {
        assert_eq!(ticks(178_301), 1_783_010_000);
        assert_eq!(ms(1_783_009_999), 178_300);
        assert_eq!(ticks(i64::MAX), i64::MAX);
    }

    #[test]
    fn a_date_gains_seven_decimals() {
        assert_eq!(date("2018-06-06T00:00:00Z"), "2018-06-06T00:00:00.0000000Z");
        assert_eq!(date("not a date"), "not a date");
        assert_eq!(now().len(), NEVER.len());
    }
}
