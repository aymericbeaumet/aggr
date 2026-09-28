//! Dates as publishers write them at the edges of an article, read to the precision they were
//! written.
//!
//! A line only reads as a date when all of it is the date: `27 Sep, 2026` is one, and so is
//! `Tuesday, September 22nd, 2026 at 3:04 pm`, but `it rained all 22 September 2026` is prose that
//! happens to hold one. Callers decide what a date means where they found it; this module only
//! says whether a line is one and which day, or which month, it names.

use chrono::{DateTime, Datelike as _, NaiveDate};

/// A date as a page wrote it. `September 2026` names a month, not its first day, and a numeric
/// `04/05/2026` names one of two days depending on where the publisher is; both keep that
/// uncertainty instead of guessing it away.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum WrittenDate {
    Day(NaiveDate),
    Month {
        year: i32,
        month: u32,
    },
    /// Month-first and day-first readings of the same numbers, when both are real days.
    EitherDay(NaiveDate, NaiveDate),
}

impl WrittenDate {
    /// Whether this names the day an item was published. A day either side is allowed, because a
    /// feed and the page it points at routinely disagree about the time zone; a month matches any
    /// day in it.
    pub(super) fn names(self, published: NaiveDate) -> bool {
        let near = |day: NaiveDate| day.signed_duration_since(published).num_days().abs() <= 1;
        match self {
            Self::Day(day) => near(day),
            Self::Month { year, month } => published.year() == year && published.month() == month,
            Self::EitherDay(first, second) => near(first) || near(second),
        }
    }
}

/// Read `raw` as a date and nothing else, or `None` when any of it is not the date.
pub(super) fn parse(raw: &str) -> Option<WrittenDate> {
    let value = raw
        .trim()
        .trim_matches(['*', '_'])
        .trim()
        .trim_start_matches(['(', '['])
        .trim_end_matches([')', ']', '.'])
        .trim();
    if value.is_empty() || value.chars().count() > 64 {
        return None;
    }
    machine(value)
        .or_else(|| numeric(value))
        .or_else(|| worded(value))
}

/// Timestamps a machine wrote: `2026-09-27T06:49:16Z`, `Sat, 27 Sep 2026 06:49:16 +0000`.
fn machine(value: &str) -> Option<WrittenDate> {
    if let Ok(stamp) = DateTime::parse_from_rfc3339(value) {
        return Some(WrittenDate::Day(stamp.date_naive()));
    }
    if let Ok(stamp) = DateTime::parse_from_rfc2822(value) {
        return Some(WrittenDate::Day(stamp.date_naive()));
    }
    // `2026-09-27 06:49` and `2026-09-27T06:49:16`: an ISO day followed by nothing but a time.
    let (day, time) = value
        .split_at_checked(10)
        .filter(|(_, time)| time.starts_with(['T', ' ']))?;
    let day = NaiveDate::parse_from_str(day, "%Y-%m-%d").ok()?;
    is_time(time[1..].trim()).then_some(WrittenDate::Day(day))
}

/// All-number dates. A four-digit year up front is unambiguous; one at the end reads day-first
/// with dots, as Europe writes it, and either way with slashes or dashes.
fn numeric(value: &str) -> Option<WrittenDate> {
    if !value
        .chars()
        .all(|ch| ch.is_ascii_digit() || matches!(ch, '-' | '/' | '.'))
    {
        return None;
    }
    if value.len() == 8 && value.bytes().all(|byte| byte.is_ascii_digit()) {
        // `20260927`, a compact permalink date. Only as good as the caller's evidence: it is kept
        // as a day and left to match the item before anything is removed on its account.
        return NaiveDate::parse_from_str(value, "%Y%m%d")
            .ok()
            .map(WrittenDate::Day);
    }
    let separator = value.chars().find(|ch| matches!(ch, '-' | '/' | '.'))?;
    let parts: Vec<&str> = value.split(separator).collect();
    let [first, second, third] = parts.as_slice() else {
        return None;
    };
    let number = |part: &str| {
        (1..=4)
            .contains(&part.len())
            .then(|| part.parse::<u32>().ok())?
    };
    let (first, second, third) = (number(first)?, number(second)?, number(third)?);
    let day = |year: u32, month: u32, day: u32| NaiveDate::from_ymd_opt(year as i32, month, day);
    if parts[0].len() == 4 {
        return day(first, second, third).map(WrittenDate::Day);
    }
    if parts[2].len() != 4 {
        return None;
    }
    if separator == '.' {
        return day(third, second, first).map(WrittenDate::Day);
    }
    match (day(third, first, second), day(third, second, first)) {
        (Some(month_first), Some(day_first)) if month_first != day_first => {
            Some(WrittenDate::EitherDay(month_first, day_first))
        }
        (Some(only), _) | (None, Some(only)) => Some(WrittenDate::Day(only)),
        (None, None) => None,
    }
}

