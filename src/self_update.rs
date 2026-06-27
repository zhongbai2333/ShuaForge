//! ShuaForge self-update support.
//!
//! The updater checks GitHub Releases, prefers a Cloudflare/CDN mirror when
//! possible, and falls back to GitHub asset URLs. Windows/Linux release assets
//! are zip files that contain the executable; macOS release assets are dmg
//! files, so the updater downloads and opens the dmg for the user.

use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::{
    error::Error,
    ffi::OsString,
    fs,
    io::{self, Read, Write},
    path::{Path, PathBuf},
    process::Command,
    time::Duration,
};
use tempfile::TempDir;

const OWNER: &str = "zhongbai2333";
const REPO: &str = "ShuaForge";
const USER_AGENT: &str = "ShuaForge-Updater/1.0";
const CDN_BASE: &str = "https://dl.zhongbai233.com/release";
const DISABLE_CDN_ENV: &str = "SHUAFORGE_DISABLE_UPDATE_CDN";
const CURRENT_VERSION: &str = env!("CARGO_PKG_VERSION");

pub type UpdateResult<T> = Result<T, Box<dyn Error + Send + Sync>>;
pub type ProgressCallback = Box<dyn Fn(u64, u64) + Send>;

#[derive(Debug, Clone)]
pub struct UpdateInfo {
    pub tag_name: String,
    pub release_name: String,
    pub asset_name: String,
    pub size: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UpdateOutcome {
    UpToDate,
    Skipped,
    UpdateLaunched,
}

#[derive(Debug, Deserialize)]
struct ReleaseInfo {
    name: Option<String>,
    tag_name: Option<String>,
    assets: Vec<ReleaseAsset>,
}

#[derive(Debug, Deserialize)]
struct ReleaseAsset {
    name: String,
    size: Option<u64>,
    browser_download_url: String,
    #[serde(default)]
    digest: Option<String>,
}

#[derive(Debug, Clone)]
struct MatchedAsset {
    info: UpdateInfo,
    download_urls: Vec<String>,
    sha256: Option<String>,
}

pub fn current_tag() -> String {
    format!("v{CURRENT_VERSION}")
}

pub fn check_latest_version() -> UpdateResult<Option<UpdateInfo>> {
    let matched = get_latest_release_asset()?;
    if matched.info.tag_name != current_tag() {
        Ok(Some(matched.info))
    } else {
        Ok(None)
    }
}

pub fn perform_update(on_progress: Option<ProgressCallback>) -> UpdateResult<UpdateOutcome> {
    if is_dev_build() {
        return Ok(UpdateOutcome::Skipped);
    }

    let matched = get_latest_release_asset()?;
    if matched.info.tag_name == current_tag() {
        if let Some(expected) = matched.sha256.as_deref() {
            let actual = compute_file_sha256(&current_executable_path()?)?;
            if !eq_hash(&actual, expected) {
                start_update(&matched, on_progress)?;
                return Ok(UpdateOutcome::UpdateLaunched);
            }
        }
        return Ok(UpdateOutcome::UpToDate);
    }

    start_update(&matched, on_progress)?;
    Ok(UpdateOutcome::UpdateLaunched)
}

fn get_latest_release_asset() -> UpdateResult<MatchedAsset> {
    let latest = fetch_latest_release()?;
    let platform_keys = detect_platform_keywords();
    let release_name = latest.name.unwrap_or_default();
    let tag_name = latest.tag_name.unwrap_or_default();

    if tag_name.is_empty() {
        return Err("latest release missing tag_name".into());
    }

    for asset in latest.assets {
        if platform_keys.iter().any(|key| asset.name.contains(key)) {
            let original_url = asset.browser_download_url;
            let sha256 = asset
                .digest
                .as_deref()
                .and_then(|digest| digest.split(':').next_back())
                .map(str::trim)
                .map(ToOwned::to_owned)
                .filter(|hash| hash.len() == 64 && hash.chars().all(|ch| ch.is_ascii_hexdigit()));

            return Ok(MatchedAsset {
                info: UpdateInfo {
                    tag_name,
                    release_name,
                    asset_name: asset.name,
                    size: asset.size.unwrap_or(0),
                },
                download_urls: build_download_urls(&original_url),
                sha256,
            });
        }
    }

    Err(format!(
        "no matching release asset for platform keys: {}",
        platform_keys.join(", ")
    )
    .into())
}

fn fetch_latest_release() -> UpdateResult<ReleaseInfo> {
    let url = format!("https://api.github.com/repos/{OWNER}/{REPO}/releases/latest");
    let agent = ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(15)))
        .build()
        .new_agent();
    let mut response = agent
        .get(&url)
        .header("accept", "application/vnd.github+json")
        .header("user-agent", USER_AGENT)
        .call()?;

    let status = response.status();
    if !status.is_success() {
        let body = response.body_mut().read_to_string().unwrap_or_default();
        return Err(format!(
            "latest release request failed: HTTP {} {body}",
            status.as_u16()
        )
        .into());
    }

    let text = response.body_mut().read_to_string()?;
    Ok(serde_json::from_str(&text)?)
}

