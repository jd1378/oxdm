//! SQLite-backed persistence.
//!
//! `rusqlite` (sync) wrapped in `tokio::task::spawn_blocking`. Single
//! `Mutex<Connection>` — queue mutations are infrequent and writes are
//! tiny, so a connection pool would be over-engineering.

use rusqlite::{Connection, params};
use serde_json::{Map, Value};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use tokio::task::spawn_blocking;

use crate::domain::{Job, JobId, Phase, Queue, QueueHook, QueueId, QueueSchedule, Settings};

/// Schema version. Bump on every breaking change. Migrations are a
/// match on the read-back version; tiny enough to keep DIY.
const SCHEMA_VERSION: i32 = 5;

/// Async-friendly handle around a blocking `rusqlite::Connection`.
#[derive(Clone)]
pub struct Store {
    inner: std::sync::Arc<Mutex<Connection>>,
}

impl Store {
    pub async fn open(path: PathBuf) -> Result<Self, StoreError> {
        // SQLite treats `":memory:"` as an in-memory DB sentinel,
        // not a file path. Skip the directory/perms dance so we
        // don't accidentally create a file literally named
        // `:memory:` in the CWD.
        let is_memory = path.as_os_str() == ":memory:";
        if !is_memory && let Some(parent) = path.parent() {
            tokio::fs::create_dir_all(parent)
                .await
                .map_err(StoreError::Io)?;
        }
        // The DB holds the extension auth token in plaintext — restrict
        // to the owner before SQLite gets a chance to create it with
        // the umask default (0644 on most distros, world-readable).
        // No-op on Windows where NTFS already inherits user-only ACLs
        // inside %APPDATA%.
        #[cfg(unix)]
        if !is_memory {
            use std::os::unix::fs::PermissionsExt;
            if !path.exists() {
                // Touch with 0600 so the create() inside Connection::open
                // doesn't widen later. If touch fails we still proceed —
                // the post-open chmod below is the real guarantee.
                if let Ok(f) = std::fs::File::create(&path) {
                    let _ = f.set_permissions(std::fs::Permissions::from_mode(0o600));
                }
            }
        }
        let path_for_chmod = path.clone();
        let conn = spawn_blocking(move || Connection::open(&path))
            .await
            .map_err(|e| StoreError::Other(e.to_string()))??;
        #[cfg(unix)]
        if !is_memory {
            use std::os::unix::fs::PermissionsExt;
            // Apply to the main file + sidecars SQLite created (WAL/SHM).
            for ext in ["", "-wal", "-shm"] {
                let mut p = path_for_chmod.clone().into_os_string();
                p.push(ext);
                let _ = std::fs::set_permissions(
                    std::path::PathBuf::from(p),
                    std::fs::Permissions::from_mode(0o600),
                );
            }
        }
        #[cfg(not(unix))]
        let _ = path_for_chmod;
        let store = Self {
            inner: std::sync::Arc::new(Mutex::new(conn)),
        };
        store.migrate().await?;
        Ok(store)
    }

