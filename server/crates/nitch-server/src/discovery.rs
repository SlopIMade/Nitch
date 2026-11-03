//! Announces the server on the local network as `_nitch._tcp`, so the TV app finds it without
//! typing an address (M5). Needs multicast on the network (host networking in Docker).

use anyhow::Result;
use mdns_sd::{ServiceDaemon, ServiceInfo};

pub const SERVICE_TYPE: &str = "_nitch._tcp.local.";

/// Keeps the announcement alive as long as it's held.
pub struct Announcement {
    _daemon: ServiceDaemon,
}

pub fn announce(port: u16) -> Result<Announcement> {
    let daemon = ServiceDaemon::new()?;
    let host = std::fs::read_to_string("/etc/hostname")
        .ok()
        .map(|h| h.trim().to_string())
        .filter(|h| !h.is_empty())
        .unwrap_or_else(|| "nitch".into());
    let info = ServiceInfo::new(
        SERVICE_TYPE,
        "Nitch",
        &format!("{host}.local."),
        "",
        port,
        &[("version", env!("CARGO_PKG_VERSION")), ("path", "/")][..],
    )?
    .enable_addr_auto();
    daemon.register(info)?;
    tracing::info!(%host, port, "announced on the local network ({SERVICE_TYPE})");
    Ok(Announcement { _daemon: daemon })
}
