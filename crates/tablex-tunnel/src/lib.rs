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
//!
//! # More than one hop
//!
//! A production database is often two hops away: a public bastion, then an
//! internal jump host that can actually see the subnet. Each hop after the
//! first is reached by opening a `direct-tcpip` channel through the one before
//! it and speaking SSH over that channel — the same thing OpenSSH's
//! `ProxyJump` does.
//!
//! Every hop authenticates separately and every hop's host key is verified
//! separately. A chain is only as trustworthy as its least-checked link, so
//! reaching a host through an already-trusted one earns it nothing.

#[cfg(test)]
mod harness;

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

/// Every session in a chain, in the order they were opened.
///
/// They are held together because they depend on each other: hop 2 speaks
/// through a channel belonging to hop 1, so dropping hop 1 collapses the
/// tunnel. Keeping only the last handle would compile and then fail at
/// runtime the moment the earlier ones were collected.
struct Chain {
    sessions: Vec<russh::client::Handle<Client>>,
}

impl Chain {
    /// The hop the database is reachable from.
    fn last(&self) -> &russh::client::Handle<Client> {
        self.sessions.last().expect("a chain has at least one hop")
    }
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

/// Connect over TCP, authenticate, and return the session and fingerprint.
///
/// Only used for the first hop; every later one arrives over a channel from the
/// hop before it and goes through [`authenticate_stream`] directly.
async fn authenticate(
    ssh: &SshConfig,
    secret: Option<&str>,
    policy: HostKeyPolicy,
) -> Result<(russh::client::Handle<Client>, String)> {
    let stream = TcpStream::connect((ssh.host.as_str(), ssh.port))
        .await
        .map_err(|e| Error::Tunnel(format!("could not reach {}:{} — {e}", ssh.host, ssh.port)))?;
    authenticate_stream(stream, ssh, secret, policy).await
}

/// Speak SSH over an already-open byte stream, authenticate, and verify the key.
///
/// Taking a stream rather than an address is what makes a chain possible: the
/// stream is a TCP socket for the first hop and a `direct-tcpip` channel
/// through the previous session for every one after it.
async fn authenticate_stream<S>(
    stream: S,
    ssh: &SshConfig,
    secret: Option<&str>,
    policy: HostKeyPolicy,
) -> Result<(russh::client::Handle<Client>, String)>
where
    S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
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

    let mut session = russh::client::connect_stream(config, stream, handler)
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