    async fn migrate(&self) -> Result<(), StoreError> {
        let stored_version = self
            .with_conn(|conn| {
                conn.execute_batch(
                    "PRAGMA journal_mode=WAL;
                 PRAGMA foreign_keys=ON;
                 CREATE TABLE IF NOT EXISTS schema_version (version INTEGER NOT NULL);
                 CREATE TABLE IF NOT EXISTS settings (
                     key   TEXT PRIMARY KEY,
                     value TEXT NOT NULL
                 );
                 CREATE TABLE IF NOT EXISTS queues (
                     id              TEXT PRIMARY KEY,
                     name            TEXT NOT NULL,
                     builtin         INTEGER NOT NULL DEFAULT 0,
                     schedule_json   TEXT NOT NULL DEFAULT '{\"kind\":\"manual\"}',
                     on_start_json   TEXT NOT NULL DEFAULT '[]',
                     on_finish_json  TEXT NOT NULL DEFAULT '[]',
                     max_concurrent  INTEGER,
                     stop_on_error   INTEGER NOT NULL DEFAULT 0,
                     position        INTEGER NOT NULL DEFAULT 0,
                     color           INTEGER
                 );
                 CREATE TABLE IF NOT EXISTS jobs (
                     id                    TEXT PRIMARY KEY,
                     url                   TEXT NOT NULL,
                     save_dir              TEXT NOT NULL,
                     filename              TEXT,
                     referrer              TEXT,
                     headers_json          TEXT NOT NULL DEFAULT '{}',
                     max_connections       INTEGER,
                     speed_limit_override  INTEGER,
                     queue_id              TEXT NOT NULL,
                     queue_position        INTEGER NOT NULL DEFAULT 0,
                     phase                 TEXT NOT NULL,
                     downloaded            INTEGER NOT NULL DEFAULT 0,
                     total                 INTEGER,
                     final_path            TEXT,
                     proxy                 TEXT,
                     auth_user             TEXT,
                     auth_password_enc     TEXT,
                     proxy_password_enc    TEXT,
                     cookies_enc           TEXT,
                     created_at            TEXT NOT NULL,
                     advanced_json         TEXT NOT NULL DEFAULT '{}',
                     checksums_json        TEXT NOT NULL DEFAULT '[]',
                     category              TEXT NOT NULL DEFAULT '\"other\"',
                     started_at            TEXT,
                     finished_at           TEXT,
                     retries               INTEGER NOT NULL DEFAULT 0,
                     response_headers_json TEXT NOT NULL DEFAULT 'null',
                     error_json            TEXT NOT NULL DEFAULT 'null',
                     FOREIGN KEY(queue_id) REFERENCES queues(id) ON DELETE RESTRICT
                 );
                 CREATE INDEX IF NOT EXISTS idx_jobs_queue ON jobs(queue_id, queue_position);
                 CREATE INDEX IF NOT EXISTS idx_jobs_filename
                   ON jobs(filename) WHERE filename IS NOT NULL;
",
                )?;
                let v: Option<i32> = conn
                    .query_row("SELECT version FROM schema_version LIMIT 1", [], |r| {
                        r.get(0)
                    })
                    .ok();
                if v.is_none() {
                    conn.execute(
                        "INSERT INTO schema_version (version) VALUES (?1)",
                        params![SCHEMA_VERSION],
                    )?;
                }
                Ok::<_, rusqlite::Error>(v)
            })
            .await
            .map_err(StoreError::Sql)?;

        // Forward migrations are applied in-place. A version newer than
        // we know about, or a version we have no migration path for,
        // surfaces as `StoreError::Corrupt` so `AppState::load` falls
        // into the same in-memory recovery branch that file corruption
        // / sqlite IO errors take, and the GUI offers Exit / Reset.
        if let Some(mut n) = stored_version {
            while n < SCHEMA_VERSION {
                let next = n + 1;
                match next {
                    2 => {
                        // v2: per-job Advanced / Checksums blobs. New
                        // columns default to empty JSON so existing
                        // rows hydrate to the Advanced::default() /
                        // empty Vec<Checksum> shape on next load.
                        self.with_conn(|conn| {
                            conn.execute_batch(
                                "ALTER TABLE jobs ADD COLUMN advanced_json TEXT NOT NULL DEFAULT '{}';
                                 ALTER TABLE jobs ADD COLUMN checksums_json TEXT NOT NULL DEFAULT '[]';",
                            )
                        })
                        .await
                        .map_err(StoreError::Sql)?;
                    }
                    3 => {
                        // v3: per-job run stats. `started_at` /
                        // `finished_at` are nullable RFC3339 timestamps
                        // (NULL = not yet started / finished);
                        // `retries` counts PartRetrying events and
                        // defaults to 0 so existing rows hydrate clean.
                        self.with_conn(|conn| {
                            conn.execute_batch(
                                "ALTER TABLE jobs ADD COLUMN started_at TEXT;
                                 ALTER TABLE jobs ADD COLUMN finished_at TEXT;
                                 ALTER TABLE jobs ADD COLUMN retries INTEGER NOT NULL DEFAULT 0;",
                            )
                        })
                        .await
                        .map_err(StoreError::Sql)?;
                    }
                    4 => {
                        // v4: captured response headers from the last
                        // evaluate probe. `'null'` hydrates to
                        // `captured_response: None` — "never probed",
                        // which is what an existing row means.
                        self.with_conn(|conn| {
                            conn.execute_batch(
                                "ALTER TABLE jobs ADD COLUMN response_headers_json TEXT NOT NULL DEFAULT 'null';",
                            )
                        })
                        .await
                        .map_err(StoreError::Sql)?;
                    }
                    5 => {
                        // v5: why the last run failed. `'null'`
                        // hydrates to `status.error: None` — an
                        // existing failed row simply has no reason
                        // recorded, which is what it meant before.
                        self.with_conn(|conn| {
                            conn.execute_batch(
                                "ALTER TABLE jobs ADD COLUMN error_json TEXT NOT NULL DEFAULT 'null';",
                            )
                        })
                        .await
                        .map_err(StoreError::Sql)?;
                    }
                    _ => {
                        return Err(StoreError::Corrupt(format!(
                            "schema_version {n} on disk, expected {SCHEMA_VERSION}, no migration to {next}"
                        )));
                    }
                }
                n = next;
            }
            if n > SCHEMA_VERSION {
                return Err(StoreError::Corrupt(format!(
                    "schema_version {n} on disk is newer than this build ({SCHEMA_VERSION})"
                )));
            }
            // Bump the recorded version. UPDATE is fine because the row
            // was inserted on first boot; never deleted.
            self.with_conn(|conn| {
                conn.execute(
                    "UPDATE schema_version SET version = ?1",
                    params![SCHEMA_VERSION],
                )
            })
            .await
            .map_err(StoreError::Sql)?;
        }

        self.bootstrap_main_queue().await?;
        Ok(())
    }

    /// Insert the built-in Main queue if it does not already exist. The
    /// id is fixed (deterministic) so cross-restart references stay
    /// stable.
    async fn bootstrap_main_queue(&self) -> Result<(), StoreError> {
        self.with_conn(|conn| {
            let exists: i64 =
                conn.query_row("SELECT COUNT(*) FROM queues WHERE builtin = 1", [], |r| {
                    r.get(0)
                })?;
            if exists > 0 {
                return Ok(());
            }
            let q = Queue::new_main();
            // Every column comes from the constructed queue: literals
            // here would be a second definition of "a new Main queue",
            // and the one that actually reaches disk.
            conn.execute(
                "INSERT INTO queues \
                   (id, name, builtin, schedule_json, on_start_json, on_finish_json, \
                    max_concurrent, stop_on_error, position) \
                 VALUES (?1, ?2, 1, ?3, ?4, ?5, ?6, ?7, 0)",
                params![
                    q.id.to_string(),
                    q.name,
                    serde_json::to_string(&q.schedule)
                        .unwrap_or_else(|_| "{\"kind\":\"manual\"}".into()),
                    serde_json::to_string(&q.on_start).unwrap_or_else(|_| "[]".into()),
                    serde_json::to_string(&q.on_finish).unwrap_or_else(|_| "[]".into()),
                    q.max_concurrent.map(|n| n as i64),
                    q.stop_on_error,
                ],
            )?;
            Ok::<_, rusqlite::Error>(())
        })
        .await
        .map_err(StoreError::Sql)?;
        Ok(())
    }

    /// Return the id of the built-in Main queue. Must always exist after
    /// `migrate` has run.
    pub async fn main_queue_id(&self) -> Result<QueueId, StoreError> {
        let s: String = self
            .with_conn(|conn| {
                conn.query_row("SELECT id FROM queues WHERE builtin = 1 LIMIT 1", [], |r| {
                    r.get(0)
                })
            })
            .await
            .map_err(StoreError::Sql)?;
        let uuid = uuid::Uuid::parse_str(&s).map_err(|e| StoreError::Other(e.to_string()))?;
        Ok(QueueId(uuid))
    }

