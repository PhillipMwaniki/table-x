//! SSH local port forwarding.
//!
//! Databases behind a bastion are reached by forwarding a local port through an
//! SSH session. The driver then connects to `127.0.0.1:<local_port>` and needs to
//! know nothing about SSH — which is why the [`tablex_core::Driver`] contract
//! says the tunnel is already established by the time `connect` is called.
//!
//! # Host key verification
//!
//! An SSH tunnel whose host key is not checked provides no protection against an
//! active attacker: the whole point is that the bastion is reachable from
//! somewhere hostile. So there is no "accept anything" mode. Either the caller
//! supplies the fingerprint it expects, or it must first call [`probe_host_key`]
//! and get explicit confirmation from the user before storing it.

use std::net::SocketAddr;
use std::sync::Arc;
use tablex_core::{
    config::{SshAuth, SshConfig},
    error::{Error, Result},
};
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::net::{TcpListener, TcpStream};
use tokio_util::sync::CancellationToken;

/// What the caller is willing to trust for the server's host key.
#[derive(Debug, Clone)]
pub enum HostKeyPolicy {
    /// Accept only this fingerprint, in OpenSSH `SHA256:...` form.
    Verify(String),
    /// Accept whatever the server presents and report it back.
    ///
    /// Only correct for [`probe_host_key`], where nothing is sent over the
    /// connection and the result is shown to the user for confirmation.
    AcceptAndReport,
}

/// A running tunnel. Dropping it tears the tunnel down.
#[derive(Debug)]
pub struct Tunnel {
    local_port: u16,
    cancel: CancellationToken,
}

impl Tunnel {
    /// The loopback port the driver should connect to.
    pub fn local_port(&self) -> u16 {
        self.local_port
    }

    /// Shut down explicitly. Equivalent to dropping, but awaitable.
    pub fn close(&self) {
        self.cancel.cancel();
    }
}

impl Drop for Tunnel {
    fn drop(&mut self) {
        // Without this, closing a connection would leave the forwarding task and
        // its listener socket alive for the life of the process.
        self.cancel.cancel();
    }
}

/// Handler that captures and checks the server's host key.
struct Client {
    policy: HostKeyPolicy,
    /// Filled in with what the server actually presented.
    seen: Arc<std::sync::Mutex<Option<String>>>,
}

impl russh::client::Handler for Client {
    type Error = russh::Error;

    async fn check_server_key(
        &mut self,
        server_public_key: &russh::keys::PublicKey,
    ) -> std::result::Result<bool, Self::Error> {
        let fingerprint = server_public_key
            .fingerprint(russh::keys::HashAlg::Sha256)
            .to_string();

        if let Ok(mut slot) = self.seen.lock() {
            *slot = Some(fingerprint.clone());
        }

        Ok(match &self.policy {
            HostKeyPolicy::AcceptAndReport => true,
            // Constant-time comparison is unnecessary here: the fingerprint is
            // public data, not a secret.
            HostKeyPolicy::Verify(expected) => expected == &fingerprint,
        })
    }
}

