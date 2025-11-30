mod dump_evts;
mod magic;

use std::{
    collections::{BTreeSet, HashSet, VecDeque},
    net::{IpAddr, Ipv6Addr, SocketAddr, ToSocketAddrs},
    process::ExitCode,
    sync::Mutex,
    time::Duration,
};

use anyhow::{Context, anyhow};
use clap::Parser;
use evdev::KeyCode;
use futures::{SinkExt, StreamExt, TryStreamExt};
use hid_over_ip::{
    codec::Codec,
    discovery::{DEFAULT_MULTICAST_SOCKET_V4, Discovery},
    init_logging,
};
use tokio_util::codec::Framed;

use self::magic::Magic;

/// HoIP -- HID-over-IP. Share keyboard and mouse (or other HID inputs) over
/// TCP/IP.
///
/// HoIP "server". This program needs to run on the system that has the physical
/// input devices, and will send events from those devices over the network.
#[derive(clap::Parser)]
#[command(version)]
struct Cli {
    /// Devices to grab events from. Either path to /dev/input/event*, a name,
    /// or a unique identifier. Use `--list-devices` to get a list.
    #[arg(long, short, required_unless_present_any = ["list_devices", "dump_events"])]
    device: Vec<String>,
    /// Clients to send events to. Only one client can be active at a time, will
    /// round-robin between them. If unspecified, LAN multicast discovery will
    /// be used.
    #[arg(long, short, value_parser = parse_socketaddr)]
    connect: Vec<SocketAddr>,
    /// List devices and exit.
    #[arg(long, short, conflicts_with_all = ["device", "connect"])]
    list_devices: bool,
    /// Dump all events to stdout from all devices listed with `--device`.
    #[arg(long, conflicts_with_all = ["list_devices", "connect"])]
    dump_events: bool,
    /// Keys, when pressed, will release the grab or connect to the next client.
    #[arg(long, short, default_values = ["KEY_LEFTCTRL","KEY_LEFTSHIFT","KEY_F12"])]
    magic_key: Vec<KeyCode>,
    /// Connect immediately on start. If not set, will wait for magic key first.
    #[arg(long)]
    connect_on_start: bool,
    /// What multicast address to use for peer discovery. If
    /// `--discovery-bind-addr` is V6 while this is V4, will default to a V6
    /// instead.
    #[arg(long, default_value = DEFAULT_MULTICAST_SOCKET_V4)]
    discovery_multicast: SocketAddr,
    /// Force IPv6 address for discovery. Ignored if `--discovery-bind-addr` is
    /// IPv4-only.
    #[arg(long)]
    discovery_force_v6: bool,
    /// Which network interface to run discovery on. Will try to guess if
    /// unspecified.
    #[arg(long)]
    discovery_ifname: Option<String>,
    /// Which address to bind to when doing peer discovery. Will default to
    /// wildcard if unspecified.
    #[arg(long)]
    discovery_bind_addr: Option<IpAddr>,
    /// How often to broadcast discovery request during peer discovery. Should
    /// not be smaller than roughly how long peers are expected to reply.
    #[arg(long, default_value = "300ms", value_parser = humantime::parse_duration)]
    discovery_request_period: Duration,
    /// How long to attempt repeated discovery, if we have cached peers that are
    /// still alive. This delay will happen every time when switching between
    /// peers, so it should be reasonably small.
    #[arg(long, default_value = "500ms", value_parser = humantime::parse_duration)]
    discovery_cache_timeout: Duration,
    /// How long to attempt discovery before giving up if we have no cached
    /// peers. When this timeout elapses, pressing magic key would be required.
    /// Serves to prevent spontaneous unexpected connections to peers that
    /// suddenly come online.
    #[arg(long, default_value = "3s", value_parser = humantime::parse_duration)]
    discovery_timeout: Duration,
}

fn parse_socketaddr(addr: &str) -> anyhow::Result<SocketAddr> {
    addr.to_socket_addrs()?
        .next()
        .ok_or_else(|| anyhow!("{addr} did not resolve to an address"))
}

#[tokio::main]
async fn main() -> ExitCode {
    init_logging();

    let config = Cli::parse();

    if config.list_devices {
        for (path, dev) in evdev::enumerate() {
            println!(
                "{}: name={}, uniq={}, {:?}",
                path.display(),
                dev.name().unwrap_or("unset"),
                dev.unique_name().unwrap_or("unset"),
                dev.supported_events(),
            );
        }
        return ExitCode::SUCCESS;
    }

    tokio::select! {
        _ = tokio::signal::ctrl_c() => ExitCode::SUCCESS,
        res = imp(config) => match res {
            Ok(()) => ExitCode::SUCCESS,
            Err(error) => {
                tracing::error!("{error:?}");
                ExitCode::FAILURE
            },
        },
    }
}

