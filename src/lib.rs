use std::net::{SocketAddr, SocketAddrV6};

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
        let iface = discovery::guess_iface(SocketAddr::V6(*addr), discovery_ifname)
            .context("Guess v6 interface")?;
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
