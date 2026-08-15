//! Turning a connection URL into a configuration.
//!
//! The GUI builds a [`ConnectionConfig`] from a form and keeps the password in
//! the OS keychain. Neither is available to a CLI running in CI: there is no
//! form, and a headless runner has no keychain to unlock. A URL is what that
//! environment already has, in the shape every other database tool accepts.
//!
//! The password comes back separately rather than inside the config, for the
//! same reason it is not a field on `ConnectionConfig` at all: that struct is
//! serialized to disk and sent to the frontend, and a secret that can be in it
//! is a secret that will eventually be written somewhere.

use crate::config::{ConnectionConfig, TlsConfig, TlsMode};
use crate::error::{Error, Result};
use indexmap::IndexMap;

/// A parsed URL: what to connect to, and the password if one was given.
pub struct ParsedUrl {
    pub config: ConnectionConfig,
    /// Never placed in the config — see the module note.
    pub password: Option<String>,
}

/// Written by hand rather than derived, and redacted.
///
/// A derived `Debug` puts the password in every `{:?}` — a log line, a test
/// failure, an `unwrap` panic printed to a CI console that keeps its output for
/// a year. The whole point of keeping the secret out of `ConnectionConfig` is
/// undone by one formatter that prints it anyway.
impl std::fmt::Debug for ParsedUrl {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ParsedUrl")
            .field("config", &self.config)
            .field(
                "password",
                &self.password.as_ref().map(|_| "<redacted>"),
            )
            .finish()
    }
}

/// Map a URL scheme onto a driver id.
///
/// The aliases are the ones other tools accept, because someone pasting a URL
/// that works everywhere else and being told it is invalid learns nothing about
/// what to type instead.
fn driver_for(scheme: &str) -> Option<&'static str> {
    match scheme {
        "postgres" | "postgresql" | "pg" => Some("postgres"),
        "mysql" | "mariadb" => Some("mysql"),
        "sqlserver" | "mssql" => Some("mssql"),
        "clickhouse" | "ch" => Some("clickhouse"),
        "sqlite" | "file" => Some("sqlite"),
        _ => None,
    }
}

fn default_port(driver: &str) -> Option<u16> {
    match driver {
        "postgres" => Some(5432),
        "mysql" => Some(3306),
        "mssql" => Some(1433),
        "clickhouse" => Some(8123),
        _ => None,
    }
}

/// Parse a connection URL.
pub fn parse(input: &str) -> Result<ParsedUrl> {
    let url = url::Url::parse(input)
        .map_err(|e| Error::Config(format!("could not read the connection URL: {e}")))?;

    let driver = driver_for(url.scheme()).ok_or_else(|| {
        Error::Config(format!(
            "unknown scheme '{}'. Use postgres, mysql, mariadb, sqlserver, clickhouse, or sqlite.",
            url.scheme()
        ))
    })?;

    let mut options: IndexMap<String, String> = IndexMap::new();
    let mut tls = TlsConfig::default();

    for (key, value) in url.query_pairs() {
        match key.as_ref() {
            // Understood by every engine here and spelled the same way by
            // libpq, so it is translated rather than passed through.
            "sslmode" | "ssl-mode" => {
                tls.mode = match value.as_ref() {
                    "disable" | "disabled" => TlsMode::Disable,
                    "verify-full" | "verify_identity" | "verify-ca" => TlsMode::VerifyFull,
                    _ => TlsMode::Prefer,
                }
            }
            "sslrootcert" | "ssl-ca" => tls.ca_cert_path = Some(value.into_owned()),
            "sslcert" | "ssl-cert" => tls.client_cert_path = Some(value.into_owned()),
            "sslkey" | "ssl-key" => tls.client_key_path = Some(value.into_owned()),
            // Anything else is a driver's business, not this function's.
            _ => {
                options.insert(key.into_owned(), value.into_owned());
            }
        }
    }

    // A file-backed engine has a path where the others have a host.
    let (host, port, database, file_path) = if driver == "sqlite" {
        let path = file_path_from(input)?;
        (None, None, None, Some(path))
    } else {
        let host = url.host_str().unwrap_or("localhost").to_string();
        let database = url.path().trim_start_matches('/');
        (
            Some(host),
            Some(url.port().or_else(|| default_port(driver)).unwrap_or(0)),
            (!database.is_empty()).then(|| decode(database)),
            None,
        )
    };

    let username = match url.username() {
        "" => None,
        name => Some(decode(name)),
    };

    Ok(ParsedUrl {
        config: ConnectionConfig {
            // Deterministic from the URL rather than random: a CLI run is not a
            // saved connection, and two invocations against the same database
            // should not look like two different ones in a log.
            id: format!("cli:{driver}"),
            name: display_name(driver, &host, database.as_deref(), file_path.as_deref()),
            driver: driver.to_string(),
            host,
            port,
            database,
            username,
            file_path,
            tls,
            ssh: None,
            folder: None,
            color: None,
            read_only: false,
            options,
        },
        password: url.password().map(decode),
    })
}

