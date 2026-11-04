//! Deployment identities and revocable sessions. Passwords never leave this module.
use crate::{
    Error, Result,
    config::{Settings, read_json, safe_path, write_json},
    store::{now, text},
};
use axum::http::{HeaderMap, StatusCode};
use rusqlite::{Connection, OptionalExtension, params};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    path::PathBuf,
    sync::{
        Mutex,
        atomic::{AtomicBool, Ordering},
    },
};

pub struct Accounts {
    db: Mutex<Connection>,
    pub(crate) enabled: AtomicBool,
    config: PathBuf,
    pub(crate) cookie_secure: bool,
    lifetime: f64,
    registration: bool,
    pub(crate) origins: Vec<String>,
    dummy_hash: String,
}

pub fn forbidden() -> Error {
    Error::new(
        StatusCode::FORBIDDEN,
        "forbidden",
        "Administrator access is required.",
    )
}
pub fn unauthorized() -> Error {
    Error::new(
        StatusCode::UNAUTHORIZED,
        "unauthorized",
        "Sign in to continue.",
    )
}
pub fn validate_id(id: &str) -> Result<()> {
    if id.is_empty()
        || id.len() > 128
        || !id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"_-".contains(&b))
    {
        return Err(Error::conflict("Invalid stored account identity."));
    }
    Ok(())
}
fn hash(password: &str) -> Result<String> {
    // Reject instead of silently truncating bcrypt's 72-byte input limit.
    if password.chars().count() < 8 || password.len() > 72 {
        return Err(Error::invalid(
            "Password must contain at least 8 characters and at most 72 UTF-8 bytes.",
        ));
    }
    bcrypt::hash(password, bcrypt::DEFAULT_COST)
        .map_err(|_| Error::conflict("Unable to hash password."))
}
fn verify(password: &str, encoded: &str) -> bool {
    password.len() <= 72 && bcrypt::verify(password, encoded).unwrap_or(false)
}
fn username(name: &str) -> Result<&str> {
    let name = name.trim();
    if name.is_empty() || name.chars().count() > 128 || name.chars().any(char::is_control) {
        return Err(Error::invalid("Username must contain 1-128 characters."));
    }
    Ok(name)
}
fn timestamp(db: &Connection) -> Result<String> {
    Ok(
        db.query_row("SELECT strftime('%Y-%m-%dT%H:%M:%fZ','now')", [], |r| {
            r.get(0)
        })?,
    )
}
fn read_user(db: &Connection, field: &str, name: &str) -> Result<Option<Value>> {
    let query = match field {
        "id" => "SELECT record FROM accounts WHERE id=?",
        _ => "SELECT record FROM accounts WHERE username=?",
    };
    db.query_row(query, [name], |r| r.get::<_, String>(0))
        .optional()?
        .map(|s| serde_json::from_str(&s).map_err(|_| Error::conflict("Invalid account record.")))
        .transpose()
}
fn save(db: &Connection, user: &Value) -> Result<()> {
    db.execute(
        "UPDATE accounts SET record=? WHERE id=?",
        params![user.to_string(), text(user, "id")],
    )?;
    Ok(())
}
pub fn public(mut user: Value) -> Value {
    if let Some(map) = user.as_object_mut() {
        map.remove("hash");
        map.remove("password_hash");
    }
    user["is_admin"] = json!(user["role"] == "admin");
    user["user_id"] = user["id"].clone();
    user["can_change_password"] = json!(true);
    user
}
pub(crate) fn token(headers: &HeaderMap) -> Option<String> {
    if let Some(value) = headers
        .get("authorization")
        .and_then(|h| h.to_str().ok())
        .and_then(|s| s.strip_prefix("Bearer "))
    {
        return Some(value.to_owned());
    }
    headers
        .get("cookie")
        .and_then(|h| h.to_str().ok())?
        .split(';')
        .find_map(|part| part.trim().strip_prefix("dt_token=").map(str::to_owned))
}
fn digest(token: &str) -> String {
    format!("{:x}", Sha256::digest(token.as_bytes()))
}

