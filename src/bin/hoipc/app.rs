use std::collections::BTreeSet;

use anyhow::Context;
use evdev::{
    AttributeSet, EventType, InputEvent, InputId, KeyCode, PropType, RelativeAxisCode,
    SynchronizationCode, uinput::VirtualDevice,
};
use futures::{TryFutureExt, TryStreamExt};
use hid_over_ip::{codec::Codec, discovery::Discovery};
use tokio_util::codec::Framed;

use crate::Cli;

pub struct App<'a> {
    config: &'a Cli,
    disc: &'a Discovery,
    dev: VirtualDevice,
    pressed_keys: BTreeSet<KeyCode>,
}

impl<'a> App<'a> {
    pub async fn run(config: &Cli, disc: &Discovery) -> anyhow::Result<()> {
        let mut app = App::new(config, disc).context("Construct App")?;
        loop {
            if let Err(e) = app.connect_loop().await {
                tracing::error!("{e:?}");
            }
        }
    }

    fn new(config: &'a Cli, disc: &'a Discovery) -> anyhow::Result<Self> {
        Ok(Self {
            dev: {
                let mut dev = builder(config)?.build().context("Build virtual device")?;
                tracing::info!(
                    path = %dev.get_syspath().context("Get device syspath")?.display(),
                    "Created virtual device"
                );
                dev
            },
            config,
            disc,
            pressed_keys: BTreeSet::new(),
        })
    }

    async fn connect_loop(&mut self) -> anyhow::Result<()> {
        struct DropGuard<'a, 'b>(&'a mut App<'b>);

        impl Drop for DropGuard<'_, '_> {
            fn drop(&mut self) {
                if !self.0.pressed_keys.is_empty() {
                    tracing::info!("Cleaning up stuck keys");
                    let mut evts = Vec::with_capacity(self.0.pressed_keys.len());
                    while let Some(key) = self.0.pressed_keys.pop_first() {
                        evts.push(InputEvent::new(EventType::KEY.0, key.0, 0));
                    }
                    if let Err(e) = self.0.dev.emit(&evts) {
                        tracing::error!("Error while cleaning up stuck keys: {e:?}")
                    }
                }
            }
        }

        let drop = DropGuard(self);
        let this = &mut *drop.0;
        let (tcp_stream, remote) = {
            let listener = tokio::net::TcpListener::bind(&this.config.listen)
                .await
                .context("Bind TCP listener")?;
            tracing::info!(address = %this.config.listen, "Started listener");
            tokio::try_join!(listener.accept().err_into(), this.disc.advertise())
                .context("Listener accept/advertise")?
                .0
        };
        tracing::info!(%remote, "Accepted remote connection");
        let mut framed = Framed::new(tcp_stream, Codec);
        tracing::info!("Starting event loop");
        let mut buf = Vec::with_capacity(16);
        while let Some(next) = framed.try_next().await.context("Get next data frame")? {
            match next.destructure() {
                evdev::EventSummary::Key(_, key_code, value) => {
                    if matches!(value, 0) {
                        this.pressed_keys.remove(&key_code);
                    } else {
                        this.pressed_keys.insert(key_code);
                    }
                }
                evdev::EventSummary::Synchronization(_, SynchronizationCode::SYN_REPORT, 0) => {
                    this.dev.emit(&buf).context("Emit events")?;
                    buf.clear();
                    continue;
                }
                _ => (),
            }
            buf.push(next);
        }
        tracing::info!(%remote, "Connection closed normally");
        anyhow::Ok(())
    }
}

fn builder(config: &Cli) -> anyhow::Result<evdev::uinput::VirtualDeviceBuilder<'_>> {
    // TODO: absolute, force feedback, switches
    anyhow::Ok(
        VirtualDevice::builder()
            .context("Construct device builder")?
            .name(&config.name)
            .input_id(InputId::new(
                config.bus,
                config.vendor_id,
                config.product_id,
                config.product_version,
            ))
            .with_properties(&AttributeSet::from_iter([PropType::POINTER]))
            .context("Set device properties")?
            .with_keys(&AttributeSet::from_iter((0..560).map(KeyCode)))
            .context("Set device keys")?
            .with_relative_axes(&AttributeSet::from_iter(
                (if config.no_high_res_scroll {
                    0..=10
                } else {
                    0..=12
                })
                .map(RelativeAxisCode),
            ))
            .context("Set device relative axes")?,
    )
}
