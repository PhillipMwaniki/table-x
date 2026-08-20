//! Asking whether a newer release exists.
//!
//! This is the only network call the app makes that is not to a database the
//! user configured, so it is deliberately small: a GET with no parameters, no
//! cookies and no identifier of any kind. There is nothing to correlate at the
//! other end, which is what lets the security model claim as much.
//!
//! # Why the endpoint does not supply the download link
//!
//! It returns a version string; the URL is built here, from [`REPO`], which is
//! compiled in. The endpoint is therefore not in the trust path for anything
//! anybody installs — compromise it and the worst it can do is show a wrong
//! version number. Letting it hand over a URL would let it point every user at
//! a binary of its choosing, which matters a great deal while the installers
//! are unsigned and users are already being asked to click past SmartScreen.
//!
//! # Why a failure says nothing
//!
//! Every error path returns `None`, which the UI renders as no badge at all.
//! What it must never do is report "you are up to date", because that is a
//! claim this code cannot make when it did not hear back — and it is how
//! somebody misses a security fix.

use serde::{Deserialize, Serialize};

/// Where the releases live. The link the user is offered is built from this and
/// nothing else.
const REPO: &str = "PhillipMwaniki/table-x";

/// The update endpoint, or empty to disable checking entirely.
///
/// Empty is a working state rather than a broken one: [`check_for_update`]
/// returns immediately without a request, so a fork that wants no update check
/// deletes this string rather than the feature.
const ENDPOINT: &str = "https://table-x-updates.mwanikiphillip.workers.dev/";

/// How long to wait for an answer nobody is waiting on.
///
/// Short on purpose: this runs at startup, and an update check is the least
/// important thing happening at that moment.
const TIMEOUT_SECS: u64 = 5;

/// What the endpoint returns. Unknown fields are ignored so the server can gain
/// them without stranding installed clients.
#[derive(Debug, Deserialize)]
struct Manifest {
    version: String,
    #[serde(default)]
    notice: Option<Notice>,
    #[serde(default)]
    check_again_in: Option<u64>,
}

/// A message from the release channel, shown beside the version.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Notice {
    /// `info` or `critical`. Anything else is treated as `info` by the UI.
    pub severity: String,
    pub text: String,
}

/// A newer release, and how to go and get it.
#[derive(Debug, Clone, Serialize)]
pub struct UpdateInfo {
    /// The version this build is, for the UI to show beside the new one.
    pub current: String,
    pub latest: String,
    /// Built here from [`REPO`], never from the response.
    pub url: String,
    pub notice: Option<Notice>,
    /// Seconds the caller should wait before asking again.
    pub check_again_in: u64,
}

/// Whether `latest` is a release worth telling somebody about.
///
/// Compared field by field rather than as strings, because string ordering puts
/// 0.10.0 before 0.9.0 — a comparison that is correct for nine releases and then
/// silently wrong forever. Anything unparseable is treated as "no news": a
/// malformed version from the server should be ignored, not shown.
fn is_newer(latest: &str, current: &str) -> bool {
    match (
        semver::Version::parse(latest),
        semver::Version::parse(current),
    ) {
        (Ok(latest), Ok(current)) => latest > current,
        _ => false,
    }
}

/// Install the rustls provider, once per process.
///
/// `rustls-no-provider` deliberately leaves this to the caller so that a process
/// linking several TLS users cannot end up with two competing providers. The
/// drivers do the same with the same choice of ring; whichever runs first wins
/// and the other call is a no-op. Missing it does not fail to compile — it
/// panics on the first request, which is why there is a test that makes one.
fn ensure_crypto_provider() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        let _ = rustls::crypto::ring::default_provider().install_default();
    });
}

