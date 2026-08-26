use serde::Deserialize;
use std::path::PathBuf;

/// Everything needed for a password-scoped Keystone v3 login. Discovered
/// the same way the `openstack` CLI does: `OS_*` env vars first, then
/// `clouds.yaml`. There's no setup wizard here (unlike stackboard) - if
/// neither is found, ostrail just says so and points at `openrc.sh`.
pub struct CloudAuth {
    pub auth_url: String,
    pub username: String,
    pub password: String,
    pub project_name: String,
    pub user_domain_name: String,
    pub project_domain_name: String,
    pub region_name: Option<String>,
}

pub fn discover() -> Option<CloudAuth> {
    discover_from_env().or_else(discover_from_clouds_yaml)
}

fn discover_from_env() -> Option<CloudAuth> {
    let auth_url = std::env::var("OS_AUTH_URL").ok()?;
    let username = std::env::var("OS_USERNAME").ok()?;
    let password = std::env::var("OS_PASSWORD").ok()?;
    let project_name = std::env::var("OS_PROJECT_NAME").ok()?;
    Some(CloudAuth {
        auth_url,
        username,
        password,
        project_name,
        user_domain_name: std::env::var("OS_USER_DOMAIN_NAME").unwrap_or_else(|_| "Default".into()),
        project_domain_name: std::env::var("OS_PROJECT_DOMAIN_NAME")
            .unwrap_or_else(|_| "Default".into()),
        region_name: std::env::var("OS_REGION_NAME").ok(),
    })
}

#[derive(Debug, Deserialize)]
struct CloudsFile {
    clouds: std::collections::HashMap<String, CloudEntry>,
}

#[derive(Debug, Deserialize)]
struct CloudEntry {
    auth: CloudEntryAuth,
    #[serde(default)]
    region_name: Option<String>,
}

#[derive(Debug, Deserialize)]
struct CloudEntryAuth {
    auth_url: String,
    username: String,
    password: String,
    project_name: String,
    #[serde(default = "default_domain")]
    user_domain_name: String,
    #[serde(default = "default_domain")]
    project_domain_name: String,
}

fn default_domain() -> String {
    "Default".to_string()
}

fn clouds_yaml_search_paths() -> Vec<PathBuf> {
    let mut paths = vec![PathBuf::from("clouds.yaml")];
    if let Some(home) = std::env::var_os("HOME") {
        paths.push(PathBuf::from(&home).join(".config/openstack/clouds.yaml"));
    }
    paths.push(PathBuf::from("/etc/openstack/clouds.yaml"));
    paths
}

fn discover_from_clouds_yaml() -> Option<CloudAuth> {
    let cloud_name = std::env::var("OS_CLOUD").ok();

    for path in clouds_yaml_search_paths() {
        let Ok(contents) = std::fs::read_to_string(&path) else {
            continue;
        };
        let Ok(parsed) = serde_yaml::from_str::<CloudsFile>(&contents) else {
            continue;
        };
        let entry = match &cloud_name {
            Some(name) => parsed.clouds.get(name),
            None => parsed.clouds.values().next(),
        }?;
        return Some(CloudAuth {
            auth_url: entry.auth.auth_url.clone(),
            username: entry.auth.username.clone(),
            password: entry.auth.password.clone(),
            project_name: entry.auth.project_name.clone(),
            user_domain_name: entry.auth.user_domain_name.clone(),
            project_domain_name: entry.auth.project_domain_name.clone(),
            region_name: entry.region_name.clone(),
        });
    }
    None
}
