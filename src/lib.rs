//! # TinyKV
//!
//! A minimal JSON-based persistent key-value store for Rust projects.
//! Supports optional TTL (expiration), auto-saving, backup, and serialization via `serde`.
//!
//! ## Features
//! - File-based storage using pretty-formatted JSON
//! - Optional TTL expiration support per key
//! - Auto-saving changes on modification
//! - Backup support with `.bak` files
//! - Simple interface with `serde` for value types
//!
//! ## Example
//!
//! ```rust
//! use tinykv::TinyKV;
//!
//! fn main() -> Result<(), Box<dyn std::error::Error>> {
//!     let mut kv = TinyKV::open("mydata.json")?.with_auto_save();
//!     kv.set("username", "hasan")?;
//!     kv.set_with_ttl("session_token", "abc123", 60)?; // 60 seconds TTL
//!     let user: Option<String> = kv.get("username")?;
//!     println!("User: {:?}", user);
//!     Ok(())
//! }
//! ```

use std::collections::HashMap;
use std::fs;
use std::io::{self, ErrorKind};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

/// Errors that can occur while using the TinyKV store.
#[derive(Debug)]
pub enum TinyKVError {
    /// File system related error
    Io(io::Error),
    /// Serialization or deserialization failure
    Serialization(serde_json::Error),
    /// System time is before the UNIX epoch
    TimeError,
}

impl From<io::Error> for TinyKVError {
    fn from(err: io::Error) -> Self {
        Self::Io(err)
    }
}

impl From<serde_json::Error> for TinyKVError {
    fn from(err: serde_json::Error) -> Self {
        Self::Serialization(err)
    }
}

impl std::fmt::Display for TinyKVError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(e) => write!(f, "IO error: {e}"),
            Self::Serialization(e) => write!(f, "Serialization error: {e}"),
            Self::TimeError => write!(f, "Time error"),
        }
    }
}

impl std::error::Error for TinyKVError {}

#[derive(Serialize, Deserialize, Debug, Clone)]
struct Entry {
    value: serde_json::Value,
    #[serde(default)]
    expires_at: Option<u64>, // UNIX timestamp (seconds)
}

/// A simple persistent key-value store with TTL and auto-save.
///
/// Values are stored in JSON format and must implement `serde::Serialize` and `serde::Deserialize`.
pub struct TinyKV {
    path: PathBuf,
    data: HashMap<String, Entry>,
    auto_save: bool,
    backup_enabled: bool,
}

impl TinyKV {
    /// Open or create a TinyKV store at the given file path.
    pub fn open<P: AsRef<Path>>(path: P) -> Result<Self, TinyKVError> {
        let path_buf = path.as_ref().to_path_buf();
        let data = match fs::read_to_string(&path_buf) {
            Ok(contents) => serde_json::from_str(&contents)
                .map_err(|e| TinyKVError::Io(io::Error::new(ErrorKind::InvalidData, e)))?,
            Err(e) if e.kind() == ErrorKind::NotFound => HashMap::new(),
            Err(e) => return Err(TinyKVError::Io(e)),
        };

        Ok(Self {
            path: path_buf,
            data,
            auto_save: false,
            backup_enabled: false,
        })
    }

    /// Enables auto-saving after every set/remove operation.
    pub fn with_auto_save(mut self) -> Self {
        self.auto_save = true;
        self
    }

    /// Enables or disables file backup before saving.
    pub fn with_backup(mut self, enabled: bool) -> Self {
        self.backup_enabled = enabled;
        self
    }

    /// Inserts a key with a value (without expiration).
    pub fn set<T: Serialize>(&mut self, key: &str, value: T) -> Result<(), TinyKVError> {
        let val = serde_json::to_value(value)?;
        self.data.insert(
            key.to_string(),
            Entry {
                value: val,
                expires_at: None,
            },
        );

        if self.auto_save {
            self.save()?;
        }
        Ok(())
    }