// Flat legacy stores assigned the first entry to the administrator. Preserve
// source order rather than serde_json::Map's alphabetical order during import.
struct LegacyUsers(Vec<(String, Value)>);
impl<'de> serde::Deserialize<'de> for LegacyUsers {
    fn deserialize<D: serde::Deserializer<'de>>(
        deserializer: D,
    ) -> std::result::Result<Self, D::Error> {
        struct Visitor;
        impl<'de> serde::de::Visitor<'de> for Visitor {
            type Value = LegacyUsers;
            fn expecting(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
                formatter.write_str("an account object")
            }
            fn visit_map<M: serde::de::MapAccess<'de>>(
                self,
                mut map: M,
            ) -> std::result::Result<Self::Value, M::Error> {
                let mut records = Vec::new();
                let mut names = std::collections::HashSet::new();
                while let Some((name, value)) = map.next_entry::<String, Value>()? {
                    if !names.insert(name.clone()) {
                        return Err(serde::de::Error::custom(
                            "Duplicate username in legacy store.",
                        ));
                    }
                    records.push((name, value));
                }
                Ok(LegacyUsers(records))
            }
        }
        deserializer.deserialize_map(Visitor)
    }
}

impl Accounts {
    pub fn open(settings: &Settings) -> Result<Self> {
        let config = settings.user_root.join("settings/auth.json");
        let options = read_json(&config, json!({}))?;
        if !options.is_object() {
            return Err(Error::conflict("Invalid authentication settings."));
        }
        for key in ["enabled", "cookie_secure", "allow_registration"] {
            if options.get(key).is_some_and(|v| !v.is_boolean()) {
                return Err(Error::conflict(
                    "Authentication toggles must be JSON booleans.",
                ));
            }
        }
        let data = settings.user_root.parent().ok_or_else(Error::missing)?;
        let path = data.join("system/auth/native.sqlite3");
        safe_path(data, &path)?;
        std::fs::create_dir_all(path.parent().unwrap())?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(
                path.parent().unwrap(),
                std::fs::Permissions::from_mode(0o700),
            )?;
        }
        let mut db = Connection::open(&path)?;
        db.busy_timeout(std::time::Duration::from_secs(10))?;
        db.execute_batch("PRAGMA foreign_keys=ON; PRAGMA journal_mode=WAL;
            CREATE TABLE IF NOT EXISTS accounts(id TEXT PRIMARY KEY,username TEXT NOT NULL UNIQUE,record TEXT NOT NULL);
            CREATE TABLE IF NOT EXISTS auth_sessions(token_hash TEXT PRIMARY KEY,user_id TEXT NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,expires REAL NOT NULL);
            CREATE INDEX IF NOT EXISTS auth_session_user ON auth_sessions(user_id);
            CREATE TABLE IF NOT EXISTS login_attempts(username TEXT PRIMARY KEY,failures INTEGER NOT NULL,until REAL NOT NULL);
            CREATE TABLE IF NOT EXISTS auth_metadata(key TEXT PRIMARY KEY,value TEXT NOT NULL);
            CREATE TABLE IF NOT EXISTS guardianships(id TEXT PRIMARY KEY,guardian_id TEXT NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,learner_id TEXT NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,permissions TEXT NOT NULL,revoked_at REAL,UNIQUE(guardian_id,learner_id));")?;
        let imported: bool = db.query_row(
            "SELECT EXISTS(SELECT 1 FROM auth_metadata WHERE key='legacy_imported')",
            [],
            |r| r.get(0),
        )?;
        if !imported {
            let tx = db.transaction()?;
            let sources = [
                data.join("system/auth/users.json"),
                settings.user_root.join("auth_users.json"),
                data.parent()
                    .unwrap()
                    .join("multi-user/_system/auth/users.json"),
            ];
            let mut users = Vec::new();
            for source in sources {
                safe_path(data.parent().unwrap(), &source)?;
                if source.try_exists()? {
                    users = serde_json::from_slice::<LegacyUsers>(&std::fs::read(&source)?)
                        .map_err(|_| Error::conflict("Invalid existing user store; restore it before enabling authentication."))?.0;
                    break;
                }
            }
            if !text(&options, "username").is_empty()
                && !text(&options, "password_hash").is_empty()
                && !users
                    .iter()
                    .any(|(name, _)| name == text(&options, "username"))
            {
                users.push((
                    text(&options, "username").to_owned(),
                    json!({"id":"env-admin","hash":options["password_hash"],"role":"admin"}),
                ));
            }
            for (index, (name, record)) in users.iter().enumerate() {
                username(name)?;
                let mut user = if let Some(encoded) = record.as_str() {
                    json!({"hash":encoded})
                } else {
                    record.clone()
                };
                if !user.is_object() {
                    return Err(Error::conflict("Invalid existing account."));
                }
                if text(&user, "hash").is_empty() {
                    user["hash"] = user["password_hash"].clone();
                }
                if text(&user, "hash").is_empty() {
                    return Err(Error::conflict("Existing account has no password hash."));
                }
                if text(&user, "id").is_empty() {
                    user["id"] = json!(format!("u_{}", uuid::Uuid::new_v4().simple()));
                }
                validate_id(text(&user, "id"))?;
                if text(&user, "role").is_empty() {
                    user["role"] = json!(if index == 0 { "admin" } else { "user" });
                }
                if !["admin", "user", "teacher", "student"].contains(&text(&user, "role")) {
                    return Err(Error::conflict("Invalid stored account role."));
                }
                user["username"] = json!(name);
                for (key, default) in [
                    ("preset", json!("standard")),
                    ("disabled", json!(false)),
                    ("avatar", json!("")),
                    ("must_change_password", json!(false)),
                    ("created_at", json!(timestamp(&tx)?)),
                ] {
                    if user[key].is_null() {
                        user[key] = default;
                    }
                }
                if !user["disabled"].is_boolean()
                    || !user["must_change_password"].is_boolean()
                    || !["standard", "learner", "custom"].contains(&text(&user, "preset"))
                {
                    return Err(Error::conflict("Invalid existing account flags or preset."));
                }
                tx.execute(
                    "INSERT INTO accounts(id,username,record) VALUES (?,?,?)",
                    params![text(&user, "id"), name, user.to_string()],
                )?;
            }
            tx.execute(
                "INSERT INTO auth_metadata VALUES ('legacy_imported','1')",
                [],
            )?;
            tx.commit()?;
        }
        let enabled = match std::env::var("DEEPTUTOR_AUTH_ENABLED") {
            Ok(value) => match value.trim().to_lowercase().as_str() {
                "1" | "true" | "yes" | "on" => true,
                "0" | "false" | "no" | "off" => false,
                "" => options["enabled"] == true,
                _ => {
                    return Err(Error::conflict(
                        "DEEPTUTOR_AUTH_ENABLED must be true or false.",
                    ));
                }
            },
            Err(_) => options["enabled"] == true,
        };
        Ok(Self {
            db: Mutex::new(db),
            enabled: AtomicBool::new(enabled),
            config,
            cookie_secure: options["cookie_secure"] == true,
            lifetime: options["token_expire_hours"]
                .as_f64()
                .unwrap_or(24.0)
                .clamp(1.0, 720.0)
                * 3600.0,
            registration: options["allow_registration"].as_bool().unwrap_or(true),
            origins: options["allowed_origins"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|v| v.as_str().map(str::to_owned))
                .collect(),
            // A fixed, valid cost-12 hash equalizes unknown-account verification
            // without doing a second expensive hash at every application open.
            dummy_hash: "$2y$12$L6Bc/AlTQHyd9liGgGEZyOFLPHNgyxeEPfgYfBCVxJ7JIlwxyVU3u".into(),
        })
    }
    pub fn is_enabled(&self) -> bool {
        self.enabled.load(Ordering::Acquire)
    }
    fn lock(&self) -> Result<std::sync::MutexGuard<'_, Connection>> {
        self.db
            .lock()
            .map_err(|_| Error::conflict("Account storage lock failed."))
    }
    pub fn first(&self) -> Result<bool> {
        Ok(self
            .lock()?
            .query_row("SELECT COUNT(*)=0 FROM accounts", [], |r| r.get(0))?)
    }
    pub fn resolve(&self, token: Option<&str>) -> Result<Option<Value>> {
        let Some(token) = token.filter(|t| t.len() <= 512) else {
            return Ok(None);
        };
        let db = self.lock()?;
        let id: Option<String> = db
            .query_row(
                "SELECT user_id FROM auth_sessions WHERE token_hash=? AND expires>?",
                params![digest(token), now()],
                |r| r.get(0),
            )
            .optional()?;
        Ok(id
            .map(|id| read_user(&db, "id", &id))
            .transpose()?
            .flatten()
            .filter(|u| u["disabled"] != true)
            .map(public))
    }
    pub fn user(&self, id: &str) -> Result<Value> {
        let db = self.lock()?;
        read_user(&db, "id", id)?
            .map(public)
            .ok_or_else(Error::missing)
    }
    pub fn list(&self) -> Result<Vec<Value>> {
        let db = self.lock()?;
        let records = db
            .prepare("SELECT record FROM accounts ORDER BY username")?
            .query_map([], |r| r.get::<_, String>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        records
            .into_iter()
            .map(|s| {
                serde_json::from_str(&s)
                    .map(public)
                    .map_err(|_| Error::conflict("Invalid account record."))
            })
            .collect()
    }
    fn insert(
        db: &Connection,
        name: &str,
        password: &str,
        preset: &str,
        admin: bool,
    ) -> Result<Value> {
        let name = username(name)?;
        if !["standard", "learner", "custom"].contains(&preset) {
            return Err(Error::invalid("Invalid account preset."));
        }
        if read_user(db, "username", name)?.is_some() {
            return Err(Error::conflict("Username already exists."));
        }
        let record = json!({"id":format!("u_{}",uuid::Uuid::new_v4().simple()),"username":name,"hash":hash(password)?,"role":if admin{"admin"}else{"user"},"preset":preset,"created_at":timestamp(db)?,"disabled":false,"avatar":"","must_change_password":!admin});
        db.execute(
            "INSERT INTO accounts VALUES (?,?,?)",
            params![text(&record, "id"), name, record.to_string()],
        )?;
        Ok(public(record))
    }
    pub fn create(
        &self,
        name: &str,
        password: &str,
        preset: &str,
        registration: bool,
    ) -> Result<Value> {
        if !self.is_enabled() {
            return Err(Error::invalid(
                "Enable multi-user mode before creating accounts.",
            ));
        }
        let mut db = self.lock()?;
        let tx = db.transaction()?;
        let first: bool = tx.query_row("SELECT COUNT(*)=0 FROM accounts", [], |r| r.get(0))?;
        if registration && !first && !self.registration {
            return Err(Error::new(
                StatusCode::FORBIDDEN,
                "registration_closed",
                "Registration is disabled. Ask an administrator to create your account.",
            ));
        }
        let mut user = Self::insert(&tx, name, password, preset, registration && first)?;
        if registration {
            let mut stored = read_user(&tx, "id", text(&user, "id"))?.unwrap();
            stored["must_change_password"] = json!(false);
            save(&tx, &stored)?;
            user = public(stored);
        }
        tx.commit()?;
        Ok(user)
    }
    fn authenticate(&self, db: &Connection, name: &str, password: &str) -> Result<Value> {
        let attempts: Option<(i64, f64)> = db
            .query_row(
                "SELECT failures,until FROM login_attempts WHERE username=?",
                [name],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?;
        if attempts.is_some_and(|(n, until)| n >= 10 && until > now()) {
            return Err(Error::new(
                StatusCode::TOO_MANY_REQUESTS,
                "login_rate_limited",
                "Too many login attempts. Try again in 15 minutes.",
            ));
        }
        let user = read_user(db, "username", name)?;
        let valid = verify(
            password,
            user.as_ref()
                .map(|u| text(u, "hash"))
                .unwrap_or(&self.dummy_hash),
        );
        if !valid || user.as_ref().is_none_or(|u| u["disabled"] == true) {
            db.execute("INSERT INTO login_attempts VALUES (?,1,?) ON CONFLICT(username) DO UPDATE SET failures=CASE WHEN until<? THEN 1 ELSE failures+1 END,until=excluded.until",params![name,now()+900.0,now()])?;
            return Err(Error::new(
                StatusCode::UNAUTHORIZED,
                "invalid_credentials",
                "Invalid username or password.",
            ));
        }
        db.execute("DELETE FROM login_attempts WHERE username=?", [name])?;
        Ok(public(user.unwrap()))
    }
    fn issue(&self, db: &Connection, user: &Value) -> Result<String> {
        let token = format!(
            "{}{}",
            uuid::Uuid::new_v4().simple(),
            uuid::Uuid::new_v4().simple()
        );
        db.execute("DELETE FROM auth_sessions WHERE expires<=?", [now()])?;
        db.execute(
            "INSERT INTO auth_sessions VALUES (?,?,?)",
            params![digest(&token), text(user, "id"), now() + self.lifetime],
        )?;
        Ok(token)
    }
    pub fn login(&self, name: &str, password: &str, setup: bool) -> Result<(Value, String)> {
        let name = username(name)?;
        let db = self.lock()?;
        if setup == self.is_enabled() {
            return Err(Error::conflict(if setup {
                "Multi-user mode is already enabled."
            } else {
                "Multi-user mode is disabled."
            }));
        }
        let first: bool = db.query_row("SELECT COUNT(*)=0 FROM accounts", [], |r| r.get(0))?;
        let user = if setup && first {
            Self::insert(&db, name, password, "standard", true)?
        } else {
            self.authenticate(&db, name, password)?
        };
        if setup {
            if user["role"] != "admin" {
                return Err(forbidden());
            }
            let mut config = read_json(&self.config, json!({}))?;
            config["enabled"] = json!(true);
            write_json(&self.config, &config)?;
            self.enabled.store(true, Ordering::Release);
        }
        let token = self.issue(&db, &user)?;
        Ok((user, token))
    }
    pub fn cookie(&self, token: &str, clear: bool) -> String {
        format!(
            "dt_token={}; Path=/; HttpOnly; SameSite=Lax; Max-Age={}{}",
            token,
            if clear { 0 } else { self.lifetime as u64 },
            if self.cookie_secure { "; Secure" } else { "" }
        )
    }
    pub fn logout(&self, token: Option<&str>) -> Result<()> {
        if let Some(token) = token {
            self.lock()?.execute(
                "DELETE FROM auth_sessions WHERE token_hash=?",
                [digest(token)],
            )?;
        }
        Ok(())
    }
    pub fn change_role_or_delete(
        &self,
        actor: &str,
        name: &str,
        role: Option<&str>,
    ) -> Result<Value> {
        if let Some(role) = role
            && !["admin", "user", "teacher", "student"].contains(&role)
        {
            return Err(Error::invalid("Invalid account role."));
        }
        let mut db = self.lock()?;
        let tx = db.transaction()?;
        let mut user = read_user(&tx, "username", name)?.ok_or_else(Error::missing)?;
        if text(&user, "id") == actor {
            return Err(Error::invalid(
                "You cannot delete your own account or change your own role.",
            ));
        }
        if user["role"] == "admin" && role != Some("admin") {
            let admins:i64=tx.query_row("SELECT COUNT(*) FROM accounts WHERE json_extract(record,'$.role')='admin' AND COALESCE(json_extract(record,'$.disabled'),0)=0",[],|r|r.get(0))?;
            if admins <= 1 {
                return Err(Error::invalid("The last administrator cannot be removed."));
            }
        }
        tx.execute(
            "DELETE FROM auth_sessions WHERE user_id=?",
            [text(&user, "id")],
        )?;
        if let Some(role) = role {
            user["role"] = json!(role);
            save(&tx, &user)?;
        } else {
            tx.execute("DELETE FROM accounts WHERE id=?", [text(&user, "id")])?;
        }
        tx.commit()?;
        Ok(public(user))
    }
    pub fn password(&self, id: &str, current: &str, new: &str) -> Result<String> {
        let mut db = self.lock()?;
        let tx = db.transaction()?;
        let mut user = read_user(&tx, "id", id)?.ok_or_else(unauthorized)?;
        if !verify(current, text(&user, "hash")) {
            return Err(Error::invalid("Current password is incorrect."));
        }
        if verify(new, text(&user, "hash")) {
            return Err(Error::invalid(
                "Choose a password different from your current password.",
            ));
        }
        user["hash"] = json!(hash(new)?);
        user["must_change_password"] = json!(false);
        save(&tx, &user)?;
        tx.execute("DELETE FROM auth_sessions WHERE user_id=?", [id])?;
        let token = self.issue(&tx, &user)?;
        tx.commit()?;
        Ok(token)
    }
    pub(crate) fn guardianships(&self, user: Option<&str>) -> Result<Vec<Value>> {
        let db = self.lock()?;
        let mut stmt = db.prepare("SELECT g.id,g.guardian_id,g.learner_id,g.permissions,g.revoked_at,a.username,l.username FROM guardianships g JOIN accounts a ON a.id=g.guardian_id JOIN accounts l ON l.id=g.learner_id WHERE (? IS NULL OR g.guardian_id=? OR g.learner_id=?) ORDER BY g.id")?;
        Ok(stmt.query_map(params![user,user,user], |r| {
            let permissions: String = r.get(3)?;
            Ok(json!({"id":r.get::<_,String>(0)?,"guardian_user_id":r.get::<_,String>(1)?,"learner_user_id":r.get::<_,String>(2)?,"permissions":serde_json::from_str::<Value>(&permissions).unwrap_or(json!([])),"revoked_at":r.get::<_,Option<f64>>(4)?,"guardian_username":r.get::<_,String>(5)?,"learner_username":r.get::<_,String>(6)?}))
        })?.collect::<rusqlite::Result<Vec<_>>>()?)
    }
    pub(crate) fn authorize_guardian(
        &self,
        guardian: &str,
        learner: &str,
        permissions: &Value,
    ) -> Result<Value> {
        let values = permissions
            .as_array()
            .filter(|a| !a.is_empty() && a.len() <= 4)
            .ok_or_else(|| Error::invalid("Select guardian permissions."))?;
        if values.iter().any(|v| {
            ![
                "assign_materials",
                "manage_restrictions",
                "view_reports",
                "reset_credentials",
            ]
            .iter()
            .any(|p| v == p)
        }) {
            return Err(Error::invalid("Invalid guardian permission."));
        }
        let mut db = self.lock()?;
        let tx = db.transaction()?;
        let adult = read_user(&tx, "id", guardian)?.ok_or_else(Error::missing)?;
        let child = read_user(&tx, "id", learner)?.ok_or_else(Error::missing)?;
        if guardian == learner
            || adult["preset"] == "learner"
            || adult["disabled"] == true
            || child["preset"] != "learner"
            || child["role"] == "admin"
        {
            return Err(Error::invalid(
                "Choose an active adult account and a learner account.",
            ));
        }
        let existing: Option<String> = tx
            .query_row(
                "SELECT id FROM guardianships WHERE guardian_id=? AND learner_id=?",
                params![guardian, learner],
                |r| r.get(0),
            )
            .optional()?;
        let id = existing.unwrap_or_else(|| format!("guardian_{}", uuid::Uuid::new_v4().simple()));
        tx.execute("INSERT INTO guardianships VALUES (?,?,?,?,NULL) ON CONFLICT(guardian_id,learner_id) DO UPDATE SET permissions=excluded.permissions,revoked_at=NULL",params![id,guardian,learner,permissions.to_string()])?;
        tx.commit()?;
        Ok(
            json!({"id":id,"guardian_user_id":guardian,"learner_user_id":learner,"guardian_username":adult["username"],"learner_username":child["username"],"permissions":permissions,"revoked_at":null}),
        )
    }
    pub(crate) fn revoke_guardian(&self, actor: &Value, id: &str) -> Result<()> {
        let db = self.lock()?;
        let changed = db.execute("UPDATE guardianships SET revoked_at=? WHERE id=? AND (?='admin' OR guardian_id=? OR learner_id=?)",params![now(),id,text(actor,"role"),text(actor,"id"),text(actor,"id")])?;
        if changed == 0 {
            return Err(Error::missing());
        }
        Ok(())
    }
    pub(crate) fn guardian_permission(
        &self,
        actor: &Value,
        learner: &str,
        permission: &str,
    ) -> Result<Value> {
        let db = self.lock()?;
        Self::check_guardian(&db, actor, learner, permission)
    }
    fn check_guardian(
        db: &Connection,
        actor: &Value,
        learner: &str,
        permission: &str,
    ) -> Result<Value> {
        let child = read_user(db, "id", learner)?
            .filter(|u| u["preset"] == "learner" && u["role"] != "admin")
            .ok_or_else(Error::missing)?;
        if actor["role"] != "admin" {
            let raw: Option<String> = db.query_row("SELECT permissions FROM guardianships WHERE guardian_id=? AND learner_id=? AND revoked_at IS NULL",params![text(actor,"id"),learner],|r|r.get(0)).optional()?;
            let permissions: Value = raw
                .and_then(|v| serde_json::from_str(&v).ok())
                .unwrap_or(json!([]));
            if !permissions
                .as_array()
                .is_some_and(|p| p.iter().any(|p| p == permission))
            {
                return Err(forbidden());
            }
        }
        Ok(public(child))
    }
    pub(crate) fn reset_learner(&self, actor: &Value, learner: &str, password: &str) -> Result<()> {
        let mut db = self.lock()?;
        let tx = db.transaction()?;
        Self::check_guardian(&tx, actor, learner, "reset_credentials")?;
        let mut user = read_user(&tx, "id", learner)?.ok_or_else(Error::missing)?;
        user["hash"] = json!(hash(password)?);
        user["must_change_password"] = json!(true);
        save(&tx, &user)?;
        tx.execute("DELETE FROM auth_sessions WHERE user_id=?", [learner])?;
        tx.commit()?;
        Ok(())
    }
    pub fn update_profile(&self, id: &str, key: &str, value: Value) -> Result<Value> {
        let db = self.lock()?;
        let mut user = read_user(&db, "id", id)?.ok_or_else(Error::missing)?;
        user[key] = value;
        save(&db, &user)?;
        Ok(public(user))
    }
}

pub(crate) fn grant_path(settings: &Settings, id: &str) -> Result<PathBuf> {
    validate_id(id)?;
    let data = settings.user_root.parent().ok_or_else(Error::missing)?;
    let path = data.join("system/grants").join(format!("{id}.json"));
    safe_path(data, &path)?;
    Ok(path)
}
pub(crate) fn grant(settings: &Settings, id: &str) -> Result<Value> {
    read_json(
        &grant_path(settings, id)?,
        json!({"version":2,"user_id":id,"models":{"llm":[]},"knowledge_bases":[],"skills":[],"partners":[],"enabled_tools":null,"mcp_tools":null,"exec_enabled":false,"learning_policy":null}),
    )
}
pub(crate) fn granted_catalog(settings: &Settings, id: &str) -> Result<Value> {
    let grant = grant(settings, id)?;
    let mut catalog = settings.catalog()?;
    // Only granted ids cross the frontend boundary. Provider references resolve
    // against the deployment catalog after Settings::model checks permissions.
    let profiles = catalog["services"]["llm"]["profiles"]
        .as_array_mut()
        .ok_or_else(|| Error::conflict("Invalid model catalog."))?;
    profiles.retain_mut(|profile| {
        let allowed = grant["models"]["llm"]
            .as_array()
            .into_iter()
            .flatten()
            .filter(|g| g["profile_id"] == profile["id"])
            .collect::<Vec<_>>();
        if allowed.is_empty() {
            return false;
        }
        if let Some(models) = profile["models"].as_array_mut() {
            models.retain(|m| {
                allowed.iter().any(|g| {
                    g["model_ids"]
                        .as_array()
                        .is_some_and(|ids| ids.iter().any(|id| id == &m["id"]))
                })
            });
        }
        profile["models"].as_array().is_some_and(|m| !m.is_empty())
    });
    catalog["connections"] = json!([]);
    for (service, value) in catalog["services"].as_object_mut().unwrap() {
        if service != "llm" {
            *value = json!({"profiles":[],"active_profile_id":null,"active_model_id":null});
        }
    }
    let first = catalog["services"]["llm"]["profiles"]
        .as_array()
        .and_then(|p| p.first())
        .cloned();
    let active = catalog["services"]["llm"]["active_profile_id"].clone();
    if !catalog["services"]["llm"]["profiles"]
        .as_array()
        .unwrap()
        .iter()
        .any(|p| {
            p["id"] == active
                && p["models"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|m| m["id"] == catalog["services"]["llm"]["active_model_id"])
        })
    {
        catalog["services"]["llm"]["active_profile_id"] = first
            .as_ref()
            .map(|p| p["id"].clone())
            .unwrap_or(Value::Null);
        catalog["services"]["llm"]["active_model_id"] = first
            .as_ref()
            .map(|p| p["models"][0]["id"].clone())
            .unwrap_or(Value::Null);
    }
    Ok(catalog)
}

pub(crate) fn effective_catalog(
    mut personal: Value,
    settings: &Settings,
    id: &str,
) -> Result<Value> {
    let assigned = granted_catalog(settings, id)?;
    for (service, value) in personal["services"]
        .as_object_mut()
        .ok_or_else(|| Error::conflict("Invalid personal catalog."))?
    {
        if service != "llm" {
            continue;
        }
        for profile in value["profiles"].as_array_mut().unwrap() {
            profile["id"] = json!(format!("personal:{}", text(profile, "id")));
        }
        let active = text(value, "active_profile_id").to_owned();
        if !active.is_empty() {
            value["active_profile_id"] = json!(format!("personal:{active}"));
        }
        let profiles = value["profiles"].as_array_mut().unwrap();
        for mut profile in assigned["services"]["llm"]["profiles"]
            .as_array()
            .into_iter()
            .flatten()
            .cloned()
        {
            profile["id"] = json!(format!("assigned:{}", text(&profile, "id")));
            profiles.push(profile);
        }
        if active.is_empty() {
            let assigned_active = text(&assigned["services"]["llm"], "active_profile_id");
            if !assigned_active.is_empty() {
                value["active_profile_id"] = json!(format!("assigned:{assigned_active}"));
                value["active_model_id"] = assigned["services"]["llm"]["active_model_id"].clone();
            }
        }
    }
    Ok(personal)
}
