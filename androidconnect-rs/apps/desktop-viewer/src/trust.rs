use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use androidconnect_protocol::{PAIRED_SECRET_BYTES, bytes_to_hex, hex_to_fixed};
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone)]
pub struct TrustStore {
    path: PathBuf,
    data: TrustData,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DesktopIdentity {
    pub desktop_id: String,
    pub desktop_name: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PairedAndroid {
    pub device_id: String,
    pub device_name: String,
    pub paired_secret: [u8; PAIRED_SECRET_BYTES],
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct TrustData {
    desktop_id: String,
    desktop_name: String,
    paired_androids: Vec<PairedAndroidRecord>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct PairedAndroidRecord {
    device_id: String,
    device_name: String,
    paired_secret_hex: String,
    last_authenticated_unix_ms: u64,
}

impl TrustStore {
    pub fn load_or_create() -> Result<Self> {
        Self::load_or_create_at(default_trust_path())
    }

    pub fn load_or_create_at(path: impl Into<PathBuf>) -> Result<Self> {
        let path = path.into();
        let data = if path.exists() {
            let raw = fs::read_to_string(&path)
                .with_context(|| format!("read desktop trust store {}", path.display()))?;
            serde_json::from_str(&raw)
                .with_context(|| format!("parse desktop trust store {}", path.display()))?
        } else {
            TrustData {
                desktop_id: String::new(),
                desktop_name: default_desktop_name(),
                paired_androids: Vec::new(),
            }
        };

        let mut store = Self { path, data };
        let mut changed = false;
        if store.data.desktop_id.trim().is_empty() {
            store.data.desktop_id = format!("desktop-{}", generate_hex_id(16));
            changed = true;
        }
        if store.data.desktop_name.trim().is_empty() {
            store.data.desktop_name = default_desktop_name();
            changed = true;
        }
        if changed {
            store.save()?;
        }
        Ok(store)
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn identity(&self) -> DesktopIdentity {
        DesktopIdentity {
            desktop_id: self.data.desktop_id.clone(),
            desktop_name: self.data.desktop_name.clone(),
        }
    }

    pub fn paired_android(&self, device_id: &str) -> Option<PairedAndroid> {
        self.data
            .paired_androids
            .iter()
            .find(|record| record.device_id == device_id)
            .and_then(|record| {
                let paired_secret = hex_to_fixed::<PAIRED_SECRET_BYTES>(&record.paired_secret_hex)?;
                Some(PairedAndroid {
                    device_id: record.device_id.clone(),
                    device_name: record.device_name.clone(),
                    paired_secret,
                })
            })
    }

    pub fn store_pairing(
        &mut self,
        device_id: &str,
        device_name: &str,
        paired_secret: [u8; PAIRED_SECRET_BYTES],
    ) -> Result<()> {
        let timestamp = now_unix_ms();
        let paired_secret_hex = bytes_to_hex(&paired_secret);
        if let Some(record) = self
            .data
            .paired_androids
            .iter_mut()
            .find(|record| record.device_id == device_id)
        {
            record.device_name = device_name.to_owned();
            record.paired_secret_hex = paired_secret_hex;
            record.last_authenticated_unix_ms = timestamp;
        } else {
            self.data.paired_androids.push(PairedAndroidRecord {
                device_id: device_id.to_owned(),
                device_name: device_name.to_owned(),
                paired_secret_hex,
                last_authenticated_unix_ms: timestamp,
            });
        }
        self.save()
    }

    pub fn mark_authenticated(&mut self, device_id: &str, device_name: &str) -> Result<()> {
        if let Some(record) = self
            .data
            .paired_androids
            .iter_mut()
            .find(|record| record.device_id == device_id)
        {
            record.device_name = device_name.to_owned();
            record.last_authenticated_unix_ms = now_unix_ms();
            self.save()?;
        }
        Ok(())
    }

    fn save(&self) -> Result<()> {
        if let Some(parent) = self.path.parent()
            && !parent.as_os_str().is_empty()
        {
            fs::create_dir_all(parent)
                .with_context(|| format!("create desktop trust dir {}", parent.display()))?;
        }
        let raw = serde_json::to_string_pretty(&self.data)?;
        fs::write(&self.path, raw)
            .with_context(|| format!("write desktop trust store {}", self.path.display()))
    }
}

fn default_trust_path() -> PathBuf {
    if let Ok(path) = std::env::var("ANDROIDCONNECT_DESKTOP_TRUST_PATH")
        && !path.trim().is_empty()
    {
        return PathBuf::from(path);
    }
    if let Ok(path) = std::env::var("XDG_STATE_HOME")
        && !path.trim().is_empty()
    {
        return PathBuf::from(path)
            .join("androidconnect")
            .join("desktop-trust.json");
    }
    if let Ok(home) = std::env::var("HOME")
        && !home.trim().is_empty()
    {
        return PathBuf::from(home)
            .join(".local")
            .join("state")
            .join("androidconnect")
            .join("desktop-trust.json");
    }
    PathBuf::from("androidconnect-desktop-trust.json")
}

fn default_desktop_name() -> String {
    std::env::var("HOSTNAME")
        .ok()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| "Desktop".to_owned())
}

fn generate_hex_id(bytes: usize) -> String {
    let mut raw = vec![0_u8; bytes];
    if fill_random(&mut raw).is_err() {
        fill_fallback_random(&mut raw);
    }
    bytes_to_hex(&raw)
}

fn fill_random(output: &mut [u8]) -> std::io::Result<()> {
    use std::io::Read;

    let mut file = fs::File::open("/dev/urandom")?;
    file.read_exact(output)
}

fn fill_fallback_random(output: &mut [u8]) {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or_default();
    let mut seed = now ^ ((std::process::id() as u128) << 64);
    for chunk in output.chunks_mut(8) {
        seed ^= seed << 7;
        seed ^= seed >> 9;
        seed = seed.wrapping_mul(0x9e37_79b9_7f4a_7c15);
        let bytes = seed.to_le_bytes();
        chunk.copy_from_slice(&bytes[..chunk.len()]);
    }
}

fn now_unix_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis() as u64)
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn trust_store_persists_identity_and_pairing() -> Result<()> {
        let path = std::env::temp_dir().join(format!(
            "androidconnect-desktop-trust-test-{}-{}.json",
            std::process::id(),
            now_unix_ms()
        ));
        let mut store = TrustStore::load_or_create_at(&path)?;
        let identity = store.identity();
        let secret = [3_u8; PAIRED_SECRET_BYTES];

        store.store_pairing("android-1", "Pixel", secret)?;
        drop(store);

        let store = TrustStore::load_or_create_at(&path)?;
        assert_eq!(store.identity(), identity);
        assert_eq!(
            store.paired_android("android-1"),
            Some(PairedAndroid {
                device_id: "android-1".to_owned(),
                device_name: "Pixel".to_owned(),
                paired_secret: secret,
            })
        );

        let _ = fs::remove_file(path);
        Ok(())
    }
}
