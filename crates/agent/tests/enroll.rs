//! Agent-side enrollment (G4 / AC35): the agent pins the fingerprint from the
//! install command and verifies central's presented identity against it. A central
//! presenting a mismatched identity is refused with no credential stored — no
//! trust-on-first-use. The transport is the injected [`CentralConnector`] seam; the
//! production HTTPS transport presents the peer certificate in its place.

use agent::enroll::{
    credential_from_enroll_response, enroll, store_credential, store_dry_run_response_credential,
    store_install_credential, validate_stored_credential, AgentCredential, CentralConnector,
    EnrollError, HttpsEnrollConnector, PinnedCommand, PresentedEnrollment,
};
use agent::tunnel::TunnelClientConfig;
use rustls::pki_types::{pem::PemObject, CertificateDer, PrivateKeyDer};
use rustls::ServerConfig;
use shared::protocol::{
    fingerprint, EnrollRequest, EnrollResponse, ENV_CENTRAL_FINGERPRINT, ENV_CENTRAL_URL,
    ENV_ENROLL_TOKEN, ENV_TUNNEL_URL, PROTOCOL_VERSION,
};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio_rustls::TlsAcceptor;

/// Central's real identity in these tests — the agent pins its fingerprint.
const CENTRAL_IDENTITY: &[u8] = b"the-real-central-identity";
const TEST_CERT: &str = "-----BEGIN CERTIFICATE-----
MIIDGjCCAgKgAwIBAgIUB8nOrMrkRWr5lR8RsPnj4J/PhBcwDQYJKoZIhvcNAQEL
BQAwFDESMBAGA1UEAwwJMTI3LjAuMC4xMB4XDTI2MDcwNTIzMzYwMFoXDTI2MDcw
NjIzMzYwMFowFDESMBAGA1UEAwwJMTI3LjAuMC4xMIIBIjANBgkqhkiG9w0BAQEF
AAOCAQ8AMIIBCgKCAQEAv+zcTZ2KTH4jWa3v76GFj0xg6+UVkmC0e8GyIitJJnQ2
DrPsa9xK1OZHNL0RjGFHZBu/DxMmaQfbhl4Izmi+pBA1OwjfWPCr6wr4N4+dtN2F
PQsY7vr6EZxnd/C49nRS+yXVmHUaCkEC2SxoXrQ7wBLpjA6Y70R3vdcoRFgdYu3u
vddYRSTL9I16x7daCs4m/L3I7I3BKS41aBGZF4dJ1yZ2LgyxB9mE+kF6ZZQLuqt+
lnLm4gwfkbZCVp9TBwJRJ766TDlGHKC0bO0dgLEBg8gRuZAv9mPPEqov/XWciUsS
c7thsQ2FG8hUMXVeKrCRvUrUo/DtB8pRlXGcmBDJCwIDAQABo2QwYjAdBgNVHQ4E
FgQUz4NF3ivpta/zCGT/my4yEvnURuYwHwYDVR0jBBgwFoAUz4NF3ivpta/zCGT/
my4yEvnURuYwDwYDVR0TAQH/BAUwAwEB/zAPBgNVHREECDAGhwR/AAABMA0GCSqG
SIb3DQEBCwUAA4IBAQAkCaT76n7ECoFqfUWAaNypbyFDufX/DY8F60yLMLeLZn3r
K8swCxa/VKLCdj+5BANJC/2l+L0a1yaiCrzfZaecTAG8LhlZECdUJKfKZ+R5zSDP
ap+EBNggS01ZBV9BINqtX6LP2s5qoBw/Y2rVPrtQWW8HOaULWZLqaWJ9NlpQE+SJ
3ERIsGI+v3NqnK4sTc8ib3FXbuYCHSooXQrZCUzIjO5H8pdvDOcDqEYn0C+Ye7vD
ISssAEPekto7X9oDQ1iIDeRZy5yYgiQ3OyXyqan4FzdvKP4HKVBM3ZYSmm05bO9r
Np7mtV1m/hMoD8X1QW1khM6/cFDTa5bjxqnuPrM0
-----END CERTIFICATE-----";
const TEST_KEY: &str = "-----BEGIN PRIVATE KEY-----
MIIEvgIBADANBgkqhkiG9w0BAQEFAASCBKgwggSkAgEAAoIBAQC/7NxNnYpMfiNZ
re/voYWPTGDr5RWSYLR7wbIiK0kmdDYOs+xr3ErU5kc0vRGMYUdkG78PEyZpB9uG
XgjOaL6kEDU7CN9Y8KvrCvg3j5203YU9Cxju+voRnGd38Lj2dFL7JdWYdRoKQQLZ
LGhetDvAEumMDpjvRHe91yhEWB1i7e6911hFJMv0jXrHt1oKzib8vcjsjcEpLjVo
EZkXh0nXJnYuDLEH2YT6QXpllAu6q36WcubiDB+RtkJWn1MHAlEnvrpMOUYcoLRs
7R2AsQGDyBG5kC/2Y88Sqi/9dZyJSxJzu2GxDYUbyFQxdV4qsJG9StSj8O0HylGV
cZyYEMkLAgMBAAECggEAHLnF63QF7BcBEX4gKFyjkeQbqZL7HJKO0OsXz1dtNm00
XhT98NLi/GSPCcy2oK06szgX65ixKg08BStz4/j3d7TZUsGsNDKpeJ+GsUI0l4qE
U7OigrpbzWD8d443EOQuO1rZUl1MjXZXh8vGv034l3H9NLJGn6E+ztIyO7B4jYLO
r9IjkEzFRAAg5FHlTMP3TI9Mj4aaI5FS+ksFoRLRuIUz0FF7mA2EL+oKNfWWgvn8
YjRW0xAeuZG30GulQQzqcvSWgvLw3NPFNAFpt7IRpE6k+6jmxQsDmhh56vYnx74m
uWuGJq2OHPd7EKO2rTd30i1N5i+7gU5jAHo2y34woQKBgQDuJFvNVwygZDvf5CYA
76fIZ3vGkSYckEaftDHtl5Zeb3GJ7pRX/hvm9cWEWrwk4pbz8ozbwFt15Np+p+nN
THYdRO6RhESctClb3Pv7yGmYRZlaXEIYfsXwvyOCjAgjfJaQdfaR/FMOdyYYpEoL
YHy/O16vVSXZAGwDIvEGp8qH2wKBgQDOUUXLv0qGhd58ySKzT63e//wqoa8OoYf4
yE8PbakTZ7weB737GHvDt/UiEg1CxjdE7RyxZP/gkYWTwxR6Jh8FCuZApa01gsHH
LGmOrZw47L9svbCTfRpeE1JgNMyHvuhGZL82HHOPMCqzZHzLazntqMDzZQnuciqL
jCAXrohikQKBgFIfJ6lAA5Kr/hnPS2u3OVzaksx+8W2YM0KPmUgdpjUaqUSviWhu
sKCM0Hg78fWmTfgCBKEjTGbzbIWQ0geB/plJVBvKSP7hAgIzypGhIwjnt2J5vjFE
Rm4m+8/hCk1ygVl/1G+zW9D5NaH5xa72rw4jIxvDeTHD+3t5aTSqWCVRAoGBAIBT
hsEbA0S24VLmXAIBzljFCdiOZm8IQ6WXGa2z/JUIUbawBBe4+8oZkowVhFADL/9c
KBuigZDxko78qLDtIyAkzmBpbFm7McIruqA3FdNGVi5RshGan5riE7upO4o3UQvv
wArtGWd3gye/meuAjzBmZVU+hDXept3TU2bHdScxAoGBAI9bN+bs9aAX/HlM7j2A
yoQBG3+VCd5W8i0IHPsbTO2dbLHXaKVkdZWlkJjSTTxZJo365WFanXCTxx7RpmYu
cb9CfyxMRjfR05GpAv9dfZpPq8CKnDXVvVb6HQ+9+HFTe0FEPRHbIVCawGPv/OrR
uiJHDAnmPhWXIOn3xsSNEtQK
-----END PRIVATE KEY-----";

