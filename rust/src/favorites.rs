use anyhow::{anyhow, ensure, Result};
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Favorite {
    pub id: String,
    pub name: String,
    pub author: String,
    #[serde(rename = "type")]
    pub comic_type: i32,
    pub tags: Vec<String>,
    #[serde(rename = "coverPath")]
    pub cover: String,
    #[serde(default)]
    pub time: String,
    #[serde(default)]
    pub folder: String,
    #[serde(default)]
    pub translated_tags: String,
    #[serde(default)]
    pub last_update_time: Option<String>,
    #[serde(default)]
    pub has_new_update: bool,
    #[serde(default)]
    pub last_check_time: Option<i64>,
}

#[derive(Deserialize)]
pub struct Key {
    pub id: String,
    #[serde(rename = "type")]
    pub comic_type: i32,
}

pub struct Favorites {
    db: Connection,
}

fn quoted(name: &str) -> String {
    format!("\"{}\"", name.replace('"', "\"\""))
}
fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && !name.contains('\0')
        && !name.to_lowercase().starts_with("sqlite_")
        && !["folder_order", "folder_sync"].contains(&name.to_lowercase().as_str())
}
fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64
}

impl Favorites {
    pub fn open(path: &str, traditional: bool) -> Result<Self> {
        let store = Self {
            db: Connection::open(path)?,
        };
        store.db.execute_batch("CREATE TABLE IF NOT EXISTS folder_order (folder_name TEXT PRIMARY KEY, order_value INT);
            CREATE TABLE IF NOT EXISTS folder_sync (folder_name TEXT PRIMARY KEY, source_key TEXT, source_folder TEXT);")?;
        let transaction = store.db.unchecked_transaction()?;
        for folder in store.folders()? {
            let table = quoted(&folder);
            let columns = store
                .db
                .prepare(&format!("PRAGMA table_info({table})"))?
                .query_map([], |row| row.get::<_, String>(1))?
                .collect::<rusqlite::Result<HashSet<_>>>()?;
            for (column, kind) in [
                ("translated_tags", "TEXT"),
                ("last_update_time", "TEXT"),
                ("has_new_update", "INT"),
                ("last_check_time", "INT"),
            ] {
                if !columns.contains(column) {
                    store.db.execute_batch(&format!(
                        "ALTER TABLE {table} ADD COLUMN {column} {kind}"
                    ))?;
                }
            }
            if !columns.contains("translated_tags") {
                for comic in store.list(Some(&folder), None, false)? {
                    store.db.execute(
                        &format!("UPDATE {table} SET translated_tags=? WHERE id=? AND type=?"),
                        params![
                            crate::api::tags::translated_tags(&comic.tags, traditional),
                            comic.id,
                            comic.comic_type
                        ],
                    )?;
                }
            }
        }
        transaction.commit()?;
        Ok(store)
    }

    pub fn folders(&self) -> Result<Vec<String>> {
        Ok(self.db.prepare("SELECT name FROM sqlite_master LEFT JOIN folder_order ON name=folder_name
            WHERE type='table' AND name NOT IN ('folder_order','folder_sync') AND name NOT LIKE 'sqlite_%'
            ORDER BY COALESCE(order_value,0), sqlite_master.rowid")?
            .query_map([], |row| row.get(0))?.collect::<rusqlite::Result<_>>()?)
    }

    fn table(&self, folder: &str) -> Result<String> {
        ensure!(
            self.folders()?.iter().any(|v| v == folder),
            "Folder does not exist: {folder}"
        );
        Ok(quoted(folder))
    }

    pub fn create(&self, name: &str, rename_invalid: bool) -> Result<String> {
        let folders = self.folders()?;
        let mut name = name.to_owned();
        if !valid_name(&name) || folders.iter().any(|v| v.eq_ignore_ascii_case(&name)) {
            ensure!(rename_invalid, "Invalid or existing folder name");
            let prefix = if valid_name(&name) {
                name.clone()
            } else {
                String::new()
            };
            let mut i = 0;
            loop {
                name = format!("{prefix}{i}");
                if !folders.iter().any(|v| v.eq_ignore_ascii_case(&name)) {
                    break;
                }
                i += 1;
            }
        }
        self.db.execute_batch(&format!("CREATE TABLE {} (id TEXT, name TEXT, author TEXT, type INT, tags TEXT,
            cover_path TEXT, time TEXT, display_order INT, translated_tags TEXT, last_update_time TEXT,
            has_new_update INT, last_check_time INT, PRIMARY KEY(id,type))", quoted(&name)))?;
        Ok(name)
    }

    pub fn list(
        &self,
        folder: Option<&str>,
        keyword: Option<&str>,
        only_updates: bool,
    ) -> Result<Vec<Favorite>> {
        let folders = match folder {
            Some(v) => vec![v.to_owned()],
            None => self.folders()?,
        };
        let words: Vec<_> = keyword
            .unwrap_or("")
            .split_whitespace()
            .map(str::to_lowercase)
            .collect();
        let mut result = Vec::new();
        for folder in folders {
            let table = self.table(&folder)?;
            let mut statement = self.db.prepare(&format!("SELECT id,name,author,type,tags,cover_path,time,
                translated_tags,last_update_time,has_new_update,last_check_time FROM {table} ORDER BY display_order"))?;
            let rows = statement.query_map([], |r| {
                Ok(Favorite {
                    id: r.get(0)?,
                    name: r.get(1)?,
                    author: r.get(2)?,
                    comic_type: r.get(3)?,
                    tags: r
                        .get::<_, String>(4)?
                        .split(',')
                        .filter(|v| !v.is_empty())
                        .map(str::to_owned)
                        .collect(),
                    cover: r.get(5)?,
                    time: r.get(6)?,
                    folder: folder.clone(),
                    translated_tags: r.get::<_, Option<String>>(7)?.unwrap_or_default(),
                    last_update_time: r.get(8)?,
                    has_new_update: r.get::<_, Option<i32>>(9)?.unwrap_or(0) == 1,
                    last_check_time: r.get(10)?,
                })
            })?;
            for row in rows {
                let comic = row?;
                if only_updates && !comic.has_new_update {
                    continue;
                }
                let text = format!(
                    "{}\n{}\n{}\n{}",
                    comic.name,
                    comic.author,
                    comic.tags.join(","),
                    comic.translated_tags
                )
                .to_lowercase();
                if words.iter().all(|word| text.contains(word)) {
                    result.push(comic);
                }
            }
        }
        Ok(result)
    }

    pub fn find(&self, id: &str, comic_type: i32) -> Result<Vec<String>> {
        let mut result = Vec::new();
        for folder in self.folders()? {
            if self.db.query_row(
                &format!(
                    "SELECT EXISTS(SELECT 1 FROM {} WHERE id=? AND type=?)",
                    quoted(&folder)
                ),
                params![id, comic_type],
                |r| r.get::<_, bool>(0),
            )? {
                result.push(folder);
            }
        }
        Ok(result)
    }

    pub fn summary(&self) -> Result<Value> {
        let mut counts = serde_json::Map::new();
        let mut keys = HashSet::new();
        for folder in self.folders()? {
            let mut count = 0;
            for key in self
                .db
                .prepare(&format!("SELECT id,type FROM {}", quoted(&folder)))?
                .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i32>(1)?)))?
            {
                keys.insert(key?);
                count += 1;
            }
            counts.insert(folder, json!(count));
        }
        Ok(json!({ "counts": counts, "total": keys.len() }))
    }

    pub fn bound(&self, folder: &str, maximum: bool) -> Result<i64> {
        let function = if maximum { "MAX" } else { "MIN" };
        Ok(self.db.query_row(
            &format!(
                "SELECT COALESCE({function}(display_order),0) FROM {}",
                self.table(folder)?
            ),
            [],
            |r| r.get(0),
        )?)
    }

    pub fn add(
        &self,
        folder: &str,
        comic: &Favorite,
        order: Option<i64>,
        at_end: bool,
        traditional: bool,
    ) -> Result<bool> {
        let table = self.table(folder)?;
        let order = order.unwrap_or(self.bound(folder, at_end)? + if at_end { 1 } else { -1 });
        let time = if comic.time.is_empty() {
            self.db
                .query_row("SELECT datetime('now','localtime')", [], |r| {
                    r.get::<_, String>(0)
                })?
        } else {
            comic.time.clone()
        };
        Ok(self.db.execute(&format!("INSERT OR IGNORE INTO {table}
            (id,name,author,type,tags,cover_path,time,display_order,translated_tags,last_update_time,has_new_update,last_check_time)
            VALUES (?,?,?,?,?,?,?,?,?,?,?,?)"), params![comic.id,comic.name,comic.author,comic.comic_type,comic.tags.join(","),
                comic.cover,time,order,crate::api::tags::translated_tags(&comic.tags,traditional),comic.last_update_time,
                comic.has_new_update as i32,comic.last_check_time])? > 0)
    }

    pub fn transfer(
        &self,
        source: &str,
        target: &str,
        keys: &[Key],
        copy: bool,
        at_end: bool,
        keep_duplicate: bool,
    ) -> Result<()> {
        let source_table = self.table(source)?;
        let target_table = self.table(target)?;
        if source == target {
            return Ok(());
        }
        let transaction = self.db.unchecked_transaction()?;
        let mut order = self.bound(target, at_end)? + if at_end { 1 } else { -1 };
        for key in keys {
            let inserted = self.db.execute(&format!("INSERT OR IGNORE INTO {target_table}
                (id,name,author,type,tags,cover_path,time,display_order,translated_tags,last_update_time,has_new_update,last_check_time)
                SELECT id,name,author,type,tags,cover_path,time,?,translated_tags,last_update_time,has_new_update,last_check_time
                FROM {source_table} WHERE id=? AND type=?"), params![order,key.id,key.comic_type])?;
            if !copy && (inserted > 0 || !keep_duplicate) {
                self.db.execute(
                    &format!("DELETE FROM {source_table} WHERE id=? AND type=?"),
                    params![key.id, key.comic_type],
                )?;
            }
            order += 1;
        }
        transaction.commit()?;
        Ok(())
    }

    pub fn delete(&self, folder: Option<&str>, keys: &[Key]) -> Result<()> {
        let folders = match folder {
            Some(v) => vec![v.to_owned()],
            None => self.folders()?,
        };
        let transaction = self.db.unchecked_transaction()?;
        for folder in folders {
            let table = self.table(&folder)?;
            for key in keys {
                self.db.execute(
                    &format!("DELETE FROM {table} WHERE id=? AND type=?"),
                    params![key.id, key.comic_type],
                )?;
            }
        }
        transaction.commit()?;
        Ok(())
    }

    pub fn delete_folder(&self, folder: &str) -> Result<()> {
        let table = self.table(folder)?;
        let transaction = self.db.unchecked_transaction()?;
        self.db.execute_batch(&format!("DROP TABLE {table}"))?;
        self.db
            .execute("DELETE FROM folder_order WHERE folder_name=?", [folder])?;
        self.db
            .execute("DELETE FROM folder_sync WHERE folder_name=?", [folder])?;
        transaction.commit()?;
        Ok(())
    }

    pub fn clear(&self) -> Result<()> {
        // Each folder deletion is transactional; keep the database connection alive.
        for folder in self.folders()? {
            self.delete_folder(&folder)?;
        }
        Ok(())
    }

    pub fn rename(&self, before: &str, after: &str) -> Result<()> {
        ensure!(valid_name(after), "Invalid folder name");
        let table = self.table(before)?;
        ensure!(
            !self
                .folders()?
                .iter()
                .any(|v| v.eq_ignore_ascii_case(after)),
            "Name already exists"
        );
        let transaction = self.db.unchecked_transaction()?;
        self.db
            .execute_batch(&format!("ALTER TABLE {table} RENAME TO {}", quoted(after)))?;
        for table in ["folder_order", "folder_sync"] {
            self.db.execute(
                &format!("UPDATE {table} SET folder_name=? WHERE folder_name=?"),
                params![after, before],
            )?;
        }
        transaction.commit()?;
        Ok(())
    }

    pub fn order_folders(&self, folders: &[String]) -> Result<()> {
        let transaction = self.db.unchecked_transaction()?;
        for (i, folder) in folders.iter().enumerate() {
            self.table(folder)?;
            self.db.execute(
                "INSERT OR REPLACE INTO folder_order VALUES (?,?)",
                params![folder, i as i64],
            )?;
        }
        transaction.commit()?;
        Ok(())
    }

    pub fn reorder(&self, folder: &str, keys: &[Key]) -> Result<()> {
        let table = self.table(folder)?;
        let transaction = self.db.unchecked_transaction()?;
        for (i, key) in keys.iter().enumerate() {
            self.db.execute(
                &format!("UPDATE {table} SET display_order=? WHERE id=? AND type=?"),
                params![i as i64, key.id, key.comic_type],
            )?;
        }
        transaction.commit()?;
        Ok(())
    }

    pub fn link(&self, folder: &str, source: &str, network_folder: &str) -> Result<()> {
        self.table(folder)?;
        self.db.execute(
            "INSERT OR REPLACE INTO folder_sync VALUES (?,?,?)",
            params![folder, source, network_folder],
        )?;
        Ok(())
    }

    pub fn linked(&self, folder: &str) -> Result<Vec<String>> {
        Ok(self
            .db
            .query_row(
                "SELECT source_key,source_folder FROM folder_sync WHERE folder_name=?",
                [folder],
                |r| Ok(vec![r.get(0)?, r.get(1)?]),
            )
            .optional()?
            .unwrap_or_default())
    }

    pub fn edit_tags(
        &self,
        folder: &str,
        id: &str,
        tags: &[String],
        prepend: bool,
        traditional: bool,
    ) -> Result<()> {
        let table = self.table(folder)?;
        let transaction = self.db.unchecked_transaction()?;
        for mut comic in self
            .list(Some(folder), None, false)?
            .into_iter()
            .filter(|c| c.id == id)
        {
            let mut updated = tags.to_vec();
            if prepend {
                updated.append(&mut comic.tags);
            }
            self.db.execute(
                &format!("UPDATE {table} SET tags=?,translated_tags=? WHERE id=? AND type=?"),
                params![
                    updated.join(","),
                    crate::api::tags::translated_tags(&updated, traditional),
                    id,
                    comic.comic_type
                ],
            )?;
        }
        transaction.commit()?;
        Ok(())
    }

    pub fn update_info(&self, folder: &str, comic: &Favorite, traditional: bool) -> Result<()> {
        self.db.execute(&format!("UPDATE {} SET name=?,author=?,cover_path=?,tags=?,translated_tags=? WHERE id=? AND type=?", self.table(folder)?),
            params![comic.name,comic.author,comic.cover,comic.tags.join(","),crate::api::tags::translated_tags(&comic.tags,traditional),comic.id,comic.comic_type])?;
        Ok(())
    }

    pub fn prepare_follow(&self, folder: &str, clear: bool) -> Result<()> {
        let table = self.table(folder)?;
        if clear {
            self.db
                .execute(&format!("UPDATE {table} SET has_new_update=0"), [])?;
        }
        Ok(())
    }

    pub fn update_check(
        &self,
        folder: &str,
        id: &str,
        comic_type: i32,
        update_time: Option<&str>,
    ) -> Result<()> {
        let table = self.table(folder)?;
        if let Some(time) = update_time {
            // Keep an unread update until it is actually read, even when polling again.
            self.db.execute(&format!("UPDATE {table} SET has_new_update=CASE WHEN last_update_time IS NOT ? THEN 1 ELSE has_new_update END,
                last_update_time=?,last_check_time=? WHERE id=? AND type=?"), params![time,time,now_ms(),id,comic_type])?;
        } else {
            self.db.execute(
                &format!("UPDATE {table} SET last_check_time=? WHERE id=? AND type=?"),
                params![now_ms(), id, comic_type],
            )?;
        }
        Ok(())
    }

    pub fn on_read(
        &self,
        id: &str,
        comic_type: i32,
        position: &str,
        follow_folder: Option<&str>,
    ) -> Result<()> {
        let transaction = self.db.unchecked_transaction()?;
        for folder in self.find(id, comic_type)? {
            let table = quoted(&folder);
            if position != "none" {
                if position == "start" || position == "end" {
                    let at_end = position == "end";
                    self.db.execute(
                        &format!("UPDATE {table} SET display_order=? WHERE id=? AND type=?"),
                        params![
                            self.bound(&folder, at_end)? + if at_end { 1 } else { -1 },
                            id,
                            comic_type
                        ],
                    )?;
                }
                self.db.execute(
                    &format!(
                        "UPDATE {table} SET time=datetime('now','localtime') WHERE id=? AND type=?"
                    ),
                    params![id, comic_type],
                )?;
            }
            if follow_folder == Some(folder.as_str()) {
                self.db.execute(
                    &format!("UPDATE {table} SET has_new_update=0 WHERE id=? AND type=?"),
                    params![id, comic_type],
                )?;
            }
        }
        transaction.commit()?;
        Ok(())
    }

    pub fn export(&self, folder: &str) -> Result<String> {
        let comics: Vec<_> = self.list(Some(folder),None,false)?.into_iter().map(|c| json!({
            "name":c.name,"author":c.author,"type":c.comic_type,"tags":c.tags,"id":c.id,"coverPath":c.cover
        })).collect();
        Ok(serde_json::to_string(
            &json!({"info":"Generated by Venera Edge","name":folder,"comics":comics}),
        )?)
    }

    pub fn import(
        &self,
        text: &str,
        legacy_types: &HashMap<i32, i32>,
        at_end: bool,
        traditional: bool,
    ) -> Result<()> {
        let data: Value = serde_json::from_str(text)?;
        let name = data["name"]
            .as_str()
            .ok_or_else(|| anyhow!("Missing folder name"))?;
        ensure!(valid_name(name), "Invalid folder name");
        let folders = self.folders()?;
        let mut name = name.to_owned();
        if folders.iter().any(|v| v.eq_ignore_ascii_case(&name)) {
            let prefix = name.clone();
            let mut i = 0;
            loop {
                name = format!("{prefix}({i})");
                if !folders.iter().any(|v| v.eq_ignore_ascii_case(&name)) {
                    break;
                }
                i += 1;
            }
        }
        let transaction = self.db.unchecked_transaction()?;
        self.create(&name, false)?;
        for comic in data["comics"]
            .as_array()
            .ok_or_else(|| anyhow!("Missing comics"))?
        {
            let mut comic = comic.clone();
            if comic["id"].is_null() {
                comic["id"] = comic["target"].clone();
            }
            if comic["tags"].is_null() {
                comic["tags"] = json!([]);
            }
            let mut comic: Favorite = serde_json::from_value(comic)?;
            if comic.comic_type != 0 || comic.cover.starts_with("http") {
                if let Some(value) = legacy_types.get(&comic.comic_type) {
                    comic.comic_type = *value;
                }
            }
            self.add(&name, &comic, None, at_end, traditional)?;
        }
        transaction.commit()?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn comic(id: &str) -> Favorite {
        serde_json::from_value(json!({"id":id,"name":"Title","author":"Author","type":42,"tags":["tag"],"coverPath":"cover"})).unwrap()
    }
    #[test]
    fn transfers_preserve_metadata_and_counts_and_handle_quoted_names() -> Result<()> {
        let db = Favorites::open(":memory:", false)?;
        let a = db.create("O\"Brien", false)?;
        let b = db.create("B", false)?;
        db.add(&a, &comic("id"), None, true, false)?;
        db.update_check(&a, "id", 42, Some("today"))?;
        db.update_check(&a, "id", 42, Some("today"))?;
        assert!(db.list(Some(&a), None, true)?[0].has_new_update);
        let keys = vec![Key {
            id: "id".into(),
            comic_type: 42,
        }];
        db.transfer(&a, &b, &keys, true, true, false)?;
        assert_eq!(db.summary()?["total"], 1);
        assert_eq!(
            db.list(Some(&b), None, false)?[0]
                .last_update_time
                .as_deref(),
            Some("today")
        );
        db.transfer(&a, &b, &keys, false, true, false)?;
        assert!(db.list(Some(&a), None, false)?.is_empty());
        db.on_read("id", 42, "start", Some(&b))?;
        assert!(db.list(Some(&b), None, true)?.is_empty());
        db.link(&b, "source", "network")?;
        db.rename(&b, "New")?;
        assert_eq!(db.linked("New")?, vec!["source", "network"]);
        db.delete(Some("New"), &keys)?;
        db.delete(Some("New"), &keys)?;
        assert_eq!(db.summary()?["total"], 0);
        Ok(())
    }
    #[test]
    fn legacy_migration_and_import_rollback() -> Result<()> {
        let tmp = tempfile::tempdir()?;
        let path = tmp.path().join("favorites.db");
        let legacy = Connection::open(&path)?;
        for folder in ["first", "second"] {
            legacy.execute_batch(&format!("CREATE TABLE {folder} (id TEXT,name TEXT,author TEXT,type INT,tags TEXT,cover_path TEXT,time TEXT,display_order INT,PRIMARY KEY(id,type));
                INSERT INTO {folder} VALUES ('id','Title','Author',42,'tag','cover','2026-01-01 00:00:00',0)"))?;
        }
        drop(legacy);
        let db = Favorites::open(path.to_str().unwrap(), false)?;
        assert_eq!(db.list(None, None, false)?.len(), 2);
        let json = db.export("first")?;
        db.import(&json, &HashMap::new(), true, false)?;
        assert_eq!(db.folders()?.len(), 3);
        assert!(db
            .import(
                r#"{"name":"broken","comics":[{}]}"#,
                &HashMap::new(),
                true,
                false
            )
            .is_err());
        assert!(!db.folders()?.contains(&"broken".into()));
        assert!(db.create("folder_sync", false).is_err());
        Ok(())
    }
}
