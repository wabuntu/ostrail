use crate::client::Session;
use serde::Deserialize;
use std::collections::BTreeSet;

/// Every host running some part of Nova or Neutron, deduplicated. This
/// deliberately doesn't try to map a service's `binary` name to a
/// specific systemd unit — that naming varies too much across DevStack,
/// RDO, and Ubuntu-packaged deployments to guess reliably. Instead
/// ostrail just SSHes to each host and greps its *entire* journal.
pub fn discover_hosts(session: &Session) -> Vec<String> {
    let mut hosts = BTreeSet::new();

    if let Ok(body) = session.get("compute", "/os-services")
        && let Ok(parsed) = serde_json::from_value::<NovaServiceList>(body)
    {
        for s in parsed.services {
            hosts.insert(s.host);
        }
    }

    if let Ok(body) = session.get("network", "/v2.0/agents")
        && let Ok(parsed) = serde_json::from_value::<NeutronAgentList>(body)
    {
        for a in parsed.agents {
            hosts.insert(a.host);
        }
    }

    hosts.into_iter().collect()
}

#[derive(Debug, Deserialize)]
struct NovaServiceList {
    services: Vec<NovaService>,
}

#[derive(Debug, Deserialize)]
struct NovaService {
    host: String,
}

#[derive(Debug, Deserialize)]
struct NeutronAgentList {
    agents: Vec<NeutronAgent>,
}

#[derive(Debug, Deserialize)]
struct NeutronAgent {
    host: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nova_service_list_parses_host_field() {
        let raw = serde_json::json!({
            "services": [
                {"host": "compute-1", "binary": "nova-compute"},
                {"host": "compute-2", "binary": "nova-compute"}
            ]
        });
        let parsed: NovaServiceList = serde_json::from_value(raw).unwrap();
        let hosts: Vec<&str> = parsed.services.iter().map(|s| s.host.as_str()).collect();
        assert_eq!(hosts, vec!["compute-1", "compute-2"]);
    }

    #[test]
    fn neutron_agent_list_parses_host_field() {
        let raw = serde_json::json!({
            "agents": [{"host": "net-1", "agent_type": "DHCP agent"}]
        });
        let parsed: NeutronAgentList = serde_json::from_value(raw).unwrap();
        assert_eq!(parsed.agents[0].host, "net-1");
    }
}
