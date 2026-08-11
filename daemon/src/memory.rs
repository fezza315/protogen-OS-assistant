//! memory.rs
//! ---------
//! Persistent "memory bank": once a plan has been researched, shown to the
//! user, and approved, its (normalized request text -> Plan) mapping is
//! cached here so the next time the user asks for the same thing, it skips
//! research and goes straight to a (still-shown, still-confirmable-if
//! system-changing) plan.
//!
//! Two tables:
//!   - `phrases` : known trigger phrases -> intent id (fast path, replaces
//!     the old commands.json keyword list but now user/LLM-extensible)
//!   - `plans`   : intent id -> serialized Plan (the actual steps)
//!
//! Nothing here executes anything. This module only reads/writes SQLite.
//!
//! `rusqlite::Connection` is neither `Sync` (it uses interior mutability
//! for statement caching) nor safe to share across threads without
//! synchronization, and `Dispatcher` (which owns a `MemoryBank`) is held
//! behind an `Arc` and used across `.await` points in tokio tasks, so the
//! connection is wrapped in a plain `std::sync::Mutex`. Locks here are
//! held only for the duration of a single synchronous sqlite call -- never
//! across an `.await` -- so a std (not tokio) Mutex is the right/cheaper
//! choice.
use anyhow::{Context, Result};
use rusqlite::{params, Connection};
use std::path::Path;
use std::sync::Mutex;

use protogen_plan::Plan;

pub struct MemoryBank {
    conn: Mutex<Connection>,
}

impl MemoryBank {
    pub fn open(db_path: &Path) -> Result<Self> {
        if let Some(parent) = db_path.parent() {
            std::fs::create_dir_all(parent).ok();
        }
        let conn = Connection::open(db_path).context("opening memory bank db")?;
        conn.execute_batch(
            r#"
            CREATE TABLE IF NOT EXISTS intents (
                id            TEXT PRIMARY KEY,
                created_at    TEXT NOT NULL,
                plan_json     TEXT NOT NULL,
                use_count     INTEGER NOT NULL DEFAULT 0,
                last_used_at  TEXT
            );
            CREATE TABLE IF NOT EXISTS phrases (
                phrase     TEXT PRIMARY KEY,
                intent_id  TEXT NOT NULL REFERENCES intents(id) ON DELETE CASCADE
            );
            CREATE INDEX IF NOT EXISTS idx_phrases_intent ON phrases(intent_id);
            "#,
        )?;
        Ok(Self { conn: Mutex::new(conn) })
    }

    /// Normalize free text into a lookup key: lowercase, trimmed,
    /// whitespace-collapsed. Deliberately simple/deterministic -- fuzzy
    /// matching happens one layer up in the daemon's dispatcher.
    pub fn normalize(text: &str) -> String {
        text.to_lowercase()
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
    }

    pub fn lookup_phrase(&self, phrase: &str) -> Result<Option<Plan>> {
        let norm = Self::normalize(phrase);
        let conn = self.conn.lock().expect("memory bank mutex poisoned");
        let mut stmt = conn.prepare(
            "SELECT i.plan_json FROM phrases p JOIN intents i ON i.id = p.intent_id WHERE p.phrase = ?1",
        )?;
        let mut rows = stmt.query(params![norm])?;
        if let Some(row) = rows.next()? {
            let json: String = row.get(0)?;
            let plan: Plan = serde_json::from_str(&json)?;
            drop(rows);
            drop(stmt);
            Self::bump_usage_by_phrase(&conn, &norm)?;
            return Ok(Some(plan));
        }
        Ok(None)
    }

    /// Not called yet -- reserved for a future "what do you remember"
    /// introspection command.
    #[allow(dead_code)]
    pub fn all_phrases(&self) -> Result<Vec<String>> {
        let conn = self.conn.lock().expect("memory bank mutex poisoned");
        let mut stmt = conn.prepare("SELECT phrase FROM phrases")?;
        let rows = stmt.query_map([], |r| r.get::<_, String>(0))?;
        Ok(rows.filter_map(|r| r.ok()).collect())
    }

    /// Store a newly-approved plan under one or more trigger phrases so
    /// future identical/similar requests skip research entirely.
    pub fn remember(&self, phrases: &[String], plan: &Plan) -> Result<()> {
        let now = chrono::Utc::now().to_rfc3339();
        let plan_json = serde_json::to_string(plan)?;
        let conn = self.conn.lock().expect("memory bank mutex poisoned");
        conn.execute(
            "INSERT INTO intents (id, created_at, plan_json, use_count) VALUES (?1, ?2, ?3, 0)
             ON CONFLICT(id) DO UPDATE SET plan_json = excluded.plan_json",
            params![plan.id, now, plan_json],
        )?;
        for phrase in phrases {
            let norm = Self::normalize(phrase);
            conn.execute(
                "INSERT OR REPLACE INTO phrases (phrase, intent_id) VALUES (?1, ?2)",
                params![norm, plan.id],
            )?;
        }
        Ok(())
    }

    fn bump_usage_by_phrase(conn: &Connection, norm_phrase: &str) -> Result<()> {
        conn.execute(
            "UPDATE intents SET use_count = use_count + 1, last_used_at = ?1
             WHERE id = (SELECT intent_id FROM phrases WHERE phrase = ?2)",
            params![chrono::Utc::now().to_rfc3339(), norm_phrase],
        )?;
        Ok(())
    }

    pub fn forget(&self, phrase: &str) -> Result<bool> {
        let norm = Self::normalize(phrase);
        let conn = self.conn.lock().expect("memory bank mutex poisoned");
        let n = conn.execute("DELETE FROM phrases WHERE phrase = ?1", params![norm])?;
        Ok(n > 0)
    }
}
