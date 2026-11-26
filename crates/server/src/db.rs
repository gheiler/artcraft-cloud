use std::path::Path;
use std::sync::{Arc, Mutex};

use rusqlite::{params, Connection, OptionalExtension};

use crate::error::Error;
use crate::paths::{mint_etag, now_secs, sha256_hex};

#[derive(Clone)]
pub struct Db {
    conn: Arc<Mutex<Connection>>,
}

#[derive(Debug, Clone)]
pub struct Account {
    pub id: i64,
    pub username: String,
    pub display_name: String,
    pub role: String,
    pub disabled: bool,
    pub password_hash: String,
    pub is_super: bool,
}

impl Account {
    pub fn is_admin(&self) -> bool {
        self.role == "admin"
    }

    pub fn public(&self) -> artcraft_cloud_types::User {
        artcraft_cloud_types::User {
            id: self.id,
            username: self.username.clone(),
            display_name: self.display_name.clone(),
            role: self.role.clone(),
            disabled: self.disabled,
            super_admin: self.is_super,
            email: None,
            mailbox: false,
            mail_sync: String::new(),
            mail_suspended: false,
            mail_access_error: None,
            apps_origin: None,
        }
    }
}

/// The mail server connection; `secret` is sealed (`vault`).
#[derive(Debug, Clone)]
pub struct MailConfigRow {
    pub base_url: String,
    pub auth: String,
    pub username: String,
    pub secret: Option<Vec<u8>>,
    pub public_host: String,
}

/// A user's email, linked mailbox address and mailbox password state (`MailboxRow::password_sync`).
#[derive(Debug, Clone, Default)]
pub struct UserMailInfo {
    pub email: Option<String>,
    pub mailbox: Option<String>,
    pub sync: Option<String>,
    pub suspended: bool,
    pub access_error: Option<String>,
}

/// Per user id.
pub type UserMailMap = std::collections::HashMap<i64, UserMailInfo>;

/// A user's mailbox on the mail server.
#[derive(Debug, Clone)]
pub struct MailboxRow {
    pub user_id: i64,
    pub address: String,
    pub account_id: String,
    /// The credential the JMAP proxy signs in with, sealed (`vault`): an app password of the
    /// account (`proxy_kind` `app`), or, for mailboxes made before app passwords, the generated
    /// account password (`password`) until it is replaced.
    pub secret: Vec<u8>,
    pub proxy_kind: String,
    /// The app password's credential id on the mail server, to revoke it.
    pub proxy_id: Option<String>,
    /// `synced`: the account password is the user's artcraftCloud password. `pending`: not yet
    /// (no plaintext was at hand), set at the next sign-in or password change. `failed`: the
    /// last update did not reach the server; `pending_secret` holds it for the retry.
    pub password_sync: String,
    pub sync_error: Option<String>,
    /// The new password waiting for a retry, sealed; cleared once it is on the server.
    pub pending_secret: Option<Vec<u8>>,
    pub sync_attempts: i64,
    /// artcraftCloud has suspended the account on the mail server (its person is disabled).
    pub suspended: bool,
    /// The permissions the suspension disabled, comma-separated, to lift exactly those.
    pub suspend_added: String,
    /// Why the last suspend or restore did not reach the mail server.
    pub access_error: Option<String>,
}

#[derive(Debug, Clone)]
pub struct Meta {
    pub etag: String,
    pub rev: i64,
    pub is_dir: bool,
}

#[derive(Debug, Clone)]
pub struct TrashRow {
    pub id: String,
    pub original_path: String,
    pub name: String,
    pub is_dir: bool,
    pub size: u64,
    pub deleted_at: i64,
}

pub struct NewShare<'a> {
    pub owner: i64,
    pub path: &'a str,
    pub grantee: Option<i64>,
    pub group: Option<i64>,
    pub permission: &'a str,
    pub token: Option<&'a str>,
    pub password_hash: Option<&'a str>,
    pub expires_at: Option<i64>,
}

#[derive(Debug, Clone)]
pub struct ShareRow {
    pub id: i64,
    pub owner_id: i64,
    pub owner_name: String,
    pub path: String,
    pub grantee_id: Option<i64>,
    pub grantee_name: Option<String>,
    pub permission: String,
    pub token: Option<String>,
    pub password_hash: Option<String>,
    pub expires_at: Option<i64>,
    pub group_id: Option<i64>,
    pub group_name: Option<String>,
}

/// A remote calendar password, sealed (`vault`), belonging to one person and one CalendarCraft account.
#[derive(Debug, Clone)]
pub struct CalendarRemoteCredential {
    pub username: String,
    pub secret: Vec<u8>,
}

