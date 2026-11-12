use anyhow::{anyhow, Result};
use std::sync::Mutex;

#[flutter_rust_bridge::frb(opaque)]
pub struct FavoriteStore {
    inner: Mutex<Option<crate::favorites::Favorites>>,
}

impl FavoriteStore {
    pub fn open(path: String, traditional: bool) -> Result<Self> {
        Ok(Self {
            inner: Mutex::new(Some(crate::favorites::Favorites::open(&path, traditional)?)),
        })
    }

    #[flutter_rust_bridge::frb(sync)]
    pub fn folders(&self) -> Result<Vec<String>> {
        self.with(|db| db.folders())
    }

    #[flutter_rust_bridge::frb(sync)]
    pub fn summary(&self) -> Result<String> {
        self.with(|db| Ok(serde_json::to_string(&db.summary()?)?))
    }

    #[flutter_rust_bridge::frb(sync)]
    pub fn create(&self, name: String, rename_invalid: bool) -> Result<String> {
        self.with(|db| db.create(&name, rename_invalid))
    }

    #[flutter_rust_bridge::frb(sync)]
    pub fn list(
        &self,
        folder: Option<String>,
        keyword: Option<String>,
        only_updates: bool,
    ) -> Result<String> {
        self.with(|db| {
            Ok(serde_json::to_string(&db.list(
                folder.as_deref(),
                keyword.as_deref(),
                only_updates,
            )?)?)
        })
    }

    #[flutter_rust_bridge::frb(sync)]
    pub fn find(&self, id: String, comic_type: i32) -> Result<Vec<String>> {
        self.with(|db| db.find(&id, comic_type))
    }

    #[flutter_rust_bridge::frb(sync)]
    pub fn bound(&self, folder: String, maximum: bool) -> Result<i64> {
        self.with(|db| db.bound(&folder, maximum))
    }

    #[flutter_rust_bridge::frb(sync)]
    pub fn add(
        &self,
        folder: String,
        comic_json: String,
        order: Option<i64>,
        at_end: bool,
        traditional: bool,
    ) -> Result<bool> {
        self.with(|db| {
            db.add(
                &folder,
                &serde_json::from_str(&comic_json)?,
                order,
                at_end,
                traditional,
            )
        })
    }

    #[flutter_rust_bridge::frb(sync)]
    pub fn transfer(
        &self,
        source: String,
        target: String,
        keys_json: String,
        copy: bool,
        at_end: bool,
        keep_duplicate: bool,
    ) -> Result<()> {
        self.with(|db| {
            db.transfer(
                &source,
                &target,
                &serde_json::from_str::<Vec<crate::favorites::Key>>(&keys_json)?,
                copy,
                at_end,
                keep_duplicate,
            )
        })
    }

    #[flutter_rust_bridge::frb(sync)]
    pub fn delete(&self, folder: Option<String>, keys_json: String) -> Result<()> {
        self.with(|db| {
            db.delete(
                folder.as_deref(),
                &serde_json::from_str::<Vec<crate::favorites::Key>>(&keys_json)?,
            )
        })
    }

    #[flutter_rust_bridge::frb(sync)]
    pub fn delete_folder(&self, folder: String) -> Result<()> {
        self.with(|db| db.delete_folder(&folder))
    }

    #[flutter_rust_bridge::frb(sync)]
    pub fn clear(&self) -> Result<()> {
        self.with(|db| db.clear())
    }

    #[flutter_rust_bridge::frb(sync)]
    pub fn rename(&self, before: String, after: String) -> Result<()> {
        self.with(|db| db.rename(&before, &after))
    }

    #[flutter_rust_bridge::frb(sync)]
    pub fn order_folders(&self, folders: Vec<String>) -> Result<()> {
        self.with(|db| db.order_folders(&folders))
    }

    #[flutter_rust_bridge::frb(sync)]
    pub fn reorder(&self, folder: String, keys_json: String) -> Result<()> {
        self.with(|db| {
            db.reorder(
                &folder,
                &serde_json::from_str::<Vec<crate::favorites::Key>>(&keys_json)?,
            )
        })
    }

    #[flutter_rust_bridge::frb(sync)]
    pub fn link(&self, folder: String, source: String, network_folder: String) -> Result<()> {
        self.with(|db| db.link(&folder, &source, &network_folder))
    }

