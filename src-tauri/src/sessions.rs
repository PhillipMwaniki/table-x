//! Live database sessions.
//!
//! One [`tablex_core::Connection`] per open connection, addressed by the saved
//! connection's id. Each session sits behind its own async mutex: the trait takes
//! `&mut self` because a database session is not safe to use concurrently, and
//! holding a per-session lock means two tabs querying *different* connections
//! never block each other while two tabs querying the *same* one are serialized
//! rather than corrupting the protocol stream.

use std::collections::HashMap;
use std::sync::Arc;
use tablex_core::{
    driver::CancelHandle,
    error::{Error, Result},
    Connection,
};
use tablex_tunnel::Tunnel;
use tokio::sync::Mutex;

/// One live session, plus the SSH tunnel it travels over if there is one.
pub struct LiveSession {
    pub connection: Mutex<Box<dyn Connection>>,
    /// Kept alive with the session: the tunnel shuts down when this is dropped.
    tunnel: Option<Tunnel>,
    /// Stops whatever the connection is running.
    ///
    /// Behind its own lock rather than reached through the connection's,
    /// because the statement being cancelled is holding that one — a cancel
    /// that queued for it would arrive after the thing it was cancelling had
    /// finished. This lock is only ever held long enough to clone the handle.
    cancel: Mutex<Option<Arc<dyn CancelHandle>>>,
}

impl LiveSession {
    pub fn new(connection: Box<dyn Connection>, tunnel: Option<Tunnel>) -> Self {
        // Taken before the connection is locked away — that is the whole point
        // of the handle.
        let cancel = connection.cancel_handle();
        LiveSession {
            connection: Mutex::new(connection),
            tunnel,
            cancel: Mutex::new(cancel),
        }
    }

    /// Stop whatever this session is running, if the engine can.
    ///
    /// Cancelling nothing succeeds: by the time a click reaches here the
    /// statement has often already finished, and reporting that as a failure
    /// would be reporting the good outcome as the bad one.
    pub async fn cancel(&self) -> Result<()> {
        let handle = self.cancel.lock().await.clone();
        match handle {
            Some(handle) => handle.cancel().await,
            None => Err(Error::Unsupported(
                "this driver cannot cancel a running statement".into(),
            )),
        }
    }

    /// The loopback port the tunnel listens on, if this session travels over one.
    ///
    /// Needed to reconnect *through the same tunnel*: PostgreSQL cannot change
    /// database on an open connection, and opening a second tunnel to do it
    /// would mean a second SSH session and another authentication.
    pub fn tunnel_port(&self) -> Option<u16> {
        self.tunnel.as_ref().map(|t| t.local_port())
    }

    /// Swap in a new connection, closing the old one.
    ///
    /// The tunnel and the session's identity survive, so everything holding this
    /// `Arc` keeps working — the connection underneath simply points somewhere
    /// else now.
    pub async fn replace(&self, connection: Box<dyn Connection>) {
        // The handle belongs to the socket, not to the session, so it has to
        // travel with the swap — otherwise cancel would keep aiming at a
        // connection that is already closed.
        *self.cancel.lock().await = connection.cancel_handle();
        let mut guard = self.connection.lock().await;
        let mut previous = std::mem::replace(&mut *guard, connection);
        // Best effort: a socket that will not close cleanly must not stop the
        // user from working in the database they just switched to.
        let _ = previous.close().await;
    }
}

/// A handle to one live session.
pub type Session = Arc<LiveSession>;

#[derive(Default)]
pub struct SessionRegistry {
    // The outer lock is held only long enough to clone an Arc, never across a
    // query — otherwise one slow statement would freeze the whole application.
    sessions: Mutex<HashMap<String, Session>>,
}