fn detect_platform_keywords() -> Vec<String> {
    let os = std::env::consts::OS;
    let arch = std::env::consts::ARCH;
    match (os, arch) {
        ("windows", "x86_64") => vec!["Windows-x64".into()],
        ("windows", "aarch64") => vec!["Windows-arm64".into(), "Windows-aarch64".into()],
        ("linux", "x86_64") => vec!["Linux-x64".into()],
        ("linux", "aarch64") => vec!["Linux-arm64".into()],
        ("macos", "x86_64") => vec!["macOS-x64".into(), "macOS-Intel".into()],
        ("macos", "aarch64") => vec!["macOS-arm64".into(), "macOS-AppleSilicon".into()],
        _ => vec![format!("{os}-{arch}")],
    }
}

fn build_download_urls(original_url: &str) -> Vec<String> {
    let mut urls = Vec::new();
    if std::env::var(DISABLE_CDN_ENV).ok().as_deref() != Some("1") {
        let accelerated = get_accelerated_url(original_url);
        if accelerated != original_url {
            urls.push(accelerated);
        }
    }
    urls.push(original_url.to_owned());
    urls
}

fn get_accelerated_url(original_url: &str) -> String {
    if let Some(tail) = original_url.split("/releases/download/").nth(1) {
        format!("{CDN_BASE}/{tail}")
    } else {
        original_url.to_owned()
    }
}

fn start_update(matched: &MatchedAsset, on_progress: Option<ProgressCallback>) -> UpdateResult<()> {
    let tmp_dir = TempDir::new()?;
    let downloaded = download_and_verify(tmp_dir.path(), matched, on_progress)?;

    if cfg!(target_os = "macos") {
        let dmg_path = persist_macos_dmg(&downloaded, &matched.info.asset_name)?;
        open_macos_dmg(&dmg_path)?;
        return Ok(());
    }

    let executable = extract_executable_from_asset(tmp_dir.path(), &downloaded)?;
    if cfg!(windows) {
        windows_apply_and_restart(&executable)?;
        std::process::exit(0);
    }

    let new_exe = unix_apply(&executable)?;
    Command::new(&new_exe)
        .args(std::env::args_os().skip(1))
        .spawn()?;
    std::process::exit(0);
}

fn download_and_verify(
    tmp_dir: &Path,
    matched: &MatchedAsset,
    on_progress: Option<ProgressCallback>,
) -> UpdateResult<PathBuf> {
    let callback = on_progress.as_ref();
    let mut errors = Vec::new();
    for url in &matched.download_urls {
        match download_and_verify_from_url(tmp_dir, matched, url, callback) {
            Ok(path) => return Ok(path),
            Err(err) => errors.push(format!("{url}: {err}")),
        }
    }
    Err(format!(
        "failed to download update from all sources:\n{}",
        errors.join("\n")
    )
    .into())
}

