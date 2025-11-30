use super::crc::CRC8_9B;

const DISC_PFX: [u8; 4] = *b"HOIP";

#[repr(C, packed(1))]
#[derive(PartialEq, Clone, Copy, Debug)]
pub struct Packet {
    pfx: [u8; 4],
    pub port: u16,
    crc: u8,
}

impl Packet {
    pub const REQUEST: Self = Self::new(0);

    pub const fn new(port: u16) -> Self {
        let mut this = Self {
            pfx: DISC_PFX,
            port,
            crc: 0,
        };
        this.update_crc();
        this
    }

    const fn update_crc(&mut self) {
        self.crc = self.crc();
    }

    pub const fn as_bytes(&self) -> &[u8; size_of::<Self>()] {
        assert!(align_of::<Self>() == 1);
        // SAFETY: tightly packed, hence casting to &[u8; size_of()] is valid.
        unsafe { &*(self as *const Self as *const [u8; size_of::<Self>()]) }
    }

    pub const fn try_from_bytes(bytes: &[u8]) -> Option<&Self> {
        assert!(align_of::<Self>() == 1);
        if bytes.len() != size_of::<Self>() {
            return None;
        }
        // SAFETY: all bit-patterns are valid for Packet, we asserted slice len
        // is exactly right, Packet is tightly packed, we asserted it's aligned.
        let pkt = unsafe { &*(bytes.as_ptr() as *const Self) };
        if !matches!(pkt.pfx, DISC_PFX) || !pkt.validate_crc() {
            return None;
        }
        Some(pkt)
    }

    const fn validate_crc(&self) -> bool {
        self.crc == self.crc()
    }

    const fn crc(&self) -> u8 {
        let bytes = self.as_bytes();
        CRC8_9B.calc(bytes, bytes.len() - 1, 0)
    }

    pub const fn is_request(&self) -> bool {
        matches!(self.port, 0)
    }
}

impl std::ops::Deref for Packet {
    type Target = [u8];

    fn deref(&self) -> &Self::Target {
        self.as_bytes()
    }
}

#[cfg(test)]
mod test {
    use super::*;

    const _: () = test_packet_const();

    #[test]
    const fn test_packet_const() {
        #[track_caller]
        const fn check(port: u16) {
            let pkt = Packet::new(port);
            let Some(pkt2) = Packet::try_from_bytes(pkt.as_bytes()) else {
                panic!("pkt2 is None");
            };
            let (bs1, bs2) = (pkt.as_bytes(), pkt2.as_bytes());
            assert!(bs1.len() == bs2.len(), "lengths don't match");
            let mut j = 0;
            while j < bs1.len() {
                assert!(bs1[j] == bs2[j], "bytes don't match");
                j += 1;
            }
        }
        check(0);
        check(0xFFFF);
        check(0x7FFF);
        check(12345);
        check(54321);
    }

    #[test]
    fn test_packet() {
        #[track_caller]
        fn check(port: u16) {
            let pkt = Packet::new(port);
            let pkt2 = Packet::try_from_bytes(pkt.as_bytes()).expect("try_from_bytes");
            assert_eq!(&pkt, pkt2);
            assert_eq!(pkt.as_bytes(), pkt2.as_bytes());
        }
        for i in u16::MIN..=u16::MAX {
            check(i);
        }
    }

    #[test]
    fn test_is_request() {
        assert!(Packet::REQUEST.is_request());
        assert!(!Packet::new(1234).is_request());
    }
}