impl SessionRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// Register a freshly opened session, replacing and closing any previous one
    /// for the same connection id.
    pub async fn insert(&self, id: &str, connection: Box<dyn Connection>, tunnel: Option<Tunnel>) {
        let previous = {
            let mut map = self.sessions.lock().await;
            map.insert(
                id.to_string(),
                Arc::new(LiveSession::new(connection, tunnel)),
            )
        };
        if let Some(old) = previous {
            // Reconnecting must not leak the old socket, or the old tunnel —
            // which is torn down when the replaced session is dropped.
            let _ = old.connection.lock().await.close().await;
        }
    }

    /// Look up a live session.
    pub async fn get(&self, id: &str) -> Result<Session> {
        self.sessions
            .lock()
            .await
            .get(id)
            .cloned()
            .ok_or_else(|| Error::UnknownConnection(id.to_string()))
    }

    /// Close and forget a session. Closing something already closed succeeds, so
    /// disconnect is idempotent.
    pub async fn remove(&self, id: &str) -> Result<()> {
        let session = self.sessions.lock().await.remove(id);
        if let Some(session) = session {
            session.connection.lock().await.close().await?;
        }
        Ok(())
    }

    /// Ids of every open session, for the UI's connection indicators.
    pub async fn open_ids(&self) -> Vec<String> {
        let mut ids: Vec<String> = self.sessions.lock().await.keys().cloned().collect();
        ids.sort();
        ids
    }

    /// Close everything, on shutdown.
    pub async fn close_all(&self) {
        let drained: Vec<Session> = {
            let mut map = self.sessions.lock().await;
            map.drain().map(|(_, s)| s).collect()
        };
        for session in drained {
            let _ = session.connection.lock().await.close().await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use tablex_core::{
        driver::{CompletionScope, FetchOptions, RowEdit},
        result::QueryOutcome,
        schema::{SchemaNode, TableDetail},
    };

    /// Counts closes so tests can assert sessions are not leaked.
    struct FakeConnection {
        closes: Arc<AtomicUsize>,
    }

    #[async_trait]
    impl Connection for FakeConnection {
        async fn execute(&mut self, _sql: &str, _opts: &FetchOptions) -> Result<QueryOutcome> {
            Ok(QueryOutcome {
                statements: vec![],
                elapsed_ms: 0,
                notices: vec![],
            })
        }
        async fn browse(&mut self, _parent: Option<&str>) -> Result<Vec<SchemaNode>> {
            Ok(vec![])
        }
        async fn table_detail(&mut self, _s: Option<&str>, _t: &str) -> Result<TableDetail> {
            Err(Error::Unsupported("fake".into()))
        }
        async fn apply_edit(&mut self, _edit: &RowEdit) -> Result<()> {
            Ok(())
        }
        async fn ping(&mut self) -> Result<()> {
            Ok(())
        }
        async fn close(&mut self) -> Result<()> {
            self.closes.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
        async fn completion_scope(&mut self) -> Result<CompletionScope> {
            Ok(CompletionScope::default())
        }
    }

    fn fake(closes: &Arc<AtomicUsize>) -> Box<dyn Connection> {
        Box::new(FakeConnection {
            closes: Arc::clone(closes),
        })
    }

    #[tokio::test]
    async fn unknown_connection_is_a_named_error() {
        let reg = SessionRegistry::new();
        match reg.get("nope").await {
            Err(Error::UnknownConnection(id)) => assert_eq!(id, "nope"),
            Err(other) => panic!("expected UnknownConnection, got {other:?}"),
            Ok(_) => panic!("expected an error"),
        }
    }

    #[tokio::test]
    async fn sessions_are_retrievable_after_insert() {
        let closes = Arc::new(AtomicUsize::new(0));
        let reg = SessionRegistry::new();
        reg.insert("a", fake(&closes), None).await;

        assert!(reg.get("a").await.is_ok());
        assert_eq!(reg.open_ids().await, vec!["a".to_string()]);
    }

    #[tokio::test]
    async fn reconnecting_closes_the_replaced_session() {
        let closes = Arc::new(AtomicUsize::new(0));
        let reg = SessionRegistry::new();
        reg.insert("a", fake(&closes), None).await;
        reg.insert("a", fake(&closes), None).await;

        // Without this, reconnecting would leak a socket every time.
        assert_eq!(closes.load(Ordering::SeqCst), 1);
        assert_eq!(reg.open_ids().await.len(), 1);
    }

    #[tokio::test]
    async fn removing_closes_the_session_and_is_idempotent() {
        let closes = Arc::new(AtomicUsize::new(0));
        let reg = SessionRegistry::new();
        reg.insert("a", fake(&closes), None).await;

        reg.remove("a").await.expect("first remove");
        assert_eq!(closes.load(Ordering::SeqCst), 1);
        assert!(reg.open_ids().await.is_empty());

        // Disconnecting twice must not error — the UI can fire it on a stale view.
        reg.remove("a").await.expect("second remove must succeed");
        assert_eq!(closes.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn close_all_closes_every_session() {
        let closes = Arc::new(AtomicUsize::new(0));
        let reg = SessionRegistry::new();
        reg.insert("a", fake(&closes), None).await;
        reg.insert("b", fake(&closes), None).await;

        reg.close_all().await;
        assert_eq!(closes.load(Ordering::SeqCst), 2);
        assert!(reg.open_ids().await.is_empty());
    }

    #[tokio::test]
    async fn a_busy_session_does_not_block_a_different_one() {
        let closes = Arc::new(AtomicUsize::new(0));
        let reg = SessionRegistry::new();
        reg.insert("slow", fake(&closes), None).await;
        reg.insert("fast", fake(&closes), None).await;

        // Hold "slow" the way a long-running query would.
        let slow = reg.get("slow").await.expect("slow session");
        let _held = slow.connection.lock().await;

        // The registry map must not still be locked, or every other connection
        // in the app would stall behind this one query.
        let fast = reg
            .get("fast")
            .await
            .expect("registry must stay responsive");
        assert!(
            fast.connection.try_lock().is_ok(),
            "an unrelated session must be free"
        );
    }
}
