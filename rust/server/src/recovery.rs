//! Offline, verified recovery packages. Never open Application or migrate the
//! source database. Restore publishes only into a new home, never over live data.
use crate::{Error, Result, config::Settings};
use rusqlite::{Connection, OpenFlags, params};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    collections::{HashMap, HashSet},
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    path::{Component, Path, PathBuf},
};

pub(crate) const INCOMPLETE: &str = ".native-recovery-incomplete";
const CATALOG: &str = "data/user/runtime/workspaces.sqlite3";
const DATABASE: &str = "data/user/chat_history.db";
const MAX_ENTRIES: usize = 100_000;
const MAX_MANIFEST_BYTES: u64 = 32 * 1024 * 1024;

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Entry {
    path: String,
    bytes: u64,
    sha256: String,
    sqlite: bool,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Workspace {
    id: String,
    original_path: String,
    path: String,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Manifest {
    format_version: u32,
    product_version: String,
    schema_version: i64,
    directories: Vec<String>,
    files: Vec<Entry>,
    workspaces: Vec<Workspace>,
}

fn invalid() -> Error {
    Error::invalid("Invalid or incomplete recovery package.")
}
fn relative(path: &str) -> Result<PathBuf> {
    // Also reject Windows separators/streams on Unix so packages are portable.
    if path.is_empty() || path.contains(['\\', ':', '\0']) {
        return Err(invalid());
    }
    if path
        .split('/')
        .any(|s| s.is_empty() || s == "." || s == "..")
    {
        return Err(invalid());
    }
    let path = PathBuf::from(path);
    if !path.components().all(|c| matches!(c, Component::Normal(_))) {
        return Err(invalid());
    }
    Ok(path)
}
fn portable(path: &Path) -> Result<String> {
    let name = path
        .to_str()
        .ok_or_else(invalid)?
        .replace(std::path::MAIN_SEPARATOR, "/");
    relative(&name)?;
    Ok(name)
}
fn regular(path: &Path, directory: bool) -> Result<()> {
    let metadata = fs::symlink_metadata(path)?;
    if metadata.file_type().is_symlink()
        || (directory && !metadata.is_dir())
        || (!directory && !metadata.is_file())
    {
        return Err(Error::conflict(
            "Recovery refuses symbolic links and special files.",
        ));
    }
    Ok(())
}
fn no_links(path: &Path) -> Result<()> {
    for ancestor in path.ancestors() {
        if !ancestor.as_os_str().is_empty() {
            regular(ancestor, true)?;
        }
    }
    Ok(())
}
fn private_dir(path: &Path) -> Result<()> {
    fs::create_dir_all(path)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o700))?;
    }
    Ok(())
}
fn private_file(path: &Path) -> Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o600))?;
    }
    OpenOptions::new()
        .read(true)
        .write(true)
        .open(path)?
        .sync_all()?;
    Ok(())
}
fn digest(path: &Path) -> Result<(u64, String)> {
    regular(path, false)?;
    let mut file = File::open(path)?;
    let mut bytes = 0;
    let mut hash = Sha256::new();
    let mut buffer = [0; 64 * 1024];
    loop {
        let count = file.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        bytes += count as u64;
        hash.update(&buffer[..count]);
    }
    Ok((bytes, format!("{:x}", hash.finalize())))
}
fn is_sqlite(path: &Path) -> Result<bool> {
    regular(path, false)?;
    let mut header = [0; 16];
    let count = File::open(path)?.read(&mut header)?;
    Ok(count == 16 && &header == b"SQLite format 3\0")
}
fn readonly(path: &Path) -> Result<Connection> {
    regular(path, false)?;
    Ok(Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_ONLY,
    )?)
}
fn check_database(path: &Path) -> Result<i64> {
    let conn = readonly(path)?;
    let checks = conn
        .prepare("PRAGMA quick_check")?
        .query_map([], |r| r.get::<_, String>(0))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    if checks != ["ok"] {
        return Err(invalid());
    }
    Ok(conn.pragma_query_value(None, "user_version", |r| r.get(0))?)
}
fn copy_tree(
    source: &Path,
    destination: &Path,
    base: &Path,
    manifest: &mut Manifest,
    depth: usize,
) -> Result<()> {
    regular(source, true)?;
    if depth > 64 || manifest.files.len() + manifest.directories.len() >= MAX_ENTRIES {
        return Err(invalid());
    }
    private_dir(destination)?;
    manifest.directories.push(portable(
        destination.strip_prefix(base).map_err(|_| invalid())?,
    )?);
    let mut children = fs::read_dir(source)?.collect::<std::io::Result<Vec<_>>>()?;
    children.sort_by_key(|e| e.file_name());
    let mut databases = HashSet::new();
    for child in &children {
        // Windows byte-range locks are mandatory: even probing the header of
        // our own locked runtime file through a second handle can fail.
        // Exclude it before any file open, not only in the later copy pass.
        let target = destination.join(child.file_name());
        let key = portable(target.strip_prefix(base).map_err(|_| invalid())?)?;
        if key == "data/user/rust-server.lock" {
            continue;
        }
        if child.file_type()?.is_file() && is_sqlite(&child.path())? {
            databases.insert(child.file_name().to_string_lossy().into_owned());
        }
    }
    for child in children {
        if manifest.files.len() + manifest.directories.len() >= MAX_ENTRIES {
            return Err(invalid());
        }
        let name = child.file_name().into_string().map_err(|_| invalid())?;
        let target = destination.join(&name);
        let key = portable(target.strip_prefix(base).map_err(|_| invalid())?)?;
        if key == "data/user/rust-server.lock"
            || key == "data/user/backups"
            || ["-wal", "-shm", "-journal"].iter().any(|suffix| {
                name.strip_suffix(suffix)
                    .is_some_and(|n| databases.contains(n))
            })
        {
            continue;
        }
        if child.file_type()?.is_dir() {
            copy_tree(&child.path(), &target, base, manifest, depth + 1)?;
        } else {
            regular(&child.path(), false)?;
            let sqlite = databases.contains(&name);
            if sqlite {
                let conn = readonly(&child.path())?;
                conn.execute("VACUUM INTO ?", [target.to_str().ok_or_else(invalid)?])?;
                check_database(&target)?;
            } else {
                let before = fs::metadata(child.path())?;
                fs::copy(child.path(), &target)?;
                let after = fs::metadata(child.path())?;
                if before.len() != after.len() || before.modified()? != after.modified()? {
                    return Err(Error::conflict(
                        "Source files changed during backup. Stop external workspace writers and retry.",
                    ));
                }
            }
            private_file(&target)?;
            let (bytes, sha256) = digest(&target)?;
            manifest.files.push(Entry {
                path: key,
                bytes,
                sha256,
                sqlite,
            });
        }
    }
    Ok(())
}

