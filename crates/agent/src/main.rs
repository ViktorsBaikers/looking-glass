//! The looking-glass agent binary. The installer consumes the one-time enrollment
//! token and stores the issued credential; normal service startup reads that
//! credential and holds the outbound authenticated tunnel to central.
//!
//! If no credential exists yet, the binary reports that enrollment must run first
//! and exits, rather than connecting unauthenticated.

use std::path::PathBuf;
use std::process::ExitCode;

use agent::dataplane;
use agent::enroll::{
    store_dry_run_response_credential, store_install_credential, validate_stored_credential,
    AgentCredential, HttpsEnrollConnector, PinnedCommand,
};
use agent::tunnel::{self, NodeExecutor, TunnelClientConfig};
use shared::exec::{ExecEngine, ExecLimits};
use shared::validate::DnsResolver;

const ENV_CREDENTIAL_PATH: &str = "LG_AGENT_CREDENTIAL";
const DEFAULT_CREDENTIAL_PATH: &str = "data/agent-credential.json";

#[tokio::main]
async fn main() -> ExitCode {
    agent::init_tracing();

    if let Some(code) = maybe_store_enrollment().await {
        return code;
    }

    let credential = match load_credential() {
        Ok(Some(credential)) => credential,
        Ok(None) => {
            eprintln!("no agent credential found; run the installer enrollment first");
            return ExitCode::FAILURE;
        }
        Err(error) => {
            eprintln!("could not read the agent credential: {error}");
            return ExitCode::FAILURE;
        }
    };
    if let Err(error) = validate_stored_credential(&credential) {
        eprintln!("{error}");
        return ExitCode::FAILURE;
    }

    let resolver = match DnsResolver::from_system() {
        Ok(resolver) => resolver,
        Err(error) => {
            eprintln!("could not initialise the DNS resolver: {error}");
            return ExitCode::FAILURE;
        }
    };
    let executor = NodeExecutor::new(ExecEngine::new(ExecLimits::default()), resolver);
    match dataplane::config_from_env() {
        Ok(Some((bind, root))) => {
            tokio::spawn(async move {
                if let Err(error) = dataplane::serve(bind, root).await {
                    tracing::error!(%error, "agent speedtest data-plane listener stopped");
                }
            });
        }
        Ok(None) => {}
        Err(error) => {
            eprintln!("invalid data-plane configuration: {error}");
            return ExitCode::FAILURE;
        }
    }

    let config = TunnelClientConfig::from_parts(
        &credential.tunnel_url,
        credential.fingerprint,
        credential.agent_id,
        credential.credential,
    );
    tracing::info!(
        host = %config.host,
        port = config.port,
        "starting agent tunnel (outbound, central fingerprint pinned)"
    );
    // Runs the reconnecting tunnel forever.
    tunnel::run(config, executor).await;
    ExitCode::SUCCESS
}

async fn maybe_store_enrollment() -> Option<ExitCode> {
    let mut args = std::env::args();
    let _program = args.next();
    let command = args.next()?;
    if command != "install-enroll" {
        eprintln!("unknown agent command: {command}");
        return Some(ExitCode::FAILURE);
    }
    let Some(credential_path) = args.next() else {
        eprintln!("usage: lg-agent install-enroll <credential-path>");
        return Some(ExitCode::FAILURE);
    };
    if args.next().is_some() {
        eprintln!("usage: lg-agent install-enroll <credential-path>");
        return Some(ExitCode::FAILURE);
    }

    let command = match PinnedCommand::from_env() {
        Ok(command) => command,
        Err(error) => {
            eprintln!("{error}");
            return Some(ExitCode::FAILURE);
        }
    };
    let credential_path = PathBuf::from(credential_path);
    let dry_run = std::env::var("LG_INSTALL_DRY_RUN").as_deref() == Ok("1");
    let response_file = std::env::var("LG_ENROLL_RESPONSE_FILE").ok();
    let result = match response_file.as_deref().filter(|path| !path.is_empty()) {
        Some(response_path) if dry_run => store_dry_run_response_credential(
            PathBuf::from(response_path).as_path(),
            &credential_path,
            &command.central_url,
            &command.tunnel_url,
            &command.fingerprint,
            true,
        ),
        Some(_) => Err(agent::enroll::EnrollError::ResponseFileOutsideDryRun),
        None => match HttpsEnrollConnector::new(&command.central_url, &command.fingerprint) {
            Ok(connector) => store_install_credential(&credential_path, &command, &connector).await,
            Err(error) => Err(error),
        },
    };
    match result {
        Ok(()) => Some(ExitCode::SUCCESS),
        Err(error) => {
            eprintln!("{error}");
            Some(ExitCode::FAILURE)
        }
    }
}

fn load_credential() -> std::io::Result<Option<AgentCredential>> {
    let path = std::env::var(ENV_CREDENTIAL_PATH)
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from(DEFAULT_CREDENTIAL_PATH));
    match std::fs::read(&path) {
        Ok(bytes) => {
            let credential = serde_json::from_slice(&bytes).map_err(|error| {
                std::io::Error::other(format!("malformed credential file: {error}"))
            })?;
            Ok(Some(credential))
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error),
    }
}