impl Db {
    pub fn open(path: &Path) -> Result<Self, Error> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let conn = Connection::open(path)?;
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "foreign_keys", "ON")?;
        conn.pragma_update(None, "busy_timeout", "5000")?;
        conn.pragma_update(None, "synchronous", "NORMAL")?;
        conn.execute_batch(SCHEMA)?;
        migrate(&conn)?;
        Ok(Self { conn: Arc::new(Mutex::new(conn)) })
    }

    fn with<T>(&self, f: impl FnOnce(&Connection) -> Result<T, Error>) -> Result<T, Error> {
        let guard = self.conn.lock().map_err(|_| Error::Locked)?;
        f(&guard)
    }

    /// Run `f` while the database mutex is held. File renames that must stay
    /// in lock-step with an etag update use this.
    pub fn with_conn<T>(&self, f: impl FnOnce(&Connection) -> Result<T, Error>) -> Result<T, Error> {
        self.with(f)
    }

    pub fn user_count(&self) -> Result<i64, Error> {
        self.with(|c| Ok(c.query_row("SELECT COUNT(*) FROM users", [], |row| row.get(0))?))
    }

    pub fn insert_user(&self, username: &str, password_hash: &str, role: &str, display_name: &str) -> Result<Account, Error> {
        self.with(|c| {
            c.execute(
                "INSERT INTO users (username, password_hash, role, disabled, display_name, created_at) VALUES (?1, ?2, ?3, 0, ?4, ?5)",
                params![username, password_hash, role, display_name, now_secs()],
            )?;
            let id = c.last_insert_rowid();
            ensure_super(c)?;
            let is_super: bool = c.query_row("SELECT is_super FROM users WHERE id = ?1", params![id], |row| row.get::<_, i64>(0))? != 0;
            Ok(Account {
                id,
                username: username.to_string(),
                display_name: display_name.to_string(),
                role: role.to_string(),
                disabled: false,
                password_hash: password_hash.to_string(),
                is_super,
            })
        })
    }

    pub fn user_by_username(&self, username: &str) -> Result<Option<Account>, Error> {
        self.with(|c| {
            c.query_row(
                "SELECT id, username, display_name, role, disabled, password_hash, is_super FROM users WHERE username = ?1 COLLATE NOCASE",
                params![username],
                map_account,
            )
            .optional()
            .map_err(Error::from)
        })
    }

    pub fn user_by_id(&self, id: i64) -> Result<Option<Account>, Error> {
        self.with(|c| {
            c.query_row(
                "SELECT id, username, display_name, role, disabled, password_hash, is_super FROM users WHERE id = ?1",
                params![id],
                map_account,
            )
            .optional()
            .map_err(Error::from)
        })
    }

    pub fn list_users(&self) -> Result<Vec<Account>, Error> {
        self.with(|c| {
            let mut stmt = c.prepare("SELECT id, username, display_name, role, disabled, password_hash, is_super FROM users ORDER BY username")?;
            let rows = stmt.query_map([], map_account)?;
            rows.collect::<Result<Vec<_>, _>>().map_err(Error::from)
        })
    }

    pub fn set_disabled(&self, id: i64, disabled: bool) -> Result<(), Error> {
        self.with(|c| {
            let changed = c.execute("UPDATE users SET disabled = ?1 WHERE id = ?2", params![disabled as i64, id])?;
            if changed == 0 {
                return Err(Error::NotFound);
            }
            if disabled {
                c.execute("DELETE FROM sessions WHERE user_id = ?1", params![id])?;
            }
            Ok(())
        })
    }

    pub fn set_password(&self, id: i64, password_hash: &str) -> Result<(), Error> {
        self.with(|c| {
            let changed = c.execute("UPDATE users SET password_hash = ?1 WHERE id = ?2", params![password_hash, id])?;
            if changed == 0 {
                return Err(Error::NotFound);
            }
            c.execute("DELETE FROM sessions WHERE user_id = ?1", params![id])?;
            Ok(())
        })
    }

    pub fn user_email(&self, id: i64) -> Result<Option<String>, Error> {
        self.with(|c| Ok(c.query_row("SELECT email FROM users WHERE id = ?1", params![id], |row| row.get::<_, Option<String>>(0)).optional()?.flatten()))
    }

    /// Set or clear a user's email. `Exists` when another user has it.
    pub fn set_user_email(&self, id: i64, email: Option<&str>) -> Result<(), Error> {
        self.with(|c| {
            match c.execute("UPDATE users SET email = ?1 WHERE id = ?2", params![email, id]) {
                Ok(0) => Err(Error::NotFound),
                Ok(_) => Ok(()),
                Err(rusqlite::Error::SqliteFailure(err, _)) if err.code == rusqlite::ErrorCode::ConstraintViolation => Err(Error::Exists),
                Err(err) => Err(err.into()),
            }
        })
    }

    /// Every user's email and linked mailbox address, by user id.
    pub fn user_mail(&self) -> Result<UserMailMap, Error> {
        self.with(|c| {
            let mut stmt = c.prepare("SELECT u.id, u.email, m.address, m.password_sync, m.suspended, m.access_error FROM users u LEFT JOIN mailboxes m ON m.user_id = u.id")?;
            let rows = stmt.query_map([], |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    UserMailInfo { email: row.get(1)?, mailbox: row.get(2)?, sync: row.get(3)?, suspended: row.get::<_, Option<bool>>(4)?.unwrap_or(false), access_error: row.get(5)? },
                ))
            })?;
            Ok(rows.collect::<Result<_, _>>()?)
        })
    }

    pub fn mail_config(&self) -> Result<Option<MailConfigRow>, Error> {
        self.with(|c| {
            c.query_row("SELECT base_url, auth, username, secret, public_host FROM mail_config WHERE id = 1", [], |row| {
                Ok(MailConfigRow { base_url: row.get(0)?, auth: row.get(1)?, username: row.get(2)?, secret: row.get(3)?, public_host: row.get(4)? })
            })
            .optional()
            .map_err(Error::from)
        })
    }

    pub fn set_mail_config(&self, row: &MailConfigRow) -> Result<(), Error> {
        self.with(|c| {
            c.execute(
                "INSERT INTO mail_config (id, base_url, auth, username, secret, public_host, updated_at) VALUES (1, ?1, ?2, ?3, ?4, ?5, ?6)
                 ON CONFLICT(id) DO UPDATE SET base_url = excluded.base_url, auth = excluded.auth, username = excluded.username, secret = excluded.secret,
                 public_host = excluded.public_host, updated_at = excluded.updated_at",
                params![row.base_url, row.auth, row.username, row.secret, row.public_host, now_secs()],
            )?;
            Ok(())
        })
    }

    pub fn clear_mail_config(&self) -> Result<(), Error> {
        self.with(|c| {
            c.execute("DELETE FROM mail_config", [])?;
            Ok(())
        })
    }

    pub fn mailbox(&self, user_id: i64) -> Result<Option<MailboxRow>, Error> {
        self.with(|c| {
            c.query_row(&format!("SELECT {MAILBOX_COLUMNS} FROM mailboxes WHERE user_id = ?1"), params![user_id], mailbox_row).optional().map_err(Error::from)
        })
    }

    /// Mailboxes with a password change waiting for the mail server.
    pub fn mailboxes_to_retry(&self) -> Result<Vec<MailboxRow>, Error> {
        self.with(|c| {
            let mut stmt = c.prepare(&format!("SELECT {MAILBOX_COLUMNS} FROM mailboxes WHERE pending_secret IS NOT NULL"))?;
            let rows = stmt.query_map([], mailbox_row)?;
            Ok(rows.collect::<Result<_, _>>()?)
        })
    }

    /// Mailboxes whose suspension on the mail server does not match their person.
    pub fn mailboxes_to_reconcile(&self) -> Result<Vec<(MailboxRow, bool)>, Error> {
        self.with(|c| {
            let columns = MAILBOX_COLUMNS.split(", ").map(|col| format!("m.{col}")).collect::<Vec<_>>().join(", ");
            let mut stmt = c.prepare(&format!("SELECT {columns}, u.disabled FROM mailboxes m JOIN users u ON u.id = m.user_id WHERE m.suspended != u.disabled"))?;
            let rows = stmt.query_map([], |row| Ok((mailbox_row(row)?, row.get::<_, bool>(13)?)))?;
            Ok(rows.collect::<Result<_, _>>()?)
        })
    }

    /// The mail server now matches (`suspended`), with what the suspension disabled.
    pub fn mailbox_access(&self, user_id: i64, suspended: bool, added: &str) -> Result<(), Error> {
        self.with(|c| {
            c.execute("UPDATE mailboxes SET suspended = ?2, suspend_added = ?3, access_error = NULL WHERE user_id = ?1", params![user_id, suspended, added])?;
            Ok(())
        })
    }

    pub fn mailbox_access_failed(&self, user_id: i64, error: &str) -> Result<(), Error> {
        self.with(|c| {
            c.execute("UPDATE mailboxes SET access_error = ?2 WHERE user_id = ?1", params![user_id, error])?;
            Ok(())
        })
    }

    /// The proxy now signs in with an app password.
    pub fn set_mailbox_proxy(&self, user_id: i64, secret: &[u8], proxy_id: &str) -> Result<(), Error> {
        self.with(|c| {
            c.execute("UPDATE mailboxes SET secret = ?2, proxy_kind = 'app', proxy_id = ?3 WHERE user_id = ?1", params![user_id, secret, proxy_id])?;
            Ok(())
        })
    }

    /// The account password is the user's password now.
    pub fn mailbox_synced(&self, user_id: i64) -> Result<(), Error> {
        self.with(|c| {
            c.execute(
                "UPDATE mailboxes SET password_sync = 'synced', sync_error = NULL, pending_secret = NULL, sync_attempts = 0, synced_at = ?2 WHERE user_id = ?1",
                params![user_id, now_secs()],
            )?;
            Ok(())
        })
    }

    /// The update did not reach the server; keep `pending` (sealed) for a retry, or `None` to stop.
    pub fn mailbox_sync_failed(&self, user_id: i64, error: &str, pending: Option<&[u8]>, attempts: i64) -> Result<(), Error> {
        self.with(|c| {
            c.execute(
                "UPDATE mailboxes SET password_sync = 'failed', sync_error = ?2, pending_secret = ?3, sync_attempts = ?4 WHERE user_id = ?1",
                params![user_id, error, pending, attempts],
            )?;
            Ok(())
        })
    }

    pub fn link_mailbox(&self, row: &MailboxRow) -> Result<(), Error> {
        self.with(|c| {
            match c.execute(
                "INSERT INTO mailboxes (user_id, address, account_id, secret, proxy_kind, proxy_id, password_sync, created_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                params![row.user_id, row.address, row.account_id, row.secret, row.proxy_kind, row.proxy_id, row.password_sync, now_secs()],
            ) {
                Ok(_) => Ok(()),
                Err(rusqlite::Error::SqliteFailure(err, _)) if err.code == rusqlite::ErrorCode::ConstraintViolation => Err(Error::Exists),
                Err(err) => Err(err.into()),
            }
        })
    }

    pub fn unlink_mailbox(&self, user_id: i64) -> Result<bool, Error> {
        self.with(|c| Ok(c.execute("DELETE FROM mailboxes WHERE user_id = ?1", params![user_id])? > 0))
    }

    pub fn enabled_admin_count(&self) -> Result<i64, Error> {
        self.with(|c| Ok(c.query_row("SELECT COUNT(*) FROM users WHERE role = 'admin' AND disabled = 0", [], |row| row.get(0))?))
    }

    pub fn insert_session(&self, token: &str, user_id: i64, ttl: i64) -> Result<(), Error> {
        let hash = sha256_hex(token.as_bytes());
        let now = now_secs();
        self.with(|c| {
            c.execute(
                "INSERT INTO sessions (token_hash, user_id, expires_at, created_at) VALUES (?1, ?2, ?3, ?4)",
                params![hash, user_id, now + ttl, now],
            )?;
            Ok(())
        })
    }

    pub fn session_user(&self, token: &str) -> Result<Option<Account>, Error> {
        let hash = sha256_hex(token.as_bytes());
        let now = now_secs();
        self.with(|c| {
            let row = c
                .query_row(
                    "SELECT u.id, u.username, u.display_name, u.role, u.disabled, u.password_hash, s.expires_at, u.is_super
                     FROM sessions s JOIN users u ON u.id = s.user_id WHERE s.token_hash = ?1",
                    params![hash],
                    |row| {
                        Ok((
                            Account {
                                id: row.get(0)?,
                                username: row.get(1)?,
                                display_name: row.get(2)?,
                                role: row.get(3)?,
                                disabled: row.get::<_, i64>(4)? != 0,
                                password_hash: row.get(5)?,
                                is_super: row.get::<_, i64>(7)? != 0,
                            },
                            row.get::<_, i64>(6)?,
                        ))
                    },
                )
                .optional()?;
            let Some((account, expires)) = row else {
                return Ok(None);
            };
            if expires < now {
                c.execute("DELETE FROM sessions WHERE token_hash = ?1", params![hash])?;
                return Ok(None);
            }
            Ok(Some(account))
        })
    }

    pub fn delete_session(&self, token: &str) -> Result<(), Error> {
        let hash = sha256_hex(token.as_bytes());
        self.with(|c| {
            c.execute("DELETE FROM sessions WHERE token_hash = ?1", params![hash])?;
            Ok(())
        })
    }

    pub fn meta(&self, owner: i64, path: &str) -> Result<Option<Meta>, Error> {
        self.with(|c| meta_of(c, owner, path))
    }

    pub fn upsert_meta(&self, owner: i64, path: &str, is_dir: bool, rev: i64, etag: &str) -> Result<(), Error> {
        self.with(|c| {
            c.execute(
                "INSERT INTO file_meta (owner_id, path, etag, rev, is_dir) VALUES (?1, ?2, ?3, ?4, ?5)
                 ON CONFLICT(owner_id, path) DO UPDATE SET etag = excluded.etag, rev = excluded.rev, is_dir = excluded.is_dir",
                params![owner, path, etag, rev, is_dir as i64],
            )?;
            Ok(())
        })
    }

    pub fn next_file_etag(&self, owner: i64, path: &str, is_dir: bool) -> Result<String, Error> {
        self.with(|c| {
            let current = meta_of(c, owner, path)?;
            let rev = current.map(|m| m.rev.saturating_add(1)).unwrap_or(1);
            let etag = mint_etag(rev);
            c.execute(
                "INSERT INTO file_meta (owner_id, path, etag, rev, is_dir) VALUES (?1, ?2, ?3, ?4, ?5)
                 ON CONFLICT(owner_id, path) DO UPDATE SET etag = excluded.etag, rev = excluded.rev, is_dir = excluded.is_dir",
                params![owner, path, etag, rev, is_dir as i64],
            )?;
            Ok(etag)
        })
    }

    pub fn ensure_etag(&self, owner: i64, path: &str, is_dir: bool) -> Result<String, Error> {
        self.with(|c| {
            if let Some(meta) = meta_of(c, owner, path)? {
                return Ok(meta.etag);
            }
            let etag = mint_etag(1);
            c.execute(
                "INSERT INTO file_meta (owner_id, path, etag, rev, is_dir) VALUES (?1, ?2, ?3, 1, ?4)",
                params![owner, path, etag, is_dir as i64],
            )?;
            Ok(etag)
        })
    }

    pub fn delete_meta_tree(&self, owner: i64, path: &str) -> Result<(), Error> {
        self.with(|c| {
            if path.is_empty() {
                return Err(Error::BadPath);
            }
            c.execute(
                "DELETE FROM file_meta WHERE owner_id = ?1 AND (path = ?2 OR path LIKE ?3)",
                params![owner, path, format!("{path}/%")],
            )?;
            Ok(())
        })
    }

    pub fn rewrite_meta_prefix(&self, owner: i64, from: &str, to: &str) -> Result<(), Error> {
        self.with(|c| {
            c.execute(
                "UPDATE file_meta SET path = ?1 || substr(path, length(?2) + 1)
                 WHERE owner_id = ?3 AND (path = ?2 OR path LIKE ?4)",
                params![to, from, owner, format!("{from}/%")],
            )?;
            Ok(())
        })
    }

    pub fn insert_trash(&self, id: &str, owner: i64, original: &str, name: &str, is_dir: bool, size: u64) -> Result<(), Error> {
        self.with(|c| {
            c.execute(
                "INSERT INTO trash (id, owner_id, original_path, name, is_dir, size, deleted_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                params![id, owner, original, name, is_dir as i64, size as i64, now_secs()],
            )?;
            Ok(())
        })
    }

    pub fn list_trash(&self, owner: i64) -> Result<Vec<TrashRow>, Error> {
        self.with(|c| {
            let mut stmt = c.prepare(
                "SELECT id, original_path, name, is_dir, size, deleted_at FROM trash WHERE owner_id = ?1 ORDER BY deleted_at DESC",
            )?;
            let rows = stmt.query_map(params![owner], |row| {
                Ok(TrashRow {
                    id: row.get(0)?,
                    original_path: row.get(1)?,
                    name: row.get(2)?,
                    is_dir: row.get::<_, i64>(3)? != 0,
                    size: row.get::<_, i64>(4)? as u64,
                    deleted_at: row.get(5)?,
                })
            })?;
            rows.collect::<Result<Vec<_>, _>>().map_err(Error::from)
        })
    }

    pub fn trash(&self, owner: i64, id: &str) -> Result<Option<TrashRow>, Error> {
        self.with(|c| {
            c.query_row(
                "SELECT id, original_path, name, is_dir, size, deleted_at FROM trash WHERE owner_id = ?1 AND id = ?2",
                params![owner, id],
                |row| {
                    Ok(TrashRow {
                        id: row.get(0)?,
                        original_path: row.get(1)?,
                        name: row.get(2)?,
                        is_dir: row.get::<_, i64>(3)? != 0,
                        size: row.get::<_, i64>(4)? as u64,
                        deleted_at: row.get(5)?,
                    })
                },
            )
            .optional()
            .map_err(Error::from)
        })
    }

    pub fn delete_trash(&self, owner: i64, id: &str) -> Result<(), Error> {
        self.with(|c| {
            c.execute("DELETE FROM trash WHERE owner_id = ?1 AND id = ?2", params![owner, id])?;
            Ok(())
        })
    }

    pub fn trash_ids(&self, owner: i64) -> Result<Vec<String>, Error> {
        self.with(|c| {
            let mut stmt = c.prepare("SELECT id FROM trash WHERE owner_id = ?1")?;
            let rows = stmt.query_map(params![owner], |row| row.get(0))?;
            rows.collect::<Result<Vec<_>, _>>().map_err(Error::from)
        })
    }

    pub fn insert_share(&self, share: NewShare<'_>) -> Result<i64, Error> {
        self.with(|c| {
            c.execute(
                "INSERT INTO shares (owner_id, path, grantee_id, permission, token, password_hash, expires_at, created_at, group_id)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
                params![share.owner, share.path, share.grantee, share.permission, share.token, share.password_hash, share.expires_at, now_secs(), share.group],
            )?;
            Ok(c.last_insert_rowid())
        })
    }

    pub fn update_user_share(&self, id: i64, permission: &str, expires_at: Option<i64>) -> Result<(), Error> {
        self.with(|c| {
            c.execute("UPDATE shares SET permission = ?1, expires_at = ?2 WHERE id = ?3", params![permission, expires_at, id])?;
            Ok(())
        })
    }

    pub fn find_user_share(&self, owner: i64, path: &str, grantee: i64) -> Result<Option<i64>, Error> {
        self.with(|c| {
            c.query_row(
                "SELECT id FROM shares WHERE owner_id = ?1 AND path = ?2 AND grantee_id = ?3",
                params![owner, path, grantee],
                |row| row.get(0),
            )
            .optional()
            .map_err(Error::from)
        })
    }

    pub fn shares_out(&self, owner: i64) -> Result<Vec<ShareRow>, Error> {
        self.with(|c| query_shares(c, "s.owner_id = ?1", owner))
    }

    pub fn shares_in(&self, grantee: i64) -> Result<Vec<ShareRow>, Error> {
        let now = now_secs();
        self.with(|c| {
            let sql = format!(
                "{} AND (s.expires_at IS NULL OR s.expires_at > ?2) AND s.owner_id != ?1 ORDER BY s.id DESC",
                share_select("(s.grantee_id = ?1 OR s.group_id IN (SELECT group_id FROM group_members WHERE user_id = ?1))")
            );
            let mut stmt = c.prepare(&sql)?;
            let rows = stmt.query_map(params![grantee, now], map_share)?;
            rows.collect::<Result<Vec<_>, _>>().map_err(Error::from)
        })
    }

    pub fn find_group_share(&self, owner: i64, path: &str, group: i64) -> Result<Option<i64>, Error> {
        self.with(|c| {
            c.query_row("SELECT id FROM shares WHERE owner_id = ?1 AND path = ?2 AND group_id = ?3", params![owner, path, group], |row| row.get(0))
                .optional()
                .map_err(Error::from)
        })
    }

    pub fn in_group(&self, user: i64, group: i64) -> Result<bool, Error> {
        self.with(|c| {
            Ok(c.query_row("SELECT COUNT(*) FROM group_members WHERE user_id = ?1 AND group_id = ?2", params![user, group], |row| row.get::<_, i64>(0))? > 0)
        })
    }

    pub fn set_role(&self, id: i64, role: &str) -> Result<(), Error> {
        self.with(|c| {
            let changed = c.execute("UPDATE users SET role = ?1 WHERE id = ?2 AND is_super = 0", params![role, id])?;
            if changed == 0 {
                return Err(Error::NotFound);
            }
            Ok(())
        })
    }

    pub fn create_group(&self, name: &str) -> Result<i64, Error> {
        self.with(|c| {
            c.execute("INSERT INTO groups (name, created_at) VALUES (?1, ?2)", params![name, now_secs()])?;
            Ok(c.last_insert_rowid())
        })
    }

    pub fn delete_group(&self, id: i64) -> Result<(), Error> {
        self.with(|c| {
            c.execute("DELETE FROM shares WHERE group_id = ?1", params![id])?;
            c.execute("DELETE FROM group_members WHERE group_id = ?1", params![id])?;
            if c.execute("DELETE FROM groups WHERE id = ?1", params![id])? == 0 {
                return Err(Error::NotFound);
            }
            Ok(())
        })
    }

    pub fn group_by_name(&self, name: &str) -> Result<Option<(i64, String)>, Error> {
        self.with(|c| {
            c.query_row("SELECT id, name FROM groups WHERE name = ?1 COLLATE NOCASE", params![name], |row| Ok((row.get(0)?, row.get(1)?)))
                .optional()
                .map_err(Error::from)
        })
    }

    /// Every group with the usernames of its members.
    pub fn list_groups(&self) -> Result<Vec<artcraft_cloud_types::Group>, Error> {
        self.with(|c| {
            let mut stmt = c.prepare("SELECT id, name FROM groups ORDER BY name COLLATE NOCASE")?;
            let groups = stmt.query_map([], |row| Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?)))?.collect::<Result<Vec<_>, _>>()?;
            let mut members = c.prepare(
                "SELECT u.username FROM group_members m JOIN users u ON u.id = m.user_id WHERE m.group_id = ?1 ORDER BY u.username",
            )?;
            let mut out = Vec::new();
            for (id, name) in groups {
                let names = members.query_map(params![id], |row| row.get::<_, String>(0))?.collect::<Result<Vec<_>, _>>()?;
                out.push(artcraft_cloud_types::Group { id, name, members: names });
            }
            Ok(out)
        })
    }

    pub fn add_member(&self, group: i64, user: i64) -> Result<(), Error> {
        self.with(|c| {
            c.execute("INSERT OR IGNORE INTO group_members (group_id, user_id) VALUES (?1, ?2)", params![group, user])?;
            Ok(())
        })
    }

    pub fn remove_member(&self, group: i64, user: i64) -> Result<(), Error> {
        self.with(|c| {
            c.execute("DELETE FROM group_members WHERE group_id = ?1 AND user_id = ?2", params![group, user])?;
            Ok(())
        })
    }

    pub fn setting(&self, user: i64, app: &str) -> Result<Option<String>, Error> {
        self.with(|c| {
            c.query_row("SELECT data FROM app_settings WHERE user_id = ?1 AND app = ?2", params![user, app], |row| row.get(0))
                .optional()
                .map_err(Error::from)
        })
    }

    pub fn put_setting(&self, user: i64, app: &str, data: &str) -> Result<(), Error> {
        self.with(|c| {
            c.execute(
                "INSERT INTO app_settings (user_id, app, data, updated_at) VALUES (?1, ?2, ?3, ?4)
                 ON CONFLICT(user_id, app) DO UPDATE SET data = excluded.data, updated_at = excluded.updated_at",
                params![user, app, data, now_secs()],
            )?;
            Ok(())
        })
    }

    pub fn share_by_id(&self, id: i64) -> Result<Option<ShareRow>, Error> {
        self.with(|c| {
            c.query_row(&share_select("s.id = ?1"), params![id], map_share).optional().map_err(Error::from)
        })
    }

    pub fn share_by_token(&self, token: &str) -> Result<Option<ShareRow>, Error> {
        self.with(|c| {
            c.query_row(&share_select("s.token = ?1"), params![token], map_share).optional().map_err(Error::from)
        })
    }

    pub fn delete_share(&self, owner: i64, id: i64) -> Result<(), Error> {
        self.with(|c| {
            let changed = c.execute("DELETE FROM shares WHERE id = ?1 AND owner_id = ?2", params![id, owner])?;
            if changed == 0 {
                Err(Error::NotFound)
            } else {
                Ok(())
            }
        })
    }

    pub fn delete_share_tree(&self, owner: i64, path: &str) -> Result<(), Error> {
        self.with(|c| {
            c.execute(
                "DELETE FROM shares WHERE owner_id = ?1 AND (path = ?2 OR path LIKE ?3)",
                params![owner, path, format!("{path}/%")],
            )?;
            Ok(())
        })
    }

    pub fn rewrite_share_prefix(&self, owner: i64, from: &str, to: &str) -> Result<(), Error> {
        self.with(|c| {
            c.execute(
                "UPDATE shares SET path = ?1 || substr(path, length(?2) + 1)
                 WHERE owner_id = ?3 AND (path = ?2 OR path LIKE ?4)",
                params![to, from, owner, format!("{from}/%")],
            )?;
            Ok(())
        })
    }


    pub fn set_calendar_remote_credential(&self, user_id: i64, account_id: &str, username: &str, secret: &[u8]) -> Result<(), Error> {
        self.with(|c| {
            c.execute(
                "INSERT INTO calendar_remote_credentials (user_id, account_id, username, secret, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5)
                 ON CONFLICT(user_id, account_id) DO UPDATE SET username = excluded.username, secret = excluded.secret, updated_at = excluded.updated_at",
                params![user_id, account_id, username, secret, now_secs()],
            )?;
            Ok(())
        })
    }

    pub fn calendar_remote_credential(&self, user_id: i64, account_id: &str) -> Result<Option<CalendarRemoteCredential>, Error> {
        self.with(|c| {
            c.query_row(
                "SELECT username, secret FROM calendar_remote_credentials WHERE user_id = ?1 AND account_id = ?2",
                params![user_id, account_id],
                |row| Ok(CalendarRemoteCredential { username: row.get(0)?, secret: row.get(1)? }),
            )
            .optional()
            .map_err(Error::from)
        })
    }

    pub fn delete_calendar_remote_credential(&self, user_id: i64, account_id: &str) -> Result<(), Error> {
        self.with(|c| {
            c.execute("DELETE FROM calendar_remote_credentials WHERE user_id = ?1 AND account_id = ?2", params![user_id, account_id])?;
            Ok(())
        })
    }

    pub fn ping(&self) -> Result<(), Error> {
        self.with(|c| {
            c.query_row("SELECT 1", [], |_| Ok(()))?;
            Ok(())
        })
    }

    #[allow(clippy::too_many_arguments)]
    pub fn append_app_audit(&self, actor: &str, action: &str, app: &str, version: Option<&str>, sha256: Option<&str>, source: Option<&str>, detail: Option<&str>) -> Result<(), Error> {
        self.with(|c| {
            c.execute(
                "INSERT INTO app_audit (at, actor, action, app, version, sha256, source, detail) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                params![now_secs(), actor, action, app, version, sha256, source, detail],
            )?;
            // Keep the log bounded; the Apps page shows the last events.
            c.execute("DELETE FROM app_audit WHERE id < (SELECT MAX(id) FROM app_audit) - 500", [])?;
            Ok(())
        })
    }

    pub fn list_app_audit(&self, limit: i64) -> Result<Vec<artcraft_cloud_types::AuditEvent>, Error> {
        self.with(|c| {
            let mut stmt = c.prepare("SELECT id, at, actor, action, app, version, sha256, source, detail FROM app_audit ORDER BY id DESC LIMIT ?1")?;
            let rows = stmt.query_map(params![limit], |row| {
                Ok(artcraft_cloud_types::AuditEvent {
                    id: row.get(0)?,
                    at: row.get(1)?,
                    actor: row.get(2)?,
                    action: row.get(3)?,
                    app: row.get(4)?,
                    version: row.get(5)?,
                    sha256: row.get(6)?,
                    source: row.get(7)?,
                    detail: row.get(8)?,
                })
            })?;
            rows.collect::<Result<Vec<_>, _>>().map_err(Error::from)
        })
    }

    pub fn ack_unverified_app(&self, user_id: i64, app: &str) -> Result<(), Error> {
        self.with(|c| {
            c.execute(
                "INSERT INTO app_acks (user_id, app, acked_at) VALUES (?1, ?2, ?3)
                 ON CONFLICT(user_id, app) DO UPDATE SET acked_at = excluded.acked_at",
                params![user_id, app, now_secs()],
            )?;
            Ok(())
        })
    }

    pub fn unverified_acks(&self, user_id: i64) -> Result<Vec<String>, Error> {
        self.with(|c| {
            let mut stmt = c.prepare("SELECT app FROM app_acks WHERE user_id = ?1")?;
            let rows = stmt.query_map(params![user_id], |row| row.get::<_, String>(0))?;
            rows.collect::<Result<Vec<_>, _>>().map_err(Error::from)
        })
    }
}

