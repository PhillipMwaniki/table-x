//! An SSH server that runs inside the test process.
//!
//! The tunnel could not be tested without one. Every previous test here checked
//! a refusal — a missing fingerprint, a missing key path — because anything that
//! actually forwarded bytes needed a server, and depending on a real one would
//! have meant tests that pass on one machine and not another.
//!
//! This is a real SSH server: a real key exchange, real authentication, real
//! `direct-tcpip` channels connected to real sockets. What it is not is
//! hardened — it accepts one password and forwards anywhere — which is why it
//! is compiled only for tests.

use russh::keys::PrivateKey;
use russh::server::{Auth, Handler, Msg, Server, Session};
use russh::{Channel, ChannelId};
use std::sync::Arc;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

/// The password every test server accepts.
pub const PASSWORD: &str = "correct horse battery staple";

/// A server listening on a loopback port, with its host key fingerprint.
pub struct TestSsh {
    pub port: u16,
    pub fingerprint: String,
}

#[derive(Clone)]
struct TestServer;

impl Server for TestServer {
    type Handler = TestHandler;
    fn new_client(&mut self, _peer: Option<std::net::SocketAddr>) -> TestHandler {
        TestHandler
    }
}

struct TestHandler;

impl Handler for TestHandler {
    type Error = russh::Error;

    async fn auth_password(&mut self, _user: &str, password: &str) -> Result<Auth, Self::Error> {
        Ok(if password == PASSWORD {
            Auth::Accept
        } else {
            Auth::reject()
        })
    }

    /// Connect the requested address for real and pump bytes both ways.
    ///
    /// This is the behaviour under test: whether the tunnel can carry a
    /// connection to something that is only reachable from here.
    async fn channel_open_direct_tcpip(
        &mut self,
        channel: Channel<Msg>,
        host_to_connect: &str,
        port_to_connect: u32,
        _originator_address: &str,
        _originator_port: u32,
        reply: russh::server::ChannelOpenHandle,
        _session: &mut Session,
    ) -> Result<(), Self::Error> {
        let target = format!("{host_to_connect}:{port_to_connect}");
        let Ok(remote) = TcpStream::connect(&target).await else {
            // Refusing rather than accepting and dropping: a client that is
            // told no can report why, and a test that hangs reports nothing.
            reply
                .reject(russh::ChannelOpenFailure::ConnectFailed)
                .await;
            return Ok(());
        };
        reply.accept().await;

        tokio::spawn(async move {
            let mut remote = remote;
            let mut stream = channel.into_stream();
            let _ = tokio::io::copy_bidirectional(&mut stream, &mut remote).await;
        });
        Ok(())
    }

    async fn data(
        &mut self,
        _channel: ChannelId,
        _data: &[u8],
        _session: &mut Session,
    ) -> Result<(), Self::Error> {
        Ok(())
    }
}

/// Start a server on an OS-chosen loopback port.
pub async fn start_ssh() -> TestSsh {
    let key = PrivateKey::random(&mut rand::rng(), russh::keys::Algorithm::Ed25519)
        .expect("generate a host key");
    let fingerprint = key
        .public_key()
        .fingerprint(russh::keys::HashAlg::Sha256)
        .to_string();

    let config = Arc::new(russh::server::Config {
        keys: vec![key],
        auth_rejection_time: std::time::Duration::from_millis(1),
        ..Default::default()
    });

    let listener = TcpListener::bind(("127.0.0.1", 0)).await.expect("bind");
    let port = listener.local_addr().expect("addr").port();

    tokio::spawn(async move {
        loop {
            let Ok((stream, peer)) = listener.accept().await else {
                break;
            };
            let config = Arc::clone(&config);
            let mut server = TestServer;
            tokio::spawn(async move {
                let handler = server.new_client(Some(peer));
                let _ = russh::server::run_stream(config, stream, handler).await;
            });
        }
    });

    TestSsh { port, fingerprint }
}

/// A TCP server that echoes whatever is sent to it, uppercased.
///
/// Uppercasing rather than echoing verbatim proves the bytes reached this
/// process rather than being reflected somewhere along the way.
pub async fn start_echo() -> u16 {
    let listener = TcpListener::bind(("127.0.0.1", 0)).await.expect("bind");
    let port = listener.local_addr().expect("addr").port();

    tokio::spawn(async move {
        while let Ok((mut stream, _)) = listener.accept().await {
            tokio::spawn(async move {
                let mut buffer = [0u8; 1024];
                loop {
                    match stream.read(&mut buffer).await {
                        Ok(0) | Err(_) => break,
                        Ok(n) => {
                            let upper = buffer[..n].to_ascii_uppercase();
                            if stream.write_all(&upper).await.is_err() {
                                break;
                            }
                        }
                    }
                }
            });
        }
    });

    port
}