/// A stubbed central that presents chosen identity material and a chosen response.
struct StubCentral {
    identity_material: Vec<u8>,
    response: EnrollResponse,
}

impl StubCentral {
    fn honest() -> Self {
        Self {
            identity_material: CENTRAL_IDENTITY.to_vec(),
            response: EnrollResponse {
                protocol_version: PROTOCOL_VERSION,
                agent_id: "agent-xyz".to_string(),
                credential: "cred-abc123".to_string(),
            },
        }
    }
}

impl CentralConnector for StubCentral {
    async fn exchange(&self, _request: EnrollRequest) -> Result<PresentedEnrollment, EnrollError> {
        Ok(PresentedEnrollment {
            identity_material: self.identity_material.clone(),
            response: self.response.clone(),
        })
    }
}

/// The pinned command an honest install command produces: the fingerprint of the
/// real central identity.
fn pinned_command() -> PinnedCommand {
    PinnedCommand {
        central_url: "https://central.test:8443".to_string(),
        tunnel_url: "https://tunnel.central.test:8443".to_string(),
        fingerprint: fingerprint(CENTRAL_IDENTITY),
        token: "enrollment-token".to_string(),
    }
}

fn temp_path(name: &str) -> std::path::PathBuf {
    let mut path = std::env::temp_dir();
    path.push(format!(
        "lg-agent-{name}-{}-{}.json",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    path
}

#[tokio::test]
async fn matching_central_identity_enrolls() {
    let command = pinned_command();
    let central = StubCentral::honest();

    let credential = enroll(&command, &central)
        .await
        .expect("enrollment succeeds");
    assert_eq!(credential.agent_id, "agent-xyz");
    assert_eq!(credential.credential, "cred-abc123");
    assert_eq!(credential.central_url, "https://central.test:8443");
    assert_eq!(credential.tunnel_url, "https://tunnel.central.test:8443");
    assert_eq!(credential.fingerprint, command.fingerprint);
}

#[tokio::test]
async fn mismatched_central_identity_aborts_with_no_credential() {
    let command = pinned_command();
    // Central presents a DIFFERENT identity than the one pinned in the command — a
    // man-in-the-middle. Its fingerprint will not match.
    let impostor = StubCentral {
        identity_material: b"an-impostor-central-identity".to_vec(),
        response: EnrollResponse {
            protocol_version: PROTOCOL_VERSION,
            agent_id: "attacker-agent".to_string(),
            credential: "attacker-credential".to_string(),
        },
    };

    let result = enroll(&command, &impostor).await;
    assert!(
        matches!(result, Err(EnrollError::IdentityMismatch)),
        "a central whose identity fails the pin must be refused, got {result:?}"
    );
    // No credential is produced to store — the fail-closed guarantee (AC35).
    assert!(result.is_err());
}

#[tokio::test]
async fn a_central_speaking_a_different_protocol_is_refused() {
    let command = pinned_command();
    let central = StubCentral {
        identity_material: CENTRAL_IDENTITY.to_vec(),
        response: EnrollResponse {
            protocol_version: PROTOCOL_VERSION + 1,
            agent_id: "agent-xyz".to_string(),
            credential: "cred".to_string(),
        },
    };

    let result = enroll(&command, &central).await;
    assert!(
        matches!(result, Err(EnrollError::ProtocolMismatch { got }) if got == PROTOCOL_VERSION + 1),
        "a version mismatch must be refused, got {result:?}"
    );
}

#[tokio::test]
async fn a_verified_credential_persists_owner_only() {
    let command = pinned_command();
    let credential = enroll(&command, &StubCentral::honest())
        .await
        .expect("enrollment succeeds");

    let mut path = std::env::temp_dir();
    path.push(format!("lg-agent-cred-{}.json", std::process::id()));
    store_credential(&path, &credential).expect("credential persists");

    let read: AgentCredential =
        serde_json::from_slice(&std::fs::read(&path).unwrap()).expect("read back credential");
    assert_eq!(read.agent_id, "agent-xyz");
    assert_eq!(read.central_url, "https://central.test:8443");
    assert_eq!(read.tunnel_url, "https://tunnel.central.test:8443");
    assert_eq!(read.fingerprint, fingerprint(CENTRAL_IDENTITY));

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(&path).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600, "credential must be owner-only");
    }
    let _ = std::fs::remove_file(&path);
}

