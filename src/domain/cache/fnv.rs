//! FNV-1a 64-bit, over whatever bytes the caller passes. The file-stamp table hashes the
//! first 4 KB of a file; capping to that slice is the caller's job, not this function's.

const OFFSET_BASIS: u64 = 0xcbf2_9ce4_8422_2325;
const PRIME: u64 = 0x0000_0100_0000_01b3;

pub fn hash(bytes: &[u8]) -> u64 {
    let mut state = OFFSET_BASIS;
    for &byte in bytes {
        state ^= u64::from(byte);
        state = state.wrapping_mul(PRIME);
    }
    state
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_empty_input_hashes_to_the_offset_basis() {
        assert_eq!(hash(b""), OFFSET_BASIS);
    }

    #[test]
    fn known_test_vectors_match_the_reference_fnv_1a_64_implementation() {
        assert_eq!(hash(b"a"), 0xaf63_dc4c_8601_ec8c);
        assert_eq!(hash(b"foobar"), 0x8594_4171_f739_67e8);
    }

    #[test]
    fn a_single_changed_byte_changes_the_hash() {
        assert_ne!(hash(b"holodeck"), hash(b"holodesk"));
    }

    #[test]
    fn the_hash_is_deterministic() {
        assert_eq!(hash(b"the grid scanner"), hash(b"the grid scanner"));
    }
}