    pub async fn list_queues(&self) -> Result<Vec<Queue>, StoreError> {
        let rows = self
            .with_conn(|conn| {
                let mut stmt = conn.prepare(
                    "SELECT id, name, builtin, schedule_json, on_start_json, on_finish_json, \
                            max_concurrent, stop_on_error, color \
                     FROM queues ORDER BY position ASC, name ASC",
                )?;
                let iter = stmt.query_map([], |row| {
                    Ok(QueueRow {
                        id: row.get(0)?,
                        name: row.get(1)?,
                        builtin: row.get::<_, i64>(2)? != 0,
                        schedule_json: row.get(3)?,
                        on_start_json: row.get(4)?,
                        on_finish_json: row.get(5)?,
                        max_concurrent: row.get(6)?,
                        stop_on_error: row.get::<_, i64>(7)? != 0,
                        color: row.get::<_, Option<i64>>(8)?,
                    })
                })?;
                iter.collect::<Result<Vec<_>, _>>()
            })
            .await
            .map_err(StoreError::Sql)?;

        // job_ids are filled in by a follow-up query. Doing a JOIN +
        // GROUP_CONCAT in SQLite would keep this in one statement but
        // costs us schema clarity; queue counts are small.
        let job_map = self.jobs_by_queue().await?;
        rows.into_iter().map(|r| r.into_queue(&job_map)).collect()
    }

    async fn jobs_by_queue(
        &self,
    ) -> Result<std::collections::HashMap<String, Vec<JobId>>, StoreError> {
        let pairs: Vec<(String, String)> = self
            .with_conn(|conn| {
                let mut stmt = conn.prepare(
                    "SELECT queue_id, id FROM jobs \
                     ORDER BY queue_position ASC, created_at ASC",
                )?;
                let iter = stmt.query_map([], |row| {
                    let q: String = row.get(0)?;
                    let j: String = row.get(1)?;
                    Ok((q, j))
                })?;
                iter.collect::<Result<Vec<_>, _>>()
            })
            .await
            .map_err(StoreError::Sql)?;
        let mut out: std::collections::HashMap<String, Vec<JobId>> =
            std::collections::HashMap::new();
        for (q, j) in pairs {
            let id = uuid::Uuid::parse_str(&j).map_err(|e| StoreError::Other(e.to_string()))?;
            out.entry(q).or_default().push(JobId(id));
        }
        Ok(out)
    }

    pub async fn upsert_queue(&self, queue: &Queue) -> Result<(), StoreError> {
        let row = QueueRow::from_queue(queue)?;
        self.with_conn(move |conn| {
            conn.execute(
                "INSERT INTO queues \
                   (id, name, builtin, schedule_json, on_start_json, on_finish_json, \
                    max_concurrent, stop_on_error, position, color) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, 0, ?9) \
                 ON CONFLICT(id) DO UPDATE SET \
                    name=excluded.name, \
                    schedule_json=excluded.schedule_json, \
                    on_start_json=excluded.on_start_json, \
                    on_finish_json=excluded.on_finish_json, \
                    max_concurrent=excluded.max_concurrent, \
                    stop_on_error=excluded.stop_on_error, \
                    color=excluded.color",
                params![
                    row.id,
                    row.name,
                    row.builtin as i64,
                    row.schedule_json,
                    row.on_start_json,
                    row.on_finish_json,
                    row.max_concurrent,
                    row.stop_on_error as i64,
                    row.color,
                ],
            )
        })
        .await
        .map_err(StoreError::Sql)?;
        Ok(())
    }

    pub async fn delete_queue(&self, id: QueueId) -> Result<(), StoreError> {
        let id_str = id.to_string();
        self.with_conn(move |conn| {
            // Built-in queue is undeletable. SQL guard mirrors the UI
            // affordance — the WHERE clause makes the delete a no-op
            // rather than erroring, matching the IndexMap semantics on
            // the in-memory side.
            conn.execute(
                "DELETE FROM queues WHERE id = ?1 AND builtin = 0",
                params![id_str],
            )
        })
        .await
        .map_err(StoreError::Sql)?;
        Ok(())
    }

    pub async fn load_settings(&self) -> Result<Settings, StoreError> {
        let map: Map<String, Value> = self
            .with_conn(|conn| {
                let mut stmt = conn.prepare("SELECT key, value FROM settings")?;
                let rows = stmt.query_map([], |row| {
                    let k: String = row.get(0)?;
                    let v: String = row.get(1)?;
                    Ok((k, v))
                })?;
                let mut m = Map::new();
                for r in rows {
                    let (k, v) = r?;
                    let parsed: Value = serde_json::from_str(&v).unwrap_or(Value::String(v));
                    m.insert(k, parsed);
                }
                Ok::<_, rusqlite::Error>(m)
            })
            .await
            .map_err(StoreError::Sql)?;

        if map.is_empty() {
            return Ok(Settings::default());
        }
        serde_json::from_value(Value::Object(map)).map_err(|e| StoreError::Other(e.to_string()))
    }

    pub async fn save_settings(&self, s: &Settings) -> Result<(), StoreError> {
        let value = serde_json::to_value(s).map_err(|e| StoreError::Other(e.to_string()))?;
        let map = match value {
            Value::Object(m) => m,
            _ => {
                return Err(StoreError::Other(
                    "settings must serialize as object".into(),
                ));
            }
        };
        self.with_conn(move |conn| {
            let tx = conn.transaction()?;
            tx.execute("DELETE FROM settings", [])?;
            for (k, v) in &map {
                tx.execute(
                    "INSERT INTO settings (key, value) VALUES (?1, ?2)",
                    params![k, serde_json::to_string(v).unwrap_or_default()],
                )?;
            }
            tx.commit()
        })
        .await
        .map_err(StoreError::Sql)?;
        Ok(())
    }

