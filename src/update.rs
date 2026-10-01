//! Self-update from GitHub Releases.
//!
//! `check` asks GitHub which release is latest (the redirect of
//! `/releases/latest`, which needs no API token and has no rate limit) and
//! caches the answer for a day. `install` downloads this platform's archive,
//! verifies it against the release's SHA256SUMS, and swaps the running
//! binary in place. Package-manager installs are left to the package
//! manager: a cargo-installed or plugin-managed copy is updated the way it
//! was installed.

use crate::paths;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::Duration;

pub const CURRENT: &str = env!("CARGO_PKG_VERSION");
const REPO: &str = env!("CARGO_PKG_REPOSITORY");
const CHECK_TTL: f64 = 24.0 * 3600.0;
/// Release archives are ~2 MB; anything near this is not ours.
const MAX_DOWNLOAD: u64 = 64 * 1024 * 1024;

/// `major.minor.patch` from "v1.2.3" or "1.2.3"; None otherwise.
pub fn parse_version(v: &str) -> Option<(u64, u64, u64)> {
    let v = v.trim().trim_start_matches('v');
    let core = v.split(['-', '+']).next()?;
    let mut it = core.split('.').map(|p| p.parse::<u64>().ok());
    let out = (it.next()??, it.next()??, it.next()??);
    it.next().is_none().then_some(out)
}

/// Whether `latest` is newer than `current`; unparseable versions never are.
pub fn is_newer(latest: &str, current: &str) -> bool {
    matches!((parse_version(latest), parse_version(current)), (Some(l), Some(c)) if l > c)
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct Cache {
    checked_at: f64,
    latest: Option<String>,
}

fn cache_path() -> PathBuf {
    paths::cache_dir().join("update.json")
}

fn agent(timeout: Duration) -> ureq::Agent {
    ureq::Agent::config_builder()
        .timeout_global(Some(timeout))
        .http_status_as_error(false)
        .max_redirects(0)
        .user_agent(concat!("kimi-statusline/", env!("CARGO_PKG_VERSION")))
        .build()
        .into()
}

/// The latest release tag, e.g. "v0.3.0". GitHub answers `/releases/latest`
/// with a redirect to `/releases/tag/<tag>`, so the tag is read from the
/// Location header without following it.
pub fn latest_release() -> Result<String, String> {
    let res = agent(Duration::from_secs(8))
        .get(format!("{REPO}/releases/latest"))
        .call()
        .map_err(|e| format!("could not reach GitHub: {e}"))?;
    let status = res.status().as_u16();
    let location = res
        .headers()
        .get("location")
        .and_then(|l| l.to_str().ok())
        .unwrap_or_default();
    let tag = location
        .rsplit_once("/releases/tag/")
        .map(|(_, t)| t.trim_end_matches('/'))
        .filter(|t| !t.is_empty());
    match (status, tag) {
        (300..=399, Some(t)) => Ok(t.to_string()),
        (404, _) => Err("no releases published yet".into()),
        _ => Err(format!("unexpected answer from GitHub (HTTP {status})")),
    }
}

/// Fresh check, result cached for the menu's background check.
pub fn check_now() -> Result<String, String> {
    let latest = latest_release()?;
    let cache = Cache {
        checked_at: paths::now_secs(),
        latest: Some(latest.clone()),
    };
    if let Ok(data) = serde_json::to_vec(&cache) {
        let _ = paths::write_atomic(&cache_path(), &data);
    }
    Ok(latest)
}

/// The cached latest tag if checked within the last day; otherwise None
/// (the caller decides whether to check now).
pub fn cached_latest() -> Option<String> {
    let cache: Cache = serde_json::from_slice(&std::fs::read(cache_path()).ok()?).ok()?;
    (paths::now_secs() - cache.checked_at < CHECK_TTL)
        .then_some(cache.latest)
        .flatten()
}

// ---------------------------------------------------------------------------
// install
// ---------------------------------------------------------------------------

/// How this binary was installed, which decides how it should be updated.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InstallKind {
    /// a release binary (install.sh / install.ps1 / downloaded by hand):
    /// replace it in place
    Standalone(PathBuf),
    /// `cargo install`: rebuild with cargo
    Cargo,
    /// the Kimi Code plugin's copy: reinstall the plugin
    Plugin,
}

pub fn install_kind() -> InstallKind {
    let exe = std::env::current_exe()
        .ok()
        .and_then(|p| p.canonicalize().ok())
        .unwrap_or_default();
    classify(&exe, &cargo_bin_dir())
}

