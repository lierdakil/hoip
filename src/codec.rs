use evdev::InputEvent;
use tokio_util::{
    bytes::{Buf, BufMut},
    codec::{Decoder, Encoder},
};

pub struct Codec;

impl Encoder<InputEvent> for Codec {
    type Error = anyhow::Error;

    fn encode(
        &mut self,
        item: InputEvent,
        dst: &mut tokio_util::bytes::BytesMut,
    ) -> Result<(), Self::Error> {
        let t: u16 = item.event_type().0;
        let c: u16 = item.code();
        let v: i32 = item.value();
        dst.put_u16(t);
        dst.put_u16(c);
        dst.put_i32(v);
        Ok(())
    }
}

impl Decoder for Codec {
    type Item = InputEvent;

    type Error = anyhow::Error;

    fn decode(
        &mut self,
        src: &mut tokio_util::bytes::BytesMut,
    ) -> Result<Option<Self::Item>, Self::Error> {
        if src.remaining() < 8 {
            Ok(None)
        } else {
            let t = src.get_u16();
            let c = src.get_u16();
            let v = src.get_i32();
            Ok(Some(InputEvent::new_now(t, c, v)))
        }
    }
}
