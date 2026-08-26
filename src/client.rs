use crate::auth::CloudAuth;
use serde_json::{Value, json};
use std::collections::HashMap;
use std::time::Duration;

/// An authenticated Keystone session: the token plus a resolved map of
/// `service_type -> public endpoint URL`, so callers just ask for e.g.
/// "compute" and get back the right URL for this cloud's region.
pub struct Session {
    http: reqwest::blocking::Client,
    token: String,
    endpoints: HashMap<String, String>,
}

impl Session {
    pub fn login(auth: &CloudAuth) -> Result<Session, String> {
        let http = reqwest::blocking::Client::builder()
            .timeout(Duration::from_secs(15))
            .build()
            .map_err(|e| e.to_string())?;

        let body = json!({
            "auth": {
                "identity": {
                    "methods": ["password"],
                    "password": {
                        "user": {
                            "name": auth.username,
                            "domain": { "name": auth.user_domain_name },
                            "password": auth.password,
                        }
                    }
                },
                "scope": {
                    "project": {
                        "name": auth.project_name,
                        "domain": { "name": auth.project_domain_name },
                    }
                }
            }
        });

        let url = format!("{}/auth/tokens", auth.auth_url.trim_end_matches('/'));
        let resp = http
            .post(&url)
            .json(&body)
            .send()
            .map_err(|e| format!("could not reach {url}: {e}"))?;

        if !resp.status().is_success() {
            let status = resp.status();
            let text = resp.text().unwrap_or_default();
            return Err(format!("Keystone rejected the login ({status}): {text}"));
        }

        let token = resp
            .headers()
            .get("X-Subject-Token")
            .and_then(|v| v.to_str().ok())
            .ok_or("Keystone response had no X-Subject-Token header")?
            .to_string();

        let parsed: Value = resp
            .json()
            .map_err(|e| format!("bad token response: {e}"))?;
        let endpoints = parse_catalog(&parsed, auth.region_name.as_deref());

        Ok(Session {
            http,
            token,
            endpoints,
        })
    }

    pub fn get(&self, service_type: &str, path: &str) -> Result<Value, String> {
        let base = self.endpoints.get(service_type).ok_or_else(|| {
            format!("no '{service_type}' endpoint in this cloud's service catalog")
        })?;
        let url = format!("{}{}", base.trim_end_matches('/'), path);
        let resp = self
            .http
            .get(&url)
            .header("X-Auth-Token", &self.token)
            .header("Accept", "application/json")
            .send()
            .map_err(|e| format!("request to {url} failed: {e}"))?;
        if !resp.status().is_success() {
            let status = resp.status();
            let text = resp.text().unwrap_or_default();
            return Err(format!("{url} returned {status}: {text}"));
        }
        resp.json()
            .map_err(|e| format!("bad response from {url}: {e}"))
    }
}

fn parse_catalog(token_response: &Value, region: Option<&str>) -> HashMap<String, String> {
    let mut endpoints = HashMap::new();
    let Some(catalog) = token_response
        .pointer("/token/catalog")
        .and_then(|v| v.as_array())
    else {
        return endpoints;
    };

    for service in catalog {
        let Some(service_type) = service.get("type").and_then(|v| v.as_str()) else {
            continue;
        };
        let Some(service_endpoints) = service.get("endpoints").and_then(|v| v.as_array()) else {
            continue;
        };

        let mut best: Option<(&str, u8)> = None;
        for ep in service_endpoints {
            let Some(url) = ep.get("url").and_then(|v| v.as_str()) else {
                continue;
            };
            let interface = ep.get("interface").and_then(|v| v.as_str()).unwrap_or("");
            let ep_region = ep.get("region").and_then(|v| v.as_str());
            if let Some(region) = region
                && ep_region.is_some_and(|r| r != region)
            {
                continue;
            }
            let priority = if interface == "public" { 2 } else { 1 };
            if best.is_none_or(|(_, p)| priority > p) {
                best = Some((url, priority));
            }
        }
        if let Some((url, _)) = best {
            endpoints.insert(service_type.to_string(), url.to_string());
        }
    }
    endpoints
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_catalog() -> Value {
        json!({
            "token": {
                "catalog": [
                    {
                        "type": "compute",
                        "endpoints": [
                            { "interface": "admin", "region": "RegionOne", "url": "http://admin:8774/v2.1" },
                            { "interface": "public", "region": "RegionOne", "url": "http://public:8774/v2.1" }
                        ]
                    }
                ]
            }
        })
    }

    #[test]
    fn parse_catalog_prefers_public_interface() {
        let endpoints = parse_catalog(&sample_catalog(), None);
        assert!(endpoints["compute"].contains("public"));
    }

    #[test]
    fn parse_catalog_filters_by_region() {
        let endpoints = parse_catalog(&sample_catalog(), Some("RegionOne"));
        assert_eq!(endpoints["compute"], "http://public:8774/v2.1");
    }
}