/// Connect, authenticate, and return the session plus the observed fingerprint.
async fn authenticate(
    ssh: &SshConfig,
    secret: Option<&str>,
    policy: HostKeyPolicy,
) -> Result<(russh::client::Handle<Client>, String)> {
    let config = Arc::new(russh::client::Config {
        // A dead bastion should fail fast rather than hanging the UI.
        inactivity_timeout: Some(std::time::Duration::from_secs(300)),
        keepalive_interval: Some(std::time::Duration::from_secs(30)),
        ..Default::default()
    });

    let seen = Arc::new(std::sync::Mutex::new(None));
    let handler = Client {
        policy: policy.clone(),
        seen: Arc::clone(&seen),
    };

    let mut session = russh::client::connect(config, (ssh.host.as_str(), ssh.port), handler)
        .await
        .map_err(|e| match &policy {
            // A rejected host key surfaces from russh as a generic failure, so
            // the message is rewritten to say what actually went wrong.
            HostKeyPolicy::Verify(expected) => Error::Tunnel(format!(
                "host key verification failed for {}:{} — expected {expected}, server presented {} ({e})",
                ssh.host,
                ssh.port,
                seen.lock()
                    .ok()
                    .and_then(|s| s.clone())
                    .unwrap_or_else(|| "an unreadable key".into()),
            )),
            HostKeyPolicy::AcceptAndReport => {
                Error::Tunnel(format!("could not reach {}:{} — {e}", ssh.host, ssh.port))
            }
        })?;

    let fingerprint = seen
        .lock()
        .ok()
        .and_then(|s| s.clone())
        .unwrap_or_default();

    let authenticated = match ssh.auth {
        SshAuth::Password => {
            let password = secret.ok_or_else(|| {
                Error::Auth("SSH password authentication needs a password".into())
            })?;
            session
                .authenticate_password(&ssh.username, password)
                .await
                .map_err(|e| Error::Tunnel(e.to_string()))?
        }

        SshAuth::PublicKey => {
            // The key path travels in `options`, since it is not a secret; only
            // the passphrase comes from the keychain.
            let path = ssh_key_path(ssh)?;
            let key = russh::keys::load_secret_key(&path, secret).map_err(|e| {
                Error::Auth(format!("could not read the private key at {path}: {e}"))
            })?;
            session
                .authenticate_publickey(
                    &ssh.username,
                    russh::keys::PrivateKeyWithHashAlg::new(
                        Arc::new(key),
                        // Let russh pick the strongest signature algorithm the
                        // server advertises rather than pinning SHA-1 RSA.
                        session.best_supported_rsa_hash().await.ok().flatten().flatten(),
                    ),
                )
                .await
                .map_err(|e| Error::Tunnel(e.to_string()))?
        }

        SshAuth::Agent => {
            // Delegating to the agent means no key material is ever loaded into
            // this process. The transport differs per platform, so the actual
            // auth loop lives in a generic helper.
            #[cfg(unix)]
            {
                let path = std::env::var("SSH_AUTH_SOCK").map_err(|_| {
                    Error::Auth("no ssh-agent found: SSH_AUTH_SOCK is not set".into())
                })?;
                let stream = tokio::net::UnixStream::connect(&path).await.map_err(|e| {
                    Error::Auth(format!("could not reach the ssh-agent at {path}: {e}"))
                })?;
                let mut agent = russh::keys::agent::client::AgentClient::connect(stream);
                agent_auth(&mut session, &ssh.username, &mut agent).await?
            }
            #[cfg(windows)]
            {
                // OpenSSH for Windows exposes the agent as a named pipe rather
                // than a socket, so SSH_AUTH_SOCK does not apply.
                const PIPE: &str = r"\\.\pipe\openssh-ssh-agent";
                let pipe = tokio::net::windows::named_pipe::ClientOptions::new()
                    .open(PIPE)
                    .map_err(|e| {
                        Error::Auth(format!(
                            "could not reach the OpenSSH agent at {PIPE}: {e}. \
                             Is the 'ssh-agent' service running?"
                        ))
                    })?;
                let mut agent = russh::keys::agent::client::AgentClient::connect(pipe);
                agent_auth(&mut session, &ssh.username, &mut agent).await?
            }
        }
    };

    if !matches!(authenticated, russh::client::AuthResult::Success) {
        return Err(Error::Auth(format!(
            "SSH authentication failed for {}@{}",
            ssh.username, ssh.host
        )));
    }

    Ok((session, fingerprint))
}

