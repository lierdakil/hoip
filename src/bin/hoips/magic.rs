use std::collections::{BTreeMap, btree_map::Entry};

use anyhow::Context;
use evdev::{EventSummary, InputEvent, KeyCode};
use futures::{Stream, StreamExt};

pub enum Error<E> {
    MagicKey,
    Other(E),
}

impl<E> From<E> for Error<E> {
    fn from(error: E) -> Self {
        Error::Other(error)
    }
}

#[derive(Debug)]
pub struct Magic {
    keys: BTreeMap<KeyCode, i32>,
    armed: bool,
}

impl Magic {
    fn from_iter<'a>(iter: impl IntoIterator<Item = &'a KeyCode>) -> Self {
        Self {
            keys: BTreeMap::from_iter(iter.into_iter().map(|k| (*k, 0))),
            armed: false,
        }
    }

    fn key(&mut self, key_code: KeyCode, value: i32) -> bool {
        if let Entry::Occupied(mut entry) = self.keys.entry(key_code) {
            entry.insert(value);
            if self.armed && self.keys.values().all(|v| *v == 0) {
                self.armed = false;
                return true;
            }
            self.armed |= self.keys.values().all(|v| *v != 0);
        }
        false
    }

    pub fn map_stream<E>(
        magic_key: &[KeyCode],
        stream: impl Stream<Item = Result<InputEvent, E>> + Unpin,
    ) -> impl Stream<Item = Result<InputEvent, Error<E>>> + Unpin {
        let mut magic = Magic::from_iter(magic_key);
        stream.map(move |evt| match evt.as_ref().map(|x| x.destructure()) {
            Ok(EventSummary::Key(_, key_code, value)) if magic.key(key_code, value) => {
                Err(Error::MagicKey)
            }
            _ => evt.map_err(Error::Other),
        })
    }

    pub async fn wait(
        magic_key: &[KeyCode],
        stream: impl Stream<Item = anyhow::Result<InputEvent>> + Unpin,
    ) -> anyhow::Result<()> {
        tracing::info!("Waiting for magic key...");
        let mut stream = Self::map_stream(magic_key, stream);
        while let Some(evt) = stream.next().await {
            match evt {
                Ok(_) => {}
                Err(Error::MagicKey) => {
                    tracing::info!("Magic key pressed");
                    return Ok(());
                }
                Err(Error::Other(other)) => {
                    return Err(other).context("Monitoring input for magic");
                }
            };
        }
        anyhow::bail!("Input stream ended unexpectedly");
    }
}

#[cfg(test)]
mod test {
    use std::time::Duration;

    use evdev::EventType;

    use super::*;

    #[tokio::test]
    async fn test_wait() {
        let magic_key = [
            KeyCode::KEY_1,
            KeyCode::KEY_2,
            KeyCode::KEY_3,
            KeyCode::KEY_4,
        ];
        let stream = futures::stream::iter([
            InputEvent::new(EventType::KEY.0, KeyCode::KEY_1.0, 1),
            InputEvent::new(EventType::KEY.0, KeyCode::KEY_2.0, 1),
            InputEvent::new(EventType::KEY.0, KeyCode::KEY_3.0, 1),
            InputEvent::new(EventType::KEY.0, KeyCode::KEY_4.0, 1),
            InputEvent::new(EventType::KEY.0, KeyCode::KEY_1.0, 0),
            InputEvent::new(EventType::KEY.0, KeyCode::KEY_2.0, 0),
            InputEvent::new(EventType::KEY.0, KeyCode::KEY_3.0, 0),
            InputEvent::new(EventType::KEY.0, KeyCode::KEY_4.0, 0),
        ])
        .map(Ok);
        Magic::wait(&magic_key, stream).await.expect("Is OK");
        let stream = futures::stream::iter([
            InputEvent::new(EventType::KEY.0, KeyCode::KEY_1.0, 1),
            InputEvent::new(EventType::KEY.0, KeyCode::KEY_2.0, 1),
            InputEvent::new(EventType::KEY.0, KeyCode::KEY_3.0, 1),
            InputEvent::new(EventType::KEY.0, KeyCode::KEY_4.0, 1),
            // one key is still pressed
            // InputEvent::new(EventType::KEY.0, KeyCode::KEY_1.0, 0),
            InputEvent::new(EventType::KEY.0, KeyCode::KEY_2.0, 0),
            InputEvent::new(EventType::KEY.0, KeyCode::KEY_3.0, 0),
            InputEvent::new(EventType::KEY.0, KeyCode::KEY_4.0, 0),
        ])
        .chain(futures::stream::pending())
        .map(Ok);
        tokio::time::timeout(Duration::from_millis(100), Magic::wait(&magic_key, stream))
            .await
            .expect_err("Is timeout");
        // no key pressed
        tokio::time::timeout(
            Duration::from_millis(100),
            Magic::wait(&magic_key, futures::stream::pending()),
        )
        .await
        .expect_err("Is timeout");
    }
}
