use anyhow::{anyhow, Result};
use std::sync::Mutex;

#[flutter_rust_bridge::frb(opaque)]
pub struct LocalStore {
    inner: Mutex<Option<crate::local::LocalLibrary>>,
}

impl LocalStore {
    pub fn open(path: String) -> Result<Self> {
        Ok(Self {
            inner: Mutex::new(Some(crate::local::LocalLibrary::open(&path)?)),
        })
    }
    #[flutter_rust_bridge::frb(sync)]
    pub fn find(&self, id: String, comic_type: i32) -> Result<Option<String>> {
        self.with(|db| {
            db.find(&id, comic_type)?
                .map(|v| serde_json::to_string(&v).map_err(Into::into))
                .transpose()
        })
    }
    #[flutter_rust_bridge::frb(sync)]
    pub fn find_name(&self, name: String) -> Result<Option<String>> {
        self.with(|db| {
            db.find_name(&name)?
                .map(|v| serde_json::to_string(&v).map_err(Into::into))
                .transpose()
        })
    }
    #[flutter_rust_bridge::frb(sync)]
    pub fn next_id(&self, comic_type: i32) -> Result<String> {
        self.with(|db| db.next_id(comic_type))
    }
    #[flutter_rust_bridge::frb(sync)]
    pub fn count(&self) -> Result<u32> {
        self.with(|db| db.count())
    }
    #[flutter_rust_bridge::frb(sync)]
    pub fn save(&self, comic_json: String) -> Result<()> {
        self.with(|db| db.save(serde_json::from_str(&comic_json)?))
    }
    #[flutter_rust_bridge::frb(sync)]
    pub fn list(
        &self,
        sort: String,
        keyword: Option<String>,
        limit: Option<u32>,
    ) -> Result<String> {
        self.with(|db| {
            Ok(serde_json::to_string(&db.list(
                &sort,
                keyword.as_deref(),
                limit,
            )?)?)
        })
    }
    #[flutter_rust_bridge::frb(sync)]
    pub fn delete(&self, keys_json: String) -> Result<()> {
        self.with(|db| {
            db.delete(&serde_json::from_str::<Vec<crate::favorites::Key>>(
                &keys_json,
            )?)
        })
    }
    #[flutter_rust_bridge::frb(sync)]
    pub fn delete_chapters(
        &self,
        id: String,
        comic_type: i32,
        chapters: Vec<String>,
    ) -> Result<()> {
        self.with(|db| db.delete_chapters(&id, comic_type, &chapters))
    }
    #[flutter_rust_bridge::frb(sync)]
    pub fn downloaded(
        &self,
        id: String,
        comic_type: i32,
        episode: Option<u32>,
        chapters_json: Option<String>,
    ) -> Result<bool> {
        self.with(|db| {
            db.downloaded(
                &id,
                comic_type,
                episode,
                chapters_json
                    .map(|v| serde_json::from_str(&v))
                    .transpose()?,
            )
        })
    }
    #[flutter_rust_bridge::frb(sync)]
    pub fn close(&self) -> Result<()> {
        self.inner
            .lock()
            .map_err(|_| anyhow!("local library lock poisoned"))?
            .take();
        Ok(())
    }
    #[flutter_rust_bridge::frb(ignore)]
    fn with<T>(
        &self,
        operation: impl FnOnce(&crate::local::LocalLibrary) -> Result<T>,
    ) -> Result<T> {
        let guard = self
            .inner
            .lock()
            .map_err(|_| anyhow!("local library lock poisoned"))?;
        operation(
            guard
                .as_ref()
                .ok_or_else(|| anyhow!("local library closed"))?,
        )
    }
}

#[flutter_rust_bridge::frb(sync)]
pub fn chapter_directory_name(name: String) -> String {
    name.chars()
        .map(|c| if "/\\:*?\"<>|".contains(c) { '_' } else { c })
        .collect()
}

/// SAF supplies file names through its platform adapter; filtering and ordering
/// use the same backend rule as native directories.
#[flutter_rust_bridge::frb(sync)]
pub fn sort_comic_images(mut paths: Vec<String>) -> Vec<String> {
    fn name(path: &str) -> &str {
        path.rsplit(['/', '\\']).next().unwrap_or(path)
    }
    paths.retain(|p| !name(p).starts_with('.') && !name(p).starts_with("cover."));
    paths.sort_by(|a, b| {
        let a = name(a);
        let b = name(b);
        match (
            a.split('.').next().unwrap_or(a).parse::<i64>(),
            b.split('.').next().unwrap_or(b).parse::<i64>(),
        ) {
            (Ok(a), Ok(b)) => a.cmp(&b),
            _ => a.cmp(b),
        }
    });
    paths.into_iter().map(|p| format!("file://{p}")).collect()
}

pub fn list_comic_images(path: String) -> Result<Vec<String>> {
    let mut paths = Vec::new();
    for entry in std::fs::read_dir(path)? {
        let entry = entry?;
        if entry.file_type()?.is_file() {
            paths.push(entry.path().to_string_lossy().into_owned());
        }
    }
    Ok(sort_comic_images(paths))
}
