/// Unix seconds of an RFC 3339 time (`2026-09-28T06:38:51.735Z`, `...+07:00`).
pub fn parse_timestamp(text: &str) -> Option<u64> {
    let text = text.trim();
    let (date, rest) = text.split_once('T')?;
    let mut parts = date.split('-');
    let year: i64 = parts.next()?.parse().ok()?;
    let month: i64 = parts.next()?.parse().ok()?;
    let day: i64 = parts.next()?.parse().ok()?;
    let (clock, offset) = match rest.find(['Z', 'z', '+', '-']) {
        Some(at) => rest.split_at(at),
        None => (rest, ""),
    };
    let mut clock_parts = clock.split(':');
    let hour: i64 = clock_parts.next()?.parse().ok()?;
    let minute: i64 = clock_parts.next()?.parse().ok()?;
    let second: i64 = clock_parts
        .next()
        .and_then(|seconds| seconds.split('.').next())
        .and_then(|seconds| seconds.parse().ok())
        .unwrap_or(0);
    let offset_seconds = match offset.chars().next() {
        Some(sign @ ('+' | '-')) => {
            let (hours, minutes) = offset[1..].split_once(':').unwrap_or((&offset[1..], "0"));
            let seconds = hours.parse::<i64>().ok()? * 3600 + minutes.parse::<i64>().ok()? * 60;
            if sign == '+' {
                seconds
            } else {
                -seconds
            }
        }
        _ => 0,
    };
    let days = days_from_civil(year, month, day);
    let seconds = days * 86_400 + hour * 3600 + minute * 60 + second - offset_seconds;
    u64::try_from(seconds).ok()
}

/// Days since 1970-01-01 of a proleptic Gregorian date.
fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let year = if month <= 2 { year - 1 } else { year };
    let era = year.div_euclid(400);
    let year_of_era = year - era * 400;
    let month_index = (month + 9) % 12;
    let day_of_year = (153 * month_index + 2) / 5 + day - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    era * 146_097 + day_of_era - 719_468
}

pub fn unix_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_secs())
}

/// The local calendar day of a unix time, as days since 1970-01-01, so turns group by the user's days.
pub fn local_day(unix: u64) -> i64 {
    let seconds = unix as libc::time_t;
    let mut local: libc::tm = unsafe { std::mem::zeroed() };
    // SAFETY: localtime_r only writes the struct it is given.
    let converted = unsafe { !libc::localtime_r(&seconds, &mut local).is_null() };
    if !converted {
        return (unix / 86_400) as i64;
    }
    days_from_civil(
        i64::from(local.tm_year) + 1900,
        i64::from(local.tm_mon) + 1,
        i64::from(local.tm_mday),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn timestamps_parse_with_fractions_and_offsets() {
        assert_eq!(parse_timestamp("1970-01-01T00:00:00Z"), Some(0));
        assert_eq!(
            parse_timestamp("2026-09-28T06:38:51.735Z"),
            Some(1_790_577_531)
        );
        assert_eq!(
            parse_timestamp("2026-09-28T13:38:51+07:00"),
            Some(1_790_577_531)
        );
        assert_eq!(parse_timestamp("yesterday"), None);
    }
}
