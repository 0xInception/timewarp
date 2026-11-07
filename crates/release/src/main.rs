//! timewarp-release: makes signing keys and signed update manifests.
//!
//!   timewarp-release keygen
//!       New signing key. The private key goes to ~/.config/timewarp/release-key
//!       (never commit it); the public key to apps/desktop/update-key.pub.
//!   timewarp-release notes <version>
//!       Prints that version's section of CHANGELOG.md.
//!   timewarp-release manifest <version> <repo-url> <dist-dir>
//!       Writes and signs dist/latest.json for the archives in dist-dir.
//!       The key comes from $TIMEWARP_UPDATE_KEY, else the key file.

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use timewarp_release::*;

const PUBLIC_KEY_FILE: &str = "apps/desktop/update-key.pub";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let args: Vec<&str> = args.iter().map(String::as_str).collect();
    let result = match args.as_slice() {
        ["keygen"] => keygen(false),
        ["keygen", "--force"] => keygen(true),
        ["notes", version] => notes(version).map(|n| print!("{n}")),
        ["manifest", version, repo, dist] => manifest(version, repo, Path::new(dist)),
        _ => Err("usage: timewarp-release keygen | notes <version> | manifest <version> <repo-url> <dist-dir>".into()),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("error: {e}");
            ExitCode::FAILURE
        }
    }
}

fn key_file() -> PathBuf {
    let home = std::env::var_os("HOME").map(PathBuf::from).unwrap_or_default();
    home.join(".config/timewarp/release-key")
}

fn keygen(force: bool) -> Result<(), String> {
    let path = key_file();
    if path.exists() && !force {
        return Err(format!(
            "{} already exists. Replacing it means older Timewarp builds can't update any more; pass --force if you really mean it.",
            path.display()
        ));
    }
    let (private, public) = generate_key()?;
    std::fs::create_dir_all(path.parent().unwrap()).map_err(|e| e.to_string())?;
    write_private(&path, &private)?;
    std::fs::write(PUBLIC_KEY_FILE, format!("{public}\n")).map_err(|e| format!("{PUBLIC_KEY_FILE}: {e}"))?;
    println!("Private key: {} (keep it secret, back it up in your password manager)", path.display());
    println!("Public key:  {PUBLIC_KEY_FILE} (commit this)");
    println!();
    println!("Add the private key to GitHub as the secret TIMEWARP_UPDATE_KEY in the `release` environment:");
    println!("  gh secret set TIMEWARP_UPDATE_KEY --env release < {}", path.display());
    Ok(())
}

#[cfg(unix)]
fn write_private(path: &Path, key: &str) -> Result<(), String> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    let mut f = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(path)
        .map_err(|e| e.to_string())?;
    f.write_all(key.as_bytes()).map_err(|e| e.to_string())
}

#[cfg(not(unix))]
fn write_private(path: &Path, key: &str) -> Result<(), String> {
    std::fs::write(path, key).map_err(|e| e.to_string())
}

/// The `## <version>` section of CHANGELOG.md, without its heading.
fn notes(version: &str) -> Result<String, String> {
    let log = std::fs::read_to_string("CHANGELOG.md").map_err(|e| format!("CHANGELOG.md: {e}"))?;
    let mut out = Vec::new();
    let mut inside = false;
    for line in log.lines() {
        if let Some(h) = line.strip_prefix("## ") {
            if inside {
                break;
            }
            inside = h.split_whitespace().next() == Some(version);
            continue;
        }
        if inside {
            out.push(line);
        }
    }
    if !inside && out.is_empty() {
        return Err(format!("CHANGELOG.md has no `## {version}` section"));
    }
    Ok(format!("{}\n", out.join("\n").trim()))
}

fn manifest(version: &str, repo: &str, dist: &Path) -> Result<(), String> {
    let private = match std::env::var("TIMEWARP_UPDATE_KEY") {
        Ok(k) if !k.trim().is_empty() => k.trim().to_string(),
        _ => std::fs::read_to_string(key_file())
            .map(|k| k.trim().to_string())
            .map_err(|_| "no signing key: set TIMEWARP_UPDATE_KEY or run `timewarp-release keygen`")?,
    };
    // Catch the wrong key here, not after every user's update fails.
    let built_in = std::fs::read_to_string(PUBLIC_KEY_FILE).unwrap_or_default();
    if public_key_of(&private)? != built_in.trim() {
        return Err(format!("the signing key doesn't match {PUBLIC_KEY_FILE}, so the app would refuse this release"));
    }

    let repo = repo.trim_end_matches('/');
    let prefix = format!("Timewarp-{version}-");
    let mut assets = Vec::new();
    let mut names: Vec<String> = std::fs::read_dir(dist)
        .map_err(|e| format!("{}: {e}", dist.display()))?
        .flatten()
        .filter_map(|e| e.file_name().into_string().ok())
        .filter(|n| n.starts_with(&prefix) && n.ends_with(".tar.gz"))
        .collect();
    names.sort();
    for name in names {
        // Timewarp-<version>-<os>-<arch>.tar.gz
        let rest = &name[prefix.len()..name.len() - ".tar.gz".len()];
        let (os, arch) = rest.split_once('-').ok_or(format!("can't tell the OS and CPU from {name}"))?;
        let bytes = std::fs::read(dist.join(&name)).map_err(|e| e.to_string())?;
        assets.push(Asset {
            os: os.into(),
            arch: arch.into(),
            url: format!("{repo}/releases/download/v{version}/{name}"),
            size: bytes.len() as u64,
            sha256: to_hex(ring::digest::digest(&ring::digest::SHA256, &bytes).as_ref()),
            name,
        });
    }
    if assets.is_empty() {
        return Err(format!("no {prefix}<os>-<arch>.tar.gz archives in {}", dist.display()));
    }
    let m = Manifest {
        app: "timewarp".into(),
        version: version.into(),
        date: now_rfc3339(),
        notes: notes(version).unwrap_or_default(),
        page: format!("{repo}/releases/tag/v{version}"),
        assets,
    };
    let json = serde_json::to_vec_pretty(&m).map_err(|e| e.to_string())?;
    let sig = sign(&json, &private)?;
    verify(&json, &sig, built_in.trim())?;
    std::fs::write(dist.join(MANIFEST), &json).map_err(|e| e.to_string())?;
    std::fs::write(dist.join(SIGNATURE), format!("{sig}\n")).map_err(|e| e.to_string())?;
    for a in &m.assets {
        println!("{} {} {} bytes", a.os, a.arch, a.size);
    }
    println!("Signed {} and {}", dist.join(MANIFEST).display(), dist.join(SIGNATURE).display());
    Ok(())
}

fn now_rfc3339() -> String {
    let secs = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0) as i64;
    let (days, rem) = (secs.div_euclid(86_400), secs.rem_euclid(86_400));
    // Civil date from days since 1970-01-01 (Howard Hinnant's algorithm).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!("{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}Z", rem / 3600, rem % 3600 / 60, rem % 60)
}
