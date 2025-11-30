pub const CRC8_9B: Crc8 = Crc8::create(0x9b);

pub struct Crc8([u8; 256]);

impl Crc8 {
    pub const fn create(polynomial: u8) -> Crc8 {
        let mut table = [0u8; 256];
        let mut crc = 0x80;
        let mut i = 1u8;
        while i > 0 {
            crc = (crc << 1) ^ if crc & 0x80 != 0 { polynomial } else { 0 };
            let mut j = 0;
            while j < i {
                table[i.wrapping_add(j) as usize] = crc ^ table[j as usize];
                j += 1;
            }
            i <<= 1;
        }
        Crc8(table)
    }

    pub const fn calc(&self, buffer: &[u8], len: usize, mut crc: u8) -> u8 {
        let mut i = 0;
        while i < len {
            crc = self.0[(crc ^ buffer[i]) as usize];
            i += 1;
        }
        crc
    }
}

#[cfg(test)]
mod test {
    use super::*;

    #[test]
    fn test_9b() {
        // CDMA2000
        assert_eq!(CRC8_9B.calc(b"123456789", 9, 0xFF), 0xDA);
        assert_eq!(CRC8_9B.calc(b"987654321", 9, 0xFF), 0x58);
        // LTE
        assert_eq!(CRC8_9B.calc(b"123456789", 9, 0x00), 0xEA);
        assert_eq!(CRC8_9B.calc(b"987654321", 9, 0x00), 0x68);
    }
}
