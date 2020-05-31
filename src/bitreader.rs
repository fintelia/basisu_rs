
pub struct BitReaderLSB<'a> {
    bytes: &'a [u8],
    pos: usize,
}

impl<'a> BitReaderLSB<'a> {
    pub fn new(bytes: &'a [u8]) -> Self {
        Self {
            bytes,
            pos: 0,
        }
    }

    pub fn read(&mut self, count: usize) -> u32 {
        assert!(count <= 32);
        let mut byte = self.pos / 8;
        let mut result: u32 = 0;
        let mut read = 0;

        {
            let bit = self.pos % 8;
            let byte_val = if byte < self.bytes.len() { self.bytes[byte] } else { 0 };
            result |= (byte_val >> bit) as u32;
            read += 8 - bit;
            byte += 1;
        }

        loop {
            if read >= count {
                self.pos += count;
                if count < 32 {
                    result &= (1 << count) - 1;
                }
                return result;
            }
            let byte_val = if byte < self.bytes.len() { self.bytes[byte] } else { 0 };
            result |= (byte_val as u32) << read;
            read += 8;
            byte += 1;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_bitreader() {
        let pattern = 0x5555_5555_5555_5555u64;
        for i in 0..16 {
            let xor_mask =
                ((0i16 - ((i >> 3) & 0x1)) as u16 as u64) << 3*16 |
                ((0i16 - ((i >> 2) & 0x1)) as u16 as u64) << 2*16 |
                ((0i16 - ((i >> 1) & 0x1)) as u16 as u64) << 1*16 |
                ((0i16 - ((i >> 0) & 0x1)) as u16 as u64) << 0*16;
            let data = pattern ^ xor_mask;
            let bytes = data.to_le_bytes();
            for len in 0..32 {
                for offset in 0..32 {
                    let mut reader = BitReaderLSB::new(&bytes);
                    let actual = reader.read(offset);
                    let expected = (data & ((1u64 << offset) - 1)) as u32;
                    assert_eq!(
                        actual, expected,
                        "offset value mismatch, left: {:b}, right: {:b}, off: {}, len: {}, data: {:b}",
                        actual, expected, offset, len, data
                    );

                    let actual = reader.read(len);
                    let expected = ((data >> offset) & ((1u64 << len) - 1)) as u32;
                    assert_eq!(
                        actual, expected,
                        "value mismatch, left: {:b}, right: {:b}, off: {}, len: {}, data: {:b}",
                        actual, expected, offset, len, data
                    );
                }
            }
        }
    }
}