/// Pull a filesystem path out of a `sqlite:` URL.
///
/// Deliberately not via the URL parser. A path is not a URL authority, and the
/// parser treats a Windows drive letter as one: `sqlite://C:/data/app.db` comes
/// back with host `C` and the colon silently gone, which then fails to open
/// with a message naming a path nobody typed. Taking the text after the scheme
/// and removing only the separators the URL syntax added is both simpler and
/// right on every platform.
///
/// The shapes people write, all of which have to reach the same place:
/// `sqlite:///abs/path`, `sqlite://./rel`, `sqlite:relative`,
/// `sqlite://C:/data/app.db`, and `sqlite:///C:/data/app.db`.
fn file_path_from(input: &str) -> Result<String> {
    let rest = input.split_once(':').map(|(_, r)| r).unwrap_or("");
    // The `//` is the URL's authority marker, not part of the path.
    let rest = rest.strip_prefix("//").unwrap_or(rest);

    // `/C:/data` is a Windows path written the URL way; that leading slash
    // belongs to the URL, not to the path.
    let trimmed = rest.trim_start_matches('/');
    let path = if is_windows_drive(trimmed) { trimmed } else { rest };

    let path = decode(path);
    if path.trim_matches('/').is_empty() {
        return Err(Error::Config(
            "a SQLite URL needs a file path, e.g. sqlite:///data/app.db".into(),
        ));
    }
    Ok(path)
}

/// Whether this starts `C:` — a drive letter, colon, then a separator or end.
fn is_windows_drive(path: &str) -> bool {
    let mut chars = path.chars();
    matches!(chars.next(), Some(c) if c.is_ascii_alphabetic())
        && chars.next() == Some(':')
        && matches!(chars.next(), None | Some('/') | Some('\\'))
}

/// Percent-decoding, because a password containing `@` or `/` has to be encoded
/// in a URL and is not that password once it arrives.
fn decode(value: &str) -> String {
    percent_encoding::percent_decode_str(value)
        .decode_utf8_lossy()
        .into_owned()
}

