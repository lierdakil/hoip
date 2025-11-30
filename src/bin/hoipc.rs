use std::{
    collections::BTreeSet,
    net::{SocketAddr, SocketAddrV6},
    process::ExitCode,
};

use anyhow::Context;
use clap::Parser;
use evdev::{
    AttributeSet, BusType, EventType, InputEvent, InputId, KeyCode, PropType, RelativeAxisCode,
    uinput::VirtualDevice,
};
use futures::{TryFutureExt, TryStreamExt};
use hid_over_ip::{
    codec::Codec,
    discovery::{DEFAULT_MULTICAST_SOCKET_V4, DEFAULT_MULTICAST_SOCKET_V6, Discovery},
    init_logging,
};
use tokio_util::codec::Framed;

/// HoIP -- HID-over-IP. Share keyboard and mouse (or other HID inputs) over
/// TCP/IP.
///
/// HoIP "client". Will receive input events from the "server" and pass them
/// through to the system it runs on via a virtual input device.
#[derive(clap::Parser)]
#[command(version)]
struct Cli {
    /// Address/port to listen on. `0.0.0.0` is any v4 address, `[::]` is
    /// usually any address, v4 or v6 (but depends on `net.ipv6.bindv6only`
    /// sysctl)
    #[arg(long, short, default_value = "[::]:27056")]
    listen: SocketAddr,
    /// Name of the virtual device.
    #[arg(long, short, default_value = "hoipc")]
    name: String,
    /// Bus type of the virtual device.
    #[arg(long, short, default_value = "BUS_USB")]
    bus: BusType,
    /// Vendor ID of the virtual device.
    #[arg(long, short, default_value_t = 1)]
    vendor_id: u16,
    /// Product ID of the virtual device.
    #[arg(long, short, default_value_t = 1)]
    product_id: u16,
    /// Product version of the virtual device.
    #[arg(long, default_value_t = 1)]
    product_version: u16,
    /// Disable high-resolution scrolling events in the device description.
    #[arg(long)]
    no_high_res_scroll: bool,
    /// What multicast address to use for peer discovery. If listen address is a
    /// V6-only address, and this is not, will default to a V6 multicast
    /// address.
    #[arg(long, default_value = DEFAULT_MULTICAST_SOCKET_V4)]
    discovery_multicast: SocketAddr,
    /// Which network interface to run discovery on. If unspecified, will try to
    /// choose based on listen address if possible. Only used for IPv6
    /// multicast.
    #[arg(long)]
    discovery_ifname: Option<String>,
}

struct App<'a> {
    config: &'a Cli,
    disc: &'a Discovery,
    dev: VirtualDevice,
    pressed_keys: BTreeSet<KeyCode>,
}

impl<'a> App<'a> {
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
            let (res, ()) = tokio::try_join!(listener.accept().err_into(), this.disc.advertise())
                .context("Listener accept/advertise")?;
            res
        };
        tracing::info!(%remote, "Accepted remote connection");
        let mut framed = Framed::new(tcp_stream, Codec);
        tracing::info!("Starting event loop");
        while let Some(next) = framed.try_next().await.context("Get next data frame")? {
            if let evdev::EventSummary::Key(_, key_code, value) = next.destructure() {
                if matches!(value, 0) {
                    this.pressed_keys.remove(&key_code);
                } else {
                    this.pressed_keys.insert(key_code);
                }
            }
            this.dev.emit(&[next]).context("Emit event")?;
        }
        tracing::info!(%remote, "Connection closed normally");
        anyhow::Ok(())
    }
}

#[tokio::main]
async fn main() -> ExitCode {
    init_logging();

    let config = Cli::parse();
    let ctrl_c = tokio::signal::ctrl_c();

    match main_imp(config, ctrl_c).await {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            tracing::error!("{e:?}");
            ExitCode::FAILURE
        }
    }
}

async fn main_imp(mut config: Cli, ctrl_c: impl Future) -> anyhow::Result<()> {
    if let SocketAddr::V6(addr) = &mut config.listen {
        let iface = hid_over_ip::discovery::guess_iface(
            SocketAddr::V6(*addr),
            config.discovery_ifname.as_deref(),
        )
        .context("Guess v6 interface")?;
        addr.set_scope_id(iface);
        if !addr.ip().is_unspecified() && config.discovery_multicast.is_ipv4() {
            let mut def: SocketAddrV6 = DEFAULT_MULTICAST_SOCKET_V6.parse().unwrap();
            def.set_port(config.discovery_multicast.port());
            def.set_scope_id(iface);
            config.discovery_multicast = def.into();
            tracing::warn!(
                discovery_multicast = %config.discovery_multicast,
                "Multicast address changed to ipv6!"
            );
        }
    }

    let disc = Discovery::new(
        config.discovery_multicast,
        config.discovery_ifname.as_deref(),
        config.listen,
    )
    .await
    .context("Bind discovery")?;

    tokio::select! {
        _ = ctrl_c => Ok(()),
        res = disc.respond() => res,
        res = imp(&config, &disc) => res,
    }
}

async fn imp(config: &Cli, disc: &Discovery) -> anyhow::Result<()> {
    let mut app = App::new(config, disc).context("Construct App")?;
    loop {
        if let Err(e) = app.connect_loop().await {
            tracing::error!("{e:?}");
        }
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
