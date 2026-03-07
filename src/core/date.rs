// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Andrea Cervesato <andrea.cervesato@suse.com>
use time::OffsetDateTime;

/// Convert a Unix timestamp into a human-readable relative label:
///
/// | Age                | Example          |
/// |--------------------|------------------|
/// | < 1 min            | `just now`       |
/// | < 1 hour           | `42m ago`        |
/// | same calendar day  | `3h ago`         |
/// | yesterday          | `Yesterday`      |
/// | < 7 days           | `Monday`         |
/// | same year          | `Mar 3`          |
/// | older              | `Mar 3 2024`     |
pub fn humanize_date(ts: Option<i64>) -> String {
    let ts = match ts {
        Some(t) => t,
        None => return "—".to_string(),
    };
    let email_dt = match OffsetDateTime::from_unix_timestamp(ts) {
        Ok(dt) => dt,
        Err(_) => return "—".to_string(),
    };
    let now = OffsetDateTime::now_utc();
    let secs = (now - email_dt).whole_seconds();

    if secs < 0 {
        return format!(
            "{} {} {}",
            month_abbr(email_dt.month()),
            email_dt.day(),
            email_dt.year()
        );
    }
    if secs < 60 {
        return "just now".to_string();
    }
    if secs < 3600 {
        return format!("{}m ago", secs / 60);
    }

    let days = (now.date() - email_dt.date()).whole_days();
    if days == 0 {
        return format!("{}h ago", secs / 3600);
    }
    if days == 1 {
        return "Yesterday".to_string();
    }
    if days < 7 {
        let name = match email_dt.weekday() {
            time::Weekday::Monday => "Monday",
            time::Weekday::Tuesday => "Tuesday",
            time::Weekday::Wednesday => "Wednesday",
            time::Weekday::Thursday => "Thursday",
            time::Weekday::Friday => "Friday",
            time::Weekday::Saturday => "Saturday",
            time::Weekday::Sunday => "Sunday",
        };
        return name.to_string();
    }
    if now.year() == email_dt.year() {
        return format!("{} {}", month_abbr(email_dt.month()), email_dt.day());
    }
    format!(
        "{} {} {}",
        month_abbr(email_dt.month()),
        email_dt.day(),
        email_dt.year()
    )
}

fn month_abbr(month: time::Month) -> &'static str {
    match month {
        time::Month::January => "Jan",
        time::Month::February => "Feb",
        time::Month::March => "Mar",
        time::Month::April => "Apr",
        time::Month::May => "May",
        time::Month::June => "Jun",
        time::Month::July => "Jul",
        time::Month::August => "Aug",
        time::Month::September => "Sep",
        time::Month::October => "Oct",
        time::Month::November => "Nov",
        time::Month::December => "Dec",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use time::{Date, Month, OffsetDateTime, Time};

    fn now_ts() -> i64 {
        OffsetDateTime::now_utc().unix_timestamp()
    }

    #[test]
    fn none_returns_dash() {
        assert_eq!(humanize_date(None), "—");
    }

    #[test]
    fn invalid_timestamp_returns_dash() {
        assert_eq!(humanize_date(Some(i64::MAX)), "—");
    }

    #[test]
    fn future_timestamp_shows_full_date() {
        let ts = now_ts() + 86400 * 365 * 10;
        let result = humanize_date(Some(ts));
        let parts: Vec<&str> = result.split_whitespace().collect();
        assert_eq!(parts.len(), 3);
    }

    #[test]
    fn just_now() {
        assert_eq!(humanize_date(Some(now_ts() - 30)), "just now");
    }

    #[test]
    fn minutes_ago() {
        assert_eq!(humanize_date(Some(now_ts() - 10 * 60)), "10m ago");
    }

    #[test]
    fn hours_ago_same_calendar_day() {
        let now_dt = OffsetDateTime::now_utc();
        if now_dt.hour() >= 2 {
            let ts = now_dt.unix_timestamp() - 2 * 3600;
            assert_eq!(humanize_date(Some(ts)), "2h ago");
        }
    }

    #[test]
    fn yesterday_label() {
        let now_dt = OffsetDateTime::now_utc();
        let yesterday = now_dt.date().previous_day().unwrap();
        let dt = OffsetDateTime::new_utc(yesterday, Time::MIDNIGHT);
        assert_eq!(humanize_date(Some(dt.unix_timestamp())), "Yesterday");
    }

    #[test]
    fn weekday_within_7_days() {
        let now_dt = OffsetDateTime::now_utc();
        let three_days_ago = now_dt.date() - time::Duration::days(3);
        let dt = OffsetDateTime::new_utc(three_days_ago, Time::MIDNIGHT);
        let expected = match dt.weekday() {
            time::Weekday::Monday => "Monday",
            time::Weekday::Tuesday => "Tuesday",
            time::Weekday::Wednesday => "Wednesday",
            time::Weekday::Thursday => "Thursday",
            time::Weekday::Friday => "Friday",
            time::Weekday::Saturday => "Saturday",
            time::Weekday::Sunday => "Sunday",
        };
        assert_eq!(humanize_date(Some(dt.unix_timestamp())), expected);
    }

    #[test]
    fn same_year_older_than_7_days() {
        let now_dt = OffsetDateTime::now_utc();
        let thirty_days_ago = now_dt.date() - time::Duration::days(30);
        if thirty_days_ago.year() == now_dt.year() {
            let dt = OffsetDateTime::new_utc(thirty_days_ago, Time::MIDNIGHT);
            let expected = format!("{} {}", month_abbr(dt.month()), dt.day());
            assert_eq!(humanize_date(Some(dt.unix_timestamp())), expected);
        }
    }

    #[test]
    fn different_year_shows_full_date() {
        let old_date = Date::from_calendar_date(2020, Month::January, 15).unwrap();
        let dt = OffsetDateTime::new_utc(old_date, Time::MIDNIGHT);
        assert_eq!(humanize_date(Some(dt.unix_timestamp())), "Jan 15 2020");
    }

    #[test]
    fn month_abbreviations_all() {
        use time::Month::*;
        let cases = [
            (January, "Jan"),
            (February, "Feb"),
            (March, "Mar"),
            (April, "Apr"),
            (May, "May"),
            (June, "Jun"),
            (July, "Jul"),
            (August, "Aug"),
            (September, "Sep"),
            (October, "Oct"),
            (November, "Nov"),
            (December, "Dec"),
        ];
        for (month, abbr) in cases {
            assert_eq!(month_abbr(month), abbr);
        }
    }
}