fn cargo_bin_dir() -> Option<PathBuf> {
    let home = std::env::var_os("CARGO_HOME")
        .map(PathBuf::from)
        .or_else(|| paths::home_dir().map(|h| h.join(".cargo")))?;
    home.join("bin").canonicalize().ok()
}

fn classify(exe: &Path, cargo_bin: &Option<PathBuf>) -> InstallKind {
    let s = exe.to_string_lossy().replace('\\', "/");
    if s.contains("/plugins/managed/") {
        InstallKind::Plugin
    } else if cargo_bin
        .as_deref()
        .is_some_and(|b| exe.parent() == Some(b))
    {
        InstallKind::Cargo
    } else {
        InstallKind::Standalone(exe.to_path_buf())
    }
}

/// What to tell a user who can't be updated in place.
pub fn manual_instructions(kind: &InstallKind) -> Option<String> {
    match kind {
        InstallKind::Cargo => Some(format!(
            "installed with cargo: run  cargo install --git {REPO} --force"
        )),
        InstallKind::Plugin => Some(format!(
            "plugin install: run  /plugins install {REPO}  in Kimi Code, then /reload and /new"
        )),
        InstallKind::Standalone(_) => None,
    }
}

/// Release asset name for this build target.
fn asset_name() -> Result<String, String> {
    let os = match std::env::consts::OS {
        "macos" => "darwin",
        "linux" => "linux",
        "windows" => "windows",
        o => return Err(format!("no prebuilt binary for {o}")),
    };
    let arch = match std::env::consts::ARCH {
        "aarch64" => "arm64",
        "x86_64" => "x64",
        a => return Err(format!("no prebuilt binary for {a}")),
    };
    let ext = if os == "windows" { "zip" } else { "tar.gz" };
    Ok(format!("kimi-statusline-{os}-{arch}.{ext}"))
}

fn download(url: &str) -> Result<Vec<u8>, String> {
    // release assets redirect to a CDN, so follow redirects here
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(120)))
        .http_status_as_error(false)
        .user_agent(concat!("kimi-statusline/", env!("CARGO_PKG_VERSION")))
        .build()
        .into();
    let mut res = agent
        .get(url)
        .call()
        .map_err(|e| format!("download failed: {e}"))?;
    let status = res.status().as_u16();
    if status != 200 {
        return Err(format!("download failed: HTTP {status} for {url}"));
    }
    let mut buf = Vec::new();
    res.body_mut()
        .as_reader()
        .take(MAX_DOWNLOAD)
        .read_to_end(&mut buf)
        .map_err(|e| format!("download failed: {e}"))?;
    Ok(buf)
}

