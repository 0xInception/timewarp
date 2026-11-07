//! The signed update manifest (`latest.json`) shared by the release tool,
//! which writes and signs it, and the app, which checks it.
//!
//! Trust comes from one Ed25519 key. The public half is built into the app
//! (`apps/desktop/update-key.pub`); the private half only ever lives in your
//! release environment. A manifest is only believed if its signature checks
//! out, and every download is then checked against the size and SHA-256 the
//! manifest lists. So publishing an update needs the key, not just access to
//! the GitHub repository.

use ring::signature::{ED25519, Ed25519KeyPair, KeyPair, UnparsedPublicKey};
use serde::{Deserialize, Serialize};

/// Mixed into every signature so a Timewarp key can't be tricked into
/// signing, or accepting, anything else.
const CONTEXT: &[u8] = b"timewarp-update-manifest-v1\n";

pub const MANIFEST: &str = "latest.json";
pub const SIGNATURE: &str = "latest.json.sig";

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Manifest {
    /// Always "timewarp": a manifest for something else is refused.
    pub app: String,
    pub version: String,
    /// RFC 3339.
    pub date: String,
    /// Markdown release notes.
    #[serde(default)]
    pub notes: String,
    /// The release page, for people who'd rather download it themselves.
    pub page: String,
    pub assets: Vec<Asset>,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Asset {
    /// "macos", "windows".
    pub os: String,
    /// "aarch64", "x86_64" or "universal".
    pub arch: String,
    pub name: String,
    pub url: String,
    pub size: u64,
    /// Lowercase hex.
    pub sha256: String,
}

impl Manifest {
    /// The download for this OS and CPU, else a universal one.
    pub fn asset_for(&self, os: &str, arch: &str) -> Option<&Asset> {
        let mine = |a: &&Asset| a.os == os;
        self.assets
            .iter()
            .filter(mine)
            .find(|a| a.arch == arch)
            .or_else(|| self.assets.iter().filter(mine).find(|a| a.arch == "universal"))
    }
}

pub fn to_hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

pub fn from_hex(s: &str) -> Option<Vec<u8>> {
    let s = s.trim();
    if s.len() % 2 != 0 {
        return None;
    }
    (0..s.len()).step_by(2).map(|i| u8::from_str_radix(s.get(i..i + 2)?, 16).ok()).collect()
}

fn message(manifest: &[u8]) -> Vec<u8> {
    [CONTEXT, manifest].concat()
}

/// A new key pair: (private key as PKCS#8 hex, public key hex).
pub fn generate_key() -> Result<(String, String), String> {
    let rng = ring::rand::SystemRandom::new();
    let pkcs8 = Ed25519KeyPair::generate_pkcs8(&rng).map_err(|_| "couldn't generate a key")?;
    let pair = Ed25519KeyPair::from_pkcs8(pkcs8.as_ref()).map_err(|_| "couldn't read the new key")?;
    Ok((to_hex(pkcs8.as_ref()), to_hex(pair.public_key().as_ref())))
}

/// The public key (hex) belonging to a private key (PKCS#8 hex).
pub fn public_key_of(private_hex: &str) -> Result<String, String> {
    let pkcs8 = from_hex(private_hex).ok_or("the private key isn't valid hex")?;
    let pair = Ed25519KeyPair::from_pkcs8(&pkcs8).map_err(|_| "that isn't a Timewarp signing key")?;
    Ok(to_hex(pair.public_key().as_ref()))
}

/// Signs the exact bytes of `latest.json`; returns the signature as hex.
pub fn sign(manifest: &[u8], private_hex: &str) -> Result<String, String> {
    let pkcs8 = from_hex(private_hex).ok_or("the private key isn't valid hex")?;
    let pair = Ed25519KeyPair::from_pkcs8(&pkcs8).map_err(|_| "that isn't a Timewarp signing key")?;
    Ok(to_hex(pair.sign(&message(manifest)).as_ref()))
}

/// Checks the signature, then parses. Nothing unsigned gets parsed.
pub fn verify(manifest: &[u8], signature_hex: &str, public_hex: &str) -> Result<Manifest, String> {
    let public = from_hex(public_hex).filter(|k| k.len() == 32).ok_or("the built-in update key is invalid")?;
    let sig = from_hex(signature_hex).ok_or("the update's signature is malformed")?;
    UnparsedPublicKey::new(&ED25519, public)
        .verify(&message(manifest), &sig)
        .map_err(|_| "the update isn't signed by the Timewarp release key")?;
    let m: Manifest = serde_json::from_slice(manifest).map_err(|e| format!("the update manifest is unreadable: {e}"))?;
    if m.app != "timewarp" {
        return Err("the update manifest is for a different app".into());
    }
    Ok(m)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn manifest() -> Vec<u8> {
        serde_json::to_vec(&Manifest {
            app: "timewarp".into(),
            version: "0.2.0".into(),
            date: "2026-11-06T00:00:00Z".into(),
            notes: String::new(),
            page: "https://example.com".into(),
            assets: vec![Asset {
                os: "macos".into(),
                arch: "universal".into(),
                name: "Timewarp-0.2.0-macos-universal.tar.gz".into(),
                url: "https://example.com/a".into(),
                size: 3,
                sha256: "00".into(),
            }],
        })
        .unwrap()
    }

    #[test]
    fn accepts_only_what_the_key_signed() {
        let (private, public) = generate_key().unwrap();
        assert_eq!(public_key_of(&private).unwrap(), public);
        let m = manifest();
        let sig = sign(&m, &private).unwrap();
        let ok = verify(&m, &sig, &public).unwrap();
        assert_eq!(ok.asset_for("macos", "aarch64").unwrap().arch, "universal");

        let mut tampered = m.clone();
        let at = tampered.windows(5).position(|w| w == b"0.2.0").unwrap();
        tampered[at] = b'9';
        assert!(verify(&tampered, &sig, &public).is_err());

        let (_, other) = generate_key().unwrap();
        assert!(verify(&m, &sig, &other).is_err());
        assert!(verify(&m, "zz", &public).is_err());
    }
}