    /// Look up the first job id whose `filename` column matches `name`
    /// (case-sensitive, exact). Returns `None` for unmatched names. Used
    /// by the Add dialog and capture flow to detect duplicates without
    /// loading the full snapshot.
    pub async fn find_job_id_by_filename(&self, name: &str) -> Result<Option<JobId>, StoreError> {
        let owned = name.to_owned();
        self.with_conn(move |conn| {
            let mut stmt = conn.prepare("SELECT id FROM jobs WHERE filename = ?1 LIMIT 1")?;
            let mut rows = stmt.query(rusqlite::params![owned])?;
            match rows.next()? {
                Some(row) => {
                    let s: String = row.get(0)?;
                    Ok(s.parse::<JobId>().ok())
                }
                None => Ok(None),
            }
        })
        .await
        .map_err(StoreError::Sql)
    }

    pub async fn list_jobs(&self) -> Result<Vec<Job>, StoreError> {
        let rows = self
            .with_conn(|conn| {
                let mut stmt = conn.prepare(
                    "SELECT id, url, save_dir, filename, referrer, headers_json, \
                            max_connections, phase, created_at, speed_limit_override, queue_id, \
                            downloaded, total, final_path, proxy, auth_user, \
                            auth_password_enc, proxy_password_enc, cookies_enc, \
                            advanced_json, checksums_json, category, \
                            started_at, finished_at, retries, response_headers_json, \
                            error_json \
                     FROM jobs ORDER BY created_at ASC",
                )?;
                let iter = stmt.query_map([], |row| {
                    Ok(JobRow {
                        id: row.get(0)?,
                        url: row.get(1)?,
                        save_dir: row.get(2)?,
                        filename: row.get(3)?,
                        referrer: row.get(4)?,
                        headers_json: row.get(5)?,
                        max_connections: row.get(6)?,
                        phase: row.get(7)?,
                        created_at: row.get(8)?,
                        speed_limit_override: row.get(9).ok(),
                        queue_id: row.get(10)?,
                        downloaded: row.get(11).unwrap_or(0),
                        total: row.get(12).ok(),
                        final_path: row.get(13).ok(),
                        proxy: row.get(14).ok(),
                        auth_user: row.get(15).ok(),
                        auth_password_enc: row.get(16).ok(),
                        proxy_password_enc: row.get(17).ok(),
                        cookies_enc: row.get(18).ok(),
                        advanced_json: row.get::<_, String>(19).unwrap_or_else(|_| "{}".into()),
                        checksums_json: row.get::<_, String>(20).unwrap_or_else(|_| "[]".into()),
                        category: row.get(21).unwrap_or_else(|_| "\"other\"".into()),
                        started_at: row.get(22).ok(),
                        finished_at: row.get(23).ok(),
                        retries: row.get(24).unwrap_or(0),
                        response_headers_json: row
                            .get::<_, String>(25)
                            .unwrap_or_else(|_| "null".into()),
                        error_json: row.get::<_, String>(26).unwrap_or_else(|_| "null".into()),
                    })
                })?;
                iter.collect::<Result<Vec<_>, _>>()
            })
            .await
            .map_err(StoreError::Sql)?;

        rows.into_iter().map(JobRow::into_job).collect()
    }

    pub async fn upsert_job(&self, job: &Job) -> Result<(), StoreError> {
        let row = JobRow::from_job(job)?;
        self.with_conn(move |conn| {
            conn.execute(
                "INSERT INTO jobs \
                   (id, url, save_dir, filename, referrer, headers_json, \
                    max_connections, phase, created_at, speed_limit_override, queue_id, \
                    downloaded, total, final_path, proxy, auth_user, \
                    auth_password_enc, proxy_password_enc, cookies_enc, \
                    advanced_json, checksums_json, category, \
                    started_at, finished_at, retries, response_headers_json, error_json) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19, ?20, ?21, ?22, ?23, ?24, ?25, ?26, ?27) \
                 ON CONFLICT(id) DO UPDATE SET \
                    url=excluded.url, save_dir=excluded.save_dir, \
                    filename=excluded.filename, referrer=excluded.referrer, \
                    headers_json=excluded.headers_json, \
                    max_connections=excluded.max_connections, \
                    phase=excluded.phase, \
                    speed_limit_override=excluded.speed_limit_override, \
                    queue_id=excluded.queue_id, \
                    downloaded=excluded.downloaded, \
                    total=excluded.total, \
                    final_path=excluded.final_path, \
                    proxy=excluded.proxy, \
                    auth_user=excluded.auth_user, \
                    auth_password_enc=excluded.auth_password_enc, \
                    proxy_password_enc=excluded.proxy_password_enc, \
                    cookies_enc=excluded.cookies_enc, \
                    advanced_json=excluded.advanced_json, \
                    checksums_json=excluded.checksums_json, \
                    category=excluded.category, \
                    started_at=excluded.started_at, \
                    finished_at=excluded.finished_at, \
                    retries=excluded.retries, \
                    response_headers_json=excluded.response_headers_json, \
                    error_json=excluded.error_json",
                params![
                    row.id,
                    row.url,
                    row.save_dir,
                    row.filename,
                    row.referrer,
                    row.headers_json,
                    row.max_connections,
                    row.phase,
                    row.created_at,
                    row.speed_limit_override,
                    row.queue_id,
                    row.downloaded,
                    row.total,
                    row.final_path,
                    row.proxy,
                    row.auth_user,
                    row.auth_password_enc,
                    row.proxy_password_enc,
                    row.cookies_enc,
                    row.advanced_json,
                    row.checksums_json,
                    row.category,
                    row.started_at,
                    row.finished_at,
                    row.retries,
                    row.response_headers_json,
                    row.error_json,
                ],
            )
        })
        .await
        .map_err(StoreError::Sql)?;
        Ok(())
    }

    pub async fn delete_job(&self, id: JobId) -> Result<(), StoreError> {
        let id_str = id.to_string();
        self.with_conn(move |conn| conn.execute("DELETE FROM jobs WHERE id = ?1", params![id_str]))
            .await
            .map_err(StoreError::Sql)?;
        Ok(())
    }

