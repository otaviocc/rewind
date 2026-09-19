//! A bounds-checked little-endian reader over a borrowed byte slice: the idiom every binary
//! decoder in the cache layer is built from. Zero `as`, zero indexing, zero panics.

pub struct Cursor<'a> {
    rest: &'a [u8],
}

impl<'a> Cursor<'a> {
    pub const fn new(bytes: &'a [u8]) -> Self {
        Self { rest: bytes }
    }

    pub const fn remaining(&self) -> usize {
        self.rest.len()
    }

    pub const fn is_empty(&self) -> bool {
        self.rest.is_empty()
    }

    pub fn take(&mut self, n: usize) -> Option<&'a [u8]> {
        let (head, tail) = self.rest.split_at_checked(n)?;
        self.rest = tail;
        Some(head)
    }

    pub fn u8(&mut self) -> Option<u8> {
        let (&byte, tail) = self.rest.split_first()?;
        self.rest = tail;
        Some(byte)
    }

    pub fn u16(&mut self) -> Option<u16> {
        let chunk = self.take(2)?;
        chunk.first_chunk::<2>().map(|bytes| u16::from_le_bytes(*bytes))
    }

    pub fn u32(&mut self) -> Option<u32> {
        let chunk = self.take(4)?;
        chunk.first_chunk::<4>().map(|bytes| u32::from_le_bytes(*bytes))
    }

    pub fn u64(&mut self) -> Option<u64> {
        let chunk = self.take(8)?;
        chunk.first_chunk::<8>().map(|bytes| u64::from_le_bytes(*bytes))
    }

    pub fn i64(&mut self) -> Option<i64> {
        let chunk = self.take(8)?;
        chunk.first_chunk::<8>().map(|bytes| i64::from_le_bytes(*bytes))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_u8_is_read_and_the_cursor_advances_by_one() {
        let mut cursor = Cursor::new(&[7, 9]);
        assert_eq!(cursor.u8(), Some(7));
        assert_eq!(cursor.remaining(), 1);
        assert_eq!(cursor.u8(), Some(9));
        assert!(cursor.is_empty());
    }

    #[test]
    fn a_u8_past_the_end_is_none_and_does_not_advance() {
        let mut cursor = Cursor::new(&[]);
        assert_eq!(cursor.u8(), None);
        assert!(cursor.is_empty());
    }

    #[test]
    fn multi_byte_integers_round_trip_little_endian() {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&1_u16.to_le_bytes());
        bytes.extend_from_slice(&2_u32.to_le_bytes());
        bytes.extend_from_slice(&3_u64.to_le_bytes());
        bytes.extend_from_slice(&(-4_i64).to_le_bytes());

        let mut cursor = Cursor::new(&bytes);
        assert_eq!(cursor.u16(), Some(1));
        assert_eq!(cursor.u32(), Some(2));
        assert_eq!(cursor.u64(), Some(3));
        assert_eq!(cursor.i64(), Some(-4));
        assert!(cursor.is_empty());
    }

    #[test]
    fn a_multi_byte_read_past_the_end_is_none_and_leaves_the_cursor_untouched() {
        let mut cursor = Cursor::new(&[1, 2, 3]);
        assert_eq!(cursor.u32(), None);
        assert_eq!(cursor.remaining(), 3, "a failed read must not consume any bytes");
        assert_eq!(cursor.u64(), None);
        assert_eq!(cursor.i64(), None);
    }

    #[test]
    fn take_returns_a_borrowed_slice_and_advances_by_exactly_n() {
        let mut cursor = Cursor::new(b"hello world");
        assert_eq!(cursor.take(5), Some(b"hello".as_slice()));
        assert_eq!(cursor.remaining(), 6);
    }

    #[test]
    fn take_zero_bytes_is_the_empty_slice_and_never_none() {
        let mut cursor = Cursor::new(b"x");
        assert_eq!(cursor.take(0), Some(b"".as_slice()));
        assert_eq!(cursor.remaining(), 1);
    }

    #[test]
    fn take_more_than_remaining_is_none() {
        let mut cursor = Cursor::new(b"ab");
        assert_eq!(cursor.take(3), None);
        assert_eq!(cursor.remaining(), 2);
    }

    #[test]
    fn reads_can_be_interleaved_and_stay_in_lockstep() {
        let mut bytes = Vec::new();
        bytes.push(b'A');
        bytes.extend_from_slice(&5_u32.to_le_bytes());
        bytes.extend_from_slice(b"blob");

        let mut cursor = Cursor::new(&bytes);
        assert_eq!(cursor.u8(), Some(b'A'));
        assert_eq!(cursor.u32(), Some(5));
        assert_eq!(cursor.take(4), Some(b"blob".as_slice()));
        assert!(cursor.is_empty());
    }
}
