use std::{
    net::{IpAddr, Ipv4Addr, SocketAddr},
    task::Poll,
    time::Duration,
};

use anyhow::Context;
use futures::{Stream, never::Never};

pub const DEFAULT_MULTICAST_SOCKET_V4: &str = "224.0.0.83:27056";
pub const DEFAULT_MULTICAST_SOCKET_V6: &str = "[ff02::686F:6970]:27056";

pub struct Discovery {
    port: [u8; 2],
    socket: tokio::net::UdpSocket,
    disc_mcst: SocketAddr,
}

impl Discovery {
    pub async fn new(
        mut discovery_multicast: SocketAddr,
        discovery_ifname: Option<&str>,
        bind_addr: SocketAddr,
    ) -> anyhow::Result<Self> {
        let mut discovery_sock = bind_addr;
        discovery_sock.set_port(discovery_multicast.port());
        let socket = tokio::net::UdpSocket::bind(discovery_sock)
            .await
            .context("Bind UDP socket")?;
        match &mut discovery_multicast {
            SocketAddr::V4(ipv4_addr) => {
                socket
                    .join_multicast_v4(
                        *ipv4_addr.ip(),
                        match bind_addr.ip() {
                            IpAddr::V4(ipv4_addr) => ipv4_addr,
                            IpAddr::V6(_) => Ipv4Addr::UNSPECIFIED,
                        },
                    )
                    .context("Join V4 multicast")?;
            }
            SocketAddr::V6(ipv6_addr) => {
                let iface =
                    guess_iface(bind_addr, discovery_ifname).context("Guess V6 interface")?;
                socket
                    .join_multicast_v6(ipv6_addr.ip(), iface)
                    .context("Join V6 multicast")?;
                ipv6_addr.set_scope_id(iface);
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
            port: bind_addr.port().to_be_bytes(),
            disc_mcst: discovery_multicast,
        })
    }

    pub async fn respond(&self) -> anyhow::Result<()> {
        let mut buf = vec![0u8; 0xFFFF].into_boxed_slice();
        loop {
            let (sz, addr) = self
                .socket
                .recv_from(&mut buf)
                .await
                .context("Recv request from multicast socket")?;
            if !matches!(sz, 2) || !matches!(buf[..2], [0, 0]) {
                continue;
            }
            tracing::info!(
                requester = %addr.ip(),
                multicast_socket = %self.disc_mcst,
                "Got discovery request"
            );
            self.socket
                .send_to(
                    &self.port,
                    SocketAddr::new(addr.ip(), self.disc_mcst.port()),
                )
                .await
                .context("Send response to UDP socket")?;
            tracing::info!(
                requester = %addr.ip(),
                self_addr = %self.socket.local_addr().context("Get socket local address")?.ip(),
                port = u16::from_be_bytes(self.port),
                "Responded to discovery request"
            );
        }
    }

    pub async fn advertise(&self) -> anyhow::Result<()> {
        self.socket
            .send_to(&self.port, self.disc_mcst)
            .await
            .context("Advertise to UDP socket")?;
        tracing::info!(
            self_addr = %self.socket.local_addr().context("Get socket local address")?.ip(),
            multicast_socket = %self.disc_mcst,
            port = u16::from_be_bytes(self.port),
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
                .send_to(&[0, 0], self.disc_mcst)
                .await
                .context("Send discovery request to UDP socket")?;
        }
    }

    pub fn discovered(&self) -> impl Stream<Item = anyhow::Result<SocketAddr>> {
        let mut buf = vec![0u8; 0xFFFF].into_boxed_slice();
        futures::stream::poll_fn(move |cx| {
            let mut buf_read = tokio::io::ReadBuf::new(&mut buf);
            let (port, ip) = loop {
                let recv = futures::ready!(self.socket.poll_recv_from(cx, &mut buf_read));
                let addr = match recv {
                    Ok(x) => x,
                    Err(e) => return Poll::Ready(Some(Err(e.into()))),
                };
                let buf_trunc = buf_read.filled();
                if !matches!(buf_trunc.len(), 2) {
                    continue;
                }
                let port = u16::from_be_bytes([buf_trunc[0], buf_trunc[1]]);
                if matches!(port, 0) {
                    continue;
                }
                break (port, addr.ip());
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

pub fn guess_iface(bind_addr: SocketAddr, discovery_ifname: Option<&str>) -> anyhow::Result<u32> {
    let from_addr = || {
        let SocketAddr::V6(bind_addr) = bind_addr else {
            return None;
        };
        if bind_addr.scope_id() != 0 {
            return Some(Ok(bind_addr.scope_id()));
        }
        let ifs = getifaddrs::InterfaceFilter::new()
            .v6()
            .collect()
            .context("Getting interface addresses");
        let ifs = match ifs {
            Ok(x) => x,
            Err(e) => return Some(Err(e)),
        };
        for (iidx, addrs) in ifs.iter() {
            if let Some(addrs) = addrs.address.get_all(getifaddrs::AddressFamily::V6)
                && addrs.iter().any(|x| {
                    x.ip_addr().is_some_and(|x| {
                        if let IpAddr::V6(addr) = x {
                            &addr == bind_addr.ip()
                        } else {
                            false
                        }
                    })
                })
            {
                return Some(Ok(*iidx));
            }
        }
        None
    };
    Ok(discovery_ifname
        .map(getifaddrs::if_nametoindex)
        .transpose()
        .context("if_nametoindex")?
        .map(Ok)
        .or_else(from_addr)
        .transpose()?
        .unwrap_or(0))
}