fn destination(path: &Path) -> Result<PathBuf> {
    let name = path.file_name().ok_or_else(invalid)?;
    let parent = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let parent = parent.canonicalize()?;
    let destination = parent.join(name);
    if fs::symlink_metadata(&destination).is_ok() {
        return Err(Error::conflict(
            "Recovery destination must not exist; select a new directory.",
        ));
    }
    Ok(destination)
}
fn publish(stage: tempfile::TempDir, target: &Path) -> Result<()> {
    // Reserve the destination without replacing even an empty existing home.
    fs::create_dir(target)?;
    private_dir(target)?;
    let marker = target.join(INCOMPLETE);
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&marker)?;
    file.write_all(b"Do not start this home; recovery publication has not completed.\n")?;
    file.sync_all()?;
    for entry in fs::read_dir(stage.path())? {
        let entry = entry?;
        fs::rename(entry.path(), target.join(entry.file_name()))?;
    }
    fs::remove_file(marker)?;
    Ok(())
}

pub fn create(home: &Path, output: &Path) -> Result<Value> {
    let home = home.canonicalize()?;
    regular(&home.join(DATABASE), false)?;
    let (root, _lock) = Settings::lock_home(&home)?;
    let target = destination(output)?;
    if target.starts_with(&home) {
        return Err(Error::invalid(
            "Write recovery packages outside the source home.",
        ));
    }
    let stage = tempfile::tempdir_in(target.parent().ok_or_else(invalid)?)?;
    private_dir(stage.path())?;
    let payload = stage.path().join("payload");
    let mut manifest = Manifest {
        format_version: 1,
        product_version: include_str!("../../../VERSION").trim().into(),
        schema_version: 0,
        directories: Vec::new(),
        files: Vec::new(),
        workspaces: Vec::new(),
    };
    copy_tree(
        &root,
        &payload.join("data/user"),
        &payload,
        &mut manifest,
        0,
    )?;
    for directory in ["data/system", "data/users"] {
        let source = home.join(directory);
        if source.try_exists()? {
            copy_tree(
                &source,
                &payload.join(directory),
                &payload,
                &mut manifest,
                0,
            )?;
        }
    }
    manifest.schema_version = check_database(&payload.join(DATABASE))?;
    let catalog = payload.join(CATALOG);
    if catalog.try_exists()? {
        let conn = readonly(&catalog)?;
        let entries = conn
            .prepare("SELECT id,payload FROM workspaces ORDER BY id")?
            .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        let mut roots = Vec::new();
        for (id, raw) in entries {
            let entry: Value = serde_json::from_str(&raw).map_err(|_| invalid())?;
            if entry["kind"] != "workspace" {
                continue;
            }
            if entry["workspace_id"] != id {
                return Err(invalid());
            }
            let original_path = entry["path"].as_str().ok_or_else(invalid)?.to_owned();
            let source = PathBuf::from(&original_path);
            if !source.is_absolute()
                || source
                    .components()
                    .any(|c| matches!(c, Component::ParentDir))
            {
                return Err(invalid());
            }
            no_links(&source)?;
            let source = source.canonicalize()?;
            roots.push((id, original_path, source));
        }
        // Parent roots first: nested registrations must remain views of the
        // same files after restore, rather than becoming independent copies.
        roots.sort_by_key(|(id, _, source)| (source.components().count(), id.clone()));
        let mut copied = HashMap::<PathBuf, String>::new();
        for (id, original_path, source) in roots {
            let path = if source.starts_with(&root) {
                let key = portable(source.strip_prefix(&home).map_err(|_| invalid())?)?;
                if !manifest.directories.contains(&key) {
                    return Err(invalid());
                }
                key
            } else if let Some((parent, key)) =
                copied.iter().find(|(parent, _)| source.starts_with(parent))
            {
                let suffix = source.strip_prefix(parent).map_err(|_| invalid())?;
                if suffix.as_os_str().is_empty() {
                    key.clone()
                } else {
                    portable(&Path::new(key).join(suffix))?
                }
            } else {
                if target.starts_with(&source)
                    || stage.path().starts_with(&source)
                    || root.starts_with(&source)
                {
                    return Err(Error::invalid(
                        "Workspace overlaps the home or backup destination.",
                    ));
                }
                let key = format!("workspaces/{}", copied.len());
                copy_tree(&source, &payload.join(&key), &payload, &mut manifest, 0)?;
                copied.insert(source, key.clone());
                key
            };
            manifest.workspaces.push(Workspace {
                id,
                original_path,
                path,
            });
        }
    }
    let bytes = serde_json::to_vec_pretty(&manifest).map_err(|_| invalid())?;
    if bytes.len() as u64 > MAX_MANIFEST_BYTES {
        return Err(invalid());
    }
    fs::write(stage.path().join("manifest.json"), bytes)?;
    private_file(&stage.path().join("manifest.json"))?;
    let result = summary(&manifest);
    publish(stage, &target)?;
    Ok(result)
}