#[tokio::test]
async fn install_time_identity_mismatch_stores_no_credential() {
    let command = pinned_command();
    let impostor = StubCentral {
        identity_material: b"an-impostor-central-identity".to_vec(),
        response: EnrollResponse {
            protocol_version: PROTOCOL_VERSION,
            agent_id: "attacker-agent".to_string(),
            credential: "attacker-credential".to_string(),
        },
    };
    let path = temp_path("identity-mismatch");

    let result = store_install_credential(&path, &command, &impostor).await;
    assert!(
        matches!(result, Err(EnrollError::IdentityMismatch)),
        "install-time enrollment must reject mismatched central identity, got {result:?}"
    );
    assert!(
        !path.exists(),
        "a refused install-time enrollment must not store a credential"
    );
}

#[tokio::test]
async fn production_https_enrollment_posts_to_api_enroll_and_verifies_the_pin() {
    let certs = CertificateDer::pem_slice_iter(TEST_CERT.as_bytes())
        .collect::<Result<Vec<_>, _>>()
        .unwrap();
    let cert_fingerprint = fingerprint(certs[0].as_ref());
    let key = PrivateKeyDer::from_pem_slice(TEST_KEY.as_bytes()).unwrap();
    let config = ServerConfig::builder_with_provider(std::sync::Arc::new(
        rustls::crypto::ring::default_provider(),
    ))
    .with_safe_default_protocol_versions()
    .unwrap()
    .with_no_client_auth()
    .with_single_cert(certs, key)
    .unwrap();
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        let acceptor = TlsAcceptor::from(std::sync::Arc::new(config));
        let (tcp, _) = listener.accept().await.unwrap();
        let mut tls = acceptor.accept(tcp).await.unwrap();
        let request = read_http_request(&mut tls).await;
        let body = format!(
            r#"{{"protocol_version":{},"agent_id":"agent-xyz","credential":"cred-abc123"}}"#,
            PROTOCOL_VERSION
        );
        let response = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
            body.len(),
            body
        );
        tls.write_all(response.as_bytes()).await.unwrap();
        tls.shutdown().await.unwrap();
        request
    });
    let command = PinnedCommand {
        central_url: format!("https://{addr}"),
        tunnel_url: "https://tunnel.central.test:8443".to_string(),
        fingerprint: cert_fingerprint,
        token: "enrollment-token".to_string(),
    };
    let connector = HttpsEnrollConnector::new(&command.central_url, &command.fingerprint)
        .expect("https connector builds from a valid central URL");

    let credential = enroll(&command, &connector)
        .await
        .expect("pinned production enrollment succeeds");

    assert_eq!(credential.agent_id, "agent-xyz");
    assert_eq!(credential.credential, "cred-abc123");
    let request = server.await.unwrap();
    assert!(
        request.starts_with("POST /api/enroll HTTP/1.1\r\n"),
        "install enrollment must use central's /api/enroll endpoint, got {request:?}"
    );
    assert!(
        request.contains(&format!("Host: {addr}\r\n")),
        "install enrollment must preserve non-default ports in the Host header, got {request:?}"
    );
    assert!(
        request.contains(r#""token":"enrollment-token""#),
        "the one-time token travels only in the TLS request body"
    );
}

