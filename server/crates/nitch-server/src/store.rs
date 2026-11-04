//! SQLite storage: a key/value table for JSON documents (the login) and the multiview sessions.

use std::path::Path;
use std::sync::Mutex;

use anyhow::Result;
use rusqlite::{Connection, OptionalExtension, Row, params};
use serde::Serialize;
use serde::de::DeserializeOwned;

use crate::sessions::{Session, Tile};

fn session_from_row(r: &Row) -> rusqlite::Result<(Session, String)> {
    Ok((
        Session { id: r.get(0)?, name: r.get(1)?, tiles: Vec::new(), created_at: r.get(3)?, updated_at: r.get(4)? },
        r.get(2)?,
    ))
}

fn with_tiles((mut s, tiles): (Session, String)) -> Result<Session> {
    s.tiles = serde_json::from_str::<Vec<Tile>>(&tiles)?;
    Ok(s)
}

pub struct Store {
    conn: Mutex<Connection>,
}

impl Store {
    pub fn open(path: &Path) -> Result<Self> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let conn = Connection::open(path)?;
        // Exclusive before WAL: then WAL needs no shared-memory file, which doesn't work on NFS
        // (the data lives on a NAS, D32). Only this process opens the database; a second server
        // on the same data fails right away instead of corrupting it.
        conn.execute_batch(
            "PRAGMA locking_mode = EXCLUSIVE;
             PRAGMA journal_mode = WAL;
             CREATE TABLE IF NOT EXISTS kv (key TEXT PRIMARY KEY, value TEXT NOT NULL);
             CREATE TABLE IF NOT EXISTS sessions (
                 id TEXT PRIMARY KEY,
                 name TEXT NOT NULL,
                 tiles TEXT NOT NULL,
                 created_at INTEGER NOT NULL,
                 updated_at INTEGER NOT NULL
             );",
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

    /// All sessions, most recently changed first.
    pub fn sessions(&self) -> Result<Vec<Session>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt =
            conn.prepare("SELECT id, name, tiles, created_at, updated_at FROM sessions ORDER BY updated_at DESC, name")?;
        let rows = stmt.query_map([], session_from_row)?.collect::<rusqlite::Result<Vec<_>>>()?;
        rows.into_iter().map(with_tiles).collect()
    }

    pub fn session(&self, id: &str) -> Result<Option<Session>> {
        let conn = self.conn.lock().unwrap();
        let row = conn
            .query_row(
                "SELECT id, name, tiles, created_at, updated_at FROM sessions WHERE id = ?1",
                params![id],
                session_from_row,
            )
            .optional()?;
        row.map(with_tiles).transpose()
    }

    /// Inserts or replaces a session.
    pub fn save_session(&self, s: &Session) -> Result<()> {
        self.conn.lock().unwrap().execute(
            "INSERT INTO sessions (id, name, tiles, created_at, updated_at) VALUES (?1, ?2, ?3, ?4, ?5)
             ON CONFLICT(id) DO UPDATE SET name = excluded.name, tiles = excluded.tiles, updated_at = excluded.updated_at",
            params![s.id, s.name, serde_json::to_string(&s.tiles)?, s.created_at, s.updated_at],
        )?;
        Ok(())
    }

    /// `false` if there was no such session.
    pub fn delete_session(&self, id: &str) -> Result<bool> {
        Ok(self.conn.lock().unwrap().execute("DELETE FROM sessions WHERE id = ?1", params![id])? > 0)
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

    #[test]
    fn sessions_roundtrip() {
        use crate::sessions::{Fit, Tile};
        let dir = std::env::temp_dir().join(format!("nitch-sessions-test-{}", std::process::id()));
        let store = Store::open(&dir.join("t.db")).unwrap();
        let tile = Tile { channel: "tarik".into(), x: 0, y: 0, w: 30, h: 30, z: 1, fit: Fit::Cover };
        let mut s = Session { id: "a".into(), name: "Evening".into(), tiles: vec![tile], created_at: 1, updated_at: 1 };
        store.save_session(&s).unwrap();
        s.name = "Late".into();
        s.updated_at = 2;
        store.save_session(&s).unwrap();
        let loaded = store.session("a").unwrap().unwrap();
        assert_eq!((loaded.name.as_str(), loaded.created_at, loaded.tiles.len()), ("Late", 1, 1));
        assert_eq!(loaded.tiles[0].fit, Fit::Cover);
        assert_eq!(store.sessions().unwrap().len(), 1);
        assert!(store.delete_session("a").unwrap());
        assert!(!store.delete_session("a").unwrap());
        let _ = std::fs::remove_dir_all(dir);
    }
}