fn download_and_verify_from_url(
    tmp_dir: &Path,
    matched: &MatchedAsset,
    url: &str,
    on_progress: Option<&ProgressCallback>,
) -> UpdateResult<PathBuf> {
    let agent = ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(300)))
        .build()
        .new_agent();
    let mut response = agent.get(url).header("user-agent", USER_AGENT).call()?;
    let status = response.status();
    if !status.is_success() {
        let body = response.body_mut().read_to_string().unwrap_or_default();
        return Err(format!("download failed: HTTP {} {body}", status.as_u16()).into());
    }

    let total = response
        .headers()
        .get("content-length")
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse::<u64>().ok())
        .filter(|value| *value > 0)
        .unwrap_or(matched.info.size);
    let out_path = tmp_dir.join(&matched.info.asset_name);
    let _ = fs::remove_file(&out_path);

    let mut reader = response.into_body().into_reader();
    let mut file = fs::File::create(&out_path)?;
    let mut hasher = Sha256::new();
    let mut downloaded = 0u64;
    let mut buf = [0u8; 8192];

    loop {
        let read = reader.read(&mut buf)?;
        if read == 0 {
            break;
        }
        file.write_all(&buf[..read])?;
        hasher.update(&buf[..read]);
        downloaded += read as u64;
        if let Some(callback) = on_progress {
            callback(downloaded, total);
        }
    }

    if let Some(expected) = matched.sha256.as_deref() {
        let actual = hex::encode(hasher.finalize());
        if !eq_hash(&actual, expected) {
            let _ = fs::remove_file(&out_path);
            return Err(format!("SHA256 mismatch: expected {expected}, got {actual}").into());
        }
    }

    Ok(out_path)
}

fn extract_executable_from_asset(tmp_dir: &Path, asset_path: &Path) -> UpdateResult<PathBuf> {
    if asset_path.extension().and_then(|value| value.to_str()) != Some("zip") {
        return Ok(asset_path.to_path_buf());
    }

    let file = fs::File::open(asset_path)?;
    let mut archive = zip::ZipArchive::new(file)?;
    let expected = executable_file_name();

    for index in 0..archive.len() {
        let mut entry = archive.by_index(index)?;
        if !entry.is_file() {
            continue;
        }
        let Some(name) = Path::new(entry.name())
            .file_name()
            .and_then(|name| name.to_str())
        else {
            continue;
        };
        if name.eq_ignore_ascii_case(&expected) || name.eq_ignore_ascii_case("ShuaForge") {
            let out_path = tmp_dir.join(&expected);
            let mut out = fs::File::create(&out_path)?;
            io::copy(&mut entry, &mut out)?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let mut perm = fs::metadata(&out_path)?.permissions();
                perm.set_mode(0o755);
                fs::set_permissions(&out_path, perm)?;
            }
            return Ok(out_path);
        }
    }

    Err(format!("no executable named {expected} found in update zip").into())
}

fn executable_file_name() -> String {
    if cfg!(windows) {
        "shuaforge.exe".into()
    } else {
        "shuaforge".into()
    }
}

fn current_executable_path() -> UpdateResult<PathBuf> {
    Ok(std::env::current_exe()?)
}

fn eq_hash(a: &str, b: &str) -> bool {
    a.trim().eq_ignore_ascii_case(b.trim())
}

fn compute_file_sha256(path: &Path) -> UpdateResult<String> {
    let mut file = fs::File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buf = [0u8; 8192];
    loop {
        let read = file.read(&mut buf)?;
        if read == 0 {
            break;
        }
        hasher.update(&buf[..read]);
    }
    Ok(hex::encode(hasher.finalize()))
}