    let fingerprint = seen.lock().ok().and_then(|s| s.clone()).unwrap_or_default();

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
                        session
                            .best_supported_rsa_hash()
                            .await
                            .ok()
                            .flatten()
                            .flatten(),
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
pub async fn probe_host_key(ssh: &SshConfig, secrets: &[Option<String>]) -> Result<String> {
    let config = Arc::new(russh::client::Config::default());
    let seen = Arc::new(std::sync::Mutex::new(None));
    let handler = Client {
        policy: HostKeyPolicy::AcceptAndReport,
        seen: Arc::clone(&seen),
    };

    // An internal jump host is exactly the one that cannot be reached directly,
    // so probing it means going through everything in front of it — each of
    // which must already be trusted. That ordering is the point: hops are
    // confirmed front to back, and none is reached through an unverified one.
    let session = if ssh.via.is_empty() {
        let stream = TcpStream::connect((ssh.host.as_str(), ssh.port))
            .await
            .map_err(|e| {
                Error::Tunnel(format!("could not reach {}:{} — {e}", ssh.host, ssh.port))
            })?;
        russh::client::connect_stream(config, stream, handler).await
    } else {
        let chain = open_chain(&ssh.via.iter().collect::<Vec<_>>(), secrets).await?;
        let channel = chain
            .last()
            .channel_open_direct_tcpip(ssh.host.as_str(), u32::from(ssh.port), "127.0.0.1", 0)
            .await
            .map_err(|e| {
                Error::Tunnel(format!(
                    "the last hop would not open a connection to {}:{} — {e}",
                    ssh.host, ssh.port
                ))
            })?;
        russh::client::connect_stream(config, channel.into_stream(), handler).await
    }
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
/// Authenticate every hop in order, each through the one before it.
///
/// Returns all the sessions rather than the last, because they hold each other
/// up: the channel hop 2 rides on belongs to hop 1.
async fn open_chain(hops: &[&SshConfig], secrets: &[Option<String>]) -> Result<Chain> {
    let mut sessions: Vec<russh::client::Handle<Client>> = Vec::with_capacity(hops.len());

    for (index, hop) in hops.iter().enumerate() {
        let expected = hop.host_key_fingerprint.clone().ok_or_else(|| {
            // Refusing here rather than trusting on first use: an unverified
            // tunnel gives no protection against exactly the attacker a bastion
            // exists to defend against. This applies to every hop — trust does
            // not flow along the chain.
            Error::Tunnel(format!(
                "no known host key for {}:{}. Verify the server's fingerprint before connecting.",
                hop.host, hop.port
            ))
        })?;
        let secret = secrets.get(index).cloned().flatten();
        let policy = HostKeyPolicy::Verify(expected);

        let session = match sessions.last() {
            None => authenticate(hop, secret.as_deref(), policy).await?.0,
            Some(previous) => {
                let channel = previous
                    .channel_open_direct_tcpip(
                        hop.host.as_str(),
                        u32::from(hop.port),
                        "127.0.0.1",
                        0,
                    )
                    .await
                    .map_err(|e| {
                        Error::Tunnel(format!(
                            "hop {} would not open a connection to {}:{} — {e}",
                            index, hop.host, hop.port
                        ))
                    })?;
                authenticate_stream(channel.into_stream(), hop, secret.as_deref(), policy)
                    .await?
                    .0
            }
        };
        sessions.push(session);
    }

    Ok(Chain { sessions })
}

pub async fn open(
    ssh: &SshConfig,
    target_host: &str,
    target_port: u16,
    secrets: &[Option<String>],
) -> Result<Tunnel> {
    let chain = open_chain(&ssh.chain(), secrets).await?;

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
        // The whole chain moves in here: every session has to outlive the
        // tunnel, not just the one the database is behind.
        let chain = Arc::new(chain);
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

            let chain = Arc::clone(&chain);
            let target_host = target_host.clone();
            let conn_cancel = task_cancel.clone();
            // One task per forwarded connection: a driver that opens several
            // sockets (pooling, a cancel channel) must not serialize on one.
            tokio::spawn(async move {
                if let Err(e) = forward(
                    chain.last(),
                    stream,
                    peer,
                    &target_host,
                    target_port,
                    conn_cancel,
                )
                .await
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
            via: Vec::new(),
        }
    }

    /// A hop pointing at one of the in-process servers.
    fn hop(server: &harness::TestSsh) -> SshConfig {
        SshConfig {
            host: "127.0.0.1".into(),
            port: server.port,
            username: "tester".into(),
            auth: SshAuth::Password,
            key_path: None,
            host_key_fingerprint: Some(server.fingerprint.clone()),
            via: Vec::new(),
        }
    }

    fn password() -> Option<String> {
        Some(harness::PASSWORD.to_string())
    }

    /// Send a line through the tunnel and read the answer back.
    async fn round_trip(port: u16) -> String {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};

        let mut socket = TcpStream::connect(("127.0.0.1", port))
            .await
            .expect("connect through the tunnel");
        socket.write_all(b"hello").await.expect("write");

        let mut buffer = [0u8; 64];
        let n = socket.read(&mut buffer).await.expect("read");
        String::from_utf8_lossy(&buffer[..n]).into_owned()
    }

    #[tokio::test]
    async fn opening_without_a_known_host_key_is_refused() {
        // Trust-on-first-use would defeat the purpose of the tunnel, so this
        // must fail before any network traffic rather than connecting anyway.
        // (`Tunnel` is not Debug-printable in the Ok arm, hence the match.)
        match open(&ssh(None), "db.internal", 5432, &[]).await {
            Err(e) => {
                assert_eq!(e.category(), tablex_core::ErrorCategory::Connection);
                assert!(e.to_string().contains("no known host key"), "{e}");
            }
            Ok(_) => panic!("an unverified host must not be tunnelled to"),
        }
    }

    #[tokio::test]
    async fn one_hop_carries_bytes_to_a_host_only_it_can_see() {
        // The first test here that actually forwards anything: a real key
        // exchange, real password auth, a real direct-tcpip channel.
        let server = harness::start_ssh().await;
        let echo = harness::start_echo().await;

        let tunnel = open(&hop(&server), "127.0.0.1", echo, &[password()])
            .await
            .expect("tunnel should open");

        assert_eq!(round_trip(tunnel.local_port()).await, "HELLO");
    }

    #[tokio::test]
    async fn two_hops_carry_bytes_through_both() {
        // The second server is reached only by a channel opened through the
        // first, which is what ProxyJump does and what a production database
        // behind a bastion and an internal jump host needs.
        let first = harness::start_ssh().await;
        let second = harness::start_ssh().await;
        let echo = harness::start_echo().await;

        let mut last = hop(&second);
        last.via = vec![hop(&first)];

        let tunnel = open(&last, "127.0.0.1", echo, &[password(), password()])
            .await
            .expect("a two-hop tunnel should open");

        assert_eq!(round_trip(tunnel.local_port()).await, "HELLO");
    }

    #[tokio::test]
    async fn a_wrong_fingerprint_on_the_second_hop_is_still_refused() {
        // Trust does not flow along the chain. Reaching a host through one that
        // is already trusted earns it nothing, and this is the case a naive
        // implementation gets wrong — the first hop verifies, so the tunnel
        // "works" until someone checks the second.
        let first = harness::start_ssh().await;
        let second = harness::start_ssh().await;
        let echo = harness::start_echo().await;

        let mut last = hop(&second);
        last.host_key_fingerprint = Some("SHA256:definitelynotthekey".into());
        last.via = vec![hop(&first)];

        let err = open(&last, "127.0.0.1", echo, &[password(), password()])
            .await
            .expect_err("a bad key on any hop must refuse");
        assert!(
            err.to_string().contains("host key verification failed"),
            "{err}"
        );
    }

    #[tokio::test]
    async fn a_hop_with_no_stored_fingerprint_is_refused_before_connecting() {
        let first = harness::start_ssh().await;
        let second = harness::start_ssh().await;

        let mut last = hop(&second);
        let mut unverified = hop(&first);
        unverified.host_key_fingerprint = None;
        last.via = vec![unverified];

        let err = open(&last, "127.0.0.1", 5432, &[password(), password()])
            .await
            .expect_err("an unverified hop must refuse");
        assert!(err.to_string().contains("no known host key"), "{err}");
    }

    #[tokio::test]
    async fn the_wrong_password_fails_rather_than_forwarding() {
        let server = harness::start_ssh().await;
        let err = open(&hop(&server), "127.0.0.1", 5432, &[Some("wrong".into())])
            .await
            .expect_err("bad credentials must not produce a tunnel");
        assert_eq!(err.category(), tablex_core::ErrorCategory::Auth);
    }

    #[tokio::test]
    async fn probing_reports_the_key_the_server_actually_presented() {
        let server = harness::start_ssh().await;
        let mut config = hop(&server);
        config.host_key_fingerprint = None;

        let seen = probe_host_key(&config, &[]).await.expect("probe");
        assert_eq!(seen, server.fingerprint);
    }

    #[tokio::test]
    async fn probing_a_jump_host_goes_through_what_is_in_front_of_it() {
        // An internal jump host is precisely the one that cannot be reached
        // directly, so its fingerprint can only be read through the hops
        // already trusted — which is also why hops are confirmed front to back.
        let first = harness::start_ssh().await;
        let second = harness::start_ssh().await;

        let mut inner = hop(&second);
        inner.host_key_fingerprint = None;
        inner.via = vec![hop(&first)];

        let seen = probe_host_key(&inner, &[password()])
            .await
            .expect("probe through the chain");
        assert_eq!(seen, second.fingerprint);
    }

    #[tokio::test]
    async fn dropping_the_tunnel_stops_accepting() {
        let server = harness::start_ssh().await;
        let echo = harness::start_echo().await;

        let tunnel = open(&hop(&server), "127.0.0.1", echo, &[password()])
            .await
            .expect("tunnel");
        let port = tunnel.local_port();
        assert_eq!(round_trip(port).await, "HELLO");

        drop(tunnel);
        // The listener closes asynchronously, so this allows for the task to
        // notice the cancellation rather than asserting on the same tick.
        for _ in 0..50 {
            if TcpStream::connect(("127.0.0.1", port)).await.is_err() {
                return;
            }
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
        panic!("the port was still accepting after the tunnel was dropped");
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
        assert!(
            observer.is_cancelled(),
            "dropping must tear the tunnel down"
        );
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