    /// `true` when at least one job row carries a non-NULL ciphertext
    /// in any of the encrypted secret columns. The daemon calls this
    /// during boot to decide whether a missing master key is recoverable
    /// (no rows ⇒ generate fresh key) or requires the
    /// "missing-key" wipe dialog.
    pub async fn any_job_has_ciphertext(&self) -> Result<bool, StoreError> {
        self.with_conn(|conn| {
            let n: i64 = conn.query_row(
                "SELECT COUNT(*) FROM jobs \
                 WHERE auth_password_enc IS NOT NULL \
                    OR proxy_password_enc IS NOT NULL \
                    OR cookies_enc IS NOT NULL",
                [],
                |r| r.get(0),
            )?;
            Ok::<_, rusqlite::Error>(n > 0)
        })
        .await
        .map_err(StoreError::Sql)
    }

    /// Null every encrypted secret column across every job. Called by
    /// `AppState` after the user acknowledges that the master key is
    /// missing from the OS keyring and the on-disk ciphertext can no
    /// longer be decrypted — wiping the columns lets the daemon
    /// continue without leaving unreadable blobs around.
    pub async fn wipe_all_job_secrets(&self) -> Result<(), StoreError> {
        self.with_conn(|conn| {
            conn.execute(
                "UPDATE jobs SET \
                    auth_password_enc = NULL, \
                    proxy_password_enc = NULL, \
                    cookies_enc = NULL",
                [],
            )
        })
        .await
        .map_err(StoreError::Sql)?;
        Ok(())
    }

    async fn with_conn<F, T>(&self, f: F) -> Result<T, rusqlite::Error>
    where
        F: FnOnce(&mut Connection) -> Result<T, rusqlite::Error> + Send + 'static,
        T: Send + 'static,
    {
        let conn = self.inner.clone();
        spawn_blocking(move || {
            let mut guard = conn.lock().expect("store mutex poisoned");
            f(&mut guard)
        })
        .await
        .expect("store task join")
    }
}

#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    #[error("sqlite: {0}")]
    Sql(#[from] rusqlite::Error),
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("database corruption: {0}")]
    Corrupt(String),
    #[error("{0}")]
    Other(String),
}

struct JobRow {
    id: String,
    url: String,
    save_dir: String,
    filename: Option<String>,
    referrer: Option<String>,
    headers_json: String,
    max_connections: Option<i64>,
    phase: String,
    created_at: String,
    speed_limit_override: Option<i64>,
    queue_id: String,
    downloaded: i64,
    total: Option<i64>,
    final_path: Option<String>,
    proxy: Option<String>,
    auth_user: Option<String>,
    auth_password_enc: Option<String>,
    proxy_password_enc: Option<String>,
    cookies_enc: Option<String>,
    advanced_json: String,
    checksums_json: String,
    category: String,
    started_at: Option<String>,
    finished_at: Option<String>,
    retries: i64,
    response_headers_json: String,
    /// Why the last run failed, as JSON (`null` when it did not). The
    /// phase alone says a job failed; this says what to tell the user
    /// about it after a restart.
    error_json: String,
}

struct QueueRow {
    id: String,
    name: String,
    builtin: bool,
    schedule_json: String,
    on_start_json: String,
    on_finish_json: String,
    max_concurrent: Option<i64>,
    stop_on_error: bool,
    color: Option<i64>,
}

impl QueueRow {
    fn from_queue(q: &Queue) -> Result<Self, StoreError> {
        Ok(Self {
            id: q.id.to_string(),
            name: q.name.clone(),
            builtin: q.builtin,
            schedule_json: serde_json::to_string(&q.schedule)
                .map_err(|e| StoreError::Other(e.to_string()))?,
            on_start_json: serde_json::to_string(&q.on_start)
                .map_err(|e| StoreError::Other(e.to_string()))?,
            on_finish_json: serde_json::to_string(&q.on_finish)
                .map_err(|e| StoreError::Other(e.to_string()))?,
            max_concurrent: q.max_concurrent.map(|v| v as i64),
            stop_on_error: q.stop_on_error,
            color: q
                .color
                .map(|[r, g, b]| ((r as i64) << 16) | ((g as i64) << 8) | (b as i64)),
        })
    }

    fn into_queue(
        self,
        job_map: &std::collections::HashMap<String, Vec<JobId>>,
    ) -> Result<Queue, StoreError> {
        let id = uuid::Uuid::parse_str(&self.id).map_err(|e| StoreError::Other(e.to_string()))?;
        let schedule: QueueSchedule = serde_json::from_str(&self.schedule_json)
            .map_err(|e| StoreError::Other(e.to_string()))?;
        let on_start: Vec<QueueHook> = serde_json::from_str(&self.on_start_json)
            .map_err(|e| StoreError::Other(e.to_string()))?;
        let on_finish: Vec<QueueHook> = serde_json::from_str(&self.on_finish_json)
            .map_err(|e| StoreError::Other(e.to_string()))?;
        let job_ids = job_map.get(&self.id).cloned().unwrap_or_default();
        Ok(Queue {
            id: QueueId(id),
            name: self.name,
            builtin: self.builtin,
            job_ids,
            schedule,
            on_start,
            on_finish,
            max_concurrent: self.max_concurrent.map(|v| v as usize),
            stop_on_error: self.stop_on_error,
            color: self.color.map(|c| {
                let v = c as u32;
                [
                    ((v >> 16) & 0xFF) as u8,
                    ((v >> 8) & 0xFF) as u8,
                    (v & 0xFF) as u8,
                ]
            }),
        })
    }
}