#[test]
fn production_https_enrollment_accepts_only_a_plain_api_origin() {
    for central_url in [
        "https://127.0.0.1/",
        "https://127.0.0.1:+8443",
        "https://127.0.0.1/api/enroll",
        "https://127.0.0.1?x=1",
        "https://127.0.0.1#fragment",
        "https://[::1]:8443/",
        "https://[::1]:+8443",
        "https://[::1]:8443/api/enroll",
        "https://[::1]:8443?x=1",
        "https://[::1]:8443#fragment",
    ] {
        assert!(
            matches!(
                HttpsEnrollConnector::new(central_url, &fingerprint(CENTRAL_IDENTITY)),
                Err(EnrollError::InvalidCredential(_))
            ),
            "production enrollment URL {central_url:?} must be rejected before POST"
        );
    }
}

#[test]
fn enrollment_response_becomes_the_service_credential_without_the_token() {
    let credential = credential_from_enroll_response(
        "https://central.test:8443",
        "https://tunnel.central.test:8443",
        &fingerprint(CENTRAL_IDENTITY),
        EnrollResponse {
            protocol_version: PROTOCOL_VERSION,
            agent_id: "agent-xyz".to_string(),
            credential: "cred-abc123".to_string(),
        },
    )
    .expect("credential is built from enroll response");

    assert_eq!(credential.agent_id, "agent-xyz");
    assert_eq!(credential.credential, "cred-abc123");
    assert_eq!(credential.central_url, "https://central.test:8443");
    assert_eq!(credential.tunnel_url, "https://tunnel.central.test:8443");
    assert_eq!(credential.fingerprint, fingerprint(CENTRAL_IDENTITY));
    let encoded = serde_json::to_string(&credential).expect("credential encodes");
    assert!(!encoded.contains("LG_ENROLL_TOKEN"));
    assert!(!encoded.contains("enrollment-token"));
}

