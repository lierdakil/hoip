use clap::Parser;
use evdev::{
    AttributeSet, BusType, InputId, KeyCode, PropType, RelativeAxisCode, uinput::VirtualDevice,
};
use futures::TryStreamExt;
use hid_over_ip::{Codec, init_logging};
use tokio_util::codec::Framed;

#[derive(clap::Parser)]
struct Cli {
    /// Address/port to listen on. `0.0.0.0` is any address.
    #[arg(long, short)]
    listen: String,
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
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    init_logging();

    let config = Cli::parse();

    loop {
        if let Err(e) = connect(&config).await {
            tracing::error!(%e);
        }
    }
}

async fn connect(config: &Cli) -> anyhow::Result<()> {
    let (tcp_stream, remote) = {
        let listener = tokio::net::TcpListener::bind(&config.listen).await?;
        tracing::info!(address = config.listen, "Started listener");
        listener.accept().await?
    };
    tracing::info!(%remote, "Accepted remote connection");
    let mut framed = Framed::new(tcp_stream, Codec);
    let dev = builder(config)?;
    let dev = dev.build()?;
    tracing::info!("Created virtual device");
    let mut udev_stream = dev.into_event_stream()?;
    tracing::info!("Starting event loop");
    while let Some(next) = framed.try_next().await? {
        udev_stream.device_mut().emit(&[next])?;
    }
    tracing::info!(%remote, "Connection closed normally");
    anyhow::Ok(())
}

fn builder(config: &Cli) -> anyhow::Result<evdev::uinput::VirtualDeviceBuilder<'_>> {
    // TODO: absolute, force feedback, switches
    anyhow::Ok(
        VirtualDevice::builder()?
            .name(&config.name)
            .input_id(InputId::new(
                config.bus,
                config.vendor_id,
                config.product_id,
                config.product_version,
            ))
            .with_properties(&AttributeSet::from_iter([PropType::POINTER]))?
            .with_keys(&AttributeSet::from_iter((0..560).map(KeyCode)))?
            .with_relative_axes(&AttributeSet::from_iter(
                (if config.no_high_res_scroll {
                    0..=10
                } else {
                    0..=12
                })
                .map(RelativeAxisCode),
            ))?,
    )
}