async fn imp(mut config: Cli) -> anyhow::Result<()> {
    let mut disc_bind_sock = SocketAddr::new(
        config
            .discovery_bind_addr
            .unwrap_or(Ipv6Addr::UNSPECIFIED.into()),
        0,
    );
    hid_over_ip::fix_socket_addr_iface(
        &mut disc_bind_sock,
        &mut config.discovery_multicast,
        config.discovery_ifname.as_deref(),
        config.discovery_force_v6,
    )?;

    let mut requested_devices: HashSet<_> = config.device.iter().map(|x| x.as_str()).collect();
    let mut devices: Vec<evdev::Device> = vec![];
    for (path, dev) in evdev::enumerate() {
        let matches = if requested_devices.remove(&*path.to_string_lossy()) {
            true
        } else if let Some(name) = dev.name()
            && requested_devices.remove(name)
        {
            true
        } else if let Some(name) = dev.unique_name()
            && requested_devices.remove(name)
        {
            true
        } else {
            false
        };
        if matches {
            devices.push(dev);
        }
    }

    for i in requested_devices.iter() {
        tracing::warn!(device = i, "Device not found");
    }

    tracing::info!("Opened devices");
    let streams: Vec<_> = devices
        .into_iter()
        .map(|dev| dev.into_event_stream())
        .collect::<Result<_, _>>()
        .context("Collect event streams")?;

    let mut udev_stream = futures::stream::select_all(streams).err_into();

    if config.dump_events {
        return dump_evts::dump_events(&mut udev_stream).await;
    }

    let discovery;
    let invalid_peers = Mutex::new(BTreeSet::<SocketAddr>::new());
    let remotes = if config.connect.is_empty() {
        discovery = Discovery::new(config.discovery_multicast, disc_bind_sock)
            .await
            .context("Create discovery")?;
        struct St<S> {
            cache: VecDeque<SocketAddr>,
            discovered: S,
        }
        let st = Box::new(St {
            cache: VecDeque::<SocketAddr>::new(),
            discovered: discovery.discovered(),
        });
        let return_on_timeout = futures::stream::unfold(st, |mut st| async {
            {
                let mut bad = invalid_peers.lock().unwrap();
                st.cache.retain(|elt| !bad.contains(elt));
                bad.clear();
            }
            let try_next = async {
                loop {
                    let next = st.discovered.next().await?;
                    // if it's already in the cache, fish for another
                    if let Ok(value) = next
                        && st.cache.contains(&value)
                    {
                        continue;
                    }
                    break Some(next);
                }
            };
            let discover = discovery.discover(config.discovery_request_period);
            let timeout = tokio::time::sleep(config.discovery_cache_timeout);
            let value = tokio::select! {
                peer = try_next => Some(peer?),
                err = discover => Some(err.map(|never| match never {})),
                _ = timeout, if !st.cache.is_empty() => None,
            };
            let value = value.unwrap_or_else(|| {
                let peer = st.cache.pop_front().unwrap();
                tracing::info!(%peer, "No new discoveries, using peer from cache");
                Ok(peer)
            });
            if let Ok(value) = value {
                st.cache.push_back(value);
            }
            Some((value, st))
        });
        return_on_timeout.left_stream()
    } else {
        futures::stream::iter(config.connect.iter().cycle())
            .map(|x| Ok(*x))
            .right_stream()
    };
    let mut remotes = std::pin::pin!(remotes);

    let mut do_wait = !config.connect_on_start;

    loop {
        if do_wait {
            Magic::wait(&config.magic_key, &mut udev_stream)
                .await
                .context("Waiting for magic")?;
        }
        let Ok(remote) = tokio::time::timeout(config.discovery_timeout, remotes.try_next()).await
        else {
            // timed out
            tracing::warn!("No remote found, timeout elapsed");
            continue;
        };
        let Some(remote) = remote.context("While getting remote peer")? else {
            // stream ended
            break Ok(());
        };
        tracing::info!(remote = %remote, "Connecting...");
        let mut magic = false;
        if let Err(e) = connect(remote, &config.magic_key, &mut udev_stream).await {
            match e {
                magic::Error::MagicKey => {
                    tracing::info!("Magic key pressed");
                    magic = true;
                }
                magic::Error::Other(e) => {
                    invalid_peers.lock().unwrap().insert(remote);
                    tracing::error!("{e:?}");
                }
            }
        }
        let is_grabbed = udev_stream
            .get_mut()
            .iter()
            .any(|x| x.device().is_grabbed());
        if is_grabbed {
            // managed to connect to a remote, however briefly. wait for magic
            // next time around.
            do_wait = true;
            if !magic {
                // connection terminated unexpectedly. to prevent
                // surprises, wait for magic key, then ungrab.
                Magic::wait(&config.magic_key, &mut udev_stream)
                    .await
                    .context("Wating for magic")?;
            }
            for dev in udev_stream.get_mut().iter_mut() {
                dev.device_mut().ungrab().context("Ungrab device")?;
            }
            tracing::info!("Ungrabbed devices");
        }
    }
}

async fn connect(
    connect: SocketAddr,
    magic_key: &[KeyCode],
    udev_stream: &mut futures::stream::ErrInto<
        futures::stream::SelectAll<evdev::EventStream>,
        anyhow::Error,
    >,
) -> Result<(), magic::Error<anyhow::Error>> {
    let tcp_stream = tokio::net::TcpStream::connect(connect)
        .await
        .context("Open TCP stream")?;
    tracing::info!(remote = %connect, "Connected to remote");
    let mut framed = Framed::new(tcp_stream, Codec).sink_err_into();
    for dev in udev_stream.get_mut().iter_mut() {
        dev.device_mut().grab().context("Grab device")?;
    }
    tracing::info!("Grabbed devices");
    framed
        .send_all(&mut Magic::map_stream(magic_key, udev_stream))
        .await?;
    Ok::<_, magic::Error<_>>(())
}
