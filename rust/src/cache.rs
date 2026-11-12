use anyhow::{ensure, Context, Result};
use rusqlite::{params, Connection, OptionalExtension};
use std::{
    fs,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

const WEEK_MS: i64 = 7 * 24 * 60 * 60 * 1000;

pub(crate) fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64
}

pub struct Cache {
    db: Connection,
    root: PathBuf,
    bucket: u32,
    pub size: u64,
    pub limit: u64,
}

impl Cache {
    pub fn open(data_path: &str, cache_path: &str) -> Result<Self> {
        fs::create_dir_all(data_path)?;
        let root = Path::new(cache_path).join("cache");
        fs::create_dir_all(&root)?;
        let db = Connection::open(Path::new(data_path).join("cache.db"))?;
        db.busy_timeout(std::time::Duration::from_secs(5))?;
        db.execute_batch("CREATE TABLE IF NOT EXISTS cache (key TEXT PRIMARY KEY NOT NULL, dir TEXT NOT NULL, name TEXT NOT NULL, expires INTEGER NOT NULL, type TEXT);")?;
        let mut cache = Self {
            db,
            root,
            bucket: 0,
            size: 0,
            limit: 2 * 1024 * 1024 * 1024,
        };
        for (key, dir, name) in cache.entries()? {
            match cache
                .file(&dir, &name)
                .and_then(|p| Ok(fs::metadata(p)?.len()))
            {
                Ok(size) => cache.size += size,
                Err(_) => {
                    cache.db.execute("DELETE FROM cache WHERE key=?", [&key])?;
                }
            }
        }
        cache.prune()?;
        Ok(cache)
    }

    fn file(&self, dir: &str, name: &str) -> Result<PathBuf> {
        ensure!(
            !dir.is_empty() && dir.bytes().all(|c| c.is_ascii_digit()),
            "invalid cache directory"
        );
        ensure!(
            !name.is_empty() && name.bytes().all(|c| c.is_ascii_hexdigit()),
            "invalid cache filename"
        );
        Ok(self.root.join(dir).join(name))
    }

    fn entries(&self) -> Result<Vec<(String, String, String)>> {
        let mut stmt = self
            .db
            .prepare("SELECT key, dir, name FROM cache ORDER BY expires ASC")?;
        let rows = stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    pub fn write(&mut self, key: &str, bytes: &[u8], duration_ms: i64) -> Result<()> {
        self.bucket = (self.bucket + 1) % 100;
        let dir = self.bucket.to_string();
        let name = format!("{:x}", md5::compute(key.as_bytes()));
        let file = self.file(&dir, &name)?;
        fs::create_dir_all(file.parent().context("cache parent missing")?)?;
        // The previous entry remains available until the new payload is fully written.
        let temporary = file.with_extension("tmp");
        fs::write(&temporary, bytes)?;
        self.delete(key)?;
        if file.exists() {
            fs::remove_file(&file)?;
        }
        fs::rename(temporary, &file)?;
        if let Err(error) = self.db.execute(
            "INSERT OR REPLACE INTO cache (key,dir,name,expires) VALUES (?,?,?,?)",
            params![key, dir, name, now_ms().saturating_add(duration_ms)],
        ) {
            let _ = fs::remove_file(file);
            return Err(error.into());
        }
        self.size += bytes.len() as u64;
        self.prune()
    }

    pub fn find(&mut self, key: &str) -> Result<Option<String>> {
        let row: Option<(String, String, i64)> = self
            .db
            .query_row(
                "SELECT dir,name,expires FROM cache WHERE key=?",
                [key],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .optional()?;
        let Some((dir, name, expires)) = row else {
            return Ok(None);
        };
        let file = self.file(&dir, &name)?;
        if expires < now_ms() || !file.is_file() {
            self.delete(key)?;
            return Ok(None);
        }
        self.db.execute(
            "UPDATE cache SET expires=? WHERE key=?",
            params![now_ms() + WEEK_MS, key],
        )?;
        Ok(Some(file.to_string_lossy().into_owned()))
    }

    pub fn delete(&mut self, key: &str) -> Result<()> {
        let row: Option<(String, String)> = self
            .db
            .query_row("SELECT dir,name FROM cache WHERE key=?", [key], |r| {
                Ok((r.get(0)?, r.get(1)?))
            })
            .optional()?;
        if let Some((dir, name)) = row {
            let file = self.file(&dir, &name)?;
            match fs::metadata(&file) {
                Ok(metadata) => {
                    fs::remove_file(file)?;
                    self.size = self.size.saturating_sub(metadata.len());
                }
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => return Err(e.into()),
            }
            self.db.execute("DELETE FROM cache WHERE key=?", [key])?;
        }
        Ok(())
    }

    pub fn prune(&mut self) -> Result<()> {
        let expired = {
            let mut stmt = self.db.prepare("SELECT key FROM cache WHERE expires < ?")?;
            let rows = stmt.query_map([now_ms()], |r| r.get::<_, String>(0))?;
            rows.collect::<rusqlite::Result<Vec<_>>>()?
        };
        for key in expired {
            self.delete(&key)?;
        }
        for (key, _, _) in self.entries()? {
            if self.size <= self.limit {
                break;
            }
            self.delete(&key)?;
        }
        Ok(())
    }

    pub fn clear(&mut self) -> Result<()> {
        // Delete only indexed files, never recursively delete a caller-supplied directory.
        for (key, _, _) in self.entries()? {
            self.delete(&key)?;
        }
        self.size = 0;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn persists_replaces_expires_and_enforces_limit() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let path = dir.path().to_str().unwrap();
        let mut cache = Cache::open(path, path)?;
        cache.write("cover", b"old", WEEK_MS)?;
        cache.write("cover", b"new image", WEEK_MS)?;
        assert_eq!(cache.size, 9);
        drop(cache);
        let mut cache = Cache::open(path, path)?;
        assert_eq!(fs::read(cache.find("cover")?.unwrap())?, b"new image");
        assert_eq!(cache.size, 9);
        cache.write("expired", b"gone", -1)?;
        assert!(cache.find("expired")?.is_none());
        cache.limit = 0;
        cache.prune()?;
        assert!(cache.find("cover")?.is_none());
        assert_eq!(cache.size, 0);
        Ok(())
    }

    #[test]
    fn reads_existing_dart_cache_schema() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let path = dir.path().to_str().unwrap();
        let mut cache = Cache::open(path, path)?;
        fs::create_dir_all(dir.path().join("cache/42"))?;
        fs::write(dir.path().join("cache/42/abc123"), b"legacy")?;
        cache.db.execute(
            "INSERT INTO cache VALUES ('legacy','42','abc123',?,NULL)",
            [now_ms() + WEEK_MS],
        )?;
        assert_eq!(fs::read(cache.find("legacy")?.unwrap())?, b"legacy");
        Ok(())
    }
}