fn map_account(row: &rusqlite::Row<'_>) -> rusqlite::Result<Account> {
    Ok(Account {
        id: row.get(0)?,
        username: row.get(1)?,
        display_name: row.get(2)?,
        role: row.get(3)?,
        disabled: row.get::<_, i64>(4)? != 0,
        password_hash: row.get(5)?,
        is_super: row.get::<_, i64>(6)? != 0,
    })
}

pub fn read_meta(c: &Connection, owner: i64, path: &str) -> Result<Option<Meta>, Error> {
    meta_of(c, owner, path)
}

pub fn write_meta(c: &Connection, owner: i64, path: &str, is_dir: bool, rev: i64, etag: &str) -> Result<(), Error> {
    c.execute(
        "INSERT INTO file_meta (owner_id, path, etag, rev, is_dir) VALUES (?1, ?2, ?3, ?4, ?5)
         ON CONFLICT(owner_id, path) DO UPDATE SET etag = excluded.etag, rev = excluded.rev, is_dir = excluded.is_dir",
        params![owner, path, etag, rev, is_dir as i64],
    )?;
    Ok(())
}

fn meta_of(c: &Connection, owner: i64, path: &str) -> Result<Option<Meta>, Error> {
    c.query_row(
        "SELECT etag, rev, is_dir FROM file_meta WHERE owner_id = ?1 AND path = ?2",
        params![owner, path],
        |row| {
            Ok(Meta { etag: row.get(0)?, rev: row.get(1)?, is_dir: row.get::<_, i64>(2)? != 0 })
        },
    )
    .optional()
    .map_err(Error::from)
}

