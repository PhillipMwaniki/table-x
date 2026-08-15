//! Connection configuration.
//!
//! Secrets are deliberately *not* part of this struct. It is serialized to disk
//! and sent to the frontend, so passwords and key passphrases live in the OS
//! keychain and are looked up by [`ConnectionConfig::id`] at connect time.

use indexmap::IndexMap;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConnectionConfig {
    /// Stable identifier; also the keychain lookup key for this connection.
    pub id: String,
    /// User-facing name.
    pub name: String,
    /// Which driver handles this connection (`postgres`, `sqlite`, …).
    pub driver: String,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub host: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub port: Option<u16>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub database: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub username: Option<String>,

    /// Filesystem path, for embedded databases (SQLite, DuckDB).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub file_path: Option<String>,

    #[serde(default)]
    pub tls: TlsConfig,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ssh: Option<SshConfig>,

    /// Group this connection is filed under in the sidebar.
    ///
    /// A flat name rather than a tree. One level separates "work" from
    /// "personal" or staging from production, which is what people actually do
    /// with folders here; nesting would add a move-between-parents problem for
    /// a list that is rarely longer than a screen.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub folder: Option<String>,

    /// Colour tag shown in the UI. A loud red on production connections is a
    /// cheap and effective guard against running the wrong statement.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color: Option<String>,

    /// Refuse writes on this connection regardless of database permissions.
    #[serde(default)]
    pub read_only: bool,

    /// Ask before running anything that would destroy data.
    ///
    /// `None` follows the colour tag, which is what people already use to mark
    /// production — so an existing red connection gains the gate without anyone
    /// setting a second flag that means the same thing. Set explicitly to
    /// override in either direction.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub confirm_destructive: Option<bool>,

    /// Driver-specific extras that do not warrant a first-class field.
    ///
    /// Always serialized, for the same reason as `QueryOutcome::notices`: the
    /// frontend types this as a map, and a field that vanishes when empty makes
    /// every consumer guard against a shape the type says cannot happen.
    #[serde(default)]
    pub options: IndexMap<String, String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct TlsConfig {
    #[serde(default)]
    pub mode: TlsMode,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ca_cert_path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub client_cert_path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub client_key_path: Option<String>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TlsMode {
    Disable,
    /// Encrypt if the server offers it, but do not verify. Convenient, not secure.
    #[default]
    Prefer,
    /// Require encryption and verify the server certificate chain and hostname.
    VerifyFull,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SshConfig {
    pub host: String,
    #[serde(default = "default_ssh_port")]
    pub port: u16,
    pub username: String,
    #[serde(default)]
    pub auth: SshAuth,
    /// Path to the private key for [`SshAuth::PublicKey`]. The path is not a
    /// secret and lives here; the passphrase lives in the keychain.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub key_path: Option<String>,
    /// Expected host key fingerprint, in OpenSSH `SHA256:...` form. Populated on
    /// first connect after the user confirms it, then checked on every
    /// subsequent connect.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub host_key_fingerprint: Option<String>,

    /// Hosts to pass through *before* reaching this one, in order.
    ///
    /// A production database is often two hops away: a public bastion, then an
    /// internal jump host that can actually see the subnet. Each hop
    /// authenticates separately and has its own host key to verify — a chain is
    /// only as trustworthy as its least-checked link, so there is no inherited
    /// trust here.
    ///
    /// Empty for the ordinary single-hop case, which is why it defaults: a
    /// connection saved before this existed still loads.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub via: Vec<SshConfig>,
}

impl SshConfig {
    /// Every hop in the order they are traversed, ending with this one.
    ///
    /// The database is reachable from the last element, which is what makes
    /// "the SSH config" and "the final hop" the same thing for a direct tunnel.
    pub fn chain(&self) -> Vec<&SshConfig> {
        let mut hops: Vec<&SshConfig> = self.via.iter().collect();
        hops.push(self);
        hops
    }
}

fn default_ssh_port() -> u16 {
    22
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SshAuth {
    Password,
    /// Private key on disk; the path is stored here, the passphrase in the keychain.
    #[default]
    PublicKey,
    /// Delegate to a running ssh-agent — no key material touches this process.
    Agent,
}

impl ConnectionConfig {
    /// Keychain entry name for this connection's primary secret.
    pub fn keychain_key(&self) -> String {
        format!("connection.{}", self.id)
    }

    /// Keychain entry name for the SSH secret, kept separate so a database
    /// password and a key passphrase never overwrite each other.
    pub fn ssh_keychain_key(&self) -> String {
        format!("connection.{}.ssh", self.id)
    }

    /// The keychain entry for each hop of the SSH chain, in chain order.
    ///
    /// The *last* hop keeps the original single-hop entry rather than the
    /// first. That is the migration-safe assignment: adding a jump host in
    /// front of a connection that already worked leaves the secret it already
    /// had attached to the host it was always for. Keying by position from the
    /// front would silently repoint it at the new hop, and the failure would
    /// look like a wrong password on a server that never had one.
    ///
    /// Earlier hops get their own indexed entries, since a bastion's key
    /// passphrase and a jump host's password are different secrets and must not
    /// overwrite each other.
    pub fn ssh_hop_keys(&self) -> Vec<String> {
        let Some(ssh) = &self.ssh else {
            return Vec::new();
        };
        let last = ssh.chain().len() - 1;
        (0..=last)
            .map(|i| {
                if i == last {
                    self.ssh_keychain_key()
                } else {
                    format!("connection.{}.ssh.{i}", self.id)
                }
            })
            .collect()
    }

    /// Whether a destructive statement should be confirmed on this connection.
    ///
    /// A read-only connection needs no gate: the write is refused before it can
    /// destroy anything, and a confirmation for something that will not happen
    /// is a dialog that teaches people to dismiss dialogs.
    pub fn confirms_destructive(&self) -> bool {
        if self.read_only {
            return false;
        }
        self.confirm_destructive.unwrap_or(self.color.is_some())
    }

    /// A short summary for the connection list, e.g. `postgres@db.example.com:5432/app`.
    pub fn summary(&self) -> String {
        if let Some(path) = &self.file_path {
            return path.clone();
        }
        let user = self.username.as_deref().unwrap_or("");
        let host = self.host.as_deref().unwrap_or("localhost");
        let db = self.database.as_deref().unwrap_or("");
        let mut s = String::new();
        if !user.is_empty() {
            s.push_str(user);
            s.push('@');
        }
        s.push_str(host);
        if let Some(p) = self.port {
            s.push(':');
            s.push_str(&p.to_string());
        }
        if !db.is_empty() {
            s.push('/');
            s.push_str(db);
        }
        s
    }
}

#[cfg(test)]
mod tests {

    #[test]
    fn the_colour_tag_turns_the_gate_on_without_a_second_flag() {
        // A red connection already means production to the person who set it;
        // making them set another flag that means the same thing is how the two
        // end up disagreeing.
        let mut c = base();
        c.read_only = false;
        assert!(
            !c.confirms_destructive(),
            "an untagged connection stays quiet"
        );

        c.color = Some("#e5484d".into());
        assert!(c.confirms_destructive());
    }

    #[test]
    fn an_explicit_setting_wins_in_either_direction() {
        let mut c = base();
        c.read_only = false;
        c.color = Some("#e5484d".into());
        c.confirm_destructive = Some(false);
        assert!(!c.confirms_destructive(), "an explicit no must be honoured");

        c.color = None;
        c.confirm_destructive = Some(true);
        assert!(c.confirms_destructive());
    }

    #[test]
    fn a_read_only_connection_needs_no_gate() {
        // The write is refused before it can destroy anything, and a
        // confirmation for something that will not happen is a dialog that
        // teaches people to dismiss dialogs.
        let mut c = base();
        c.read_only = true;
        c.color = Some("#e5484d".into());
        c.confirm_destructive = Some(true);
        assert!(!c.confirms_destructive());
    }
    use super::*;

    fn base() -> ConnectionConfig {
        ConnectionConfig {
            id: "abc".into(),
            name: "Prod".into(),
            driver: "postgres".into(),
            host: Some("db.example.com".into()),
            port: Some(5432),
            database: Some("app".into()),
            username: Some("readonly".into()),
            file_path: None,
            tls: TlsConfig::default(),
            ssh: None,
            folder: None,
            color: None,
            read_only: true,
            confirm_destructive: None,
            options: IndexMap::new(),
        }
    }

    #[test]
    fn config_never_carries_a_password() {
        let json = serde_json::to_string(&base()).expect("serialize");
        // The struct has no password field at all; this guards against one being
        // added carelessly later, since this value is written to disk as-is.
        assert!(
            !json.contains("password"),
            "secrets must stay in the keychain"
        );
        assert!(!json.contains("passphrase"));
    }

    #[test]
    fn an_empty_option_map_still_crosses_the_wire() {
        // Same contract as QueryOutcome::notices: the frontend types this as a
        // map, so a field that disappears when empty forces every consumer to
        // guard against a shape the type says cannot occur.
        let json = serde_json::to_string(&base()).expect("serialize");
        assert!(json.contains("\"options\":{}"), "{json}");
    }

    #[test]
    fn database_and_ssh_secrets_use_distinct_keychain_entries() {
        let c = base();
        assert_ne!(c.keychain_key(), c.ssh_keychain_key());
    }

    #[test]
    fn adding_a_jump_host_does_not_move_the_secret_that_already_worked() {
        // The last hop keeps the original entry. Keying from the front would
        // repoint an existing secret at the newly added host, and the failure
        // would look like a wrong password on a server that never had one.
        let mut c = base();
        c.ssh = Some(SshConfig {
            host: "bastion.example.com".into(),
            port: 22,
            username: "deploy".into(),
            auth: SshAuth::PublicKey,
            key_path: None,
            host_key_fingerprint: None,
            via: Vec::new(),
        });
        let single = c.ssh_hop_keys();
        assert_eq!(single, vec![c.ssh_keychain_key()]);

        let jump = SshConfig {
            host: "edge.example.com".into(),
            port: 22,
            username: "deploy".into(),
            auth: SshAuth::PublicKey,
            key_path: None,
            host_key_fingerprint: None,
            via: Vec::new(),
        };
        c.ssh.as_mut().expect("ssh").via = vec![jump];

        let chained = c.ssh_hop_keys();
        assert_eq!(chained.len(), 2);
        assert_eq!(
            chained[1],
            c.ssh_keychain_key(),
            "the bastion keeps the entry it already had"
        );
        assert_ne!(chained[0], chained[1], "each hop gets its own secret");
    }

    #[test]
    fn summary_reads_like_a_dsn() {
        assert_eq!(base().summary(), "readonly@db.example.com:5432/app");
    }

    #[test]
    fn file_backed_databases_summarize_as_their_path() {
        let mut c = base();
        c.driver = "sqlite".into();
        c.file_path = Some("/data/app.db".into());
        assert_eq!(c.summary(), "/data/app.db");
    }

    #[test]
    fn tls_defaults_are_explicit() {
        // Default must be deserializable from a config that predates the field.
        let c: ConnectionConfig =
            serde_json::from_str(r#"{"id":"x","name":"n","driver":"sqlite"}"#).expect("parse");
        assert_eq!(c.tls.mode, TlsMode::Prefer);
        assert!(!c.read_only);
    }
}