/// Dates with a month name: `27 Sep, 2026`, `September 27th, 2026`, `Tue 22 Sept. 2026`,
/// `27-Sep-2026`, `the 27th of September 2026`, `September 2026`, each optionally followed by the
/// time of day.
fn worded(value: &str) -> Option<WrittenDate> {
    let normalized = value.replace([',', '·'], " ");
    let mut tokens: Vec<&str> = normalized
        .split(|ch: char| ch.is_whitespace() || ch == '-')
        .filter(|token| !token.is_empty())
        .collect();
    if tokens.first().is_some_and(|first| {
        WEEKDAYS.contains(&first.trim_end_matches('.').to_ascii_lowercase().as_str())
    }) {
        tokens.remove(0);
    }
    if tokens
        .first()
        .is_some_and(|first| first.eq_ignore_ascii_case("the"))
    {
        tokens.remove(0);
    }
    tokens.retain(|token| !token.eq_ignore_ascii_case("of"));

    let (date, rest) = worded_date(&tokens)?;
    (rest.is_empty() || is_time_of_day(rest)).then_some(date)
}

/// The date at the start of `tokens`, and whatever follows it.
fn worded_date<'a>(tokens: &'a [&'a str]) -> Option<(WrittenDate, &'a [&'a str])> {
    let day = |token: &str| {
        let digits = strip_ordinal(token);
        (1..=2)
            .contains(&digits.len())
            .then(|| digits.parse::<u32>().ok())?
    };
    let year = |token: &str| (token.len() == 4).then(|| token.parse::<i32>().ok())?;
    let date = |year: i32, month: u32, day: u32| NaiveDate::from_ymd_opt(year, month, day);
    match tokens {
        // `27 Sep 2026`
        [d, m, y, rest @ ..] if day(d).is_some() && month(m).is_some() && year(y).is_some() => {
            Some((WrittenDate::Day(date(year(y)?, month(m)?, day(d)?)?), rest))
        }
        // `Sep 27 2026`
        [m, d, y, rest @ ..] if month(m).is_some() && day(d).is_some() && year(y).is_some() => {
            Some((WrittenDate::Day(date(year(y)?, month(m)?, day(d)?)?), rest))
        }
        // `2026 Sep 27`
        [y, m, d, rest @ ..] if year(y).is_some() && month(m).is_some() && day(d).is_some() => {
            Some((WrittenDate::Day(date(year(y)?, month(m)?, day(d)?)?), rest))
        }
        // `September 2026`
        [m, y] if month(m).is_some() && year(y).is_some() => Some((
            WrittenDate::Month {
                year: year(y)?,
                month: month(m)?,
            },
            &[],
        )),
        _ => None,
    }
}

const WEEKDAYS: [&str; 17] = [
    "monday",
    "tuesday",
    "wednesday",
    "thursday",
    "friday",
    "saturday",
    "sunday",
    "mon",
    "tue",
    "tues",
    "wed",
    "thu",
    "thur",
    "thurs",
    "fri",
    "sat",
    "sun",
];

/// The month a word names, in English, however it is abbreviated: `Sep`, `Sept.`, `September`.
fn month(token: &str) -> Option<u32> {
    const MONTHS: [&str; 12] = [
        "january",
        "february",
        "march",
        "april",
        "may",
        "june",
        "july",
        "august",
        "september",
        "october",
        "november",
        "december",
    ];
    let word = token.trim_end_matches('.').to_ascii_lowercase();
    if word.len() < 3 {
        return None;
    }
    MONTHS
        .iter()
        .position(|name| *name == word || (word.len() <= 4 && name.starts_with(&word)))
        .map(|index| index as u32 + 1)
}

/// `27th`, `1st`, `22nd`, `3rd` without the suffix; anything else unchanged.
fn strip_ordinal(token: &str) -> &str {
    ["st", "nd", "rd", "th"]
        .iter()
        .find_map(|suffix| {
            token
                .strip_suffix(suffix)
                .filter(|digits| !digits.is_empty() && digits.bytes().all(|b| b.is_ascii_digit()))
        })
        .unwrap_or(token)
}

/// What may follow a worded date on the same line: `at 3:04 pm`, `10:30 UTC`, `@ 18:00`.
fn is_time_of_day(tokens: &[&str]) -> bool {
    let tokens = match tokens {
        [first, rest @ ..] if first.eq_ignore_ascii_case("at") || *first == "@" => rest,
        _ => tokens,
    };
    let Some((time, zone)) = tokens.split_first() else {
        return false;
    };
    let time = time.trim_end_matches(['a', 'p', 'm', 'A', 'P', 'M']);
    if !is_clock(time) {
        return false;
    }
    let meridiem = |token: &str| {
        matches!(
            token.to_ascii_lowercase().trim_end_matches('.'),
            "am" | "pm" | "a.m" | "p.m"
        )
    };
    let zone_word = |token: &str| {
        (2..=5).contains(&token.len()) && token.bytes().all(|byte| byte.is_ascii_uppercase())
            || token.starts_with(['+', '-'])
                && token[1..].bytes().all(|b| b.is_ascii_digit() || b == b':')
    };
    match zone {
        [] => true,
        [only] => meridiem(only) || zone_word(only),
        [meridiem_word, zone] => meridiem(meridiem_word) && zone_word(zone),
        _ => false,
    }
}

/// `06:49`, `6:49:16`, `06:49:16.850`.
fn is_clock(value: &str) -> bool {
    let mut parts = value.split(':');
    let (Some(hours), Some(minutes)) = (parts.next(), parts.next()) else {
        return false;
    };
    let seconds = parts.next();
    parts.next().is_none()
        && (1..=2).contains(&hours.len())
        && hours.parse::<u32>().is_ok_and(|hours| hours < 24)
        && minutes.len() == 2
        && minutes.parse::<u32>().is_ok_and(|minutes| minutes < 60)
        && seconds.is_none_or(|seconds| {
            let whole = seconds.split('.').next().unwrap_or_default();
            whole.len() == 2 && whole.parse::<u32>().is_ok_and(|seconds| seconds < 61)
        })
}

/// A time after an ISO day: `06:49`, `06:49:16`, `06:49:16.850Z`, `06:49:16+02:00`.
fn is_time(value: &str) -> bool {
    let clock_end = value
        .find(|ch: char| matches!(ch, 'Z' | 'z' | '+' | '-'))
        .unwrap_or(value.len());
    let (clock, zone) = value.split_at(clock_end);
    is_clock(clock)
        && (zone.is_empty()
            || zone.eq_ignore_ascii_case("z")
            || zone[1..]
                .bytes()
                .all(|byte| byte.is_ascii_digit() || byte == b':'))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn day(year: i32, month: u32, day: u32) -> Option<WrittenDate> {
        Some(WrittenDate::Day(
            NaiveDate::from_ymd_opt(year, month, day).unwrap(),
        ))
    }

    #[test]
    fn reads_the_ways_publishers_write_one_day() {
        for written in [
            // The two that were missed on a real archive.
            "27 Sep, 2026",
            "*27 Sep, 2026*",
            // Day first.
            "27 Sep 2026",
            "27 September 2026",
            "27 September, 2026",
            "27th September 2026",
            "the 27th of September, 2026",
            "27 Sept. 2026",
            "27-Sep-2026",
            "27 sep 2026",
            // Month first.
            "Sep 27, 2026",
            "Sep. 27, 2026",
            "Sept 27, 2026",
            "September 27, 2026",
            "September 27th, 2026",
            "September 27 2026",
            "SEPTEMBER 27, 2026",
            // With the weekday the date already implies.
            "Sunday, September 27, 2026",
            "Sun, 27 Sep 2026",
            "Sun. 27th Sept. 2026",
            // With the time of day.
            "Sep 27, 2026, 10:30 AM",
            "September 27, 2026 at 3:04 pm",
            "27 Sep 2026 18:00 UTC",
            "Sep 27, 2026 @ 10:30",
            "September 27, 2026 at 10:30 am PDT",
            // Year first, and all-number forms.
            "2026 Sep 27",
            "2026-09-27",
            "2026/09/27",
            "2026.09.27",
            "20260927",
            "27.09.2026",
            // Written by a machine.
            "2026-09-27T06:49:16Z",
            "2026-09-27T06:49:16.850+02:00",
            "2026-09-27 06:49",
            "Sun, 27 Sep 2026 06:49:16 +0000",
            // Wrapped the way a Markdown line or caption holds it.
            "(September 27, 2026)",
            "[27 Sep 2026]",
            "27 Sep 2026.",
        ] {
            assert_eq!(parse(written), day(2026, 9, 27), "{written:?}");
        }
    }

    #[test]
    fn a_month_and_year_name_a_month_not_its_first_day() {
        for written in [
            "September 2026",
            "Sep 2026",
            "Sept. 2026",
            "september, 2026",
        ] {
            assert_eq!(
                parse(written),
                Some(WrittenDate::Month {
                    year: 2026,
                    month: 9
                }),
                "{written:?}"
            );
        }
        let september = parse("September 2026").unwrap();
        for published in ["2026-09-01", "2026-09-28", "2026-09-30"] {
            assert!(september.names(published.parse().unwrap()), "{published}");
        }
        for published in ["2026-08-31", "2026-10-01", "2025-09-28"] {
            assert!(!september.names(published.parse().unwrap()), "{published}");
        }
    }

    #[test]
    fn slashed_numbers_keep_both_readings_until_the_item_settles_them() {
        let both = parse("04/05/2026").unwrap();
        assert_eq!(
            both,
            WrittenDate::EitherDay(
                NaiveDate::from_ymd_opt(2026, 4, 5).unwrap(),
                NaiveDate::from_ymd_opt(2026, 5, 4).unwrap()
            )
        );
        assert!(both.names("2026-04-05".parse().unwrap()));
        assert!(both.names("2026-05-04".parse().unwrap()));
        assert!(!both.names("2026-04-20".parse().unwrap()));
        // Only one reading is a real day, or both readings are the same day.
        assert_eq!(parse("09/27/2026"), day(2026, 9, 27));
        assert_eq!(parse("27/09/2026"), day(2026, 9, 27));
        assert_eq!(parse("05/05/2026"), day(2026, 5, 5));
        assert_eq!(parse("9-27-2026"), day(2026, 9, 27));
    }

    #[test]
    fn a_day_either_side_still_names_the_published_day() {
        let written = parse("27 Sep 2026").unwrap();
        assert!(written.names("2026-09-26".parse().unwrap()));
        assert!(written.names("2026-09-28".parse().unwrap()));
        assert!(!written.names("2026-09-29".parse().unwrap()));
    }

    #[test]
    fn prose_that_holds_a_date_is_not_a_date() {
        for written in [
            "",
            "Tuesday morning 2026",
            "it rained all 22 September 2026 long",
            "27 Sep 2026 was the day everything changed",
            "Published on 27 Sep 2026",
            "Eric Gullichsen, September 2026",
            "May the force be with you 2026",
            "May",
            "2026",
            "September",
            "Chapter 27 of 2026",
            "27 Sep",
            "Sep 27",
            // Impossible or partial numbers.
            "31 Sep 2026",
            "2026-13-01",
            "2026-09-27 and more",
            "2026-09-27T99:00",
            "27/13/2026",
            "1.2.3",
            "10.24",
            "3.10.24",
            "123456789",
            "Sep 27, 2026 at noon-ish",
            "Sep 27, 2026 at 25:00",
        ] {
            assert_eq!(parse(written), None, "{written:?}");
        }
    }

    #[test]
    fn month_names_read_in_any_case_and_abbreviation_but_not_as_fragments() {
        assert_eq!(month("Sep"), Some(9));
        assert_eq!(month("Sept."), Some(9));
        assert_eq!(month("SEPTEMBER"), Some(9));
        assert_eq!(month("may"), Some(5));
        assert_eq!(month("Ma"), None);
        assert_eq!(month("Septembre"), None);
        assert_eq!(month("Marching"), None);
    }
}