#[test]
fn response_file_bypass_is_dry_run_only() {
    let response = temp_path("response");
    let credential_path = temp_path("response-credential");
    std::fs::write(
        &response,
        br#"{"protocol_version":1,"agent_id":"agent-xyz","credential":"cred-abc123"}"#,
    )
    .unwrap();

    let blocked = store_dry_run_response_credential(
        &response,
        &credential_path,
        "https://central.test:8443",
        "https://tunnel.central.test:8443",
        &fingerprint(CENTRAL_IDENTITY),
        false,
    );
    assert!(matches!(
        blocked,
        Err(EnrollError::ResponseFileOutsideDryRun)
    ));
    assert!(!credential_path.exists());

    store_dry_run_response_credential(
        &response,
        &credential_path,
        "https://central.test:8443",
        "https://tunnel.central.test:8443",
        &fingerprint(CENTRAL_IDENTITY),
        true,
    )
    .expect("dry-run response file may seed a credential");
    assert!(credential_path.exists());
    let _ = std::fs::remove_file(response);
    let _ = std::fs::remove_file(credential_path);
}

#[test]
fn store_credential_replaces_a_permissive_existing_file_owner_only() {
    let path = temp_path("permissive");
    std::fs::write(&path, b"old").unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
    }
    let credential = AgentCredential {
        agent_id: "agent-xyz".to_string(),
        credential: "cred-abc123".to_string(),
        central_url: "https://central.test:8443".to_string(),
        tunnel_url: "https://tunnel.central.test:8443".to_string(),
        fingerprint: fingerprint(CENTRAL_IDENTITY),
    };

    store_credential(&path, &credential).expect("credential persists");

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(&path).unwrap().permissions().mode();
        assert_eq!(
            mode & 0o777,
            0o600,
            "credential must stay owner-only over a permissive existing file"
        );
    }
    let _ = std::fs::remove_file(path);
}

