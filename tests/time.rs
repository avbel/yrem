#![allow(clippy::unwrap_used, clippy::expect_used, clippy::float_cmp)]

use chrono::{DateTime, TimeZone, Utc};
use yrem::Error;
use yrem::time::{TimeWindow, iso, parse, unix_secs};

fn now() -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 3, 15, 12, 0, 0).unwrap()
}

fn ts(secs: i64) -> DateTime<Utc> {
    DateTime::from_timestamp(secs, 0).unwrap()
}

#[test]
fn parse_valid_forms() {
    let n = now();
    let base = n.timestamp();
    let cases: Vec<(&str, i64)> = vec![
        ("2026-01-02T03:04:05Z", 1_767_323_045),
        ("2026-01-02T03:04:05+02:00", 1_767_323_045 - 7200),
        ("2026-01-02T03:04:05-05:30", 1_767_323_045 + 19_800),
        ("  2026-01-02T03:04:05Z  ", 1_767_323_045),
        ("2026-01-02", 1_767_312_000),
        ("1700000000", 1_700_000_000),
        ("0", 0),
        ("-5", -5),
        ("7d", base - 7 * 86_400),
        ("12h", base - 12 * 3600),
        ("30m", base - 30 * 60),
        ("45s", base - 45),
        ("2w", base - 14 * 86_400),
        ("0d", base),
    ];
    for (input, expected) in cases {
        assert_eq!(parse(input, n).unwrap(), ts(expected), "input {input:?}");
    }
}

#[test]
fn parse_invalid_inputs() {
    for input in [
        "",
        "   ",
        "7x",
        "abc",
        "d7",
        "7",
        "7 d",
        "2026-13-45",
        "99999999999999d",
        "99999999999999999999999d",
        "99999999999999999999",
        "-7d",
        "1.5h",
    ] {
        // "7" parses as unix seconds, everything else must fail.
        if input == "7" {
            assert!(parse(input, now()).is_ok());
            continue;
        }
        match parse(input, now()) {
            Err(Error::Invalid(m)) => assert!(m.contains("time"), "{input:?}: {m}"),
            other => panic!("{input:?} -> {other:?}"),
        }
    }
}

#[test]
fn window_none_when_no_bounds() {
    assert_eq!(TimeWindow::from_args(None, None, now()).unwrap(), None);
}

#[test]
fn window_only_since_ends_now() {
    let w = TimeWindow::from_args(Some("7d"), None, now())
        .unwrap()
        .unwrap();
    assert_eq!(w.until, now());
    assert_eq!(w.since, ts(now().timestamp() - 7 * 86_400));
}

#[test]
fn window_only_until_starts_at_epoch() {
    let w = TimeWindow::from_args(None, Some("1d"), now())
        .unwrap()
        .unwrap();
    assert_eq!(w.since, DateTime::UNIX_EPOCH);
    assert_eq!(w.until, ts(now().timestamp() - 86_400));
}

#[test]
fn window_since_after_until_is_invalid() {
    let e = TimeWindow::from_args(Some("1h"), Some("2h"), now()).unwrap_err();
    assert!(matches!(e, Error::Invalid(_)), "{e:?}");
}

#[test]
fn window_with_equal_bounds_is_valid() {
    let w = TimeWindow::from_args(Some("1h"), Some("1h"), now())
        .unwrap()
        .unwrap();
    assert_eq!(w.since, w.until);
}

#[test]
fn window_bad_bound_is_invalid() {
    assert!(matches!(
        TimeWindow::from_args(Some("zzz"), None, now()),
        Err(Error::Invalid(_))
    ));
    assert!(matches!(
        TimeWindow::from_args(None, Some("zzz"), now()),
        Err(Error::Invalid(_))
    ));
}

#[test]
fn window_contains_is_closed() {
    let w = TimeWindow::from_args(Some("1000"), Some("2000"), now())
        .unwrap()
        .unwrap();
    assert!(w.contains(1000.0));
    assert!(w.contains(1500.5));
    assert!(w.contains(2000.0));
    assert!(!w.contains(999.999));
    assert!(!w.contains(2000.001));
    assert_eq!(w.as_unix(), (1000.0, 2000.0));
}

#[test]
fn unix_secs_has_subsecond_precision() {
    let t = DateTime::from_timestamp_micros(1_700_000_000_250_000).unwrap();
    assert_eq!(unix_secs(t), 1_700_000_000.25);
    assert_eq!(unix_secs(DateTime::UNIX_EPOCH), 0.0);
}

#[test]
fn iso_formats_and_rejects() {
    assert_eq!(iso(0.0).as_deref(), Some("1970-01-01T00:00:00Z"));
    assert_eq!(
        iso(1_767_323_045.0).as_deref(),
        Some("2026-01-02T03:04:05Z")
    );
    assert_eq!(iso(f64::NAN), None);
    assert_eq!(iso(f64::INFINITY), None);
    assert_eq!(iso(f64::NEG_INFINITY), None);
}
