use aes_gcm::{
    aead::{Aead, KeyInit},
    Aes256Gcm, Nonce,
};
use anyhow::{anyhow, Context, Result};
use argon2::{password_hash::SaltString, Argon2, PasswordHasher};
use base64::{engine::general_purpose::STANDARD as BASE64, Engine};
use rand::RngCore;
use std::collections::HashMap;

/// Verify a password against the local sudo. Returns true if correct.
pub fn verify_sudo_password(password: &str) -> bool {
    use std::io::Write;
    use std::process::{Command, Stdio};
    let mut child = match Command::new("sudo")
        .args(["-S", "-k", "true"])
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
    {
        Ok(c) => c,
        Err(_) => return false,
    };
    if let Some(ref mut stdin) = child.stdin {
        let _ = write!(stdin, "{}\n", password);
    }
    child.wait().map(|s| s.success()).unwrap_or(false)
}

pub fn save_plain(credentials: &HashMap<String, String>, path: &std::path::Path) -> Result<()> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    if let Some(p) = path.parent() {
        std::fs::create_dir_all(p)?;
    }
    let json = serde_json::to_string_pretty(credentials)?;
    // Create with mode 0o600 atomically — no race window between create and chmod.
    let mut f = std::fs::OpenOptions::new()
        .write(true).create(true).truncate(true)
        .mode(0o600)
        .open(path)?;
    f.write_all(json.as_bytes())?;
    Ok(())
}

pub fn load_plain(path: &std::path::Path) -> Result<HashMap<String, String>> {
    if !path.exists() {
        return Ok(HashMap::new());
    }
    let json = std::fs::read_to_string(path)?;
    Ok(serde_json::from_str(&json)?)
}

pub struct CredentialStore;

impl CredentialStore {
    pub fn generate_salt() -> [u8; 32] {
        let mut salt = [0u8; 32];
        rand::thread_rng().fill_bytes(&mut salt);
        salt
    }

    pub fn salt_to_base64(salt: &[u8; 32]) -> String {
        BASE64.encode(salt)
    }

    pub fn salt_from_base64(s: &str) -> Result<[u8; 32]> {
        let bytes = BASE64.decode(s).context("Failed to decode salt from base64")?;
        let mut salt = [0u8; 32];
        if bytes.len() != 32 {
            return Err(anyhow!("Salt must be 32 bytes, got {}", bytes.len()));
        }
        salt.copy_from_slice(&bytes);
        Ok(salt)
    }

    fn derive_key(master_pw: &str, salt: &[u8; 32]) -> Result<[u8; 32]> {
        let argon2 = Argon2::default();
        let salt_str = SaltString::encode_b64(salt)
            .map_err(|e| anyhow!("Invalid salt: {}", e))?;
        let hash = argon2
            .hash_password(master_pw.as_bytes(), &salt_str)
            .map_err(|e| anyhow!("Argon2 hashing failed: {}", e))?;
        let hash_bytes = hash.hash.ok_or_else(|| anyhow!("No hash output"))?;
        let bytes = hash_bytes.as_bytes();
        let mut key = [0u8; 32];
        let len = bytes.len().min(32);
        key[..len].copy_from_slice(&bytes[..len]);
        Ok(key)
    }

    pub fn encrypt(
        master_pw: &str,
        salt: &[u8; 32],
        credentials: &HashMap<String, String>,
    ) -> Result<String> {
        let key = Self::derive_key(master_pw, salt)?;
        let cipher = Aes256Gcm::new_from_slice(&key)
            .map_err(|e| anyhow!("Failed to create cipher: {}", e))?;

        let mut nonce_bytes = [0u8; 12];
        rand::thread_rng().fill_bytes(&mut nonce_bytes);
        let nonce = Nonce::from_slice(&nonce_bytes);

        let plaintext = serde_json::to_vec(credentials)
            .context("Failed to serialize credentials")?;
        let ciphertext = cipher
            .encrypt(nonce, plaintext.as_ref())
            .map_err(|e| anyhow!("Encryption failed: {}", e))?;

        // Prepend nonce to ciphertext
        let mut combined = nonce_bytes.to_vec();
        combined.extend_from_slice(&ciphertext);

        Ok(BASE64.encode(&combined))
    }

    pub fn decrypt(
        master_pw: &str,
        salt: &[u8; 32],
        ciphertext_b64: &str,
    ) -> Result<HashMap<String, String>> {
        let key = Self::derive_key(master_pw, salt)?;
        let cipher = Aes256Gcm::new_from_slice(&key)
            .map_err(|e| anyhow!("Failed to create cipher: {}", e))?;

        let combined = BASE64.decode(ciphertext_b64)
            .context("Failed to decode base64 ciphertext")?;
        if combined.len() < 12 {
            return Err(anyhow!("Ciphertext too short"));
        }
        let (nonce_bytes, ciphertext) = combined.split_at(12);
        let nonce = Nonce::from_slice(nonce_bytes);

        let plaintext = cipher
            .decrypt(nonce, ciphertext)
            .map_err(|_| anyhow!("Decryption failed — wrong master password?"))?;

        let credentials: HashMap<String, String> = serde_json::from_slice(&plaintext)
            .context("Failed to deserialize credentials")?;
        Ok(credentials)
    }
}
