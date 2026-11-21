//! Clock and formatting helpers.
//!
//! These keep the exact output that rmcp has always produced, so switching the
//! underlying date-time library does not change any timestamp or message text.

use jiff::{Timestamp, fmt::temporal::DateTimePrinter, tz::Offset};

/// The current time as an RFC 3339 string, for example
/// `2025-01-12T15:00:58.123+00:00`.
pub(crate) fn now_rfc3339() -> String {
    rfc3339(Timestamp::now())
}

/// Milliseconds since the Unix epoch.
#[cfg(feature = "request-state")]
pub(crate) fn now_unix_millis() -> i64 {
    Timestamp::now().as_millisecond()
}

/// Formats `timestamp` in UTC with a `+00:00` offset and 0, 3, 6 or 9
/// fractional digits, whichever is the shortest that is exact.
fn rfc3339(timestamp: Timestamp) -> String {
    let nanos = timestamp.subsec_nanosecond();
    let digits = if nanos == 0 {
        0
    } else if nanos % 1_000_000 == 0 {
        3
    } else if nanos % 1_000 == 0 {
        6
    } else {
        9
    };
    DateTimePrinter::new()
        .precision(Some(digits))
        .timestamp_with_offset_to_string(&timestamp, Offset::UTC)
}

/// Displays a duration in ISO 8601 form, such as `PT30S` or `PT1.5S`.
///
/// A zero duration displays as `P0D`, as do durations longer than
/// `i64::MAX` milliseconds.
#[cfg(any(feature = "client", feature = "server"))]
pub(crate) struct Iso8601Duration(pub(crate) std::time::Duration);

#[cfg(any(feature = "client", feature = "server"))]
impl std::fmt::Display for Iso8601Duration {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        const MAX_SECS: u64 = (i64::MAX / 1_000) as u64;
        const MAX_SUBSEC_NANOS: u32 = (i64::MAX % 1_000) as u32 * 1_000_000;

        let secs = self.0.as_secs();
        let nanos = self.0.subsec_nanos();
        let too_long = secs > MAX_SECS || (secs == MAX_SECS && nanos > MAX_SUBSEC_NANOS);
        if self.0.is_zero() || too_long {
            return f.write_str("P0D");
        }
        write!(f, "PT{secs}")?;
        if nanos > 0 {
            let mut fraction = nanos;
            let mut digits = 9;
            while fraction.is_multiple_of(10) {
                fraction /= 10;
                digits -= 1;
            }
            write!(f, ".{fraction:0digits$}")?;
        }
        f.write_str("S")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(second: i64, nanosecond: i32) -> String {
        rfc3339(Timestamp::new(second, nanosecond).unwrap())
    }

    // Expected strings are the output of chrono's `DateTime<Utc>::to_rfc3339`,
    // which rmcp used before.
    #[test]
    fn rfc3339_matches_previous_output() {
        assert_eq!(at(1_736_694_058, 0), "2025-01-12T15:00:58+00:00");
        assert_eq!(
            at(1_736_694_058, 123_000_000),
            "2025-01-12T15:00:58.123+00:00"
        );
        assert_eq!(
            at(1_736_694_058, 100_000_000),
            "2025-01-12T15:00:58.100+00:00"
        );
        assert_eq!(
            at(1_736_694_058, 123_456_000),
            "2025-01-12T15:00:58.123456+00:00"
        );
        assert_eq!(
            at(1_736_694_058, 123_456_789),
            "2025-01-12T15:00:58.123456789+00:00"
        );
        assert_eq!(at(1_736_694_058, 1), "2025-01-12T15:00:58.000000001+00:00");
        assert_eq!(at(0, 0), "1970-01-01T00:00:00+00:00");
        assert_eq!(at(0, -500_000_000), "1969-12-31T23:59:59.500+00:00");
    }

    #[test]
    fn now_rfc3339_round_trips() {
        let before = Timestamp::now();
        let now: Timestamp = now_rfc3339().parse().unwrap();
        assert!(now >= before);
        assert!(now_rfc3339().ends_with("+00:00"));
    }

    #[cfg(feature = "request-state")]
    #[test]
    fn now_unix_millis_is_after_2025() {
        assert!(now_unix_millis() > 1_735_689_600_000);
    }

    // Expected strings are the output of chrono's `TimeDelta` `Display`.
    #[cfg(any(feature = "client", feature = "server"))]
    #[test]
    fn iso8601_duration_matches_previous_output() {
        use std::time::Duration;

        let show = |d: Duration| Iso8601Duration(d).to_string();
        assert_eq!(show(Duration::ZERO), "P0D");
        assert_eq!(show(Duration::from_secs(30)), "PT30S");
        assert_eq!(show(Duration::from_millis(1500)), "PT1.5S");
        assert_eq!(show(Duration::from_nanos(1)), "PT0.000000001S");
        assert_eq!(show(Duration::from_secs(5400)), "PT5400S");
        assert_eq!(show(Duration::new(1, 10)), "PT1.00000001S");
        assert_eq!(
            show(Duration::new(9_223_372_036_854_775, 807_000_000)),
            "PT9223372036854775.807S"
        );
        assert_eq!(
            show(Duration::new(9_223_372_036_854_775, 807_000_001)),
            "P0D"
        );
        assert_eq!(show(Duration::MAX), "P0D");
    }

    #[cfg(any(feature = "client", feature = "server"))]
    #[test]
    fn timeout_error_message_is_unchanged() {
        let error = crate::service::ServiceError::Timeout {
            timeout: std::time::Duration::from_millis(1500),
        };
        assert_eq!(error.to_string(), "request timeout after PT1.5S");
    }
}
