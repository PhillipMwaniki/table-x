//! Live database sessions.
//!
//! One [`tablex_core::Connection`] per open connection, addressed by the saved
//! connection's id. Each session sits behind its own async mutex: the trait takes
//! `&mut self` because a database session is not safe to use concurrently, and
//! holding a per-session lock means two tabs querying *different* connections
//! never block each other while two tabs querying the *same* one are serialized
//! rather than corrupting the protocol stream.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tablex_core::{
    driver::CancelHandle,
    error::{Error, Result},
    sql::TxEffect,
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
    /// Whether this session is currently inside a transaction.
    ///
    /// Tracked here rather than asked of the server on every keystroke: three of
    /// the four engines that have transactions will answer, but each answers
    /// differently and one round trip per poll to redraw an indicator is a poor
    /// trade. Atomic rather than behind the connection's lock so the indicator
    /// can be read while a statement is running — which is exactly when somebody
    /// looks at it.
    in_transaction: AtomicBool,
    /// The database this session is pointed at, as far as this process knows.
    ///
    /// Remembered rather than asked of the server, because the moment it is
    /// needed most is the moment the server cannot be asked: rebuilding a
    /// broken link has to come back on the database the user was working in,
    /// and the socket that knew which one that was is gone.
    database: Mutex<Option<String>>,
}

impl LiveSession {
    pub fn new(
        connection: Box<dyn Connection>,
        tunnel: Option<Tunnel>,
        database: Option<String>,
    ) -> Self {
        // Taken before the connection is locked away — that is the whole point
        // of the handle.
        let cancel = connection.cancel_handle();
        LiveSession {
            connection: Mutex::new(connection),
            tunnel,
            cancel: Mutex::new(cancel),
            in_transaction: AtomicBool::new(false),
            database: Mutex::new(database),
        }
    }

    /// The database this session is pointed at, for a reconnect to aim at.
    pub async fn database(&self) -> Option<String> {
        self.database.lock().await.clone()
    }

    /// Record where the session now points, after a switch that succeeded.
    pub async fn note_database(&self, database: Option<String>) {
        *self.database.lock().await = database;
    }

    /// Whether a transaction is open on this session.
    pub fn in_transaction(&self) -> bool {
        self.in_transaction.load(Ordering::SeqCst)
    }

    /// Record what a submission did to the transaction state.
    ///
    /// Called after every successful execute, so a `BEGIN` somebody typed into
    /// the editor moves the indicator exactly as the button does. An indicator
    /// that says "no transaction" while the connection is holding locks is worse
    /// than no indicator, because it is believed.
    pub fn note_effect(&self, effect: Option<TxEffect>) {
        match effect {
            Some(TxEffect::Opened) => self.in_transaction.store(true, Ordering::SeqCst),
            Some(TxEffect::Closed) => self.in_transaction.store(false, Ordering::SeqCst),
            None => {}
        }
    }

    /// Open a transaction.
    pub async fn begin(&self) -> Result<()> {
        // The flag moves only after the server agreed. Setting it first would
        // show an open transaction that does not exist, and the next statement
        // would silently auto-commit under a badge saying it had not.
        self.connection.lock().await.begin().await?;
        self.in_transaction.store(true, Ordering::SeqCst);
        Ok(())
    }

    /// Commit the open transaction.
    pub async fn commit(&self) -> Result<()> {
        self.connection.lock().await.commit().await?;
        self.in_transaction.store(false, Ordering::SeqCst);
        Ok(())
    }

