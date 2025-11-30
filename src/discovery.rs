use std::{
    net::{IpAddr, Ipv4Addr, SocketAddr},
    task::Poll,
    time::Duration,
};

use anyhow::Context;
use futures::{Stream, never::Never};

pub const DEFAULT_MULTICAST_SOCKET_V4: &str = "224.0.0.83:27056";
pub const DEFAULT_MULTICAST_SOCKET_V6: &str = "[ff02::686F:6970]:27056";
const DISC_REQ: Packet = Packet::new(0);
const DISC_REQ_REF: &[u8] = DISC_REQ.as_bytes();
const DISC_PFX: [u8; 4] = *b"HOIP";
const CRC8: Crc8 = Crc8::create(0x9b);

pub struct Crc8([u8; 256]);

impl Crc8 {
    pub const fn create(polynomial: u8) -> Crc8 {
        let mut table = [0u8; 256];
        let mut crc = 0x80;
        let mut i = 1u8;
        while i > 0 {
            crc = (crc << 1) ^ if crc & 0x80 != 0 { polynomial } else { 0 };
            let mut j = 0;
            while j < i {
                table[i.wrapping_add(j) as usize] = crc ^ table[j as usize];
                j += 1;
            }
            i <<= 1;
        }
        Crc8(table)
    }

    pub const fn calc(&self, buffer: &[u8], len: usize, mut crc: u8) -> u8 {
        let mut i = 0;
        while i < len {
            crc = self.0[(crc ^ buffer[i]) as usize];
            i += 1;
        }
        crc
    }
}

#[repr(C, packed(1))]
struct Packet {
    pfx: [u8; 4],
    port: u16,
    crc: u8,
}

impl Packet {
    const fn new(port: u16) -> Self {
        let mut this = Self {
            pfx: DISC_PFX,
            port,
            crc: 0,
        };
        this.update_crc();
        this
    }

    const fn update_crc(&mut self) {
        self.crc = self.crc();
    }

    const fn as_bytes(&self) -> &[u8; size_of::<Self>()] {
        assert!(align_of::<Self>() == 1);
        // SAFETY: tightly packed, hence casting to &[u8; size_of()] is valid.
        unsafe { &*(self as *const Self as *const [u8; size_of::<Self>()]) }
    }

    const fn try_from_bytes(bytes: &[u8]) -> Option<&Self> {
        assert!(align_of::<Self>() == 1);
        if bytes.len() != size_of::<Self>() {
            return None;
        }
        // SAFETY: all bit-patterns are valid for Packet, we asserted slice len
        // is exactly right, Packet is tightly packed, we asserted it's aligned.
        let pkt = unsafe { &*(bytes.as_ptr() as *const Self) };
        if !matches!(pkt.pfx, DISC_PFX) || !pkt.validate_crc() {
            return None;
        }
        Some(pkt)
    }

    const fn validate_crc(&self) -> bool {
        self.crc == self.crc()
    }

    const fn crc(&self) -> u8 {
        let bytes = self.as_bytes();
        CRC8.calc(bytes, bytes.len() - 1, 0)
    }
}

impl std::ops::Deref for Packet {
    type Target = [u8];

    fn deref(&self) -> &Self::Target {
        self.as_bytes()
    }
}

pub struct Discovery {
    bind: SocketAddr,
    socket: tokio::net::UdpSocket,
    disc_mcst: SocketAddr,
}