    /// Inserts a key with value and expiration (TTL in seconds).
    pub fn set_with_ttl<T: Serialize>(
        &mut self,
        key: &str,
        value: T,
        ttl_secs: u64,
    ) -> Result<(), TinyKVError> {
        let val = serde_json::to_value(value)?;
        let expires_at = Some(
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map_err(|_| TinyKVError::TimeError)?
                .as_secs()
                + ttl_secs,
        );

        self.data.insert(
            key.to_string(),
            Entry {
                value: val,
                expires_at,
            },
        );

        if self.auto_save {
            self.save()?;
        }
        Ok(())
    }

    /// Retrieves the value for a given key if it exists and hasn't expired.
    pub fn get<T: for<'de> Deserialize<'de>>(
        &mut self,
        key: &str,
    ) -> Result<Option<T>, TinyKVError> {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|_| TinyKVError::TimeError)?
            .as_secs();

        if let Some(entry) = self.data.get(key) {
            if let Some(expiry) = entry.expires_at
                && now > expiry
            {
                self.data.remove(key);
                if self.auto_save {
                    self.save()?;
                }
                return Ok(None);
            }

            let value = serde_json::from_value(entry.value.clone())?;
            return Ok(Some(value));
        }

        Ok(None)
    }

    /// Removes a key from the store.
    pub fn remove(&mut self, key: &str) -> Result<bool, TinyKVError> {
        let removed = self.data.remove(key).is_some();
        if removed && self.auto_save {
            self.save()?;
        }
        Ok(removed)
    }

    /// Checks if the store contains a given key and it's not expired.
    pub fn contains_key(&self, key: &str) -> bool {
        if let Some(entry) = self.data.get(key) {
            if let Some(expiry) = entry.expires_at {
                let now = SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .map(|d| d.as_secs())
                    .unwrap_or(0);
                return now <= expiry;
            }
            return true;
        }
        false
    }

    /// Returns a list of all unexpired keys in the store.
    pub fn keys(&self) -> Vec<String> {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);

        self.data
            .iter()
            .filter(|(_, entry)| match entry.expires_at {
                Some(expiry) => now <= expiry,
                None => true,
            })
            .map(|(k, _)| k.clone())
            .collect()
    }

    /// Returns number of unexpired entries.
    pub fn len(&self) -> usize {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);

        self.data
            .iter()
            .filter(|(_, entry)| match entry.expires_at {
                Some(expiry) => now <= expiry,
                None => true,
            })
            .count()
    }

    /// Returns true if the store is empty.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Save contents to disk. Creates a `.bak` file if backup is enabled.
    pub fn save(&self) -> Result<(), TinyKVError> {
        if self.backup_enabled && self.path.exists() {
            let backup_path = self.path.with_extension("bak");
            fs::copy(&self.path, &backup_path)?;
        }

        let json = serde_json::to_string_pretty(&self.data)?;
        let temp_path = self.path.with_extension("tmp");
        fs::write(&temp_path, json)?;
        fs::rename(&temp_path, &self.path)?;

        Ok(())
    }

    /// Removes all expired entries from memory.
    pub fn purge_expired(&mut self) -> Result<usize, TinyKVError> {
        if self.data.is_empty() {
            return Ok(0);
        }

        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|_| TinyKVError::TimeError)?
            .as_secs();

        let before = self.data.len();
        self.data.retain(|_, entry| match entry.expires_at {
            Some(expiry) => now <= expiry,
            None => true,
        });

        let removed = before - self.data.len();

        if removed > 0 && self.auto_save {
            self.save()?;
        }

        Ok(removed)
    }

    /// Clears all entries from memory.
    pub fn clear(&mut self) -> Result<(), TinyKVError> {
        self.data.clear();
        if self.auto_save {
            self.save()?;
        }
        Ok(())
    }

    /// Reloads the store contents from disk.
    pub fn reload(&mut self) -> Result<(), TinyKVError> {
        let data = match fs::read_to_string(&self.path) {
            Ok(contents) => serde_json::from_str(&contents)
                .map_err(|e| TinyKVError::Io(io::Error::new(ErrorKind::InvalidData, e)))?,
            Err(e) if e.kind() == ErrorKind::NotFound => HashMap::new(),
            Err(e) => return Err(TinyKVError::Io(e)),
        };

        self.data = data;
        Ok(())
    }
}

impl Drop for TinyKV {
    fn drop(&mut self) {
        if self.auto_save {
            let _ = self.save();
        }
    }
}
