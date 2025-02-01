const KILOBYTE: f64 = 1024.0;
const MEGABYTE: f64 = KILOBYTE * KILOBYTE;
const GIGABYTE: f64 = KILOBYTE * MEGABYTE;

pub fn pretty_bytes_size(len: u64) -> String {
    let float_len = len as f64;
    let (unit, value) = if float_len > GIGABYTE {
        ("GiB", float_len / GIGABYTE)
    } else if float_len > MEGABYTE {
        ("MiB", float_len / MEGABYTE)
    } else if float_len > KILOBYTE {
        ("KiB", float_len / KILOBYTE)
    } else {
        ("bytes", float_len)
    };
    format!("{value:.2}{unit}")
}

#[cfg(test)]
mod tests {
    use super::pretty_bytes_size;

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
}
