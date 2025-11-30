use anyhow::Context;
use evdev::{EventSummary, InputEvent};
use futures::{TryStream, TryStreamExt};

pub async fn dump_events(
    mut udev_stream: impl TryStream<Ok = InputEvent, Error = anyhow::Error> + Unpin,
) -> anyhow::Result<()> {
    while let Some(event) = udev_stream
        .try_next()
        .await
        .context("Monitoring events to dump")?
    {
        macro_rules! dump {
            ($($i:ident),* $(,)*) => {
                match event.destructure() {
                    $(
                    EventSummary::$i(event, code, value) => {
                        println!(
                            "type={:?} code={code:?} value={value}",
                            event.event_type(),
                        );
                    }
                    )*
                }
            };
        }
        dump!(
            Synchronization,
            Key,
            RelativeAxis,
            AbsoluteAxis,
            Misc,
            Switch,
            Led,
            Sound,
            Repeat,
            ForceFeedback,
            Power,
            ForceFeedbackStatus,
            UInput,
            Other,
        );
    }
    Ok(())
}
