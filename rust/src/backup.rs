use anyhow::{ensure, Context, Result};
use serde_json::{json, Value};
use std::{fs, path::Path};
const DATABASES: &[&str] = &["history.db", "local_favorite.db", "cookie.db"];

pub fn export(directory: &str, cache: &str, sync: bool) -> Result<String> {
    fs::create_dir_all(cache)?;
    let staging = tempfile::tempdir_in(cache)?;
    let source = Path::new(directory);
    for name in DATABASES {
        let path = source.join(name);
        if path.exists() {
            let db = rusqlite::Connection::open_with_flags(
                path,
                rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
            )?;
            db.busy_timeout(std::time::Duration::from_secs(10))?;
            db.execute(
                "VACUUM main INTO ?1",
                [staging.path().join(name).to_string_lossy().as_ref()],
            )?;
        }
    }
    let output = Path::new(cache).join(format!("{}.venera", uuid::Uuid::new_v4()));
    crate::archives::write_zip(&output, |zip| {
        for name in DATABASES {
            let path = staging.path().join(name);
            if path.exists() {
                crate::archives::add_file(zip, name, &path)?;
            }
        }
        crate::archives::add_file(
            zip,
            "appdata.json",
            &source.join(if sync {
                "syncdata.json"
            } else {
                "appdata.json"
            }),
        )?;
        let sources = source.join("comic_source");
        if sources.exists() {
            for entry in fs::read_dir(sources)? {
                let entry = entry?;
                if entry.file_type()?.is_file() {
                    crate::archives::add_file(
                        zip,
                        &format!("comic_source/{}", entry.file_name().to_string_lossy()),
                        &entry.path(),
                    )?;
                }
            }
        }
        Ok(())
    })?;
    Ok(output.to_string_lossy().into_owned())
}

pub struct Prepared {
    pub staging: tempfile::TempDir,
    pub data: Option<Value>,
}
pub fn prepare(input: &str, cache: &str, current_version: Option<i64>) -> Result<Option<Prepared>> {
    fs::create_dir_all(cache)?;
    let staging = tempfile::tempdir_in(cache)?;
    crate::archives::extract(Path::new(input), staging.path())?;
    let data = if staging.path().join("appdata.json").exists() {
        let data: Value = serde_json::from_slice(&fs::read(staging.path().join("appdata.json"))?)?;
        ensure!(
            data["settings"].is_object() && data["searchHistory"].is_array(),
            "Invalid backup settings"
        );
        if current_version
            .zip(data["settings"]["dataVersion"].as_i64())
            .is_some_and(|(current, incoming)| incoming <= current)
        {
            return Ok(None);
        }
        Some(data)
    } else {
        None
    };
    for name in DATABASES {
        let path = staging.path().join(name);
        if path.exists() {
            let db = rusqlite::Connection::open_with_flags(
                path,
                rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
            )?;
            let check: String = db.query_row("PRAGMA quick_check", [], |r| r.get(0))?;
            ensure!(check == "ok", "Corrupt database in backup: {name}");
        }
    }
    ensure!(
        data.is_some() || DATABASES.iter().any(|n| staging.path().join(n).exists()),
        "Empty or invalid backup"
    );
    Ok(Some(Prepared { staging, data }))
}
fn copy_tree(from: &Path, to: &Path) -> Result<()> {
    if from.is_dir() {
        fs::create_dir_all(to)?;
        for entry in fs::read_dir(from)? {
            let entry = entry?;
            ensure!(!entry.file_type()?.is_symlink(), "Backup symlink rejected");
            copy_tree(&entry.path(), &to.join(entry.file_name()))?;
        }
    } else {
        fs::copy(from, to)?;
    }
    Ok(())
}
impl Prepared {
    pub fn apply(&self, directory: &str) -> Result<String> {
        let root = Path::new(directory);
        fs::create_dir_all(root)?;
        let staging = tempfile::tempdir_in(root)?;
        let rollback = tempfile::tempdir_in(root)?;
        let mut names = Vec::new();
        for name in DATABASES
            .iter()
            .copied()
            .chain(std::iter::once("comic_source"))
        {
            let source = self.staging.path().join(name);
            if source.exists() {
                copy_tree(&source, &staging.path().join(name))?;
                names.push(name.to_owned());
            }
        }
        let mut current = crate::settings::load(directory)?;
        if let Some(data) = &self.data {
            current = crate::settings::merge(current, data.clone())?;
            crate::settings::save(
                staging.path().to_str().context("Invalid backup path")?,
                current.clone(),
            )?;
            names.extend(["appdata.json".into(), "syncdata.json".into()]);
        }
        let mut installed = Vec::new();
        let mut saved = Vec::new();
        let result = (|| -> Result<()> {
            // Caller closes its stores first. Include SQLite sidecars in rollback.
            for name in &names {
                for candidate in std::iter::once(name.clone()).chain(if name.ends_with(".db") {
                    vec![
                        format!("{name}-wal"),
                        format!("{name}-shm"),
                        format!("{name}-journal"),
                    ]
                } else {
                    vec![]
                }) {
                    if root.join(&candidate).exists() {
                        fs::rename(root.join(&candidate), rollback.path().join(&candidate))?;
                        saved.push(candidate);
                    }
                }
                fs::rename(staging.path().join(name), root.join(name))?;
                installed.push(name.clone());
            }
            Ok(())
        })();
        if let Err(error) = result {
            let recovery = (|| -> Result<()> {
                for name in installed.iter().rev() {
                    let path = root.join(name);
                    if path.is_dir() {
                        fs::remove_dir_all(path)?;
                    } else {
                        fs::remove_file(path)?;
                    }
                }
                for name in saved.iter().rev() {
                    fs::rename(rollback.path().join(name), root.join(name))?;
                }
                Ok(())
            })();
            if let Err(recovery_error) = recovery {
                let retained = rollback.keep();
                anyhow::bail!("Restore failed: {error:#}; rollback failed: {recovery_error:#}. Original files retained at {}",retained.display());
            }
            return Err(error);
        }
        Ok(
            json!({"data":current,"sourcesChanged":names.iter().any(|n|n=="comic_source")})
                .to_string(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn backup_snapshots_live_sqlite_and_restores_settings() -> Result<()> {
        let root = tempfile::tempdir()?;
        let cache = tempfile::tempdir()?;
        let path = root.path().to_str().unwrap();
        let db = rusqlite::Connection::open(root.path().join("history.db"))?;
        db.execute_batch("PRAGMA journal_mode=WAL;CREATE TABLE test(value TEXT);INSERT INTO test VALUES ('原始');")?;
        crate::settings::save(
            path,
            json!({"settings":{"dataVersion":2,"proxy":"local"},"searchHistory":["漫画"]}),
        )?;
        let backup = export(path, cache.path().to_str().unwrap(), false)?;
        db.execute("UPDATE test SET value='changed'", [])?;
        drop(db);
        let prepared = prepare(&backup, cache.path().to_str().unwrap(), Some(1))?.unwrap();
        prepared.apply(path)?;
        let db = rusqlite::Connection::open(root.path().join("history.db"))?;
        assert_eq!(
            db.query_row("SELECT value FROM test", [], |r| r.get::<_, String>(0))?,
            "原始"
        );
        assert!(prepare(&backup, cache.path().to_str().unwrap(), Some(2))?.is_none());
        Ok(())
    }
}