fn share_select(filter: &str) -> String {
    format!(
        "SELECT s.id, s.owner_id, ou.username, s.path, s.grantee_id, gu.username, s.permission, s.token,
                s.password_hash, s.expires_at, s.group_id, g.name
         FROM shares s
         JOIN users ou ON ou.id = s.owner_id
         LEFT JOIN users gu ON gu.id = s.grantee_id
         LEFT JOIN groups g ON g.id = s.group_id
         WHERE {filter}"
    )
}

fn query_shares(c: &Connection, filter: &str, id: i64) -> Result<Vec<ShareRow>, Error> {
    let sql = format!("{} ORDER BY s.id DESC", share_select(filter));
    let mut stmt = c.prepare(&sql)?;
    let rows = stmt.query_map(params![id], map_share)?;
    rows.collect::<Result<Vec<_>, _>>().map_err(Error::from)
}

fn map_share(row: &rusqlite::Row<'_>) -> rusqlite::Result<ShareRow> {
    Ok(ShareRow {
        id: row.get(0)?,
        owner_id: row.get(1)?,
        owner_name: row.get(2)?,
        path: row.get(3)?,
        grantee_id: row.get(4)?,
        grantee_name: row.get(5)?,
        permission: row.get(6)?,
        token: row.get(7)?,
        password_hash: row.get(8)?,
        expires_at: row.get(9)?,
        group_id: row.get(10)?,
        group_name: row.get(11)?,
    })
}

const SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS users (
    id INTEGER PRIMARY KEY,
    username TEXT NOT NULL UNIQUE COLLATE NOCASE,
    password_hash TEXT NOT NULL,
    role TEXT NOT NULL CHECK (role IN ('admin', 'user')),
    disabled INTEGER NOT NULL DEFAULT 0,
    display_name TEXT NOT NULL,
    created_at INTEGER NOT NULL
);
CREATE TABLE IF NOT EXISTS mail_config (
    id INTEGER PRIMARY KEY CHECK (id = 1),
    base_url TEXT NOT NULL,
    auth TEXT NOT NULL CHECK (auth IN ('basic', 'bearer')),
    username TEXT NOT NULL,
    secret BLOB,
    updated_at INTEGER NOT NULL
);
CREATE TABLE IF NOT EXISTS mailboxes (
    user_id INTEGER PRIMARY KEY REFERENCES users(id) ON DELETE CASCADE,
    address TEXT NOT NULL UNIQUE COLLATE NOCASE,
    account_id TEXT NOT NULL,
    secret BLOB NOT NULL,
    created_at INTEGER NOT NULL
);
CREATE TABLE IF NOT EXISTS sessions (
    token_hash TEXT PRIMARY KEY,
    user_id INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    expires_at INTEGER NOT NULL,
    created_at INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_sessions_user ON sessions(user_id);
CREATE TABLE IF NOT EXISTS file_meta (
    owner_id INTEGER NOT NULL,
    path TEXT NOT NULL,
    etag TEXT NOT NULL,
    rev INTEGER NOT NULL,
    is_dir INTEGER NOT NULL,
    PRIMARY KEY (owner_id, path)
);
CREATE TABLE IF NOT EXISTS trash (
    id TEXT PRIMARY KEY,
    owner_id INTEGER NOT NULL,
    original_path TEXT NOT NULL,
    name TEXT NOT NULL,
    is_dir INTEGER NOT NULL,
    size INTEGER NOT NULL,
    deleted_at INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_trash_owner ON trash(owner_id);
CREATE TABLE IF NOT EXISTS shares (
    id INTEGER PRIMARY KEY,
    owner_id INTEGER NOT NULL REFERENCES users(id),
    path TEXT NOT NULL,
    grantee_id INTEGER REFERENCES users(id),
    permission TEXT NOT NULL CHECK (permission IN ('view', 'edit')),
    token TEXT UNIQUE,
    password_hash TEXT,
    expires_at INTEGER,
    created_at INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_shares_owner ON shares(owner_id);
CREATE INDEX IF NOT EXISTS idx_shares_grantee ON shares(grantee_id);
CREATE TABLE IF NOT EXISTS groups (
    id INTEGER PRIMARY KEY,
    name TEXT NOT NULL UNIQUE COLLATE NOCASE,
    created_at INTEGER NOT NULL
);
CREATE TABLE IF NOT EXISTS group_members (
    group_id INTEGER NOT NULL REFERENCES groups(id) ON DELETE CASCADE,
    user_id INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    PRIMARY KEY (group_id, user_id)
);
CREATE INDEX IF NOT EXISTS idx_group_members_user ON group_members(user_id);
CREATE TABLE IF NOT EXISTS app_settings (
    user_id INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    app TEXT NOT NULL,
    data TEXT NOT NULL,
    updated_at INTEGER NOT NULL,
    PRIMARY KEY (user_id, app)
);
CREATE TABLE IF NOT EXISTS calendar_remote_credentials (
    user_id INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    account_id TEXT NOT NULL,
    username TEXT NOT NULL DEFAULT '',
    secret BLOB NOT NULL,
    updated_at INTEGER NOT NULL,
    PRIMARY KEY (user_id, account_id)
);
CREATE TABLE IF NOT EXISTS app_audit (
    id INTEGER PRIMARY KEY,
    at INTEGER NOT NULL,
    actor TEXT NOT NULL,
    action TEXT NOT NULL,
    app TEXT NOT NULL,
    version TEXT,
    sha256 TEXT,
    source TEXT,
    detail TEXT
);
CREATE INDEX IF NOT EXISTS idx_app_audit_at ON app_audit(at DESC);
CREATE TABLE IF NOT EXISTS app_acks (
    user_id INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    app TEXT NOT NULL,
    acked_at INTEGER NOT NULL,
    PRIMARY KEY (user_id, app)
);
"#;

const MAILBOX_COLUMNS: &str = "user_id, address, account_id, secret, proxy_kind, proxy_id, password_sync, sync_error, pending_secret, sync_attempts, suspended, suspend_added, access_error";

fn mailbox_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<MailboxRow> {
    Ok(MailboxRow {
        user_id: row.get(0)?,
        address: row.get(1)?,
        account_id: row.get(2)?,
        secret: row.get(3)?,
        proxy_kind: row.get(4)?,
        proxy_id: row.get(5)?,
        password_sync: row.get(6)?,
        sync_error: row.get(7)?,
        pending_secret: row.get(8)?,
        sync_attempts: row.get(9)?,
        suspended: row.get(10)?,
        suspend_added: row.get(11)?,
        access_error: row.get(12)?,
    })
}

/// Columns added after the first release. `ALTER TABLE ... ADD COLUMN` is not idempotent in
/// SQLite, so each one is added only when `table_info` does not list it yet.
fn migrate(c: &Connection) -> Result<(), Error> {
    if !has_column(c, "users", "email")? {
        c.execute_batch("ALTER TABLE users ADD COLUMN email TEXT")?;
    }
    c.execute_batch("CREATE UNIQUE INDEX IF NOT EXISTS idx_users_email ON users(email COLLATE NOCASE) WHERE email IS NOT NULL")?;
    if !has_column(c, "users", "is_super")? {
        c.execute_batch("ALTER TABLE users ADD COLUMN is_super INTEGER NOT NULL DEFAULT 0")?;
    }
    if !has_column(c, "mail_config", "public_host")? {
        c.execute_batch("ALTER TABLE mail_config ADD COLUMN public_host TEXT NOT NULL DEFAULT ''")?;
    }
    for (column, kind) in [
        ("proxy_kind", "TEXT NOT NULL DEFAULT 'password'"),
        ("proxy_id", "TEXT"),
        ("password_sync", "TEXT NOT NULL DEFAULT 'pending'"),
        ("sync_error", "TEXT"),
        ("pending_secret", "BLOB"),
        ("sync_attempts", "INTEGER NOT NULL DEFAULT 0"),
        ("synced_at", "INTEGER"),
        ("suspended", "INTEGER NOT NULL DEFAULT 0"),
        ("suspend_added", "TEXT NOT NULL DEFAULT ''"),
        ("access_error", "TEXT"),
    ] {
        if !has_column(c, "mailboxes", column)? {
            c.execute_batch(&format!("ALTER TABLE mailboxes ADD COLUMN {column} {kind}"))?;
        }
    }
    if !has_column(c, "shares", "group_id")? {
        c.execute_batch("ALTER TABLE shares ADD COLUMN group_id INTEGER REFERENCES groups(id) ON DELETE CASCADE")?;
    }
    c.execute_batch("CREATE INDEX IF NOT EXISTS idx_shares_group ON shares(group_id)")?;
    c.execute_batch("CREATE TABLE IF NOT EXISTS calendar_remote_credentials (
    user_id INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    account_id TEXT NOT NULL,
    username TEXT NOT NULL DEFAULT '',
    secret BLOB NOT NULL,
    updated_at INTEGER NOT NULL,
    PRIMARY KEY (user_id, account_id)
)")?;
    c.execute_batch("CREATE TABLE IF NOT EXISTS app_audit (
    id INTEGER PRIMARY KEY,
    at INTEGER NOT NULL,
    actor TEXT NOT NULL,
    action TEXT NOT NULL,
    app TEXT NOT NULL,
    version TEXT,
    sha256 TEXT,
    source TEXT,
    detail TEXT
)")?;
    c.execute_batch("CREATE INDEX IF NOT EXISTS idx_app_audit_at ON app_audit(at DESC)")?;
    c.execute_batch("CREATE TABLE IF NOT EXISTS app_acks (
    user_id INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    app TEXT NOT NULL,
    acked_at INTEGER NOT NULL,
    PRIMARY KEY (user_id, app)
)")?;
    ensure_super(c)
}

fn has_column(c: &Connection, table: &str, column: &str) -> Result<bool, Error> {
    let mut stmt = c.prepare(&format!("PRAGMA table_info({table})"))?;
    let names = stmt.query_map([], |row| row.get::<_, String>(1))?.collect::<Result<Vec<_>, _>>()?;
    Ok(names.iter().any(|name| name == column))
}

/// The first admin is the super admin. Runs at start-up (existing installs) and after every new
/// account (the bootstrap admin), and only ever picks one when there is none.
fn ensure_super(c: &Connection) -> Result<(), Error> {
    c.execute(
        "UPDATE users SET is_super = 1
         WHERE id = (SELECT MIN(id) FROM users WHERE role = 'admin')
           AND NOT EXISTS (SELECT 1 FROM users WHERE is_super = 1)",
        [],
    )?;
    Ok(())
}

#[cfg(test)]
mod migration_tests {
    use super::*;

    #[test]
    fn an_existing_install_gets_a_super_admin_and_group_shares() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("old.db");
        {
            let c = Connection::open(&path).unwrap();
            c.execute_batch(
                "CREATE TABLE users (id INTEGER PRIMARY KEY, username TEXT NOT NULL UNIQUE COLLATE NOCASE, password_hash TEXT NOT NULL,
                     role TEXT NOT NULL CHECK (role IN ('admin', 'user')), disabled INTEGER NOT NULL DEFAULT 0, display_name TEXT NOT NULL, created_at INTEGER NOT NULL);
                 CREATE TABLE shares (id INTEGER PRIMARY KEY, owner_id INTEGER NOT NULL REFERENCES users(id), path TEXT NOT NULL, grantee_id INTEGER REFERENCES users(id),
                     permission TEXT NOT NULL CHECK (permission IN ('view', 'edit')), token TEXT UNIQUE, password_hash TEXT, expires_at INTEGER, created_at INTEGER NOT NULL);
                 INSERT INTO users VALUES (1, 'zed', 'h', 'user', 0, 'Zed', 0), (2, 'root', 'h', 'admin', 0, 'Root', 0), (3, 'second', 'h', 'admin', 0, 'Second', 0);",
            )
            .unwrap();
        }
        let db = Db::open(&path).unwrap();
        let supers: Vec<String> = db.list_users().unwrap().into_iter().filter(|a| a.is_super).map(|a| a.username).collect();
        assert_eq!(supers, vec!["root".to_string()], "the lowest-id admin becomes the super admin");
        drop(db);
        let db = Db::open(&path).unwrap();
        assert_eq!(db.list_users().unwrap().iter().filter(|a| a.is_super).count(), 1, "opening again changes nothing");
        assert!(db.shares_out(2).unwrap().is_empty());
    }

    #[test]
    fn an_existing_install_gains_calendar_remote_credentials() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("old.db");
        {
            let c = Connection::open(&path).unwrap();
            c.execute_batch(
                "CREATE TABLE users (id INTEGER PRIMARY KEY, username TEXT NOT NULL UNIQUE COLLATE NOCASE, password_hash TEXT NOT NULL,
                     role TEXT NOT NULL CHECK (role IN ('admin', 'user')), disabled INTEGER NOT NULL DEFAULT 0, display_name TEXT NOT NULL, created_at INTEGER NOT NULL);
                 INSERT INTO users VALUES (1, 'ada', 'h', 'user', 0, 'Ada', 0);",
            )
            .unwrap();
        }
        let db = Db::open(&path).unwrap();
        db.set_calendar_remote_credential(1, "acc_one", "ada", b"not-a-real-sealed-blob-but-opaque").unwrap();
        let row = db.calendar_remote_credential(1, "acc_one").unwrap().expect("row");
        assert_eq!(row.username, "ada");
        assert_eq!(row.secret, b"not-a-real-sealed-blob-but-opaque");
        assert!(db.calendar_remote_credential(1, "acc_other").unwrap().is_none());
        db.delete_calendar_remote_credential(1, "acc_one").unwrap();
        assert!(db.calendar_remote_credential(1, "acc_one").unwrap().is_none());
    }
}