/// Offer each identity the agent holds, exactly as OpenSSH does.
///
/// Generic over the agent transport so the Unix socket and the Windows named
/// pipe share one implementation.
async fn agent_auth<S>(
    session: &mut russh::client::Handle<Client>,
    username: &str,
    agent: &mut russh::keys::agent::client::AgentClient<S>,
) -> Result<russh::client::AuthResult>
where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + Send + 'static,
{
    let identities = agent
        .request_identities()
        .await
        .map_err(|e| Error::Auth(format!("ssh-agent refused to list keys: {e}")))?;

    if identities.is_empty() {
        return Err(Error::Auth(
            "ssh-agent holds no identities; add one with ssh-add".into(),
        ));
    }

    let mut last = None;
    for identity in identities {
        // The agent reports plain keys and certificates alike; both expose the
        // public key that the server needs to see.
        let public_key = identity.public_key().into_owned();
        match session
            .authenticate_publickey_with(username, public_key, None, agent)
            .await
        {
            Ok(russh::client::AuthResult::Success) => {
                return Ok(russh::client::AuthResult::Success)
            }
            // A key the server does not accept is normal — try the next one
            // rather than failing on the first rejection.
            Ok(other) => last = Some(other),
            Err(e) => return Err(Error::Auth(format!("ssh-agent signing failed: {e}"))),
        }
    }
    last.ok_or_else(|| Error::Auth("ssh-agent offered no usable identity".into()))
}

fn ssh_key_path(ssh: &SshConfig) -> Result<String> {
    ssh.key_path
        .clone()
        .ok_or_else(|| Error::Config("SSH key authentication needs a private key path".into()))
}

/// Connect just far enough to read the server's host key, then disconnect.
///
/// Used to show the user a fingerprint to confirm on first connection. No
/// authentication is attempted and nothing is forwarded.
pub async fn probe_host_key(ssh: &SshConfig) -> Result<String> {
    let config = Arc::new(russh::client::Config::default());
    let seen = Arc::new(std::sync::Mutex::new(None));
    let handler = Client {
        policy: HostKeyPolicy::AcceptAndReport,
        seen: Arc::clone(&seen),
    };

    let session = russh::client::connect(config, (ssh.host.as_str(), ssh.port), handler)
        .await
        .map_err(|e| Error::Tunnel(format!("could not reach {}:{} — {e}", ssh.host, ssh.port)))?;

    let fingerprint = seen
        .lock()
        .ok()
        .and_then(|s| s.clone())
        .ok_or_else(|| Error::Tunnel("the server presented no host key".into()))?;

    let _ = session
        .disconnect(russh::Disconnect::ByApplication, "", "en")
        .await;
    Ok(fingerprint)
}

/// Open a tunnel forwarding a local loopback port to `target_host:target_port`
/// through the SSH server.
///
/// The local port is chosen by the OS, so several tunnels can be open at once
/// without the caller having to manage a port range or handle collisions.
pub async fn open(
    ssh: &SshConfig,
    target_host: &str,
    target_port: u16,
    secret: Option<&str>,
) -> Result<Tunnel> {
    let expected = ssh.host_key_fingerprint.clone().ok_or_else(|| {
        // Refusing here rather than trusting on first use: an unverified tunnel
        // gives no protection against exactly the attacker a bastion exists to
        // defend against.
        Error::Tunnel(format!(
            "no known host key for {}:{}. Verify the server's fingerprint before connecting.",
            ssh.host, ssh.port
        ))
    })?;

    let (session, _) = authenticate(ssh, secret, HostKeyPolicy::Verify(expected)).await?;

    // Bind before returning so a failure to bind is reported synchronously
    // rather than silently killing the tunnel task later.
    let listener = TcpListener::bind(("127.0.0.1", 0))
        .await
        .map_err(|e| Error::Tunnel(format!("could not open a local port: {e}")))?;
    let local_port = listener
        .local_addr()
        .map_err(|e| Error::Tunnel(e.to_string()))?
        .port();

    let cancel = CancellationToken::new();
    let task_cancel = cancel.clone();
    let target_host = target_host.to_string();

    tokio::spawn(async move {
        let session = Arc::new(session);
        loop {
            let accepted = tokio::select! {
                _ = task_cancel.cancelled() => break,
                accepted = listener.accept() => accepted,
            };

            let (stream, peer) = match accepted {
                Ok(pair) => pair,
                Err(e) => {
                    tracing::warn!("tunnel listener stopped accepting: {e}");
                    break;
                }
            };

            let session = Arc::clone(&session);
            let target_host = target_host.clone();
            let conn_cancel = task_cancel.clone();
            // One task per forwarded connection: a driver that opens several
            // sockets (pooling, a cancel channel) must not serialize on one.
            tokio::spawn(async move {
                if let Err(e) =
                    forward(&session, stream, peer, &target_host, target_port, conn_cancel).await
                {
                    tracing::warn!("tunnel forwarding failed: {e}");
                }
            });
        }
        tracing::debug!("tunnel on port {local_port} closed");
    });

    Ok(Tunnel { local_port, cancel })
}