impl Discovery {
    pub async fn new(
        mut discovery_multicast: SocketAddr,
        bind_addr: SocketAddr,
    ) -> anyhow::Result<Self> {
        let mut discovery_sock = bind_addr;
        discovery_sock.set_port(discovery_multicast.port());
        let socket = tokio::net::UdpSocket::bind(discovery_sock)
            .await
            .context("Bind UDP socket")?;
        match &mut discovery_multicast {
            SocketAddr::V4(mcast_v4) => {
                socket
                    .join_multicast_v4(
                        *mcast_v4.ip(),
                        match bind_addr.ip() {
                            IpAddr::V4(ipv4_addr) => ipv4_addr,
                            IpAddr::V6(_) => Ipv4Addr::UNSPECIFIED,
                        },
                    )
                    .context("Join V4 multicast")?;
            }
            SocketAddr::V6(mcast_v6) => {
                let iface = match bind_addr {
                    SocketAddr::V4(_) => anyhow::bail!("Bind address is V4 but multicast is V6"),
                    SocketAddr::V6(v6) => v6.scope_id(),
                };
                socket
                    .join_multicast_v6(mcast_v6.ip(), iface)
                    .context("Join V6 multicast")?;
                mcast_v6.set_scope_id(iface);
            }
        }
        socket
            .set_multicast_loop_v4(false)
            .context("Disable V4 multicast loop")?;
        socket
            .set_multicast_loop_v6(false)
            .context("Disable V6 multicast loop")?;
        Ok(Self {
            socket,
            bind: bind_addr,
            disc_mcst: discovery_multicast,
        })
    }

    pub async fn respond(&self) -> anyhow::Result<()> {
        let mut buf = [0u8; size_of::<Packet>() + 1];
        loop {
            let (sz, addr) = self
                .socket
                .recv_from(&mut buf)
                .await
                .context("Recv request from multicast socket")?;
            if sz != size_of::<Packet>() || !matches!(&buf[..sz], DISC_REQ_REF) {
                continue;
            }
            tracing::info!(
                requester = %addr.ip(),
                multicast_socket = %self.disc_mcst,
                "Got discovery request"
            );
            self.socket
                .send_to(
                    &Packet::new(self.bind.port()),
                    SocketAddr::new(addr.ip(), self.disc_mcst.port()),
                )
                .await
                .context("Send response to UDP socket")?;
            tracing::info!(
                requester = %addr.ip(),
                self_addr = %self.bind,
                "Responded to discovery request"
            );
        }
    }

    pub async fn advertise(&self) -> anyhow::Result<()> {
        self.socket
            .send_to(&Packet::new(self.bind.port()), self.disc_mcst)
            .await
            .context("Advertise to UDP socket")?;
        tracing::info!(
            self_addr = %self.bind,
            multicast_socket = %self.disc_mcst,
            "Discovery advertisement"
        );
        Ok(())
    }

    pub async fn discover(&self, period: Duration) -> anyhow::Result<Never> {
        let mut interval = tokio::time::interval(period);
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            interval.tick().await;
            tracing::info!(
                multicast_socket = %self.disc_mcst,
                "Broadcast discovery request"
            );
            self.socket
                .send_to(DISC_REQ.as_bytes(), self.disc_mcst)
                .await
                .context("Send discovery request to UDP socket")?;
        }
    }

    pub fn discovered(&self) -> impl Stream<Item = anyhow::Result<SocketAddr>> {
        let mut buf = [0u8; size_of::<Packet>() + 1];
        futures::stream::poll_fn(move |cx| {
            let mut buf = tokio::io::ReadBuf::new(&mut buf);
            let (port, ip) = loop {
                let recv = futures::ready!(self.socket.poll_recv_from(cx, &mut buf));
                let addr = match recv {
                    Ok(x) => x,
                    Err(e) => return Poll::Ready(Some(Err(e.into()))),
                };
                let Some(pkt) = Packet::try_from_bytes(buf.filled()) else {
                    continue;
                };
                if matches!(pkt.port, 0) {
                    continue;
                }
                break (pkt.port, addr.ip());
            };
            let mut sock_addr = SocketAddr::new(ip, port);
            if let SocketAddr::V6(sock_addr) = &mut sock_addr
                && let SocketAddr::V6(mcast_addr) = &self.disc_mcst
            {
                sock_addr.set_scope_id(mcast_addr.scope_id());
            }
            tracing::info!(
                addr = %sock_addr,
                multicast_socket = %self.disc_mcst,
                "Got discovery response"
            );
            Poll::Ready(Some(Ok(sock_addr)))
        })
    }
}
