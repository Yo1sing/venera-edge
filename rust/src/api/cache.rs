use anyhow::{anyhow, Result};
use std::sync::Mutex;

/// Owns both the cache index and files. SQLite connections never cross into Dart.
#[flutter_rust_bridge::frb(opaque)]
pub struct CacheStore {
    inner: std::sync::Arc<Mutex<crate::cache::Cache>>,
}

impl CacheStore {
    pub fn open(data_path: String, cache_path: String) -> Result<Self> {
        Ok(Self {
            inner: crate::cache::shared(&data_path, &cache_path)?,
        })
    }

    pub fn write(&self, key: String, data: Vec<u8>, duration_ms: i64) -> Result<()> {
        self.inner
            .lock()
            .map_err(|_| anyhow!("cache lock poisoned"))?
            .write(&key, &data, duration_ms)
    }

    pub fn find(&self, key: String) -> Result<Option<String>> {
        self.inner
            .lock()
            .map_err(|_| anyhow!("cache lock poisoned"))?
            .find(&key)
    }

    pub fn delete(&self, key: String) -> Result<()> {
        self.inner
            .lock()
            .map_err(|_| anyhow!("cache lock poisoned"))?
            .delete(&key)
    }

    pub fn clear(&self) -> Result<()> {
        self.inner
            .lock()
            .map_err(|_| anyhow!("cache lock poisoned"))?
            .clear()
    }

    pub fn prune(&self) -> Result<()> {
        self.inner
            .lock()
            .map_err(|_| anyhow!("cache lock poisoned"))?
            .prune()
    }

    pub fn set_limit(&self, megabytes: u32) -> Result<()> {
        let mut cache = self
            .inner
            .lock()
            .map_err(|_| anyhow!("cache lock poisoned"))?;
        cache.limit = u64::from(megabytes) * 1024 * 1024;
        cache.prune()
    }

    pub fn size(&self) -> Result<u64> {
        Ok(self
            .inner
            .lock()
            .map_err(|_| anyhow!("cache lock poisoned"))?
            .size)
    }
}