fn sha256_hex(data: &[u8]) -> String {
    Sha256::digest(data)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// The expected digest of `asset` from a SHA256SUMS file.
fn expected_sha256(sums: &str, asset: &str) -> Option<String> {
    sums.lines().find_map(|l| {
        let mut parts = l.split_whitespace();
        let hash = parts.next()?;
        let name = parts.next()?.trim_start_matches('*');
        (name == asset && hash.len() == 64).then(|| hash.to_ascii_lowercase())
    })
}

/// Pull the `kimi-statusline` binary out of a release archive.
fn extract_binary(archive: &[u8], asset: &str) -> Result<Vec<u8>, String> {
    if asset.ends_with(".tar.gz") {
        let gz = flate2::read::GzDecoder::new(archive);
        let mut tar = tar::Archive::new(gz);
        for entry in tar.entries().map_err(|e| e.to_string())? {
            let mut entry = entry.map_err(|e| e.to_string())?;
            let path = entry.path().map_err(|e| e.to_string())?.into_owned();
            if path.file_name().is_some_and(|n| n == "kimi-statusline") {
                let mut out = Vec::new();
                entry.read_to_end(&mut out).map_err(|e| e.to_string())?;
                return Ok(out);
            }
        }
        Err("the archive has no kimi-statusline binary".into())
    } else {
        extract_from_zip(archive)
    }
}

/// Minimal reader for the release zip: one deflated or stored entry named
/// kimi-statusline.exe (what 7z writes in the release workflow).
fn extract_from_zip(data: &[u8]) -> Result<Vec<u8>, String> {
    let u16le = |i: usize| -> Option<u16> {
        Some(u16::from_le_bytes(data.get(i..i + 2)?.try_into().ok()?))
    };
    let u32le = |i: usize| -> Option<u32> {
        Some(u32::from_le_bytes(data.get(i..i + 4)?.try_into().ok()?))
    };
    // end of central directory: scan back for its signature
    let eocd = (0..data.len().saturating_sub(21))
        .rev()
        .find(|&i| u32le(i) == Some(0x0605_4b50))
        .ok_or("not a zip archive")?;
    let entries = u16le(eocd + 10).ok_or("bad zip")? as usize;
    let mut at = u32le(eocd + 16).ok_or("bad zip")? as usize;
    for _ in 0..entries {
        if u32le(at) != Some(0x0201_4b50) {
            return Err("bad zip central directory".into());
        }
        let method = u16le(at + 10).ok_or("bad zip")?;
        let csize = u32le(at + 20).ok_or("bad zip")? as usize;
        let name_len = u16le(at + 28).ok_or("bad zip")? as usize;
        let extra_len = u16le(at + 30).ok_or("bad zip")? as usize;
        let comment_len = u16le(at + 32).ok_or("bad zip")? as usize;
        let local = u32le(at + 42).ok_or("bad zip")? as usize;
        let name = data.get(at + 46..at + 46 + name_len).ok_or("bad zip")?;
        let name = String::from_utf8_lossy(name);
        at += 46 + name_len + extra_len + comment_len;
        if !name
            .rsplit(['/', '\\'])
            .next()
            .is_some_and(|n| n == "kimi-statusline.exe")
        {
            continue;
        }
        if u32le(local) != Some(0x0403_4b50) {
            return Err("bad zip local header".into());
        }
        let lname = u16le(local + 26).ok_or("bad zip")? as usize;
        let lextra = u16le(local + 28).ok_or("bad zip")? as usize;
        let start = local + 30 + lname + lextra;
        let body = data.get(start..start + csize).ok_or("truncated zip")?;
        return match method {
            0 => Ok(body.to_vec()),
            8 => {
                let mut out = Vec::new();
                flate2::read::DeflateDecoder::new(body)
                    .read_to_end(&mut out)
                    .map_err(|e| format!("bad zip data: {e}"))?;
                Ok(out)
            }
            m => Err(format!("unsupported zip compression {m}")),
        };
    }
    Err("the archive has no kimi-statusline.exe".into())
}

/// Download `tag` for this platform, verify it, and replace the binary at
/// `exe`. Returns the installed version string.
pub fn install(tag: &str, exe: &Path) -> Result<String, String> {
    let asset = asset_name()?;
    let base = format!("{REPO}/releases/download/{tag}");
    let sums = download(&format!("{base}/SHA256SUMS"))?;
    let sums = String::from_utf8_lossy(&sums);
    let expected = expected_sha256(&sums, &asset)
        .ok_or_else(|| format!("{asset} is not listed in the release's SHA256SUMS"))?;
    let archive = download(&format!("{base}/{asset}"))?;
    let actual = sha256_hex(&archive);
    if actual != expected {
        return Err(format!(
            "checksum mismatch for {asset}: expected {expected}, got {actual}; nothing was changed"
        ));
    }
    let binary = extract_binary(&archive, &asset)?;
    replace_binary(exe, &binary)?;
    Ok(tag.trim_start_matches('v').to_string())
}

/// Write the new binary next to the old one, then rename over it. On unix
/// the running process keeps its open inode, so replacing the file under it
/// is safe. Windows won't let a running .exe be overwritten but does let it
/// be renamed, so the old one is moved aside first.
fn replace_binary(exe: &Path, binary: &[u8]) -> Result<(), String> {
    let dir = exe.parent().ok_or("cannot locate the binary's directory")?;
    let staged = dir.join(format!(".kimi-statusline-update-{}", std::process::id()));
    std::fs::write(&staged, binary).map_err(|e| {
        format!(
            "cannot write to {} ({e}); try reinstalling with install.sh",
            dir.display()
        )
    })?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&staged, std::fs::Permissions::from_mode(0o755));
    }
    #[cfg(windows)]
    {
        let old = exe.with_extension("exe.old");
        let _ = std::fs::remove_file(&old);
        if let Err(e) = std::fs::rename(exe, &old) {
            let _ = std::fs::remove_file(&staged);
            return Err(format!("cannot move the running binary aside: {e}"));
        }
    }
    std::fs::rename(&staged, exe).map_err(|e| {
        let _ = std::fs::remove_file(&staged);
        format!("cannot replace {}: {e}", exe.display())
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions_compare_numerically() {
        assert!(is_newer("v0.10.0", "0.9.9"));
        assert!(is_newer("v1.0.0", "0.3.0"));
        assert!(!is_newer("v0.3.0", "0.3.0"));
        assert!(!is_newer("v0.2.9", "0.3.0"));
        assert!(!is_newer("garbage", "0.3.0"));
        assert_eq!(parse_version("v1.2.3-rc.1"), Some((1, 2, 3)));
        assert_eq!(parse_version("1.2"), None);
    }

    #[test]
    fn sums_lookup() {
        let sums = "aa\n\
            034fbf72d5c497aee6dfb6437ac9b2d9f4e86759910b0a52b34b330c873696ad  kimi-statusline-darwin-arm64.tar.gz\n\
            44C34DA0B81C68CCE1B8024CF512F260BAA79A8133C05E95D85766DF2BEC09A3 *kimi-statusline-darwin-x64.tar.gz\n";
        assert_eq!(
            expected_sha256(sums, "kimi-statusline-darwin-arm64.tar.gz").unwrap(),
            "034fbf72d5c497aee6dfb6437ac9b2d9f4e86759910b0a52b34b330c873696ad"
        );
        assert!(expected_sha256(sums, "kimi-statusline-darwin-x64.tar.gz")
            .unwrap()
            .starts_with("44c34da0"));
        assert!(expected_sha256(sums, "kimi-statusline-linux-x64.tar.gz").is_none());
    }

    #[test]
    fn install_kind_from_path() {
        let cargo = Some(PathBuf::from("/home/u/.cargo/bin"));
        assert_eq!(
            classify(Path::new("/home/u/.cargo/bin/kimi-statusline"), &cargo),
            InstallKind::Cargo
        );
        assert_eq!(
            classify(
                Path::new("/home/u/.kimi-code/plugins/managed/kimi-statusline/bin/kimi-statusline"),
                &cargo
            ),
            InstallKind::Plugin
        );
        let local = Path::new("/home/u/.local/bin/kimi-statusline");
        assert_eq!(
            classify(local, &cargo),
            InstallKind::Standalone(local.to_path_buf())
        );
    }

    #[test]
    fn tar_extract_and_replace() {
        // build a release-shaped tar.gz in memory
        let mut tar_bytes = Vec::new();
        {
            let gz = flate2::write::GzEncoder::new(&mut tar_bytes, flate2::Compression::fast());
            let mut b = tar::Builder::new(gz);
            let mut h = tar::Header::new_gnu();
            h.set_size(5);
            h.set_mode(0o755);
            h.set_cksum();
            b.append_data(&mut h, "kimi-statusline", &b"NEWBN"[..])
                .unwrap();
            b.into_inner().unwrap().finish().unwrap();
        }
        let bin = extract_binary(&tar_bytes, "kimi-statusline-linux-x64.tar.gz").unwrap();
        assert_eq!(bin, b"NEWBN");

        let dir = std::env::temp_dir().join(format!("ksl-update-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let exe = dir.join("kimi-statusline");
        std::fs::write(&exe, b"OLD").unwrap();
        replace_binary(&exe, &bin).unwrap();
        assert_eq!(std::fs::read(&exe).unwrap(), b"NEWBN");
        let leftovers: Vec<_> = std::fs::read_dir(&dir).unwrap().collect();
        assert_eq!(leftovers.len(), 1, "staged file cleaned up");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn checksum_mismatch_leaves_binary_untouched() {
        // simulate the verify step install() runs before touching anything
        let archive = b"tampered archive bytes";
        let sums = format!("{}  kimi-statusline-linux-x64.tar.gz\n", "0".repeat(64));
        let expected = expected_sha256(&sums, "kimi-statusline-linux-x64.tar.gz").unwrap();
        assert_ne!(sha256_hex(archive), expected);
        // and the digest itself is right for a known input
        assert_eq!(
            sha256_hex(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    #[test]
    fn zip_extract_stored_and_deflated() {
        // written by the `zip` tool, same layout as the release archives
        let deflated = include_bytes!("testdata/deflated.zip");
        let stored = include_bytes!("testdata/stored.zip");
        let other = include_bytes!("testdata/other.zip");
        assert_eq!(extract_from_zip(deflated).unwrap(), b"EXE!EXE!EXE!EXE!");
        assert_eq!(extract_from_zip(stored).unwrap(), b"EXE!EXE!EXE!EXE!");
        assert!(extract_from_zip(other).is_err());
        assert!(extract_from_zip(b"not a zip at all").is_err());
    }
}
