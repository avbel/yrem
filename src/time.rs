//! Time arguments: RFC 3339, `YYYY-MM-DD`, unix seconds, or a span back from now (`7d`).

use chrono::{DateTime, NaiveDate, TimeDelta, Utc};

use crate::error::{Error, Result};

/// Parses one time argument. Relative spans count back from `now`.
///
/// # Errors
/// `Error::Invalid` when the input matches no accepted form.
pub fn parse(input: &str, now: DateTime<Utc>) -> Result<DateTime<Utc>> {
    let s = input.trim();
    let invalid = || {
        Error::Invalid(format!(
            "time `{input}`: expected RFC 3339, YYYY-MM-DD, unix seconds, or a span like 7d / 12h / 30m"
        ))
    };
    if let Ok(dt) = DateTime::parse_from_rfc3339(s) {
        return Ok(dt.with_timezone(&Utc));
    }
    if let Ok(d) = NaiveDate::parse_from_str(s, "%Y-%m-%d") {
        return Ok(d.and_time(chrono::NaiveTime::MIN).and_utc());
    }
    if let Ok(secs) = s.parse::<i64>() {
        return DateTime::from_timestamp(secs, 0).ok_or_else(invalid);
    }
    let span = parse_span(s).ok_or_else(invalid)?;
    now.checked_sub_signed(span).ok_or_else(invalid)
}

/// Parses `<n><unit>` with unit `s`, `m`, `h`, `d` or `w`.
fn parse_span(s: &str) -> Option<TimeDelta> {
    let split = s.find(|c: char| !c.is_ascii_digit())?;
    let (num, unit) = s.split_at(split);
    let n: i64 = num.parse().ok()?;
    match unit {
        "s" => TimeDelta::try_seconds(n),
        "m" => TimeDelta::try_minutes(n),
        "h" => TimeDelta::try_hours(n),
        "d" => TimeDelta::try_days(n),
        "w" => TimeDelta::try_weeks(n),
        _ => None,
    }
}

/// A closed `[since, until]` window.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TimeWindow {
    pub since: DateTime<Utc>,
    pub until: DateTime<Utc>,
}

impl TimeWindow {
    /// Builds a window from optional bounds; returns `None` when both are absent.
    ///
    /// # Errors
    /// Unparseable bounds, or `since` later than `until`.
    pub fn from_args(
        since: Option<&str>,
        until: Option<&str>,
        now: DateTime<Utc>,
    ) -> Result<Option<Self>> {
        if since.is_none() && until.is_none() {
            return Ok(None);
        }
        let since = since.map(|s| parse(s, now)).transpose()?;
        let until = until.map(|s| parse(s, now)).transpose()?;
        let window = Self {
            since: since.unwrap_or(DateTime::UNIX_EPOCH),
            until: until.unwrap_or(now),
        };
        if window.since > window.until {
            return Err(Error::Invalid("`since` is later than `until`".into()));
        }
        Ok(Some(window))
    }

    /// Whether a unix timestamp falls inside the window.
    #[must_use]
    pub fn contains(&self, unix: f64) -> bool {
        unix >= unix_secs(self.since) && unix <= unix_secs(self.until)
    }

    /// The window as unix seconds.
    #[must_use]
    pub fn as_unix(&self) -> (f64, f64) {
        (unix_secs(self.since), unix_secs(self.until))
    }
}

/// Seconds since the epoch, with sub-second precision.
#[must_use]
#[expect(clippy::cast_precision_loss, reason = "microsecond timestamps fit f64")]
pub fn unix_secs(t: DateTime<Utc>) -> f64 {
    t.timestamp_micros() as f64 / 1e6
}

/// RFC 3339 UTC string for a unix timestamp, or `None` if out of range.
#[must_use]
#[expect(
    clippy::cast_possible_truncation,
    reason = "rounded to whole microseconds"
)]
pub fn iso(unix: f64) -> Option<String> {
    if !unix.is_finite() {
        return None;
    }
    DateTime::from_timestamp_micros((unix * 1e6).round() as i64)
        .map(|t| t.to_rfc3339_opts(chrono::SecondsFormat::Secs, true))
}
