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
