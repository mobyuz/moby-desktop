//! Where a pairing lives. The key goes to Windows Credential Manager through the vault in
//! `api.rs`; the `devices` table gets the device ID and a name, nothing else. The key is never in
//! SQLite, the activity log, a log line or the webview.
//!
//! Pairings belong to the Windows user, not to a Moby account: the robot is plugged into this
//! computer, and Credential Manager and the history file are already per Windows user. Signing
//! out or deleting an account does not touch them (ADR 0006).

use moby_protocol::StableId;
use moby_protocol::usb::PairKey;
use rusqlite::params;
use tauri::{AppHandle, Manager};

use crate::api::{vault_clear, vault_get, vault_set};
use crate::store::{Db, Store, StoreError, now_ms};

/// A storage failure. It carries no detail on purpose: details of a vault or database failure
/// could name the thing being stored.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Fault;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeviceRecord {
    pub id: String,
    pub name: String,
    pub created_at: i64,
}

pub trait PairingStore: Send + Sync {
    /// The key stored for a robot, if there is one.
    fn key(&self, device: &StableId) -> Result<Option<PairKey>, Fault>;
    /// Stores the key and the device record together. If the second part fails the first is undone.
    fn save(&self, device: &StableId, name: &str, key: &PairKey) -> Result<(), Fault>;
    /// Removes the key and the record. Safe to repeat.
    fn forget(&self, device: &StableId) -> Result<(), Fault>;
    fn list(&self) -> Result<Vec<DeviceRecord>, Fault>;
}

fn vault_name(device: &StableId) -> String {
    format!("robot-key-{}", device.as_str())
}

impl Db {
    fn upsert_device(&self, id: &str, name: &str, now: i64) -> Result<(), StoreError> {
        self.conn.execute(
            "INSERT INTO devices (device_id, name, created_at) VALUES (?1, ?2, ?3)
             ON CONFLICT(device_id) DO UPDATE SET name = excluded.name",
            params![id, name, now],
        )?;
        Ok(())
    }

    fn remove_device(&self, id: &str) -> Result<(), StoreError> {
        self.conn
            .execute("DELETE FROM devices WHERE device_id = ?1", params![id])?;
        Ok(())
    }

    fn devices(&self) -> Result<Vec<DeviceRecord>, StoreError> {
        let mut statement = self
            .conn
            .prepare("SELECT device_id, name, created_at FROM devices ORDER BY created_at, device_id LIMIT 50")?;
        let rows = statement.query_map([], |row| {
            Ok(DeviceRecord {
                id: row.get(0)?,
                name: row.get(1)?,
                created_at: row.get(2)?,
            })
        })?;
        Ok(rows.collect::<Result<_, _>>()?)
    }
}

/// The real storage: Credential Manager and the app's SQLite file.
pub struct SystemPairings {
    app: AppHandle,
}

impl SystemPairings {
    pub fn new(app: AppHandle) -> Self {
        Self { app }
    }

    fn store(&self) -> tauri::State<'_, Store> {
        self.app.state::<Store>()
    }
}

impl PairingStore for SystemPairings {
    fn key(&self, device: &StableId) -> Result<Option<PairKey>, Fault> {
        match vault_get(&vault_name(device)) {
            Ok(Some(text)) => Ok(PairKey::try_from(text).ok()),
            Ok(None) => Ok(None),
            Err(_) => Err(Fault),
        }
    }

    fn save(&self, device: &StableId, name: &str, key: &PairKey) -> Result<(), Fault> {
        let entry = vault_name(device);
        vault_set(&entry, key.as_str()).map_err(|_| Fault)?;
        let saved = self
            .store()
            .with(|db| db.upsert_device(device.as_str(), name, now_ms()));
        if saved.is_err() {
            let _ = vault_clear(&entry);
            return Err(Fault);
        }
        Ok(())
    }

    fn forget(&self, device: &StableId) -> Result<(), Fault> {
        // Both are tried, so one failure does not leave the other behind.
        let vault = vault_clear(&vault_name(device));
        let record = self.store().with(|db| db.remove_device(device.as_str()));
        if vault.is_err() || record.is_err() {
            return Err(Fault);
        }
        Ok(())
    }

    fn list(&self) -> Result<Vec<DeviceRecord>, Fault> {
        self.store().with(|db| db.devices()).map_err(|_| Fault)
    }
}

#[cfg(test)]
pub use memory::MemoryPairings;

#[cfg(test)]
mod memory {
    use std::collections::BTreeMap;
    use std::sync::Mutex;

    use super::*;

    /// Pairings in memory, with switches to make storage fail. Keys print as redacted.
    #[derive(Default, Debug)]
    pub struct MemoryPairings {
        inner: Mutex<Inner>,
    }

    #[derive(Default, Debug)]
    struct Inner {
        keys: BTreeMap<String, PairKey>,
        records: BTreeMap<String, String>,
        fail_save: bool,
        fail_read: bool,
        reads: u32,
    }

    impl MemoryPairings {
        pub fn with_key(device: &str, key: PairKey) -> Self {
            let store = Self::default();
            let id = StableId::new(device).unwrap();
            store.save(&id, "Moby test", &key).unwrap();
            store
        }

        pub fn fail_save(&self, fail: bool) {
            self.inner.lock().unwrap().fail_save = fail;
        }

        pub fn fail_read(&self, fail: bool) {
            self.inner.lock().unwrap().fail_read = fail;
        }

        /// How many times a key was looked up: each one is a Credential Manager read in the app.
        pub fn reads(&self) -> u32 {
            self.inner.lock().unwrap().reads
        }

        pub fn has_key(&self, device: &str) -> bool {
            self.inner.lock().unwrap().keys.contains_key(device)
        }

        pub fn has_record(&self, device: &str) -> bool {
            self.inner.lock().unwrap().records.contains_key(device)
        }

        pub fn key_text(&self, device: &str) -> Option<String> {
            self.inner
                .lock()
                .unwrap()
                .keys
                .get(device)
                .map(|key| key.as_str().to_owned())
        }
    }

    impl PairingStore for MemoryPairings {
        fn key(&self, device: &StableId) -> Result<Option<PairKey>, Fault> {
            let mut inner = self.inner.lock().unwrap();
            inner.reads += 1;
            if inner.fail_read {
                return Err(Fault);
            }
            Ok(inner.keys.get(device.as_str()).cloned())
        }

        fn save(&self, device: &StableId, name: &str, key: &PairKey) -> Result<(), Fault> {
            let mut inner = self.inner.lock().unwrap();
            if inner.fail_save {
                return Err(Fault);
            }
            inner.keys.insert(device.as_str().to_owned(), key.clone());
            inner
                .records
                .insert(device.as_str().to_owned(), name.to_owned());
            Ok(())
        }

        fn forget(&self, device: &StableId) -> Result<(), Fault> {
            let mut inner = self.inner.lock().unwrap();
            inner.keys.remove(device.as_str());
            inner.records.remove(device.as_str());
            Ok(())
        }

        fn list(&self) -> Result<Vec<DeviceRecord>, Fault> {
            let inner = self.inner.lock().unwrap();
            Ok(inner
                .records
                .iter()
                .map(|(id, name)| DeviceRecord {
                    id: id.clone(),
                    name: name.clone(),
                    created_at: 0,
                })
                .collect())
        }
    }
}
