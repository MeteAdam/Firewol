//! The date sections of a grouped listing, the ones Explorer shows in Downloads.
use anyhow::Result;
use windows::Win32::Foundation::{FILETIME, SYSTEMTIME};
use windows::Win32::Globalization::{GetLocaleInfoEx, LOCALE_IFIRSTDAYOFWEEK};
use windows::Win32::System::SystemInformation::GetLocalTime;
use windows::Win32::System::Time::{SystemTimeToFileTime, TzSpecificLocalTimeToSystemTime};
use windows_core::PCWSTR;

use super::unix_seconds;

/// Where each section starts, newest first. Anything older is the last section.
pub(super) type SectionStarts = [i64; 7];

/// Days since 1970-01-01 of a calendar date (Howard Hinnant's algorithm).
fn days_from_civil(year: i32, month: u32, day: u32) -> i64 {
    let year = i64::from(year) - i64::from(month <= 2);
    let era = year.div_euclid(400);
    let of_era = year - era * 400;
    let shifted_month = i64::from((month + 9) % 12);
    let of_year = (153 * shifted_month + 2) / 5 + i64::from(day) - 1;
    let of_cycle = of_era * 365 + of_era / 4 - of_era / 100 + of_year;
    era * 146_097 + of_cycle - 719_468
}

fn civil_from_days(days: i64) -> (i32, u32, u32) {
    let shifted = days + 719_468;
    let era = shifted.div_euclid(146_097);
    let of_cycle = shifted - era * 146_097;
    let of_era = (of_cycle - of_cycle / 1460 + of_cycle / 36_524 - of_cycle / 146_096) / 365;
    let of_year = of_cycle - (365 * of_era + of_era / 4 - of_era / 100);
    let month_index = (5 * of_year + 2) / 153;
    let day = (of_year - (153 * month_index + 2) / 5 + 1) as u32;
    let month = if month_index < 10 {
        month_index + 3
    } else {
        month_index - 9
    } as u32;
    let year = of_era + era * 400 + i64::from(month <= 2);
    (year as i32, month, day)
}

/// The first day of each section as a day number. `into_week` counts days since the week began.
fn section_days(year: i32, month: u32, day: u32, into_week: i64) -> [i64; 7] {
    let today = days_from_civil(year, month, day);
    let week = today - into_week;
    let (last_year, last_month) = if month == 1 {
        (year - 1, 12)
    } else {
        (year, month - 1)
    };
    [
        today,
        today - 1,
        week,
        week - 7,
        days_from_civil(year, month, 1),
        days_from_civil(last_year, last_month, 1),
        days_from_civil(year, 1, 1),
    ]
}

/// Sunday is 0, as in `SYSTEMTIME`; Windows stores the setting with Monday as 0.
fn first_day_of_week() -> u16 {
    let mut text = [0u16; 4];
    let length =
        unsafe { GetLocaleInfoEx(PCWSTR::null(), LOCALE_IFIRSTDAYOFWEEK, Some(&mut text)) };
    let monday_based = if length >= 1 {
        text[0].wrapping_sub(u16::from(b'0')) % 7
    } else {
        0
    };
    (monday_based + 1) % 7
}

fn local_midnight(day: i64) -> Result<i64> {
    let (year, month, date) = civil_from_days(day);
    let local = SYSTEMTIME {
        wYear: year as u16,
        wMonth: month as u16,
        wDay: date as u16,
        ..Default::default()
    };
    let mut utc = SYSTEMTIME::default();
    let mut filetime = FILETIME::default();
    unsafe {
        TzSpecificLocalTimeToSystemTime(None, &local, &mut utc)?;
        SystemTimeToFileTime(&utc, &mut filetime)?;
    }
    Ok(unix_seconds(
        u64::from(filetime.dwHighDateTime) << 32 | u64::from(filetime.dwLowDateTime),
    ))
}

/// Section starts in Unix seconds, by the user's clock, time zone and first day of the week.
pub(super) fn section_starts() -> Result<SectionStarts> {
    let now = unsafe { GetLocalTime() };
    let into_week = i64::from((now.wDayOfWeek + 7 - first_day_of_week()) % 7);
    let days = section_days(
        i32::from(now.wYear),
        u32::from(now.wMonth),
        u32::from(now.wDay),
        into_week,
    );
    let mut starts = [0; 7];
    for (start, day) in starts.iter_mut().zip(days) {
        *start = local_midnight(day)?;
    }
    Ok(starts)
}

/// An SQL expression for an item's section. The order of the tests matters: early in a month
/// "last week" reaches into the month before, and the first matching section wins.
pub(super) fn section_sql(starts: &SectionStarts) -> String {
    let tests: String = starts
        .iter()
        .enumerate()
        .map(|(section, start)| format!("WHEN modified >= {start} THEN {section} "))
        .collect();
    format!("CASE {tests}ELSE {} END", starts.len())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn calendar_dates_survive_the_round_trip() {
        for (year, month, day) in [(1970, 1, 1), (2024, 2, 29), (2026, 12, 31), (1999, 3, 1)] {
            assert_eq!(
                civil_from_days(days_from_civil(year, month, day)),
                (year, month, day)
            );
        }
        assert_eq!(days_from_civil(1970, 1, 2), 1);
    }

    #[test]
    fn sections_follow_the_calendar() {
        // Wednesday 2026-01-14, with weeks starting on Monday.
        let days = section_days(2026, 1, 14, 2).map(civil_from_days);
        assert_eq!(days[0], (2026, 1, 14));
        assert_eq!(days[1], (2026, 1, 13));
        assert_eq!(days[2], (2026, 1, 12));
        assert_eq!(days[3], (2026, 1, 5));
        assert_eq!(days[4], (2026, 1, 1));
        assert_eq!(days[5], (2025, 12, 1), "January's last month is December");
        assert_eq!(days[6], (2026, 1, 1));
    }

    #[test]
    fn section_starts_run_from_today_backwards() -> Result<()> {
        let starts = section_starts()?;
        assert!(starts[0] > starts[1], "today starts after yesterday");
        assert!(starts[2] >= starts[3] + 6 * 86_400 - 7200);
        Ok(())
    }
}