    #[flutter_rust_bridge::frb(sync)]
    pub fn linked(&self, folder: String) -> Result<Vec<String>> {
        self.with(|db| db.linked(&folder))
    }

    #[flutter_rust_bridge::frb(sync)]
    pub fn edit_tags(
        &self,
        folder: String,
        id: String,
        tags: Vec<String>,
        prepend: bool,
        traditional: bool,
    ) -> Result<()> {
        self.with(|db| db.edit_tags(&folder, &id, &tags, prepend, traditional))
    }

    #[flutter_rust_bridge::frb(sync)]
    pub fn update_info(&self, folder: String, comic_json: String, traditional: bool) -> Result<()> {
        self.with(|db| db.update_info(&folder, &serde_json::from_str(&comic_json)?, traditional))
    }

    #[flutter_rust_bridge::frb(sync)]
    pub fn prepare_follow(&self, folder: String, clear: bool) -> Result<()> {
        self.with(|db| db.prepare_follow(&folder, clear))
    }

    #[flutter_rust_bridge::frb(sync)]
    pub fn update_check(
        &self,
        folder: String,
        id: String,
        comic_type: i32,
        update_time: Option<String>,
    ) -> Result<()> {
        self.with(|db| db.update_check(&folder, &id, comic_type, update_time.as_deref()))
    }

    #[flutter_rust_bridge::frb(sync)]
    pub fn on_read(
        &self,
        id: String,
        comic_type: i32,
        position: String,
        follow_folder: Option<String>,
    ) -> Result<()> {
        self.with(|db| db.on_read(&id, comic_type, &position, follow_folder.as_deref()))
    }

    #[flutter_rust_bridge::frb(sync)]
    pub fn export(&self, folder: String) -> Result<String> {
        self.with(|db| db.export(&folder))
    }

    #[flutter_rust_bridge::frb(sync)]
    pub fn import(
        &self,
        text: String,
        legacy_types: std::collections::HashMap<i32, i32>,
        at_end: bool,
        traditional: bool,
    ) -> Result<()> {
        self.with(|db| db.import(&text, &legacy_types, at_end, traditional))
    }

    #[flutter_rust_bridge::frb(sync)]
    pub fn get(&self, folder: String, id: String, comic_type: i32) -> Result<Option<String>> {
        self.with(|db| {
            db.list(Some(&folder), None, false)?
                .into_iter()
                .find(|c| c.id == id && c.comic_type == comic_type)
                .map(|c| serde_json::to_string(&c).map_err(Into::into))
                .transpose()
        })
    }
    #[flutter_rust_bridge::frb(sync)]
    pub fn all_unique(&self, keyword: Option<String>) -> Result<String> {
        self.unique(keyword)
    }
    pub fn all_unique_async(&self) -> Result<String> {
        self.unique(None)
    }
    #[flutter_rust_bridge::frb(ignore)]
    fn unique(&self, keyword: Option<String>) -> Result<String> {
        self.with(|db| {
            let mut keys = std::collections::HashSet::new();
            let comics: Vec<_> = db
                .list(None, keyword.as_deref(), false)?
                .into_iter()
                .filter(|c| keys.insert((c.id.clone(), c.comic_type)))
                .collect();
            Ok(serde_json::to_string(&comics)?)
        })
    }
    pub fn list_async(&self, folder: Option<String>) -> Result<String> {
        self.with(|db| {
            Ok(serde_json::to_string(&db.list(
                folder.as_deref(),
                None,
                false,
            )?)?)
        })
    }
    #[flutter_rust_bridge::frb(sync)]
    pub fn close(&self) -> Result<()> {
        self.inner
            .lock()
            .map_err(|_| anyhow!("favorites lock poisoned"))?
            .take();
        Ok(())
    }
    #[flutter_rust_bridge::frb(ignore)]
    fn with<T>(
        &self,
        operation: impl FnOnce(&crate::favorites::Favorites) -> Result<T>,
    ) -> Result<T> {
        let guard = self
            .inner
            .lock()
            .map_err(|_| anyhow!("favorites lock poisoned"))?;
        operation(
            guard
                .as_ref()
                .ok_or_else(|| anyhow!("favorites store closed"))?,
        )
    }
}
