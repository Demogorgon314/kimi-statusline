//! Plan quota (5h / 7d / monthly limits) from the managed Kimi Code API —
//! the same `GET <base>/usages` the TUI's `/usage` command makes, with the
//! OAuth access token Kimi Code keeps in `credentials/<key>.json`.
//!
//! A network round trip doesn't fit the 300ms budget, so the status line
//! only ever reads a cache file; when that is stale it spawns
//! `kimi-statusline fetch-quota` detached, and a later refresh picks the
//! answer up. Tokens are never refreshed here: Kimi Code rotates the refresh
//! token under a cross-process lock, and racing it could log the user out.
//! An expired token just keeps the last known numbers on screen.

use crate::paths;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::path::PathBuf;

const DEFAULT_BASE_URL: &str = "https://api.kimi.com/coding/v1";
const GLOBAL_BASE_URL: &str = "https://api.kimi.ai/coding/v1";
const DEFAULT_OAUTH_HOST: &str = "https://auth.kimi.com";
const DEFAULT_KEY: &str = "oauth/kimi-code";
/// Don't spawn another fetch while one may still be running.
const FETCH_LOCK_S: f64 = 20.0;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Entry {
    pub used_ratio: f64,
    pub reset_at: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct Quota {
    pub limit_5h: Option<Entry>,
    pub limit_7d: Option<Entry>,
    pub month: Option<Entry>,
    /// unix seconds the numbers were fetched at (0 when unknown); filled in
    /// from the cache, so it is not part of the stored value
    #[serde(skip)]
    pub fetched_at: f64,
}

#[derive(Default, Serialize, Deserialize)]
struct Cache {
    /// last fetch attempt
    t: f64,
    /// when `v` was fetched
    fetched_at: f64,
    v: Option<Quota>,
    error: Option<String>,
}

fn cache_path() -> PathBuf {
    paths::cache_dir().join("quota.json")
}

fn lock_path() -> PathBuf {
    paths::cache_dir().join("quota.lock")
}

fn read_cache() -> Cache {
    std::fs::read(cache_path())
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_default()
}

/// After a failed fetch, retry this soon rather than waiting a full `ttl`.
const RETRY_AFTER_ERROR_S: f64 = 15.0;

fn mtime_secs(path: &std::path::Path) -> Option<f64> {
    let m = std::fs::metadata(path).ok()?.modified().ok()?;
    Some(m.duration_since(std::time::UNIX_EPOCH).ok()?.as_secs_f64())
}

/// Cached quota; kicks off a background refresh when it is due:
/// - the cache is older than `ttl`, or
/// - the last attempt failed and either `RETRY_AFTER_ERROR_S` passed or
///   Kimi Code has since rewritten the credentials (a refreshed token
///   usually fixes an "expired" failure immediately).
pub fn get(ttl: f64) -> Option<Quota> {
    let cache = read_cache();
    let now = paths::now_secs();
    let failed = cache.error.is_some() || cache.v.is_none();
    let creds_changed = failed && mtime_secs(&login().credential).is_some_and(|m| m > cache.t);
    let due =
        now - cache.t >= ttl || (failed && (creds_changed || now - cache.t >= RETRY_AFTER_ERROR_S));
    if due {
        let lock_age = std::fs::read_to_string(lock_path())
            .ok()
            .and_then(|s| s.trim().parse::<f64>().ok())
            .map_or(f64::MAX, |t| now - t);
        if lock_age > FETCH_LOCK_S {
            let _ = paths::write_atomic(&lock_path(), now.to_string().as_bytes());
            paths::spawn_self(&["fetch-quota"]);
        }
    }
    let fetched_at = cache.fetched_at;
    cache.v.map(|q| Quota { fetched_at, ..q })
}

/// `fetch-quota`: one request, result into the cache. Errors are recorded
/// next to the last good value instead of replacing it.
pub fn fetch_and_store() -> Result<Quota, String> {
    let result = fetch();
    let mut cache = read_cache();
    cache.t = paths::now_secs();
    match &result {
        Ok(q) => {
            cache.v = Some(q.clone());
            cache.fetched_at = cache.t;
            cache.error = None;
        }
        Err(e) => cache.error = Some(e.clone()),
    }
    if let Ok(data) = serde_json::to_vec(&cache) {
        let _ = paths::write_atomic(&cache_path(), &data);
    }
    let _ = std::fs::remove_file(lock_path());
    paths::debug(&format!("fetch-quota: {:?}", result.as_ref().map(|_| "ok")));
    result.map(|q| Quota {
        fetched_at: cache.fetched_at,
        ..q
    })
}

/// Last fetch error, for `kimi-statusline quota` diagnostics.
pub fn last_error() -> Option<String> {
    read_cache().error
}

struct Login {
    base_url: String,
    credential: PathBuf,
}

/// Where to ask and with which credential slot, following Kimi Code's
/// resolveKimiCodeRuntimeAuth: the base URL is `KIMI_CODE_BASE_URL`, else
/// the managed provider in config.toml, else the region marker (`global` →
/// api.kimi.ai), else mainland. The slot is the one config.toml names unless
/// an env override is set or it disagrees with the slot those endpoints
/// imply (any non-default endpoint gets its own `kimi-code-env-<hash>`).
fn login() -> Login {
    let home = paths::kimi_home();
    let env = |k: &str| std::env::var(k).ok();
    let region_global =
        std::fs::read_to_string(home.join("region")).is_ok_and(|r| r.trim() == "global");
    let provider = std::fs::read_to_string(home.join("config.toml"))
        .ok()
        .and_then(|t| t.parse::<toml::Table>().ok())
        .and_then(|doc| {
            doc.get("providers")?
                .get("managed:kimi-code")?
                .as_table()
                .cloned()
        });
    let cfg_str = |path: &[&str]| -> Option<String> {
        let mut v = provider.as_ref()?.get(path[0])?;
        for k in &path[1..] {
            v = v.get(k)?;
        }
        v.as_str().map(str::to_string)
    };
    let env_base = env("KIMI_CODE_BASE_URL");
    let env_host = env("KIMI_CODE_OAUTH_HOST").or_else(|| env("KIMI_OAUTH_HOST"));
    let has_env = env_base.is_some() || env_host.is_some();
    let base_url = env_base
        .clone()
        .or_else(|| cfg_str(&["base_url"]))
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| {
            if region_global {
                GLOBAL_BASE_URL
            } else {
                DEFAULT_BASE_URL
            }
            .into()
        });
    let host = if has_env {
        env_host
    } else {
        cfg_str(&["oauth", "oauth_host"])
    };
    // upstream resolves the slot from the raw configured base URL, without
    // the region fallback
    let key_base = env_base.or_else(|| cfg_str(&["base_url"]));
    let expected = oauth_key(host.as_deref(), key_base.as_deref());
    let key = match cfg_str(&["oauth", "key"]) {
        Some(k) if !has_env && k == expected => k,
        _ => expected,
    };
    let name = key.strip_prefix("oauth/").unwrap_or(&key).to_string();
    Login {
        base_url: base_url.trim_end_matches('/').to_string(),
        credential: home.join("credentials").join(format!("{name}.json")),
    }
}