impl JobRow {
    fn from_job(job: &Job) -> Result<Self, StoreError> {
        Ok(Self {
            id: job.id.to_string(),
            url: job.url.to_string(),
            save_dir: job.save_dir.to_string_lossy().into_owned(),
            filename: job.filename.clone(),
            referrer: job.referrer.as_ref().map(|u| u.to_string()),
            headers_json: serde_json::to_string(&job.headers)
                .map_err(|e| StoreError::Other(e.to_string()))?,
            max_connections: job.max_connections.map(|v| v as i64),
            phase: phase_to_str(job.status.phase).to_owned(),
            created_at: job.created_at.to_rfc3339(),
            speed_limit_override: job.speed_limit_override.map(|v| v as i64),
            queue_id: job.queue_id.to_string(),
            downloaded: job.status.downloaded as i64,
            total: job.status.total.map(|v| v as i64),
            final_path: job
                .status
                .final_path
                .as_ref()
                .map(|p| p.to_string_lossy().into_owned()),
            proxy: job.proxy.clone(),
            auth_user: job.auth_user.clone(),
            auth_password_enc: job.enc_auth_password.clone(),
            proxy_password_enc: job.enc_proxy_password.clone(),
            cookies_enc: job.enc_cookies.clone(),
            advanced_json: serde_json::to_string(&job.advanced)
                .map_err(|e| StoreError::Other(e.to_string()))?,
            checksums_json: serde_json::to_string(&job.checksums)
                .map_err(|e| StoreError::Other(e.to_string()))?,
            category: serde_json::to_string(&job.category)
                .map_err(|e| StoreError::Other(e.to_string()))?,
            started_at: job.started_at.map(|d| d.to_rfc3339()),
            finished_at: job.finished_at.map(|d| d.to_rfc3339()),
            retries: job.retries as i64,
            response_headers_json: serde_json::to_string(&job.captured_response)
                .map_err(|e| StoreError::Other(e.to_string()))?,
            error_json: serde_json::to_string(&job.status.error)
                .map_err(|e| StoreError::Other(e.to_string()))?,
        })
    }

    fn into_job(self) -> Result<Job, StoreError> {
        let id = uuid::Uuid::parse_str(&self.id).map_err(|e| StoreError::Other(e.to_string()))?;
        let url = url::Url::parse(&self.url).map_err(|e| StoreError::Other(e.to_string()))?;
        let referrer = self
            .referrer
            .as_deref()
            .map(url::Url::parse)
            .transpose()
            .map_err(|e| StoreError::Other(e.to_string()))?;
        let headers = serde_json::from_str(&self.headers_json)
            .map_err(|e| StoreError::Other(e.to_string()))?;
        let created_at = chrono::DateTime::parse_from_rfc3339(&self.created_at)
            .map_err(|e| StoreError::Other(e.to_string()))?
            .with_timezone(&chrono::Utc);
        let phase = phase_from_str(&self.phase).unwrap_or(Phase::Queued);

        // Phases that can't survive a restart get demoted to Paused / Queued;
        // the runner re-evaluates from disk metadata on next start anyway.
        let status = crate::domain::JobStatus {
            phase: match phase {
                Phase::Completed
                | Phase::Failed
                | Phase::Cancelled
                | Phase::Queued
                | Phase::Paused => phase,
                _ => Phase::Paused,
            },
            downloaded: self.downloaded.max(0) as u64,
            total: self.total.map(|v| v.max(0) as u64),
            final_path: self.final_path.map(PathBuf::from),
            // A blob written by an older build (or a hand-edited row)
            // reads as "no reason recorded" rather than wedging the
            // job; the next failure rewrites it.
            error: serde_json::from_str(&self.error_json).unwrap_or_default(),
            ..crate::domain::JobStatus::default()
        };

        let queue_id = QueueId(
            uuid::Uuid::parse_str(&self.queue_id).map_err(|e| StoreError::Other(e.to_string()))?,
        );

        // Tolerate corrupted JSON blobs by falling back to defaults — a
        // garbled `advanced_json` cell should not wedge the whole row.
        // The next save will rewrite the column to a well-formed value.
        let advanced = serde_json::from_str(&self.advanced_json).unwrap_or_default();
        let checksums = serde_json::from_str(&self.checksums_json).unwrap_or_default();

        // Run-stat timestamps are best-effort: a garbled value yields
        // None rather than wedging the row. `retries` clamps negatives
        // (column is NOT NULL DEFAULT 0, but tolerate manual edits).
        let parse_ts = |s: &Option<String>| -> Option<chrono::DateTime<chrono::Utc>> {
            s.as_deref()
                .and_then(|v| chrono::DateTime::parse_from_rfc3339(v).ok())
                .map(|d| d.with_timezone(&chrono::Utc))
        };
        let started_at = parse_ts(&self.started_at);
        let finished_at = parse_ts(&self.finished_at);
        let retries = self.retries.max(0) as u32;

        Ok(Job {
            id: JobId(id),
            url,
            save_dir: PathBuf::from(self.save_dir),
            filename: self.filename,
            referrer,
            headers,
            max_connections: self.max_connections.map(|v| v as u64),
            proxy: self.proxy,
            auth_user: self.auth_user,
            enc_auth_password: self.auth_password_enc.filter(|s| !s.is_empty()),
            enc_proxy_password: self.proxy_password_enc.filter(|s| !s.is_empty()),
            enc_cookies: self.cookies_enc.filter(|s| !s.is_empty()),
            speed_limit_override: self.speed_limit_override.map(|v| v as u64),
            queue_id,
            created_at,
            started_at,
            finished_at,
            retries,
            status,
            advanced,
            checksums,
            category: serde_json::from_str(&self.category)
                .unwrap_or(crate::domain::Category::Other),
            // A garbled blob reads as "never probed" rather than
            // wedging the row; the next probe rewrites the column.
            captured_response: serde_json::from_str(&self.response_headers_json)
                .unwrap_or_default(),
        })
    }
}

fn phase_to_str(p: Phase) -> &'static str {
    match p {
        Phase::Queued => "queued",
        Phase::Evaluating => "evaluating",
        Phase::ResolvingConflicts => "resolving",
        Phase::Downloading => "downloading",
        Phase::Assembling => "assembling",
        Phase::Flushing => "flushing",
        Phase::Verifying => "verifying",
        Phase::Reconnecting => "reconnecting",
        Phase::Paused => "paused",
        Phase::Completed => "completed",
        Phase::Failed => "failed",
        Phase::Cancelled => "cancelled",
    }
}

