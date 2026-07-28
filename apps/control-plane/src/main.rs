use std::env;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use codebox_agent_codex::{
    CloudBranch, CloudEnvironmentId, CloudRunnerConfig, CloudTaskOrchestrator, CredentialScope,
    CredentialScopeConfig, LoginBroker,
};
use codebox_control_plane::{OperatorBootstrapToken, P0ControlPlane, P0HttpConfig, P0PublicOrigin};
use codebox_session_runtime::{P0SessionConfig, P0SessionRuntime};

#[tokio::main]
async fn main() {
    if run().await.is_err() {
        eprintln!("codebox control plane could not start or shut down safely");
        std::process::exit(1);
    }
}

async fn run() -> Result<(), ()> {
    let (control_plane, listen) = build_control_plane()?;
    let router = control_plane.router();
    let listener = match tokio::net::TcpListener::bind(listen).await {
        Ok(listener) => listener,
        Err(_) => {
            let _ = control_plane.shutdown().await;
            return Err(());
        }
    };
    let serve_result = axum::serve(listener, router)
        .with_graceful_shutdown(async {
            let _ = tokio::signal::ctrl_c().await;
        })
        .await;
    let shutdown_result = control_plane.shutdown().await;
    serve_result.map_err(|_| ())?;
    shutdown_result.map_err(|_| ())
}

fn build_control_plane() -> Result<(P0ControlPlane, SocketAddr), ()> {
    let executable = required_path("CODEBOX_CODEX_EXECUTABLE")?;
    let codex_home = required_path("CODEBOX_CODEX_HOME")?;
    let state_dir = required_path("CODEBOX_STATE_DIR")?;
    let working_dir = required_path("CODEBOX_WORKING_DIR")?;

    let login_scope = CredentialScope::validate(CredentialScopeConfig::new(
        executable.clone(),
        codex_home.clone(),
        state_dir.clone(),
        working_dir.clone(),
    ))
    .map_err(|_| ())?;
    let cloud_scope = CredentialScope::validate(CredentialScopeConfig::new(
        executable,
        codex_home,
        state_dir,
        working_dir,
    ))
    .map_err(|_| ())?;
    let login = LoginBroker::new(login_scope).map_err(|_| ())?;
    let environment =
        CloudEnvironmentId::try_new(required_text("CODEBOX_CLOUD_ENVIRONMENT")?).map_err(|_| ())?;
    let branch = CloudBranch::try_new(required_text("CODEBOX_CLOUD_BRANCH")?).map_err(|_| ())?;
    let orchestrator =
        CloudTaskOrchestrator::new(CloudRunnerConfig::new(cloud_scope, environment, branch))
            .map_err(|_| ())?;
    let session =
        P0SessionRuntime::new(orchestrator, P0SessionConfig::default()).map_err(|_| ())?;

    let origin =
        P0PublicOrigin::try_new(&required_text("CODEBOX_PUBLIC_ORIGIN")?).map_err(|_| ())?;
    let bootstrap = OperatorBootstrapToken::try_new(required_text("CODEBOX_BOOTSTRAP_TOKEN")?)
        .map_err(|_| ())?;
    let mut http = P0HttpConfig::new(origin, bootstrap);
    if let Ok(value) = env::var("CODEBOX_APP_SESSION_SECONDS") {
        let seconds = value.parse::<u64>().map_err(|_| ())?;
        http = http
            .try_with_session_lifetime(Duration::from_secs(seconds))
            .map_err(|_| ())?;
    }
    let control_plane = P0ControlPlane::new(http, login, Arc::new(session)).map_err(|_| ())?;
    let listen = required_text("CODEBOX_LISTEN_ADDRESS")?
        .parse::<SocketAddr>()
        .map_err(|_| ())?;
    Ok((control_plane, listen))
}

fn required_text(name: &str) -> Result<String, ()> {
    env::var(name).map_err(|_| ())
}

fn required_path(name: &str) -> Result<PathBuf, ()> {
    env::var_os(name).map(PathBuf::from).ok_or(())
}
