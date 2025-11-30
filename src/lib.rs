use std::net::{IpAddr, SocketAddr, SocketAddrV6};

use anyhow::Context;

pub mod codec;
pub mod discovery;

pub fn init_logging() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::builder()
                .with_default_directive(tracing::level_filters::LevelFilter::INFO.into())
                .from_env_lossy(),
        )
        .pretty()
        .init();
}

pub fn fix_socket_addr_iface(
    socket_to_fix: &mut SocketAddr,
    discovery_multicast: &mut SocketAddr,
    discovery_ifname: Option<&str>,
    force_v6: bool,
) -> anyhow::Result<()> {
    if let SocketAddr::V6(addr) = &mut *socket_to_fix {
        let iface =
            guess_iface(SocketAddr::V6(*addr), discovery_ifname).context("Guess v6 interface")?;
        addr.set_scope_id(iface);
        if force_v6 || !socket_to_fix.ip().is_unspecified() && discovery_multicast.is_ipv4() {
            let mut def: SocketAddrV6 = discovery::DEFAULT_MULTICAST_SOCKET_V6.parse().unwrap();
            def.set_port(discovery_multicast.port());
            def.set_scope_id(iface);
            *discovery_multicast = SocketAddr::V6(def);
            tracing::warn!(
                discovery_multicast = %discovery_multicast,
                "Multicast address changed to ipv6!"
            );
        }
    }
    Ok(())
}

fn guess_iface(bind_addr: SocketAddr, discovery_ifname: Option<&str>) -> anyhow::Result<u32> {
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
        .map(|ifname| getifaddrs::if_nametoindex(ifname).context("if_nametoindex"))
        .or_else(from_addr)
        .transpose()?
        .unwrap_or(0))
}
