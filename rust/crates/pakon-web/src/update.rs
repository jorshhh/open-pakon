//! New-build check: is there a GitHub release newer than this app? (port of
//! `web/update.py`)
//!
//! The app's version comes from PAKON_VERSION (the window app sets it); a
//! checkout has none and never checks. The latest release is asked from the
//! GitHub API once at startup and then every few hours, in the background;
//! any failure (offline, rate limit) just means no banner.
//! PAKON_UPDATE_CHECK=0 turns the check off.

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde_json::{json, Value};

use crate::paths::env;

pub const REPO: &str = "jorshhh/open-pakon";
pub const RELEASES: &str = "https://github.com/jorshhh/open-pakon/releases";
const API: &str = "https://api.github.com/repos/jorshhh/open-pakon/releases/latest";
const EVERY: Duration = Duration::from_secs(6 * 3600);
const TIMEOUT: Duration = Duration::from_secs(8);

#[derive(Default)]
struct State {
    latest: Option<String>,
    url: Option<String>,
    checked: Option<Instant>,
}

#[derive(Default)]
pub struct Update {
    state: Mutex<State>,
}

pub fn current() -> Option<String> {
    env("PAKON_VERSION").map(|v| v.trim().to_string())
}

pub fn enabled() -> bool {
    current().is_some() && std::env::var("PAKON_UPDATE_CHECK").as_deref() != Ok("0")
}

/// `v0.3.0` / `0.3` -> (0, 3, 0); None when it is not a plain version.
fn parse(v: &str) -> Option<(u64, u64, u64)> {
    let v = v.trim();
    let v = v.strip_prefix('v').unwrap_or(v);
    let parts: Vec<&str> = v.split('.').collect();
    if parts.is_empty() || parts.len() > 3 || parts.iter().any(|p| p.is_empty() || !p.bytes().all(|b| b.is_ascii_digit())) {
        return None;
    }
    let n = |i: usize| parts.get(i).map_or(Some(0), |p| p.parse().ok());
    Some((n(0)?, n(1)?, n(2)?))
}

pub fn newer(latest: &str, installed: &str) -> bool {
    matches!((parse(latest), parse(installed)), (Some(a), Some(b)) if a > b)
}

/// What a release from the API means: (version, page), or None for a draft,
/// a prerelease or a tag that is not a plain version. Only this repo's
/// release pages are ever opened.
fn release(rel: &Value) -> Option<(String, String)> {
    let tag = rel["tag_name"].as_str().unwrap_or("");
    let url = rel["html_url"].as_str().unwrap_or("");
    if rel["draft"].as_bool().unwrap_or(false) || rel["prerelease"].as_bool().unwrap_or(false) || parse(tag).is_none() {
        return None;
    }
    let url = if url.starts_with(&format!("{RELEASES}/")) { url.to_string() } else { format!("{RELEASES}/latest") };
    Some((tag.trim_start_matches('v').to_string(), url))
}

impl Update {
    fn state(&self) -> std::sync::MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn stale(&self) -> bool {
        self.state().checked.is_none_or(|t| t.elapsed() >= EVERY)
    }

    /// Ask GitHub for the latest release (blocking; run it in a thread).
    pub fn check(&self) {
        let Some(version) = current().filter(|_| enabled()) else { return };
        {
            let mut s = self.state();
            if s.checked.is_some_and(|t| t.elapsed() < EVERY) {
                return;
            }
            s.checked = Some(Instant::now());
        }
        let agent = ureq::AgentBuilder::new().timeout(TIMEOUT).build();
        let rel: Value = match agent.get(API).set("Accept", "application/vnd.github+json").set("User-Agent", &format!("Pakon/{version}")).call() {
            Ok(r) => match r.into_string().ok().and_then(|t| serde_json::from_str(&t).ok()) {
                Some(v) => v,
                None => return,
            },
            Err(e) => {
                // offline, rate limited, ...: no banner
                println!("update check failed: {e}");
                return;
            }
        };
        if let Some((latest, url)) = release(&rel) {
            let mut s = self.state();
            s.latest = Some(latest);
            s.url = Some(url);
        }
    }

    /// `{"current", "latest", "url"}` when a newer release is out, else
    /// None. Starts a background check when the last one is old.
    pub fn status(self: &Arc<Self>) -> Option<Value> {
        let version = current().filter(|_| enabled())?;
        if self.stale() {
            let me = self.clone();
            std::thread::spawn(move || me.check());
        }
        let s = self.state();
        let (latest, url) = (s.latest.clone()?, s.url.clone());
        newer(&latest, &version).then(|| json!({"current": version, "latest": latest, "url": url}))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions() {
        assert_eq!(parse("v0.3.0"), Some((0, 3, 0)));
        assert_eq!(parse("0.3"), Some((0, 3, 0)));
        assert_eq!(parse(" 2 "), Some((2, 0, 0)));
        assert_eq!(parse("0.3.0-rc1"), None);
        assert_eq!(parse("1.2.3.4"), None);
        assert_eq!(parse("v"), None);
        assert!(newer("0.10.0", "0.9.5"));
        assert!(!newer("0.3.0", "0.3"));
        assert!(!newer("nightly", "0.3"));
    }

    #[test]
    fn releases() {
        let ok = json!({"tag_name": "v0.4.1", "html_url": format!("{RELEASES}/tag/v0.4.1")});
        assert_eq!(release(&ok), Some(("0.4.1".into(), format!("{RELEASES}/tag/v0.4.1"))));
        let elsewhere = json!({"tag_name": "v0.4.1", "html_url": "https://example.com/x"});
        assert_eq!(release(&elsewhere).unwrap().1, format!("{RELEASES}/latest"));
        assert_eq!(release(&json!({"tag_name": "v0.4.1", "prerelease": true})), None);
        assert_eq!(release(&json!({"tag_name": "latest"})), None);
    }
}
