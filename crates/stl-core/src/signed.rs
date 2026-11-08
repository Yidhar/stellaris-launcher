//! Files on the mirror (`https://dl.stellarismod.com/`) that the launcher acts on carry an Ed25519 signature:
//! `{"manifest": "<the signed text>", "signature": "<hex>"}`. The private keys are not on the mirror (the launcher's own key is on the
//! release machine, the plugin index's in its repository's CI), so whoever can write to the bucket can serve files but cannot make one the
//! launcher takes.

use crate::{anyhow, Context, Result};

/// Hex to bytes, exactly `N` of them.
pub fn hex<const N: usize>(s: &str) -> Option<[u8; N]> {
    let s = s.trim();
    if s.len() != N * 2 || !s.is_ascii() {
        return None;
    }
    let mut out = [0u8; N];
    for (i, b) in out.iter_mut().enumerate() {
        *b = u8::from_str_radix(&s[i * 2..i * 2 + 2], 16).ok()?;
    }
    Some(out)
}

pub fn key(public_hex: &str) -> Result<ed25519_dalek::VerifyingKey> {
    let bytes = hex::<32>(public_hex).context("not a public key")?;
    ed25519_dalek::VerifyingKey::from_bytes(&bytes).context("not a public key")
}

/// The signed text of a mirror file, once its signature verifies against `key`. `what` names the file in errors.
pub fn open(text: &str, key: &ed25519_dalek::VerifyingKey, what: &str) -> Result<String> {
    #[derive(serde::Deserialize)]
    struct Envelope {
        manifest: String,
        signature: String,
    }
    let env: Envelope = serde_json::from_str(text.trim_start_matches('\u{feff}')).with_context(|| format!("{what} is not a signed manifest"))?;
    let sig = hex::<64>(&env.signature).with_context(|| format!("{what} has no valid signature"))?;
    key.verify_strict(env.manifest.as_bytes(), &ed25519_dalek::Signature::from_bytes(&sig))
        .map_err(|_| anyhow!("{what} is not signed with the expected key"))?;
    Ok(env.manifest)
}

#[cfg(test)]
pub fn sign(key: &ed25519_dalek::SigningKey, manifest: &str) -> String {
    use ed25519_dalek::Signer;
    let sig: String = key.sign(manifest.as_bytes()).to_bytes().iter().map(|b| format!("{b:02x}")).collect();
    serde_json::json!({ "manifest": manifest, "signature": sig }).to_string()
}