fn phase_from_str(s: &str) -> Option<Phase> {
    Some(match s {
        "queued" => Phase::Queued,
        "evaluating" => Phase::Evaluating,
        "resolving" => Phase::ResolvingConflicts,
        "downloading" => Phase::Downloading,
        "assembling" => Phase::Assembling,
        "flushing" => Phase::Flushing,
        "verifying" => Phase::Verifying,
        "reconnecting" => Phase::Reconnecting,
        "paused" => Phase::Paused,
        "completed" => Phase::Completed,
        "failed" => Phase::Failed,
        "cancelled" => Phase::Cancelled,
        _ => return None,
    })
}

pub fn default_db_path() -> PathBuf {
    let dir = dirs::data_dir()
        .or_else(dirs::home_dir)
        .unwrap_or_else(|| PathBuf::from("."));
    dir.join("oxdm").join("oxdm.db")
}

#[allow(dead_code)]
pub fn _path_eq(a: &Path, b: &Path) -> bool {
    a == b
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::{Job, JobId, JobStatus, Phase, Settings};
    use indexmap::IndexMap;
    use std::time::Duration;

    async fn sample_job(store: &Store, filename: &str, phase: Phase) -> Job {
        let mut headers = IndexMap::new();
        headers.insert("X-Test".to_string(), "1".to_string());
        let status = JobStatus {
            phase,
            ..JobStatus::default()
        };
        let queue_id = store.main_queue_id().await.unwrap();
        Job {
            id: JobId::new(),
            url: url::Url::parse("https://example.com/file.zip").unwrap(),
            save_dir: PathBuf::from("/tmp/oxdm-test"),
            filename: Some(filename.to_string()),
            referrer: Some(url::Url::parse("https://example.com/page").unwrap()),
            headers,
            max_connections: Some(8),
            speed_limit_override: None,
            proxy: None,
            auth_user: None,
            enc_auth_password: None,
            enc_proxy_password: None,
            enc_cookies: None,
            queue_id,
            created_at: chrono::Utc::now(),
            started_at: None,
            finished_at: None,
            retries: 0,
            status,
            advanced: crate::domain::Advanced::default(),
            checksums: Vec::new(),
            category: crate::domain::Category::Other,
            captured_response: None,
        }
    }

    #[tokio::test]
    async fn settings_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("oxdm.db");

        let store = Store::open(db.clone()).await.unwrap();

        // First open: no rows yet → defaults.
        let loaded = store.load_settings().await.unwrap();
        let defaults = Settings::default();
        assert_eq!(loaded.max_connections, defaults.max_connections);

        // Mutate, save.
        let mut s = defaults.clone();
        s.max_connections = Some(12);
        s.max_concurrent_downloads = 7;
        s.user_agent = Some("oxdm/test".into());
        s.wait_between_retries = Duration::from_millis(1234);
        s.headers
            .insert("Authorization".into(), "Bearer xyz".into());
        s.proxy = crate::domain::ProxyAdv {
            mode: crate::domain::ProxyMode::Http,
            host: "localhost".into(),
            port: "8080".into(),
            ..Default::default()
        };
        let videos_queue = crate::domain::QueueId::new();
        s.category_folders.insert(
            crate::domain::Category::Videos,
            PathBuf::from("/tmp/oxdm-videos"),
        );
        s.category_queues
            .insert(crate::domain::Category::Videos, videos_queue);
        s.first_run_seen = true;
        store.save_settings(&s).await.unwrap();

        // Drop and reopen — full restart simulation.
        drop(store);
        let store = Store::open(db).await.unwrap();
        let reloaded = store.load_settings().await.unwrap();

        assert_eq!(reloaded.max_connections, Some(12));
        assert_eq!(reloaded.max_concurrent_downloads, 7);
        assert_eq!(reloaded.user_agent.as_deref(), Some("oxdm/test"));
        assert_eq!(reloaded.wait_between_retries, Duration::from_millis(1234));
        assert_eq!(
            reloaded.headers.get("Authorization").map(String::as_str),
            Some("Bearer xyz")
        );
        assert_eq!(reloaded.proxy.host, "localhost");
        assert_eq!(reloaded.proxy.port, "8080");
        assert_eq!(reloaded.proxy.mode, crate::domain::ProxyMode::Http);
        assert_eq!(
            reloaded
                .category_folders
                .get(&crate::domain::Category::Videos),
            Some(&PathBuf::from("/tmp/oxdm-videos"))
        );
        assert_eq!(
            reloaded
                .category_queues
                .get(&crate::domain::Category::Videos),
            Some(&videos_queue)
        );
        assert!(reloaded.first_run_seen);
    }

    #[tokio::test]
    async fn jobs_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("oxdm.db");
        let store = Store::open(db.clone()).await.unwrap();

        let a = sample_job(&store, "a.zip", Phase::Paused).await;
        let b = sample_job(&store, "b.zip", Phase::Completed).await;
        let c = sample_job(&store, "c.zip", Phase::Queued).await;
        store.upsert_job(&a).await.unwrap();
        store.upsert_job(&b).await.unwrap();
        store.upsert_job(&c).await.unwrap();

        // Reopen.
        drop(store);
        let store = Store::open(db).await.unwrap();
        let mut listed = store.list_jobs().await.unwrap();
        listed.sort_by_key(|j| j.created_at);

        assert_eq!(listed.len(), 3);
        let by_name: std::collections::HashMap<_, _> = listed
            .iter()
            .map(|j| (j.filename.clone().unwrap(), j))
            .collect();

        let ra = by_name["a.zip"];
        assert_eq!(ra.id, a.id);
        assert_eq!(ra.url, a.url);
        assert_eq!(ra.referrer, a.referrer);
        assert_eq!(ra.headers, a.headers);
        assert_eq!(ra.max_connections, Some(8));
        assert_eq!(ra.status.phase, Phase::Paused);

        // Completed survives as Completed.
        assert_eq!(by_name["b.zip"].status.phase, Phase::Completed);

        // Queued stays Queued.
        assert_eq!(by_name["c.zip"].status.phase, Phase::Queued);

        // Delete one.
        store.delete_job(a.id).await.unwrap();
        let after = store.list_jobs().await.unwrap();
        assert_eq!(after.len(), 2);
        assert!(after.iter().all(|j| j.id != a.id));
    }

    #[tokio::test]
    async fn running_phases_demote_to_paused_on_reload() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("oxdm.db");
        let store = Store::open(db.clone()).await.unwrap();

        // Persist a job mid-flight.
        let mid = sample_job(&store, "mid.zip", Phase::Downloading).await;
        store.upsert_job(&mid).await.unwrap();

        drop(store);
        let store = Store::open(db).await.unwrap();
        let listed = store.list_jobs().await.unwrap();
        assert_eq!(listed.len(), 1);
        // No runner exists post-restart; transient phases must demote.
        assert_eq!(listed[0].status.phase, Phase::Paused);
    }

    #[tokio::test]
    async fn migrates_v2_db_to_current_adding_new_columns() {
        // Hand-build a v2-shaped DB (no started_at/finished_at/retries),
        // then open it through `Store` and confirm the v3 ALTER arm runs
        // cleanly and the old row hydrates with the new defaults.
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("oxdm.db");
        let queue_id = QueueId(uuid::Uuid::new_v4());
        {
            let conn = Connection::open(&db).unwrap();
            conn.execute_batch(
                "PRAGMA foreign_keys=ON;
                 CREATE TABLE schema_version (version INTEGER NOT NULL);
                 INSERT INTO schema_version (version) VALUES (2);
                 CREATE TABLE queues (
                     id TEXT PRIMARY KEY, name TEXT NOT NULL,
                     builtin INTEGER NOT NULL DEFAULT 0,
                     schedule_json TEXT NOT NULL DEFAULT '{\"kind\":\"manual\"}',
                     on_start_json TEXT NOT NULL DEFAULT '[]',
                     on_finish_json TEXT NOT NULL DEFAULT '[]',
                     max_concurrent INTEGER, stop_on_error INTEGER NOT NULL DEFAULT 0,
                     position INTEGER NOT NULL DEFAULT 0, color INTEGER );
                 CREATE TABLE jobs (
                     id TEXT PRIMARY KEY, url TEXT NOT NULL, save_dir TEXT NOT NULL,
                     filename TEXT, referrer TEXT,
                     headers_json TEXT NOT NULL DEFAULT '{}',
                     max_connections INTEGER, speed_limit_override INTEGER,
                     queue_id TEXT NOT NULL, queue_position INTEGER NOT NULL DEFAULT 0,
                     phase TEXT NOT NULL, downloaded INTEGER NOT NULL DEFAULT 0,
                     total INTEGER, final_path TEXT, proxy TEXT, auth_user TEXT,
                     auth_password_enc TEXT, proxy_password_enc TEXT, cookies_enc TEXT,
                     created_at TEXT NOT NULL,
                     advanced_json TEXT NOT NULL DEFAULT '{}',
                     checksums_json TEXT NOT NULL DEFAULT '[]',
                     category TEXT NOT NULL DEFAULT '\"other\"',
                     FOREIGN KEY(queue_id) REFERENCES queues(id) ON DELETE RESTRICT );",
            )
            .unwrap();
            conn.execute(
                "INSERT INTO queues (id, name, builtin) VALUES (?1, 'Main', 1)",
                params![queue_id.to_string()],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO jobs (id, url, save_dir, filename, queue_id, phase, created_at) \
                 VALUES (?1, 'https://example.com/old.zip', '/tmp', 'old.zip', ?2, 'completed', ?3)",
                params![
                    JobId::new().to_string(),
                    queue_id.to_string(),
                    chrono::Utc::now().to_rfc3339(),
                ],
            )
            .unwrap();
        }

        // Open via Store → migration ladder runs 2 → 3.
        let store = Store::open(db).await.unwrap();
        let listed = store.list_jobs().await.unwrap();
        assert_eq!(listed.len(), 1);
        let j = &listed[0];
        assert_eq!(j.filename.as_deref(), Some("old.zip"));
        // Pre-existing row hydrates with the new-column defaults.
        assert_eq!(j.started_at, None);
        assert_eq!(j.finished_at, None);
        assert_eq!(j.retries, 0);
        // v4 column: a row written before response capture existed
        // reads as "never probed", not as an empty capture.
        assert_eq!(j.captured_response, None);

        // And the new columns are now writable end-to-end.
        let mut updated = j.clone();
        updated.retries = 4;
        updated.finished_at = Some(chrono::Utc::now());
        updated.captured_response = Some(crate::domain::CapturedResponse {
            headers: vec![crate::domain::ResponseHeader {
                name: "content-type".into(),
                value: "application/zip".into(),
            }],
            probed_at: 1_700_000_000,
        });
        store.upsert_job(&updated).await.unwrap();
        let reread = store.list_jobs().await.unwrap();
        assert_eq!(reread[0].retries, 4);
        assert!(reread[0].finished_at.is_some());
        assert_eq!(reread[0].captured_response, updated.captured_response);
    }

    #[tokio::test]
    async fn settings_overwrite_replaces_all_keys() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("oxdm.db");
        let store = Store::open(db).await.unwrap();

        let mut s = Settings::default();
        s.headers.insert("A".into(), "1".into());
        store.save_settings(&s).await.unwrap();

        s.headers.clear();
        s.headers.insert("B".into(), "2".into());
        store.save_settings(&s).await.unwrap();

        let r = store.load_settings().await.unwrap();
        // Old "A" must be gone; only "B" remains. save_settings does
        // a full replace inside a transaction.
        assert!(!r.headers.contains_key("A"));
        assert_eq!(r.headers.get("B").map(String::as_str), Some("2"));
    }
}