/// Pump one accepted local socket through a `direct-tcpip` channel.
async fn forward(
    session: &russh::client::Handle<Client>,
    local: TcpStream,
    peer: SocketAddr,
    target_host: &str,
    target_port: u16,
    cancel: CancellationToken,
) -> Result<()> {
    let channel = session
        .channel_open_direct_tcpip(
            target_host,
            u32::from(target_port),
            peer.ip().to_string(),
            u32::from(peer.port()),
        )
        .await
        .map_err(|e| Error::Tunnel(format!("the SSH server refused to forward: {e}")))?;

    let mut remote = channel.into_stream();
    let mut local = local;

    tokio::select! {
        _ = cancel.cancelled() => Ok(()),
        result = copy_both(&mut local, &mut remote) => result,
    }
}

async fn copy_both<A, B>(a: &mut A, b: &mut B) -> Result<()>
where
    A: AsyncRead + AsyncWrite + Unpin + ?Sized,
    B: AsyncRead + AsyncWrite + Unpin + ?Sized,
{
    // Bidirectional copy ends as soon as either side closes, which is the
    // correct semantics for a forwarded TCP connection.
    tokio::io::copy_bidirectional(a, b)
        .await
        .map(|_| ())
        .map_err(|e| Error::Tunnel(e.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tablex_core::config::SshConfig;

    fn ssh(fingerprint: Option<&str>) -> SshConfig {
        SshConfig {
            host: "bastion.example.com".into(),
            port: 22,
            username: "deploy".into(),
            auth: SshAuth::PublicKey,
            key_path: Some("/home/deploy/.ssh/id_ed25519".into()),
            host_key_fingerprint: fingerprint.map(str::to_string),
        }
    }

    #[tokio::test]
    async fn opening_without_a_known_host_key_is_refused() {
        // Trust-on-first-use would defeat the purpose of the tunnel, so this
        // must fail before any network traffic rather than connecting anyway.
        // (`Tunnel` is not Debug-printable in the Ok arm, hence the match.)
        match open(&ssh(None), "db.internal", 5432, None).await {
            Err(e) => {
                assert_eq!(e.category(), tablex_core::ErrorCategory::Connection);
                assert!(e.to_string().contains("no known host key"), "{e}");
            }
            Ok(_) => panic!("an unverified host must not be tunnelled to"),
        }
    }

    #[test]
    fn key_auth_requires_a_key_path() {
        let mut config = ssh(Some("SHA256:abc"));
        config.key_path = None;
        let err = ssh_key_path(&config).expect_err("must require a path");
        assert_eq!(err.category(), tablex_core::ErrorCategory::Config);
    }

    #[test]
    fn a_dropped_tunnel_cancels_its_task() {
        let cancel = CancellationToken::new();
        let observer = cancel.clone();
        {
            let _tunnel = Tunnel {
                local_port: 1234,
                cancel,
            };
            assert!(!observer.is_cancelled());
        }
        // Otherwise the listener socket would outlive the connection.
        assert!(observer.is_cancelled(), "dropping must tear the tunnel down");
    }

    #[test]
    fn close_is_idempotent() {
        let tunnel = Tunnel {
            local_port: 1,
            cancel: CancellationToken::new(),
        };
        tunnel.close();
        tunnel.close();
        assert_eq!(tunnel.local_port(), 1);
    }
}
