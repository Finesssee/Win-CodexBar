//! CodexBar-owned SQLite sidecar for Workspaces snapshots.
//!
//! Never attaches to or writes Codex's `state_5.sqlite`. Schema version 5 and
//! payload format 3 match upstream; foreign `user_version` values are refused.

use std::fs;
use std::path::{Path, PathBuf};

use rusqlite::{Connection, OpenFlags, OptionalExtension, params};

use super::types::CodexLocalProjectUsageSnapshot;

pub const SCHEMA_VERSION: i32 = 5;
pub const PAYLOAD_FORMAT_VERSION: i32 = 3;

#[derive(Debug, thiserror::Error)]
pub enum SidecarError {
    #[error("workspaces sidecar path unavailable")]
    PathUnavailable,
    #[error("workspaces sidecar open failed: {0}")]
    Open(String),
    #[error(
        "workspaces sidecar schema incompatible (user_version={found}, expected {SCHEMA_VERSION})"
    )]
    Incompatible { found: i32 },
    #[error("workspaces sidecar error: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error("workspaces sidecar encode/decode failed: {0}")]
    Serde(#[from] serde_json::Error),
    #[error("workspaces sidecar io failed: {0}")]
    Io(#[from] std::io::Error),
}

/// Persistence for complete workspaces snapshots.
#[derive(Debug, Clone)]
pub struct WorkspaceUsageSidecar {
    path: PathBuf,
}

impl WorkspaceUsageSidecar {
    pub fn new(path: PathBuf) -> Self {
        Self { path }
    }

    /// Default: `%LOCALAPPDATA%\CodexBar\local-usage\codex-workspaces-v1.sqlite`.
    pub fn default_path() -> Option<PathBuf> {
        dirs::data_local_dir().map(|root| {
            root.join("CodexBar")
                .join("local-usage")
                .join("codex-workspaces-v1.sqlite")
        })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn load_latest_snapshot(
        &self,
        scope_signature: &str,
        history_days: u32,
    ) -> Result<Option<CodexLocalProjectUsageSnapshot>, SidecarError> {
        if !self.path.exists() {
            return Ok(None);
        }
        let conn = self.open(true)?;
        let row: Option<(Vec<u8>, i32)> = conn
            .query_row(
                "SELECT payload, payload_format_version
                 FROM snapshot_payloads
                 WHERE scope_signature = ?1
                   AND history_days = ?2
                   AND is_complete = 1
                 ORDER BY updated_at_ms DESC
                 LIMIT 1",
                params![scope_signature, history_days as i64],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?;
        let Some((payload, format_version)) = row else {
            return Ok(None);
        };
        if format_version != PAYLOAD_FORMAT_VERSION {
            return Ok(None);
        }
        let snapshot = serde_json::from_slice(&payload)?;
        Ok(Some(snapshot))
    }

    /// Atomically replace the complete snapshot for `(scope, history_days)`.
    pub fn publish_snapshot(
        &self,
        snapshot: &CodexLocalProjectUsageSnapshot,
    ) -> Result<(), SidecarError> {
        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent)?;
        }
        let conn = self.open(false)?;
        let payload = serde_json::to_vec(snapshot)?;
        let updated_at_ms = snapshot.updated_at.timestamp_millis();
        let tx = conn.unchecked_transaction()?;
        tx.execute(
            "INSERT INTO snapshot_payloads (
                scope_signature, history_days, updated_at_ms, is_complete,
                payload_format_version, payload
             ) VALUES (?1, ?2, ?3, 1, ?4, ?5)
             ON CONFLICT(scope_signature, history_days) DO UPDATE SET
                updated_at_ms = excluded.updated_at_ms,
                is_complete = 1,
                payload_format_version = excluded.payload_format_version,
                payload = excluded.payload",
            params![
                snapshot.scope_signature,
                snapshot.history_days as i64,
                updated_at_ms,
                PAYLOAD_FORMAT_VERSION,
                payload,
            ],
        )?;
        tx.commit()?;
        Ok(())
    }

    pub fn clear_snapshots(&self) -> Result<(), SidecarError> {
        if !self.path.exists() {
            return Ok(());
        }
        let conn = self.open(false)?;
        conn.execute("DELETE FROM snapshot_payloads", [])?;
        Ok(())
    }

    fn open(&self, read_only: bool) -> Result<Connection, SidecarError> {
        if read_only {
            let conn = Connection::open_with_flags(
                &self.path,
                OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
            )
            .map_err(|e| SidecarError::Open(e.to_string()))?;
            conn.busy_timeout(std::time::Duration::from_millis(250))?;
            Self::assert_compatible(&conn)?;
            return Ok(conn);
        }

        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent)?;
        }
        let conn = Connection::open_with_flags(
            &self.path,
            OpenFlags::SQLITE_OPEN_READ_WRITE
                | OpenFlags::SQLITE_OPEN_CREATE
                | OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )
        .map_err(|e| SidecarError::Open(e.to_string()))?;
        conn.busy_timeout(std::time::Duration::from_millis(250))?;
        Self::ensure_schema(&conn)?;
        Ok(conn)
    }

    fn assert_compatible(conn: &Connection) -> Result<(), SidecarError> {
        let version: i32 = conn.pragma_query_value(None, "user_version", |r| r.get(0))?;
        if version == 0 || version == SCHEMA_VERSION {
            Ok(())
        } else {
            Err(SidecarError::Incompatible { found: version })
        }
    }

    fn ensure_schema(conn: &Connection) -> Result<(), SidecarError> {
        let version: i32 = conn.pragma_query_value(None, "user_version", |r| r.get(0))?;
        if version != 0 && version != SCHEMA_VERSION {
            return Err(SidecarError::Incompatible { found: version });
        }
        if version == 0 {
            conn.execute_batch(
                "CREATE TABLE IF NOT EXISTS snapshot_payloads (
                    scope_signature TEXT NOT NULL,
                    history_days INTEGER NOT NULL,
                    updated_at_ms INTEGER NOT NULL,
                    is_complete INTEGER NOT NULL,
                    payload_format_version INTEGER NOT NULL,
                    payload BLOB NOT NULL,
                    PRIMARY KEY (scope_signature, history_days)
                );
                PRAGMA user_version = 5;",
            )?;
        }
        Ok(())
    }
}