fn is_dev_build() -> bool {
    if std::env::var_os("CARGO").is_some() {
        return true;
    }
    let Ok(exe) = std::env::current_exe() else {
        return false;
    };
    let exe = exe.to_string_lossy().to_ascii_lowercase();
    exe.contains("\\target\\debug\\")
        || exe.contains("/target/debug/")
        || exe.contains("\\target\\release\\")
        || exe.contains("/target/release/")
}

fn move_or_copy(src: &Path, dst: &Path) -> UpdateResult<()> {
    match fs::rename(src, dst) {
        Ok(()) => Ok(()),
        Err(_) => {
            fs::copy(src, dst)?;
            let _ = fs::remove_file(src);
            Ok(())
        }
    }
}

fn unix_apply(tmp_file: &Path) -> UpdateResult<PathBuf> {
    let local_exe = current_executable_path()?;
    let target_exe = local_exe.clone();
    let staged = target_exe.with_extension("new");
    let _ = fs::remove_file(&staged);
    move_or_copy(tmp_file, &staged)?;
    fs::rename(&staged, &target_exe)?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perm = fs::metadata(&target_exe)?.permissions();
        perm.set_mode(0o755);
        fs::set_permissions(&target_exe, perm)?;
    }

    Ok(target_exe)
}

fn windows_apply_and_restart(tmp_file: &Path) -> UpdateResult<()> {
    let local_exe = current_executable_path()?;
    let parent = local_exe
        .parent()
        .ok_or("cannot determine executable directory")?;
    let exe_name = local_exe
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or("invalid executable name")?;

    let staged_name = format!("{exe_name}.new");
    let staged = parent.join(&staged_name);
    let _ = fs::remove_file(&staged);
    move_or_copy(tmp_file, &staged)?;

    let args: Vec<OsString> = std::env::args_os().skip(1).collect();
    let bat_path = std::env::temp_dir().join("shuaforge_update.bat");
    let bat_content = [
        "@echo off".to_string(),
        "echo Updating ShuaForge, please wait...".to_string(),
        "timeout /t 3 /nobreak > nul".to_string(),
        format!("cd /d \"{}\"", parent.display()),
        format!("del /F /Q \"{exe_name}\""),
        format!("ren \"{staged_name}\" \"{exe_name}\""),
        format!("start \"\" \"{exe_name}\" %*"),
        "del \"%~f0\"".to_string(),
    ]
    .join("\r\n");
    fs::write(&bat_path, bat_content)?;

    Command::new("cmd")
        .args(["/C", bat_path.to_string_lossy().as_ref()])
        .args(args)
        .spawn()?;
    Ok(())
}

fn open_macos_dmg(path: &Path) -> UpdateResult<()> {
    Command::new("open").arg(path).spawn()?;
    Ok(())
}

fn persist_macos_dmg(downloaded: &Path, asset_name: &str) -> UpdateResult<PathBuf> {
    let target = std::env::temp_dir().join(asset_name);
    let _ = fs::remove_file(&target);
    fs::copy(downloaded, &target)?;
    Ok(target)
}

#[cfg(test)]
mod tests {
    use super::{build_download_urls, detect_platform_keywords, get_accelerated_url};

    #[test]
    fn accelerated_url_maps_github_release_path() {
        let original = "https://github.com/zhongbai2333/ShuaForge/releases/download/v0.1.1/ShuaForge-Windows-x64-v0.1.1.zip";
        assert_eq!(
            get_accelerated_url(original),
            "https://dl.zhongbai233.com/release/v0.1.1/ShuaForge-Windows-x64-v0.1.1.zip"
        );
    }

    #[test]
    fn download_urls_include_github_fallback() {
        let original = "https://github.com/zhongbai2333/ShuaForge/releases/download/v0.1.1/ShuaForge-Linux-x64-v0.1.1.zip";
        let urls = build_download_urls(original);
        assert!(urls.iter().any(|url| url == original));
    }

    #[test]
    fn platform_keywords_are_not_empty() {
        assert!(!detect_platform_keywords().is_empty());
    }
}
