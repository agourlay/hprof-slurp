const KILOBYTE: f64 = 1024.0;
const MEGABYTE: f64 = KILOBYTE * KILOBYTE;
const GIGABYTE: f64 = KILOBYTE * MEGABYTE;

// Renders epoch milliseconds as a `YYYY-MM-DD HH:MM:SS UTC` date.
pub fn pretty_timestamp_utc(epoch_millis: u64) -> String {
    let secs = epoch_millis / 1000;
    let (year, month, day) = civil_from_days(secs / 86_400);
    let secs_of_day = secs % 86_400;
    let (hours, minutes, seconds) = (
        secs_of_day / 3600,
        (secs_of_day % 3600) / 60,
        secs_of_day % 60,
    );
    format!("{year:04}-{month:02}-{day:02} {hours:02}:{minutes:02}:{seconds:02} UTC")
}

// Computes the civil date from days since the epoch (valid for 1970 onwards).
// http://howardhinnant.github.io/date_algorithms.html#civil_from_days
fn civil_from_days(days: u64) -> (u64, u64, u64) {
    let z = days + 719_468;
    let era = z / 146_097;
    let doe = z % 146_097; // day of era [0, 146096]
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365; // year of era [0, 399]
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100); // day of year [0, 365]
    let mp = (5 * doy + 2) / 153; // March-based month [0, 11]
    let day = doy - (153 * mp + 2) / 5 + 1;
    let year = yoe + era * 400;
    if mp < 10 {
        (year, mp + 3, day)
    } else {
        (year + 1, mp - 9, day)
    }
}

pub fn pretty_bytes_size(len: u64) -> String {
    let float_len = len as f64;
    let (unit, value) = if float_len >= GIGABYTE {
        ("GiB", float_len / GIGABYTE)
    } else if float_len >= MEGABYTE {
        ("MiB", float_len / MEGABYTE)
    } else if float_len >= KILOBYTE {
        ("KiB", float_len / KILOBYTE)
    } else {
        ("bytes", float_len)
    };
    format!("{value:.2}{unit}")
}

#[cfg(test)]
mod tests {
    use super::pretty_bytes_size;
    use super::pretty_timestamp_utc;

    #[test]
    fn pretty_timestamp_epoch() {
        assert_eq!(pretty_timestamp_utc(0), "1970-01-01 00:00:00 UTC");
    }

    #[test]
    fn pretty_timestamp_modern_date() {
        // timestamp of the 64-bit test dump header
        assert_eq!(
            pretty_timestamp_utc(1_608_192_273_831),
            "2020-12-17 08:04:33 UTC"
        );
    }

    #[test]
    fn pretty_timestamp_leap_day() {
        assert_eq!(
            pretty_timestamp_utc(1_709_208_000_000),
            "2024-02-29 12:00:00 UTC"
        );
    }

    #[test]
    fn pretty_timestamp_century_rule() {
        // 2000 is a leap year (400-year rule)
        assert_eq!(
            pretty_timestamp_utc(951_782_400_000),
            "2000-02-29 00:00:00 UTC"
        );
        // 2100 is not a leap year (100-year rule): Feb 28 is followed by Mar 1
        assert_eq!(
            pretty_timestamp_utc(4_107_542_400_000),
            "2100-03-01 00:00:00 UTC"
        );
    }

    #[test]
    fn pretty_size_gb() {
        let size: u64 = 1_200_000_000;
        assert_eq!(pretty_bytes_size(size), "1.12GiB");
    }

    #[test]
    fn pretty_size_mb() {
        let size: u64 = 1_200_000;
        assert_eq!(pretty_bytes_size(size), "1.14MiB");
    }

    #[test]
    fn pretty_size_kb() {
        let size: u64 = 1_200;
        assert_eq!(pretty_bytes_size(size), "1.17KiB");
    }

    #[test]
    fn pretty_size_bytes() {
        let size: u64 = 512;
        assert_eq!(pretty_bytes_size(size), "512.00bytes");
    }
}
