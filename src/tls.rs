use rustls::pki_types::CertificateDer;
use rustls::pki_types::pem::PemObject;
use std::path::Path;
use std::sync::Arc;

/// Build the TLS settings for talking to Keystone and the other OpenStack
/// APIs.
///
/// Certificates are checked by the operating system's own verifier
/// (Windows CryptoAPI, macOS Security.framework, or the system CA store on
/// Linux), through `rustls-platform-verifier`. That way ostrail follows
/// the OS when it distrusts or blocks a CA, instead of relying on a root
/// list frozen into the binary at build time.
///
/// `extra_ca_file` adds private CA certificates (for example a cloud's
/// own internal CA) on top of the OS trust store. It never replaces it.
///
/// There is deliberately no way to turn verification off: ostrail sends
/// OpenStack passwords and tokens over this connection.
pub fn client_config(extra_ca_file: Option<&Path>) -> Result<rustls::ClientConfig, String> {
    let provider = Arc::new(rustls::crypto::ring::default_provider());

    let verifier = match extra_ca_file {
        None => rustls_platform_verifier::Verifier::new(provider.clone()),
        Some(path) => {
            let roots = load_pem_certs(path)?;
            rustls_platform_verifier::Verifier::new_with_extra_roots(roots, provider.clone())
        }
    }
    .map_err(|e| format!("could not set up certificate verification: {e}"))?;

    let config = rustls::ClientConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .map_err(|e| format!("could not set up TLS: {e}"))?
        .dangerous()
        // "dangerous" is only rustls's name for "plug in a custom
        // verifier". The verifier here is the OS one, which checks
        // everything (chain, name, expiry, and revocation/blocklists
        // where the OS supports them).
        .with_custom_certificate_verifier(Arc::new(verifier))
        .with_no_client_auth();
    Ok(config)
}

/// Read every certificate from a PEM file (a file may hold several).
pub fn load_pem_certs(path: &Path) -> Result<Vec<CertificateDer<'static>>, String> {
    let data =
        std::fs::read(path).map_err(|e| format!("cannot read CA file {}: {e}", path.display()))?;
    let certs = CertificateDer::pem_slice_iter(&data)
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| format!("CA file {} is not valid PEM: {e}", path.display()))?;
    if certs.is_empty() {
        return Err(format!(
            "CA file {} has no certificates (expected PEM blocks starting with \
             -----BEGIN CERTIFICATE-----)",
            path.display()
        ));
    }
    Ok(certs)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    // A real self-signed CA certificate (CN=ostrail-test-ca), only used to
    // check that PEM parsing works. It is not trusted anywhere.
    const TEST_CA_PEM: &str = include_str!("../tests/data/test-ca.pem");

    fn temp_file(name: &str, contents: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("ostrail-tls-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(name);
        std::fs::write(&path, contents).unwrap();
        path
    }

    #[test]
    fn loads_all_certificates_from_a_pem_bundle() {
        let bundle = format!("{TEST_CA_PEM}\n{TEST_CA_PEM}");
        let path = temp_file("bundle.pem", &bundle);
        let certs = load_pem_certs(&path).unwrap();
        assert_eq!(certs.len(), 2);
    }

    #[test]
    fn rejects_a_file_with_no_certificates() {
        let path = temp_file("empty.pem", "");
        let err = load_pem_certs(&path).unwrap_err();
        assert!(err.contains("has no certificates"), "{err}");
    }

    #[test]
    fn rejects_a_missing_file() {
        let path = std::env::temp_dir().join("ostrail-no-such-file-1d0f.pem");
        let err = load_pem_certs(&path).unwrap_err();
        assert!(err.contains("cannot read CA file"), "{err}");
    }

    #[test]
    fn builds_a_config_with_extra_roots() {
        let path = temp_file("ca.pem", TEST_CA_PEM);
        assert!(client_config(Some(&path)).is_ok());
    }
}