/// Fetch and parse the manifest. `None` for every failure, including a refusal.
///
/// Split from [`check_for_update`] so the wire path can be exercised against the
/// real endpoint without depending on a newer release existing — which it will
/// not, on the machine cutting that release.
async fn fetch_manifest(endpoint: &str) -> Option<Manifest> {
    if endpoint.is_empty() {
        return None;
    }

    ensure_crypto_provider();

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(TIMEOUT_SECS))
        .build()
        .ok()?;

    let response = client.get(endpoint).send().await.ok()?;
    if !response.status().is_success() {
        return None;
    }

    response.json().await.ok()
}

/// Ask whether a newer release exists. `None` means no news, for any reason.
#[tauri::command(rename_all = "snake_case")]
pub async fn check_for_update() -> Option<UpdateInfo> {
    let manifest = fetch_manifest(ENDPOINT).await?;
    let current = env!("CARGO_PKG_VERSION");
    if !is_newer(&manifest.version, current) {
        return None;
    }

    Some(UpdateInfo {
        current: current.to_string(),
        latest: manifest.version.clone(),
        url: format!(
            "https://github.com/{REPO}/releases/tag/v{}",
            manifest.version
        ),
        notice: manifest.notice,
        check_again_in: manifest.check_again_in.unwrap_or(86_400),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_higher_version_is_news() {
        assert!(is_newer("0.5.0", "0.4.0"));
        assert!(is_newer("1.0.0", "0.9.9"));
    }

    #[test]
    fn the_same_version_and_older_ones_are_not() {
        assert!(!is_newer("0.4.0", "0.4.0"));
        assert!(!is_newer("0.3.0", "0.4.0"));
    }

    #[test]
    fn ten_comes_after_nine() {
        // The reason this is a semver comparison and not a string one. As
        // strings, "0.10.0" sorts before "0.9.0", so a naive check would go
        // quiet exactly when the tenth release shipped and stay quiet.
        assert!(is_newer("0.10.0", "0.9.0"));
        assert!(!is_newer("0.9.0", "0.10.0"));
    }

    #[test]
    fn a_version_that_does_not_parse_is_no_news() {
        // Silence is the safe direction: a malformed version from the server
        // should be ignored rather than shown to somebody as a release.
        assert!(!is_newer("banana", "0.4.0"));
        assert!(!is_newer("", "0.4.0"));
        assert!(
            !is_newer("v0.5.0", "0.4.0"),
            "the leading v is the server's to strip"
        );
    }

    /// Exercises the real endpoint, and is skipped unless `TABLEX_TEST_UPDATES`
    /// is set — the same bargain the driver suites make about needing a server.
    ///
    /// It asserts the manifest arrives and parses, not that an update exists:
    /// the machine running this is usually the one that just cut the release, so
    /// "no newer version" is the expected answer and would make an assertion on
    /// `check_for_update` pass for the wrong reason.
    #[tokio::test]
    async fn the_live_endpoint_answers_in_the_shape_this_expects() {
        if std::env::var("TABLEX_TEST_UPDATES").is_err() {
            eprintln!("skipping: TABLEX_TEST_UPDATES not set");
            return;
        }

        let manifest = fetch_manifest(ENDPOINT)
            .await
            .expect("the endpoint should answer");

        assert!(
            semver::Version::parse(&manifest.version).is_ok(),
            "the endpoint must report a version this can compare: {:?}",
            manifest.version
        );
        assert!(
            manifest.check_again_in.unwrap_or(0) > 0,
            "a zero interval would check on every launch"
        );
    }

    #[tokio::test]
    async fn an_empty_endpoint_makes_no_request() {
        // The switch a fork flips to opt out entirely. It must not merely fail a
        // request; it must not make one.
        assert!(fetch_manifest("").await.is_none());
    }

    #[test]
    fn this_build_reports_a_version_semver_can_read() {
        // Guards the other half of every comparison above: if the crate version
        // ever stopped parsing, every check would silently return no news.
        assert!(semver::Version::parse(env!("CARGO_PKG_VERSION")).is_ok());
    }
}
