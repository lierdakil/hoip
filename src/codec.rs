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
        dst.put_u16(item.event_type().0);
        dst.put_u16(item.code());
        dst.put_i32(item.value());
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
            Ok(Some(InputEvent::new_now(
                src.get_u16(),
                src.get_u16(),
                src.get_i32(),
            )))
        }
    }
}