/// Upstream resolveKimiCodeOAuthKey: the default host and endpoint share
/// `oauth/kimi-code`; any other pair gets a slot named after its hash.
fn oauth_key(host: Option<&str>, base_url: Option<&str>) -> String {
    let norm = |s: &str| s.trim().trim_end_matches('/').to_string();
    let host = norm(host.unwrap_or(DEFAULT_OAUTH_HOST));
    let base = norm(base_url.unwrap_or(DEFAULT_BASE_URL));
    if host == DEFAULT_OAUTH_HOST && base == DEFAULT_BASE_URL {
        return DEFAULT_KEY.into();
    }
    // key order matters for the hash: JSON.stringify keeps insertion order,
    // serde_json's map would sort it
    let q = |s: &str| serde_json::Value::from(s).to_string();
    let json = format!(r#"{{"oauthHost":{},"baseUrl":{}}}"#, q(&host), q(&base));
    let digest: String = Sha256::digest(json.as_bytes())
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    format!("oauth/kimi-code-env-{}", &digest[..16])
}

fn access_token(path: &PathBuf) -> Result<String, String> {
    let v: serde_json::Value = serde_json::from_slice(
        &std::fs::read(path).map_err(|_| "not logged in to Kimi Code (/login)".to_string())?,
    )
    .map_err(|_| "unreadable credentials".to_string())?;
    let token = v
        .get("access_token")
        .and_then(|t| t.as_str())
        .filter(|t| !t.is_empty())
        .ok_or("no access token")?;
    if let Some(exp) = v.get("expires_at").and_then(|e| e.as_f64()) {
        if exp <= paths::now_secs() {
            return Err("access token expired; Kimi Code refreshes it on its next request".into());
        }
    }
    Ok(token.to_string())
}

fn fetch() -> Result<Quota, String> {
    let login = login();
    let token = access_token(&login.credential)?;
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(Some(std::time::Duration::from_secs(8)))
        .http_status_as_error(false)
        .build()
        .into();
    let mut res = agent
        .get(format!("{}/usages", login.base_url))
        .header("Authorization", format!("Bearer {token}"))
        .header("Accept", "application/json")
        .call()
        .map_err(|e| format!("request failed: {e}"))?;
    let status = res.status().as_u16();
    if status != 200 {
        return Err(match status {
            401 => "authorization failed (try /login)".into(),
            404 => "usage endpoint not available for this login".into(),
            s => format!("HTTP {s}"),
        });
    }
    let body: serde_json::Value = res
        .body_mut()
        .read_json()
        .map_err(|e| format!("bad response: {e}"))?;
    Ok(parse(&body))
}

/// Mirrors upstream parseManagedUsagePayload: `usages.limit_5h` etc., each
/// `{ used_ratio, reset_time }`, numbers possibly sent as strings.
pub fn parse(body: &serde_json::Value) -> Quota {
    let usages = body.get("usages");
    let entry = |key: &str| -> Option<Entry> {
        let raw = usages?.get(key)?;
        let ratio = raw.get("used_ratio")?;
        let used_ratio = ratio
            .as_f64()
            .or_else(|| ratio.as_str()?.trim().parse().ok())?;
        let reset_at = raw
            .get("reset_time")
            .and_then(|r| r.as_str())
            .filter(|r| !r.is_empty())
            .map(str::to_string);
        Some(Entry {
            used_ratio,
            reset_at,
        })
    };
    Quota {
        limit_5h: entry("limit_5h"),
        limit_7d: entry("limit_7d"),
        month: entry("limit_month_total"),
        fetched_at: 0.0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_upstream_payload() {
        let body = serde_json::json!({
            "usages": {
                "limit_5h": { "used_ratio": 0.5, "reset_time": "2026-09-11T18:00:00Z" },
                "limit_7d": { "used_ratio": "0.1", "reset_time": "" },
            }
        });
        let q = parse(&body);
        assert_eq!(q.limit_5h.as_ref().unwrap().used_ratio, 0.5);
        assert_eq!(
            q.limit_5h.unwrap().reset_at.as_deref(),
            Some("2026-09-11T18:00:00Z")
        );
        assert_eq!(q.limit_7d.as_ref().unwrap().used_ratio, 0.1);
        assert!(q.limit_7d.unwrap().reset_at.is_none());
        assert!(q.month.is_none());
    }

    #[test]
    fn oauth_slot_matches_upstream() {
        assert_eq!(oauth_key(None, None), "oauth/kimi-code");
        assert_eq!(
            oauth_key(Some("https://auth.kimi.com/"), Some(DEFAULT_BASE_URL)),
            "oauth/kimi-code"
        );
        // node: sha256(JSON.stringify({oauthHost, baseUrl})).slice(0, 16)
        assert_eq!(
            oauth_key(None, Some("https://example.test/coding/v1/")),
            "oauth/kimi-code-env-2cad1c19794545b8"
        );
    }
}
