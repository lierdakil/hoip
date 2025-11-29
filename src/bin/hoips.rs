use std::{
    collections::{HashMap, HashSet, hash_map::Entry},
    time::Duration,
};

use clap::Parser;
use evdev::{EventSummary, KeyCode};
use futures::{SinkExt, StreamExt, TryStreamExt};
use hid_over_ip::{Codec, init_logging};
use tokio::sync::Mutex;
use tokio_util::codec::Framed;

#[derive(clap::Parser)]
struct Cli {
    /// Devices to grab events from. Either path to /dev/input/event*, a name,
    /// or a unique identifier. Use `--list-devices` to get a list.
    #[arg(long, short)]
    device: Vec<String>,
    /// Clients to send events to. Only one client can be active at a time, will
    /// round-robin between them.
    #[arg(long, short)]
    connect: Vec<String>,
    /// List devices and exit.
    #[arg(long, short)]
    list_devices: bool,
    /// Keys, when pressed, will release the grab or connect to the next client.
    #[arg(long, short, default_values = ["KEY_LEFTCTRL","KEY_LEFTSHIFT","KEY_F12"])]
    magic_key: Vec<KeyCode>,
}

enum Error {
    MagicKey,
    Other(anyhow::Error),
}

impl<E> From<E> for Error
where
    E: std::error::Error + Send + Sync + 'static,
{
    #[cold]
    fn from(error: E) -> Self {
        Error::Other(anyhow::Error::from(error))
    }
}

struct Magic {
    keys: HashMap<KeyCode, i32>,
    armed: bool,
}

impl Magic {
    fn from_iter<'a>(iter: impl IntoIterator<Item = &'a KeyCode>) -> Self {
        Self {
            keys: HashMap::from_iter(iter.into_iter().map(|k| (*k, 0))),
            armed: false,
        }
    }

    fn key(&mut self, key_code: KeyCode, value: i32) -> bool {
        if let Entry::Occupied(mut entry) = self.keys.entry(key_code) {
            entry.insert(value);
            let next_armed = self.keys.values().all(|v| *v != 0);
            let prev_armed = std::mem::replace(&mut self.armed, next_armed);
            prev_armed && !next_armed
        } else {
            false
        }
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
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
        return Ok(());
    }

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
        .collect::<Result<_, _>>()?;

    let udev_stream = Mutex::new(futures::stream::select_all(streams).err_into());

    let connect = async |connect| {
        let tcp_stream = tokio::net::TcpStream::connect(connect).await?;
        tracing::info!(remote = connect, "Connected to remote");
        let mut framed = Framed::new(tcp_stream, Codec).sink_map_err(Error::Other);
        let udev_stream = &mut *udev_stream.lock().await;
        for dev in udev_stream.get_mut().iter_mut() {
            dev.device_mut().grab()?;
        }
        tracing::info!("Grabbed devices");
        let mut magic = Magic::from_iter(&config.magic_key);
        let mut udev_stream = udev_stream.map(|evt| match evt.as_ref().map(|x| x.destructure()) {
            Ok(EventSummary::Key(_, key_code, value)) if magic.key(key_code, value) => {
                Err(Error::MagicKey)
            }
            _ => evt.map_err(Error::Other),
        });
        framed.send_all(&mut udev_stream).await?;
        Ok::<_, Error>(())
    };

    let mut remotes = config.connect.iter().cycle();

    loop {
        let mut magic = false;
        let Some(remote) = remotes.next() else {
            break Ok(());
        };
        tracing::info!(remote = remote, "Connecting...");
        if let Err(e) = connect(remote).await {
            match e {
                Error::MagicKey => {
                    tracing::info!("Magic key pressed");
                    magic = true;
                }
                Error::Other(e) => {
                    tracing::error!(%e);
                }
            }
        }
        for dev in udev_stream.lock().await.get_mut().iter_mut() {
            dev.device_mut().ungrab()?;
        }
        tracing::info!("Ungrabbed devices");
        if magic {
            tracing::info!("Waiting for magic key...");
            let mut stream = udev_stream.lock().await;
            let mut magic = Magic::from_iter(&config.magic_key);
            while let Some(evt) = stream.try_next().await? {
                if let EventSummary::Key(_, key_code, value) = evt.destructure()
                    && magic.key(key_code, value)
                {
                    tracing::info!("Magic key pressed");
                    break;
                }
            }
        } else {
            // if it's not magic, it's error, wait a bit
            tokio::time::sleep(Duration::from_secs(3)).await;
        }
    }
}
