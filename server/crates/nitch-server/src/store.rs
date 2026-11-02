//! SQLite storage. For now a small key/value table holding JSON documents (the login);
//! multiview sessions get their own table in M4.

use std::path::Path;
use std::sync::Mutex;

use anyhow::Result;
use rusqlite::{Connection, OptionalExtension, params};
use serde::Serialize;
use serde::de::DeserializeOwned;

pub struct Store {
    conn: Mutex<Connection>,
}

impl Store {
    pub fn open(path: &Path) -> Result<Self> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let conn = Connection::open(path)?;
        conn.execute_batch(
            "PRAGMA journal_mode = WAL;
             CREATE TABLE IF NOT EXISTS kv (key TEXT PRIMARY KEY, value TEXT NOT NULL);",
        )?;
        Ok(Self { conn: Mutex::new(conn) })
    }

    pub fn get<T: DeserializeOwned>(&self, key: &str) -> Result<Option<T>> {
        let conn = self.conn.lock().unwrap();
        let value: Option<String> =
            conn.query_row("SELECT value FROM kv WHERE key = ?1", params![key], |r| r.get(0)).optional()?;
        Ok(value.map(|v| serde_json::from_str(&v)).transpose()?)
    }

    pub fn set<T: Serialize>(&self, key: &str, value: &T) -> Result<()> {
        let json = serde_json::to_string(value)?;
        self.conn.lock().unwrap().execute(
            "INSERT INTO kv (key, value) VALUES (?1, ?2) ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            params![key, json],
        )?;
        Ok(())
    }

    pub fn delete(&self, key: &str) -> Result<()> {
        self.conn.lock().unwrap().execute("DELETE FROM kv WHERE key = ?1", params![key])?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip() {
        let dir = std::env::temp_dir().join(format!("nitch-store-test-{}", std::process::id()));
        let store = Store::open(&dir.join("t.db")).unwrap();
        assert_eq!(store.get::<String>("a").unwrap(), None);
        store.set("a", &"x".to_string()).unwrap();
        store.set("a", &"y".to_string()).unwrap();
        assert_eq!(store.get::<String>("a").unwrap().as_deref(), Some("y"));
        store.delete("a").unwrap();
        assert_eq!(store.get::<String>("a").unwrap(), None);
        let _ = std::fs::remove_dir_all(dir);
    }
}