#[test]
fn loaded_credential_validation_rejects_empty_fields_and_bad_central_urls() {
    let valid = AgentCredential {
        agent_id: "agent-xyz".to_string(),
        credential: "cred-abc123".to_string(),
        central_url: "https://central.test:8443".to_string(),
        tunnel_url: "https://tunnel.central.test:8443".to_string(),
        fingerprint: fingerprint(CENTRAL_IDENTITY),
    };
    validate_stored_credential(&valid).expect("valid credential accepted");
    let mut ipv6_url = valid.clone();
    ipv6_url.central_url = "https://[::1]:8443".to_string();
    validate_stored_credential(&ipv6_url).expect("valid bracketed IPv6 credential accepted");

    let mut empty_agent = valid.clone();
    empty_agent.agent_id.clear();
    assert!(matches!(
        validate_stored_credential(&empty_agent),
        Err(EnrollError::InvalidCredential(_))
    ));

    let mut empty_secret = valid.clone();
    empty_secret.credential.clear();
    assert!(matches!(
        validate_stored_credential(&empty_secret),
        Err(EnrollError::InvalidCredential(_))
    ));

    let mut empty_url = valid.clone();
    empty_url.central_url.clear();
    assert!(matches!(
        validate_stored_credential(&empty_url),
        Err(EnrollError::InvalidCredential(_))
    ));

    let mut bad_fingerprint = valid.clone();
    bad_fingerprint.fingerprint = "not-hex".to_string();
    assert!(matches!(
        validate_stored_credential(&bad_fingerprint),
        Err(EnrollError::InvalidCredential(_))
    ));

    for central_url in [
        "https://central.test:notaport",
        "https://central.test:+8443",
        "https://central.test:0",
        "https://bad host:8443",
        "https://[not-an-ip]:8443",
        "https://[::1]:+8443",
        "https://[::1]:0",
        "https://[::1]:8443/path",
        "https://[::1]:8443?x=1",
        "https://[::1]:8443#fragment",
        "https://central.test:8443/",
        "https://central.test:8443#fragment",
        "https://central.test:8443/api/enroll",
    ] {
        let mut bad_url = valid.clone();
        bad_url.central_url = central_url.to_string();
        assert!(
            matches!(
                validate_stored_credential(&bad_url),
                Err(EnrollError::InvalidCredential(_))
            ),
            "stored credential URL {central_url:?} must fail before tunnel startup"
        );
    }

    for tunnel_url in [
        "https://tunnel.central.test:notaport",
        "https://tunnel.central.test:+8443",
        "https://tunnel.central.test:0",
        "https://bad host:8443",
        "https://[not-an-ip]:8443",
        "https://[::1]:+8443",
        "https://[::1]:0",
        "https://[::1]:8443/path",
        "https://[::1]:8443?x=1",
        "https://[::1]:8443#fragment",
        "https://tunnel.central.test:8443/",
        "https://tunnel.central.test:8443#fragment",
        "https://tunnel.central.test:8443/tunnel",
    ] {
        let mut bad_url = valid.clone();
        bad_url.tunnel_url = tunnel_url.to_string();
        assert!(
            matches!(
                validate_stored_credential(&bad_url),
                Err(EnrollError::InvalidCredential(_))
            ),
            "stored credential tunnel URL {tunnel_url:?} must fail before tunnel startup"
        );
    }
}

#[test]
fn tunnel_config_uses_stored_tunnel_url_not_api_url() {
    let credential = AgentCredential {
        agent_id: "agent-xyz".to_string(),
        credential: "cred-abc123".to_string(),
        central_url: "https://api.central.test:443".to_string(),
        tunnel_url: "https://tunnel.central.test:8443".to_string(),
        fingerprint: fingerprint(CENTRAL_IDENTITY),
    };
    validate_stored_credential(&credential).expect("split API/tunnel origins are valid");

    let config = TunnelClientConfig::from_parts(
        &credential.tunnel_url,
        credential.fingerprint,
        credential.agent_id,
        credential.credential,
    );

    assert_eq!(config.host, "tunnel.central.test");
    assert_eq!(config.port, 8443);
}

#[tokio::test]
async fn a_missing_install_parameter_fails_closed() {
    // No LG_* vars set for these keys → from_env refuses rather than enrolling
    // against an unpinned central.
    for key in [
        ENV_CENTRAL_URL,
        ENV_TUNNEL_URL,
        ENV_CENTRAL_FINGERPRINT,
        ENV_ENROLL_TOKEN,
    ] {
        std::env::remove_var(key);
    }
    let result = PinnedCommand::from_env();
    assert!(matches!(result, Err(EnrollError::MissingParam(_))));
}

async fn read_http_request<T>(tls: &mut T) -> String
where
    T: AsyncRead + Unpin,
{
    let mut bytes = Vec::new();
    let mut chunk = [0u8; 1024];
    loop {
        let read = tls.read(&mut chunk).await.unwrap();
        assert_ne!(read, 0, "client closed before sending an HTTP request");
        bytes.extend_from_slice(&chunk[..read]);
        let Some(header_end) = bytes.windows(4).position(|w| w == b"\r\n\r\n") else {
            continue;
        };
        let headers = String::from_utf8_lossy(&bytes[..header_end]);
        let content_length = headers
            .lines()
            .find_map(|line| line.strip_prefix("Content-Length: "))
            .and_then(|value| value.parse::<usize>().ok())
            .unwrap_or(0);
        if bytes.len() >= header_end + 4 + content_length {
            return String::from_utf8(bytes).unwrap();
        }
    }
}
