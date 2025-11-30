mod app;

use std::{net::SocketAddr, process::ExitCode};

use anyhow::Context;
use clap::Parser;
use evdev::BusType;
use hid_over_ip::{
    discovery::{DEFAULT_MULTICAST_SOCKET_V4, Discovery},
    init_logging,
};

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

#[tokio::main]
async fn main() -> ExitCode {
    init_logging();

    match main_imp(Cli::parse()).await {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            tracing::error!("{e:?}");
            ExitCode::FAILURE
        }
    }
}

async fn main_imp(mut config: Cli) -> anyhow::Result<()> {
    let ctrl_c = tokio::signal::ctrl_c();

    hid_over_ip::fix_socket_addr_iface(
        &mut config.listen,
        &mut config.discovery_multicast,
        config.discovery_ifname.as_deref(),
        false,
    )?;

    let disc = Discovery::new(config.discovery_multicast, config.listen)
        .await
        .context("Bind discovery")?;

    tokio::select! {
        _ = ctrl_c => Ok(()),
        res = disc.respond() => res,
        res = app::App::run(&config, &disc) => res,
    }
}
