use std::{
    collections::HashMap,
    net::{SocketAddr, SocketAddrV4, SocketAddrV6},
    str::FromStr,
    time::Duration,
};

use async_trait::async_trait;
use mdns_sd::{ScopedIp, ServiceDaemon, ServiceEvent, ServiceInfo};
use tracing::warn;

use crate::{
    Capabilities, DiscoveryProvider, Endpoint, Observation, PresenceId, ProtocolRange,
    ProviderContext, ProviderError, ProviderId, ProviderKind, ProviderState,
};

use super::presence::eligible_ip_addresses;

pub const HALO_SERVICE_TYPE: &str = "_halo._udp.local.";

#[derive(Clone, Debug)]
pub struct MdnsConfig {
    pub service_type: String,
    pub observation_ttl: Duration,
}

impl Default for MdnsConfig {
    fn default() -> Self {
        Self {
            service_type: HALO_SERVICE_TYPE.to_owned(),
            observation_ttl: Duration::from_secs(120),
        }
    }
}

pub struct MdnsProvider {
    id: ProviderId,
    config: MdnsConfig,
}

impl MdnsProvider {
    #[must_use]
    pub fn new(config: MdnsConfig) -> Self {
        Self {
            id: ProviderId::builtin(ProviderKind::Mdns, "mdns"),
            config,
        }
    }
}

impl Default for MdnsProvider {
    fn default() -> Self {
        Self::new(MdnsConfig::default())
    }
}

#[async_trait]
impl DiscoveryProvider for MdnsProvider {
    fn id(&self) -> ProviderId {
        self.id.clone()
    }

    async fn run(&self, context: ProviderContext) -> Result<(), ProviderError> {
        validate_service_type(&self.config.service_type)?;
        if self.config.observation_ttl.is_zero() {
            return Err(ProviderError::InvalidConfig(
                "mDNS observation TTL must be non-zero".to_owned(),
            ));
        }

        let daemon = ServiceDaemon::new().map_err(mdns_error)?;
        let browse = daemon
            .browse(&self.config.service_type)
            .map_err(mdns_error)?;
        let local = context.local();
        let instance = format!("halo-{}", local.presence_id.short_hex());
        let hostname = format!("{instance}.local.");
        let properties = [
            ("id", local.presence_id.to_string()),
            ("min", local.protocol.min().to_string()),
            ("max", local.protocol.max().to_string()),
            ("cap", format!("{:016x}", local.capabilities.bits())),
        ];
        let addresses = eligible_ip_addresses()?;
        if addresses.is_empty() {
            return Err(ProviderError::Unavailable(
                "no eligible addresses to publish through mDNS".to_owned(),
            ));
        }
        let service = ServiceInfo::new(
            &self.config.service_type,
            &instance,
            &hostname,
            addresses.as_slice(),
            local.quic_port,
            &properties[..],
        )
        .map_err(mdns_error)?;
        let fullname = service.get_fullname().to_owned();
        daemon.register(service).map_err(mdns_error)?;
        context.set_state(self.id(), ProviderState::Ready).await?;

        let cancel = context.cancellation_token();
        let mut resolved_names: HashMap<String, PresenceId> = HashMap::new();
        let result = loop {
            tokio::select! {
                () = cancel.cancelled() => break Ok(()),
                event = browse.recv_async() => {
                    let event = match event {
                        Ok(event) => event,
                        Err(_) => break Err(ProviderError::EventStreamClosed),
                    };
                    match event {
                        ServiceEvent::ServiceResolved(service) => {
                            if let Some(observation) = resolved_observation(
                                &self.id,
                                &service,
                                self.config.observation_ttl,
                            ) {
                                resolved_names.insert(
                                    service.get_fullname().to_owned(),
                                    observation.presence_id,
                                );
                                context.observe(observation).await?;
                            }
                        }
                        ServiceEvent::ServiceRemoved(_, removed_fullname) => {
                            if let Some(presence_id) = resolved_names.remove(&removed_fullname) {
                                context.withdraw(self.id(), presence_id).await?;
                            }
                        }
                        ServiceEvent::SearchStopped(_) if !cancel.is_cancelled() => {
                            break Err(ProviderError::EventStreamClosed);
                        }
                        _ => {}
                    }
                }
            }
        };

        if let Err(error) = daemon.stop_browse(&self.config.service_type) {
            warn!(provider = %self.id, %error, "failed to stop mDNS browse cleanly");
        }
        if let Err(error) = daemon.unregister(&fullname) {
            warn!(provider = %self.id, %error, "failed to unregister mDNS service cleanly");
        }
        if let Err(error) = daemon.shutdown() {
            warn!(provider = %self.id, %error, "failed to shut down mDNS daemon cleanly");
        }
        result
    }
}

fn validate_service_type(service_type: &str) -> Result<(), ProviderError> {
    if service_type.starts_with('_')
        && (service_type.ends_with("._udp.local.") || service_type.ends_with("._tcp.local."))
    {
        Ok(())
    } else {
        Err(ProviderError::InvalidConfig(format!(
            "invalid mDNS service type '{service_type}'"
        )))
    }
}

fn resolved_observation(
    provider: &ProviderId,
    service: &mdns_sd::ResolvedService,
    ttl: Duration,
) -> Option<Observation> {
    let presence_id = PresenceId::from_str(service.get_property_val_str("id")?).ok()?;
    let min = service.get_property_val_str("min")?.parse::<u16>().ok()?;
    let max = service.get_property_val_str("max")?.parse::<u16>().ok()?;
    let protocol = ProtocolRange::new(min, max).ok()?;
    let capabilities = u64::from_str_radix(service.get_property_val_str("cap")?, 16).ok()?;
    let port = service.get_port();

    let mut endpoints = service
        .get_addresses()
        .iter()
        .filter_map(|address| match address {
            ScopedIp::V4(address) => {
                Endpoint::quic(SocketAddr::V4(SocketAddrV4::new(*address.addr(), port))).ok()
            }
            ScopedIp::V6(address) => {
                let scope = address.ip_scope_id();
                Endpoint::quic(SocketAddr::V6(SocketAddrV6::new(
                    *address.addr(),
                    port,
                    0,
                    scope,
                )))
                .ok()
            }
            _ => None,
        })
        .collect::<Vec<_>>();
    endpoints.sort_unstable();
    endpoints.dedup();

    Some(Observation {
        provider: provider.clone(),
        presence_id,
        protocol,
        capabilities: Capabilities::from_bits(capabilities),
        sequence: 0,
        endpoints,
        ttl,
        round_trip_time: None,
    })
}

fn mdns_error(error: mdns_sd::Error) -> ProviderError {
    ProviderError::Network(error.to_string())
}

trait ScopedIpV6Ext {
    fn ip_scope_id(&self) -> u32;
}

impl ScopedIpV6Ext for mdns_sd::ScopedIpV6 {
    fn ip_scope_id(&self) -> u32 {
        if self.addr().is_unicast_link_local() {
            self.scope_id().index
        } else {
            0
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_invalid_service_type() {
        assert!(validate_service_type("halo.local.").is_err());
        assert!(validate_service_type(HALO_SERVICE_TYPE).is_ok());
    }
}