fn display_name(
    driver: &str,
    host: &Option<String>,
    database: Option<&str>,
    file_path: Option<&str>,
) -> String {
    if let Some(path) = file_path {
        return path.to_string();
    }
    match (host, database) {
        (Some(h), Some(d)) => format!("{d} on {h}"),
        (Some(h), None) => h.clone(),
        _ => driver.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_postgres_url_fills_in_every_field() {
        let p = parse("postgres://alice:s3cret@db.example.com:5433/app").expect("parse");
        assert_eq!(p.config.driver, "postgres");
        assert_eq!(p.config.host.as_deref(), Some("db.example.com"));
        assert_eq!(p.config.port, Some(5433));
        assert_eq!(p.config.database.as_deref(), Some("app"));
        assert_eq!(p.config.username.as_deref(), Some("alice"));
        assert_eq!(p.password.as_deref(), Some("s3cret"));
    }

    #[test]
    fn the_password_never_lands_in_the_config() {
        // The config is serialized to disk and sent to the frontend. A secret
        // that can be in it is a secret that will eventually be written down.
        let p = parse("postgres://alice:s3cret@host/app").expect("parse");
        let json = serde_json::to_string(&p.config).expect("serialize");
        assert!(!json.contains("s3cret"), "{json}");
    }

    #[test]
    fn a_password_with_url_characters_survives_decoding() {
        // Passwords containing @ : / # have to be percent-encoded to be a valid
        // URL, and are not that password once they arrive undecoded.
        let p = parse("mysql://root:p%40ss%3Aword%2F1@localhost/app").expect("parse");
        assert_eq!(p.password.as_deref(), Some("p@ss:word/1"));
    }

    #[test]
    fn each_engine_gets_its_own_default_port() {
        assert_eq!(parse("postgres://h/db").unwrap().config.port, Some(5432));
        assert_eq!(parse("mysql://h/db").unwrap().config.port, Some(3306));
        assert_eq!(parse("sqlserver://h/db").unwrap().config.port, Some(1433));
        assert_eq!(parse("clickhouse://h/db").unwrap().config.port, Some(8123));
    }

    #[test]
    fn the_aliases_other_tools_accept_work_here_too() {
        // Someone pasting a URL that works everywhere else and being told it is
        // invalid learns nothing about what to type instead.
        for url in ["postgresql://h/db", "pg://h/db"] {
            assert_eq!(parse(url).unwrap().config.driver, "postgres", "{url}");
        }
        assert_eq!(parse("mariadb://h/db").unwrap().config.driver, "mysql");
        assert_eq!(parse("mssql://h/db").unwrap().config.driver, "mssql");
    }

    #[test]
    fn sqlite_paths_arrive_whole_in_all_three_spellings() {
        assert_eq!(
            parse("sqlite:///data/app.db").unwrap().config.file_path.as_deref(),
            Some("/data/app.db")
        );
        assert_eq!(
            parse("sqlite://./local.db").unwrap().config.file_path.as_deref(),
            Some("./local.db")
        );
        assert_eq!(
            parse("sqlite:relative.db").unwrap().config.file_path.as_deref(),
            Some("relative.db")
        );
    }

    #[test]
    fn a_windows_path_keeps_its_drive_letter() {
        // The URL parser reads `C:` as a port separator and drops the colon,
        // which then fails to open with a message naming a path nobody typed.
        for url in [
            "sqlite://C:/data/app.db",
            "sqlite:///C:/data/app.db",
        ] {
            assert_eq!(
                parse(url).unwrap().config.file_path.as_deref(),
                Some("C:/data/app.db"),
                "{url}"
            );
        }
    }

    #[test]
    fn a_unix_absolute_path_keeps_its_leading_slash() {
        // The same trimming that rescues a drive letter must not eat this one.
        assert_eq!(
            parse("sqlite:////var/lib/app.db").unwrap().config.file_path.as_deref(),
            Some("//var/lib/app.db")
        );
        assert_eq!(
            parse("sqlite:///var/lib/app.db").unwrap().config.file_path.as_deref(),
            Some("/var/lib/app.db")
        );
    }

    #[test]
    fn a_path_with_spaces_survives_encoded_or_not() {
        assert_eq!(
            parse("sqlite:///data/my%20app.db").unwrap().config.file_path.as_deref(),
            Some("/data/my app.db")
        );
    }

    #[test]
    fn a_sqlite_url_with_no_path_is_an_error_rather_than_an_empty_file() {
        // Otherwise it opens or creates something unnamed in the working
        // directory, which is a surprising thing for a typo to do.
        assert!(parse("sqlite://").is_err());
    }

    #[test]
    fn sslmode_becomes_a_tls_setting_rather_than_a_driver_option() {
        assert_eq!(parse("postgres://h/db?sslmode=disable").unwrap().config.tls.mode, TlsMode::Disable);
        assert_eq!(
            parse("postgres://h/db?sslmode=verify-full").unwrap().config.tls.mode,
            TlsMode::VerifyFull
        );
        // And is not left in options for a driver to read a second time.
        let p = parse("postgres://h/db?sslmode=disable").unwrap();
        assert!(!p.config.options.contains_key("sslmode"));
    }

    #[test]
    fn unrecognized_query_parameters_are_passed_through_untouched() {
        // They are a driver's business. Dropping them silently would be the
        // wrong kind of opinionated.
        let p = parse("clickhouse://h/db?max_execution_time=30").unwrap();
        assert_eq!(p.config.options.get("max_execution_time").map(String::as_str), Some("30"));
    }

    #[test]
    fn debugging_a_parsed_url_does_not_print_the_password() {
        // A derived Debug would put it in every log line, test failure, and
        // panic message — including ones a CI console keeps for a year.
        let p = parse("postgres://alice:s3cret@host/app").expect("parse");
        let rendered = format!("{p:?}");
        assert!(!rendered.contains("s3cret"), "{rendered}");
        assert!(rendered.contains("redacted"), "{rendered}");
    }

    #[test]
    fn an_unknown_scheme_says_what_is_accepted() {
        let err = parse("oracle://h/db").expect_err("unknown scheme");
        let message = err.to_string();
        assert!(message.contains("postgres"), "{message}");
        assert!(message.contains("sqlite"), "{message}");
    }

    #[test]
    fn a_url_with_no_credentials_is_fine() {
        // Trust authentication, a socket, an unauthenticated local ClickHouse —
        // all normal, none of them an error.
        let p = parse("postgres://localhost/app").unwrap();
        assert_eq!(p.config.username, None);
        assert_eq!(p.password, None);
    }

    #[test]
    fn the_id_is_stable_across_runs() {
        // Two invocations against the same database should not look like two
        // different connections in a log.
        assert_eq!(
            parse("postgres://h/db").unwrap().config.id,
            parse("postgres://other/db2").unwrap().config.id
        );
    }
}
