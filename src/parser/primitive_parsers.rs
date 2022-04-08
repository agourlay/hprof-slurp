use nom::sequence::terminated;
use nom::{bytes, number, IResult};

pub fn parse_c_string(i: &[u8]) -> IResult<&[u8], &[u8]> {
    terminated(
        bytes::streaming::take_until("\0"),
        bytes::streaming::tag("\0"),
    )(i)
}

pub fn parse_i8(i: &[u8]) -> IResult<&[u8], i8> {
    number::streaming::be_i8(i)
}

pub fn parse_i16(i: &[u8]) -> IResult<&[u8], i16> {
    number::streaming::be_i16(i)
}

pub fn parse_i32(i: &[u8]) -> IResult<&[u8], i32> {
    number::streaming::be_i32(i)
}

pub fn parse_i64(i: &[u8]) -> IResult<&[u8], i64> {
    number::streaming::be_i64(i)
}

pub fn parse_u16(i: &[u8]) -> IResult<&[u8], u16> {
    number::streaming::be_u16(i)
}

pub fn parse_u32(i: &[u8]) -> IResult<&[u8], u32> {
    number::streaming::be_u32(i)
}

pub fn parse_u64(i: &[u8]) -> IResult<&[u8], u64> {
    number::streaming::be_u64(i)
}

pub fn parse_f32(i: &[u8]) -> IResult<&[u8], f32> {
    number::streaming::be_f32(i)
}

pub fn parse_f64(i: &[u8]) -> IResult<&[u8], f64> {
    number::streaming::be_f64(i)
}

pub fn parse_u8(i: &[u8]) -> IResult<&[u8], u8> {
    number::streaming::be_u8(i)
}
