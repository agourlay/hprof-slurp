const KIBIBYTE: u64 = 1024;
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

// The `--filter` predicate: a plain case sensitive substring match, shared by
// every report so that the text and JSON outputs can never disagree on what a
// pattern selects. No filter matches everything.
pub fn matches_class_filter(class_name: &str, filter: Option<&str>) -> bool {
    filter.is_none_or(|pattern| class_name.contains(pattern))
}

// Like [`pretty_bytes_size`] but for deltas, with an explicit sign.
pub fn pretty_signed_bytes_size(delta: i64) -> String {
    if delta < 0 {
        format!("-{}", pretty_bytes_size(delta.unsigned_abs()))
    } else {
        format!("+{}", pretty_bytes_size(delta.unsigned_abs()))
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

// The inverse of [`pretty_bytes_size`]: reads a byte size written either as a
// plain number of bytes or as a number followed by a unit. Every unit is a
// power of 1024, so `KB` and `K` are accepted as aliases for `KiB` rather than
// meaning 1000 bytes.
pub fn parse_bytes_size(raw: &str) -> Result<u64, String> {
    let input = raw.trim();
    let unit_start = input
        .find(|c: char| !c.is_ascii_digit())
        .unwrap_or(input.len());
    let (digits, unit) = input.split_at(unit_start);
    if digits.is_empty() {
        return Err(format!("'{raw}' does not start with a number"));
    }
    let value: u64 = digits
        .parse()
        .map_err(|_| format!("'{digits}' is too large to be a number of bytes"))?;
    let multiplier = match unit.trim().to_ascii_lowercase().as_str() {
        "" | "b" | "byte" | "bytes" => 1,
        "k" | "kb" | "kib" => KIBIBYTE,
        "m" | "mb" | "mib" => KIBIBYTE.pow(2),
        "g" | "gb" | "gib" => KIBIBYTE.pow(3),
        "t" | "tb" | "tib" => KIBIBYTE.pow(4),
        other => {
            return Err(format!(
                "'{other}' is not a known unit (expected one of bytes, KiB, MiB, GiB, TiB)"
            ));
        }
    };
    value
        .checked_mul(multiplier)
        .ok_or_else(|| format!("'{raw}' is larger than {} bytes", u64::MAX))
}

#[cfg(test)]
mod tests {
    use super::matches_class_filter;
    use super::parse_bytes_size;
    use super::pretty_bytes_size;
    use super::pretty_timestamp_utc;

    #[test]
    fn class_filter_matches_everything_when_absent() {
        assert!(matches_class_filter("java.lang.String", None));
    }

    #[test]
    fn class_filter_is_a_case_sensitive_substring_match() {
        assert!(matches_class_filter("java.util.HashMap", Some("java.util")));
        assert!(matches_class_filter("java.util.HashMap", Some("HashMap")));
        assert!(matches_class_filter("char[]", Some("[]")));
        assert!(!matches_class_filter(
            "java.util.HashMap",
            Some("java.lang")
        ));
        assert!(!matches_class_filter("java.util.HashMap", Some("hashmap")));
    }

    #[test]
    fn pretty_timestamp_epoch() {
        assert_eq!(pretty_timestamp_utc(0), "1970-01-01 00:00:00 UTC");
    }

    #[test]
    fn pretty_timestamp_modern_date() {
        // timestamp of the `hprof-64.bin` test dump header
        assert_eq!(
            pretty_timestamp_utc(1_515_934_059_480),
            "2018-01-14 12:47:39 UTC"
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

    #[test]
    fn parse_size_without_unit_is_bytes() {
        assert_eq!(parse_bytes_size("0"), Ok(0));
        assert_eq!(parse_bytes_size("10485760"), Ok(10_485_760));
        assert_eq!(parse_bytes_size("512B"), Ok(512));
    }

    #[test]
    fn parse_size_units_are_powers_of_1024() {
        assert_eq!(parse_bytes_size("10MiB"), Ok(10 * 1024 * 1024));
        assert_eq!(parse_bytes_size("10MB"), Ok(10 * 1024 * 1024));
        assert_eq!(parse_bytes_size("10M"), Ok(10 * 1024 * 1024));
        assert_eq!(parse_bytes_size("1KiB"), Ok(1024));
        assert_eq!(parse_bytes_size("2GiB"), Ok(2 * 1024 * 1024 * 1024));
        assert_eq!(parse_bytes_size("1TiB"), Ok(1024 * 1024 * 1024 * 1024));
    }

    #[test]
    fn parse_size_is_case_and_space_insensitive() {
        assert_eq!(parse_bytes_size("10mib"), Ok(10 * 1024 * 1024));
        assert_eq!(parse_bytes_size(" 10 MiB "), Ok(10 * 1024 * 1024));
    }

    #[test]
    fn parse_size_rejects_garbage() {
        assert!(parse_bytes_size("").is_err());
        assert!(parse_bytes_size("MiB").is_err());
        assert!(parse_bytes_size("-1").is_err());
        assert!(parse_bytes_size("10 potatoes").is_err());
        assert!(parse_bytes_size("1.5MiB").is_err());
    }

    #[test]
    fn parse_size_rejects_overflow() {
        assert!(parse_bytes_size("99999999999999999999").is_err());
        assert!(parse_bytes_size("18446744073709551615TiB").is_err());
    }
}