pub fn database_snapshot(home: &Path) -> Result<Value> {
    let home = home.canonicalize()?;
    regular(&home.join(DATABASE), false)?;
    let (root, _lock) = Settings::lock_home(&home)?;
    crate::backup::snapshot(
        &readonly(&home.join(DATABASE))?,
        &root.join("backups"),
        "manual",
    )
}

fn summary(manifest: &Manifest) -> Value {
    json!({"format_version":manifest.format_version,"schema_version":manifest.schema_version,
        "files":manifest.files.len(),"workspaces":manifest.workspaces.len(),
        "bytes":manifest.files.iter().map(|e|e.bytes as u128).sum::<u128>().to_string(),
        "scope":"data/user, data/users, data/system and registered workspace directories",
        "excluded":["data/user/backups","runtime lock and SQLite sidecars"],
        "contains_credentials":true})
}
fn load(package: &Path) -> Result<Manifest> {
    regular(package, true)?;
    if package.join(INCOMPLETE).try_exists()? {
        return Err(invalid());
    }
    let path = package.join("manifest.json");
    regular(&path, false)?;
    let mut bytes = Vec::new();
    File::open(path)?
        .take(MAX_MANIFEST_BYTES + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() as u64 > MAX_MANIFEST_BYTES {
        return Err(invalid());
    }
    let manifest: Manifest = serde_json::from_slice(&bytes).map_err(|_| invalid())?;
    if manifest.format_version != 1
        || manifest.files.len() + manifest.directories.len() > MAX_ENTRIES
    {
        return Err(invalid());
    }
    let base = package.join("payload");
    regular(&base, true)?;
    let mut names = HashSet::new();
    for path in &manifest.directories {
        let path = relative(path)?;
        if !(path.starts_with("data/user")
            || path.starts_with("data/users")
            || path.starts_with("data/system")
            || path.starts_with("workspaces"))
        {
            return Err(invalid());
        }
        if !names.insert(path.clone()) {
            return Err(invalid());
        }
        let full = base.join(&path);
        crate::config::safe_path(&base, &full)?;
        regular(&full, true)?;
    }
    for entry in &manifest.files {
        let path = relative(&entry.path)?;
        if !names.insert(path.clone()) {
            return Err(invalid());
        }
        let full = base.join(&path);
        crate::config::safe_path(&base, &full)?;
        if digest(&full)? != (entry.bytes, entry.sha256.clone()) {
            return Err(Error::conflict("Recovery file checksum mismatch."));
        }
        if entry.sqlite {
            // Packages must be self-contained, never read external WAL changes.
            for suffix in ["-wal", "-shm", "-journal"] {
                if fs::symlink_metadata(full.with_file_name(format!(
                    "{}{suffix}",
                    full.file_name().ok_or_else(invalid)?.to_string_lossy()
                )))
                .is_ok()
                {
                    return Err(invalid());
                }
            }
            let version = check_database(&full)?;
            if entry.path == DATABASE && version != manifest.schema_version {
                return Err(invalid());
            }
        }
    }
    if !manifest
        .files
        .iter()
        .any(|e| e.path == DATABASE && e.sqlite)
    {
        return Err(invalid());
    }
    let mut ids = HashSet::new();
    for workspace in &manifest.workspaces {
        relative(&workspace.path)?;
        if !ids.insert(&workspace.id) || !manifest.directories.contains(&workspace.path) {
            return Err(invalid());
        }
    }
    Ok(manifest)
}
pub fn verify(package: &Path) -> Result<Value> {
    let manifest = load(package)?;
    let mut result = summary(&manifest);
    result["verified"] = json!(true);
    result["compatible"] = json!(manifest.schema_version <= crate::backup::SCHEMA_VERSION);
    Ok(result)
}
pub fn restore(package: &Path, home: &Path) -> Result<Value> {
    let manifest = load(package)?;
    if manifest.schema_version > crate::backup::SCHEMA_VERSION {
        return Err(Error::conflict(
            "Recovery database schema is newer than this binary.",
        ));
    }
    let target = destination(home)?;
    if target.starts_with(package.canonicalize()?) {
        return Err(Error::invalid("Restore outside the recovery package."));
    }
    let stage = tempfile::tempdir_in(target.parent().ok_or_else(invalid)?)?;
    private_dir(stage.path())?;
    for directory in &manifest.directories {
        private_dir(&stage.path().join(relative(directory)?))?;
    }
    for entry in &manifest.files {
        let path = relative(&entry.path)?;
        if !(path.starts_with("data/user")
            || path.starts_with("data/users")
            || path.starts_with("data/system")
            || path.starts_with("workspaces"))
        {
            return Err(invalid());
        }
        let source = package.join("payload").join(&path);
        let destination = stage.path().join(path);
        if let Some(parent) = destination.parent() {
            private_dir(parent)?;
        }
        fs::copy(source, &destination)?;
        private_file(&destination)?;
        if digest(&destination)? != (entry.bytes, entry.sha256.clone()) {
            return Err(Error::conflict("Recovery package changed while copying."));
        }
    }
    // Rebase catalog roots to the new home, including archived workspaces. Never
    // point restored imports at the original external directories.
    if stage.path().join(CATALOG).try_exists()? {
        let mut conn = Connection::open(stage.path().join(CATALOG))?;
        let tx = conn.transaction()?;
        let rows = tx
            .prepare("SELECT id,payload FROM workspaces")?
            .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        for (id, raw) in rows {
            let mut entry: Value = serde_json::from_str(&raw).map_err(|_| invalid())?;
            if entry["kind"] != "workspace" {
                continue;
            }
            let mapping = manifest
                .workspaces
                .iter()
                .find(|w| w.id == id)
                .ok_or_else(invalid)?;
            if entry["workspace_id"] != id || entry["path"] != mapping.original_path {
                return Err(invalid());
            }
            entry["path"] = json!(target.join(relative(&mapping.path)?));
            tx.execute(
                "UPDATE workspaces SET payload=? WHERE id=?",
                params![entry.to_string(), id],
            )?;
        }
        tx.commit()?;
        conn.execute_batch("PRAGMA wal_checkpoint(TRUNCATE); PRAGMA journal_mode=DELETE;")?;
    } else if !manifest.workspaces.is_empty() {
        return Err(invalid());
    }
    // Account catalogs also contain absolute paths. Restore them within their
    // own account root, never retain pointers into the source deployment.
    for entry in &manifest.files {
        let path = relative(&entry.path)?;
        if path.starts_with("data/users") && path.ends_with("user/runtime/workspaces.sqlite3") {
            let root = path.parent().and_then(Path::parent).ok_or_else(invalid)?;
            let conn = Connection::open(stage.path().join(&path))?;
            let rows = conn
                .prepare("SELECT id,payload FROM workspaces")?
                .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            for (id, raw) in rows {
                let mut value: Value = serde_json::from_str(&raw).map_err(|_| invalid())?;
                let suffix = if id.is_empty() {
                    PathBuf::from("workspace")
                } else {
                    crate::accounts::validate_id(&id)?;
                    PathBuf::from("workspaces").join(&id)
                };
                value["path"] = json!(target.join(root).join(suffix));
                conn.execute(
                    "UPDATE workspaces SET payload=? WHERE id=?",
                    params![value.to_string(), id],
                )?;
            }
            conn.execute_batch("PRAGMA wal_checkpoint(TRUNCATE); PRAGMA journal_mode=DELETE;")?;
        }
    }
    let identities = stage.path().join("data/system/auth/native.sqlite3");
    if identities.try_exists()? {
        let conn = Connection::open(identities)?;
        conn.execute("DELETE FROM auth_sessions", [])?;
        conn.execute_batch("PRAGMA wal_checkpoint(TRUNCATE); PRAGMA journal_mode=DELETE;")?;
    }
    let result = summary(&manifest);
    publish(stage, &target)?;
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn backup_excludes_the_live_lock_and_restores_registered_sources() {
        let temp = tempfile::tempdir().unwrap();
        let home = temp.path().join("home");
        let settings = Settings::open(&home).unwrap();
        let store = crate::store::Store::open(&settings.user_root.join("chat_history.db")).unwrap();
        let page = crate::documents::create(
            &store.connect().unwrap(),
            "a",
            &json!({"title":"恢复测试","content":"不丢失的原文"}),
        )
        .unwrap();
        let registration =
            crate::workspaces::save(&settings, None, &json!({"name":"资料"})).unwrap();
        let workspace = registration["workspace"]["path"].as_str().unwrap();
        fs::write(Path::new(workspace).join("source.txt"), "原始文档").unwrap();
        drop(store);
        drop(settings);
        let backup = temp.path().join("backup");
        create(&home, &backup).unwrap();
        assert_eq!(verify(&backup).unwrap()["verified"], true);
        assert!(!backup.join("payload/data/user/rust-server.lock").exists());
        let restored = temp.path().join("restored");
        restore(&backup, &restored).unwrap();
        let restored_store = crate::store::Store::open(&restored.join(DATABASE)).unwrap();
        assert_eq!(
            crate::documents::read(
                &restored_store.connect().unwrap(),
                "a",
                crate::store::text(&page, "id")
            )
            .unwrap(),
            page
        );
    }
    #[test]
    fn timestamp_serialization_preserves_the_exact_committed_value() {
        for bits in (1_762_254_385.155_707_6_f64.to_bits()..).take(1024) {
            let committed =
                json!({"created_at":f64::from_bits(bits),"updated_at":f64::from_bits(bits)});
            let stored: Value = serde_json::from_str(&committed.to_string()).unwrap();
            assert_eq!(
                stored, committed,
                "Receipt timestamps changed during persistence"
            );
        }
    }
}
