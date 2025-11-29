use evdev::{EventType, InputEvent};
use serde::{Deserialize, Serialize};
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
        postcard::to_io(&InputEventWrap(item), dst.writer())?;
        Ok(())
    }
}

#[derive(Serialize, Deserialize)]
#[repr(transparent)]
struct InputEventWrap(#[serde(with = "InputEventDef")] InputEvent);

#[derive(Serialize, Deserialize)]
#[serde(remote = "InputEvent")]
struct InputEventDef {
    #[serde(getter = "InputEvent::event_type")]
    type_: EventType,
    #[serde(getter = "InputEvent::code")]
    code: u16,
    #[serde(getter = "InputEvent::value")]
    value: i32,
}

impl From<InputEvent> for InputEventDef {
    fn from(value: InputEvent) -> Self {
        InputEventDef {
            type_: value.event_type(),
            code: value.code(),
            value: value.value(),
        }
    }
}

impl From<InputEventDef> for InputEvent {
    fn from(value: InputEventDef) -> Self {
        InputEvent::new_now(value.type_.0, value.code, value.value)
    }
}

impl Decoder for Codec {
    type Item = InputEvent;

    type Error = anyhow::Error;

    fn decode(
        &mut self,
        src: &mut tokio_util::bytes::BytesMut,
    ) -> Result<Option<Self::Item>, Self::Error> {
        match postcard::take_from_bytes(&*src) {
            Ok((InputEventWrap(res), rest)) => {
                src.advance(src.len() - rest.len());
                Ok(Some(res))
            }
            Err(err) => match err {
                postcard::Error::DeserializeUnexpectedEnd => Ok(None),
                e => Err(e.into()),
            },
        }
    }
}

pub fn init_logging() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::builder()
                .with_default_directive(tracing::level_filters::LevelFilter::INFO.into())
                .from_env_lossy(),
        )
        .pretty()
        .init();
}