    /// Roll back the open transaction.
    ///
    /// A failed rollback still clears the flag. The usual cause is that the
    /// transaction is already gone — the server rolled it back itself on a fatal
    /// error, or the connection dropped — and in every one of those cases there
    /// is no transaction left to be in. Leaving the badge lit would strand the
    /// user with a button that cannot succeed.
    pub async fn rollback(&self) -> Result<()> {
        let result = self.connection.lock().await.rollback().await;
        self.in_transaction.store(false, Ordering::SeqCst);
        result
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
    /// else now. `database` says where that is, and is not optional to work out
    /// from the outside: a session whose remembered database disagrees with the
    /// socket underneath it sends the next statement to the wrong place.
    pub async fn replace(&self, connection: Box<dyn Connection>, database: Option<String>) {
        // The handle belongs to the socket, not to the session, so it has to
        // travel with the swap — otherwise cancel would keep aiming at a
        // connection that is already closed.
        *self.cancel.lock().await = connection.cancel_handle();
        // A new socket is not inside anything. Whatever was open went away with
        // the old one, rolled back by the server.
        self.in_transaction.store(false, Ordering::SeqCst);
        self.note_database(database).await;
        let mut previous = {
            let mut guard = self.connection.lock().await;
            std::mem::replace(&mut *guard, connection)
        };
        close_politely(previous.close()).await;
    }

    /// Close this session's connection, on the way out of the registry.
    ///
    /// Best effort by design — see [`close_politely`].
    pub async fn close(&self) {
        close_politely(async { self.connection.lock().await.close().await }).await;
    }
}

/// How long a discarded connection gets to close before it is simply dropped.
const CLOSE_TIMEOUT: Duration = Duration::from_secs(5);

/// Close a connection that is on its way out, without letting it hang.
///
/// Every caller is replacing or removing the session anyway, so there is nothing
/// to report and nothing to retry. The clock matters because the usual reason
/// for closing is a link that has already failed, and a socket in that state can
/// take the operating system's full TCP timeout to admit it — waiting that out
/// would make recovering from a break slower than the break. Dropping the
/// connection instead closes its file descriptor regardless.
async fn close_politely(closing: impl std::future::Future<Output = Result<()>>) {
    match tokio::time::timeout(CLOSE_TIMEOUT, closing).await {
        Ok(Ok(())) => {}
        Ok(Err(e)) => tracing::debug!("a discarded session did not close cleanly: {e}"),
        Err(_) => tracing::warn!("a discarded session did not close within {CLOSE_TIMEOUT:?}"),
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
    ///
    /// `database` is where the new session points, so a later reconnect can aim
    /// at the same place rather than at the config's default.
    pub async fn insert(
        &self,
        id: &str,
        connection: Box<dyn Connection>,
        tunnel: Option<Tunnel>,
        database: Option<String>,
    ) {
        let previous = {
            let mut map = self.sessions.lock().await;
            map.insert(
                id.to_string(),
                Arc::new(LiveSession::new(connection, tunnel, database)),
            )
        };
        if let Some(old) = previous {
            // Reconnecting must not leak the old socket, or the old tunnel —
            // which is torn down when the replaced session is dropped.
            old.close().await;
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

    /// Close and forget a session. Removing something that is not there succeeds,
    /// so disconnect is idempotent.
    ///
    /// The close itself cannot fail the call: the session is out of the map
    /// either way, and the one time closing goes wrong is the one time the user
    /// most needs the disconnect to land — a link that has already broken. An
    /// error there would leave the UI showing a connection that no longer exists.
    pub async fn remove(&self, id: &str) {
        let session = self.sessions.lock().await.remove(id);
        if let Some(session) = session {
            session.close().await;
        }
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
            session.close().await;
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
    async fn a_typed_transaction_statement_moves_the_indicator() {
        // The buttons are not the only way in. Someone who types BEGIN into the
        // editor is just as inside a transaction, and an indicator that missed
        // it would be believed anyway.
        let closes = Arc::new(AtomicUsize::new(0));
        let session = LiveSession::new(fake(&closes), None, None);
        assert!(!session.in_transaction());

        session.note_effect(Some(TxEffect::Opened));
        assert!(session.in_transaction());

        session.note_effect(None);
        assert!(
            session.in_transaction(),
            "an ordinary statement changes nothing"
        );

        session.note_effect(Some(TxEffect::Closed));
        assert!(!session.in_transaction());
    }

    #[tokio::test]
    async fn switching_database_leaves_no_transaction_behind() {
        // The transaction went away with the old socket. Reporting one on the
        // new one would offer a commit that cannot land.
        let closes = Arc::new(AtomicUsize::new(0));
        let session = LiveSession::new(fake(&closes), None, None);
        session.note_effect(Some(TxEffect::Opened));

        session.replace(fake(&closes), Some("other".into())).await;
        assert!(!session.in_transaction());
    }

    #[tokio::test]
    async fn a_session_remembers_where_it_points() {
        // This is what a reconnect aims at. Coming back on the config's default
        // after a link broke would run the next statement against the wrong
        // database while the tab still says otherwise.
        let closes = Arc::new(AtomicUsize::new(0));
        let session = LiveSession::new(fake(&closes), None, Some("app".into()));
        assert_eq!(session.database().await.as_deref(), Some("app"));

        session.note_database(Some("app_staging".into())).await;
        assert_eq!(session.database().await.as_deref(), Some("app_staging"));

        // A swap carries the new socket's database with it, rather than leaving
        // the old answer behind for a reconnect to trust.
        session
            .replace(fake(&closes), Some("reporting".into()))
            .await;
        assert_eq!(session.database().await.as_deref(), Some("reporting"));
    }

    #[tokio::test]
    async fn a_reconnect_starts_from_where_the_old_session_pointed() {
        let closes = Arc::new(AtomicUsize::new(0));
        let reg = SessionRegistry::new();
        reg.insert("a", fake(&closes), None, Some("app".into()))
            .await;

        let was = reg.get("a").await.expect("session").database().await;
        reg.insert("a", fake(&closes), None, was.clone()).await;

        assert_eq!(
            reg.get("a").await.expect("session").database().await,
            Some("app".to_string())
        );
    }

    #[tokio::test]
    async fn a_driver_without_transactions_says_so() {
        let closes = Arc::new(AtomicUsize::new(0));
        let session = LiveSession::new(fake(&closes), None, None);
        match session.begin().await {
            Err(Error::Unsupported(_)) => {}
            other => panic!("expected Unsupported, got {other:?}"),
        }
        // And it must not claim to be in one it could not open.
        assert!(!session.in_transaction());
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
        reg.insert("a", fake(&closes), None, None).await;

        assert!(reg.get("a").await.is_ok());
        assert_eq!(reg.open_ids().await, vec!["a".to_string()]);
    }

    #[tokio::test]
    async fn reconnecting_closes_the_replaced_session() {
        let closes = Arc::new(AtomicUsize::new(0));
        let reg = SessionRegistry::new();
        reg.insert("a", fake(&closes), None, None).await;
        reg.insert("a", fake(&closes), None, None).await;

        // Without this, reconnecting would leak a socket every time.
        assert_eq!(closes.load(Ordering::SeqCst), 1);
        assert_eq!(reg.open_ids().await.len(), 1);
    }

    #[tokio::test]
    async fn removing_closes_the_session_and_is_idempotent() {
        let closes = Arc::new(AtomicUsize::new(0));
        let reg = SessionRegistry::new();
        reg.insert("a", fake(&closes), None, None).await;

        reg.remove("a").await;
        assert_eq!(closes.load(Ordering::SeqCst), 1);
        assert!(reg.open_ids().await.is_empty());

        // Disconnecting twice must be harmless — the UI can fire it on a stale
        // view, and on a link that broke before anyone pressed anything.
        reg.remove("a").await;
        assert_eq!(closes.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn close_all_closes_every_session() {
        let closes = Arc::new(AtomicUsize::new(0));
        let reg = SessionRegistry::new();
        reg.insert("a", fake(&closes), None, None).await;
        reg.insert("b", fake(&closes), None, None).await;

        reg.close_all().await;
        assert_eq!(closes.load(Ordering::SeqCst), 2);
        assert!(reg.open_ids().await.is_empty());
    }

    #[tokio::test]
    async fn a_busy_session_does_not_block_a_different_one() {
        let closes = Arc::new(AtomicUsize::new(0));
        let reg = SessionRegistry::new();
        reg.insert("slow", fake(&closes), None, None).await;
        reg.insert("fast", fake(&closes), None, None).await;

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
