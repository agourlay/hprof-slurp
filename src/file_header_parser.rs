use crate::primitive_parsers::*;
use nom::combinator::map;
use nom::sequence::tuple;
use nom::IResult;

#[derive(Debug, PartialEq, Eq)]
pub struct FileHeader {
    pub format: String,
    pub size_pointers: u32,
    pub timestamp: u64,
}

impl FileHeader {
    fn from_bytes(format_b: &[u8], size_pointers: u32, timestamp: u64) -> FileHeader {
        FileHeader {
            format: String::from_utf8_lossy(format_b).to_string(),
            size_pointers,
            timestamp,
        }
    }
}

pub fn parse_file_header(i: &[u8]) -> IResult<&[u8], FileHeader> {
    map(
        tuple((parse_c_string, parse_u32, parse_u64)),
        |(format, size_pointers, timestamp)| {
            FileHeader::from_bytes(format, size_pointers, timestamp)
        },
    )(i)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_well_formed_header() {
        let binary: [u8; 31] = [
            74, 65, 86, 65, 32, 80, 82, 79, 70, 73, 76, 69, 32, 49, 46, 48, 46, 50, 0, 0, 0, 0, 8,
            0, 0, 1, 118, 111, 186, 173, 167,
        ];
        let expected = FileHeader {
            format: "JAVA PROFILE 1.0.2".to_string(),
            size_pointers: 8,
            timestamp: 1608192273831,
        };
        let (rest, header) = parse_file_header(&binary).unwrap();
        assert_eq!(header, expected);
        assert_eq!(rest.is_empty(), true);
    }
}
