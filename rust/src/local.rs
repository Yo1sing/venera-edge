use anyhow::{anyhow, ensure, Result};
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct LocalComic {
    pub id: String,
    pub title: String,
    pub subtitle: String,
    pub tags: Vec<String>,
    pub directory: String,
    pub chapters: Value,
    pub cover: String,
    pub comic_type: i32,
    pub downloaded_chapters: Vec<String>,
    pub created_at: i64,
}

pub struct LocalLibrary {
    db: Connection,
}

fn row(r: &rusqlite::Row<'_>) -> rusqlite::Result<LocalComic> {
    fn json<T: serde::de::DeserializeOwned>(
        r: &rusqlite::Row<'_>,
        index: usize,
    ) -> rusqlite::Result<T> {
        let text: String = r.get(index)?;
        serde_json::from_str(&text).map_err(|e| {
            rusqlite::Error::FromSqlConversionFailure(
                index,
                rusqlite::types::Type::Text,
                Box::new(e),
            )
        })
    }
    Ok(LocalComic {
        id: r.get(0)?,
        title: r.get(1)?,
        subtitle: r.get(2)?,
        tags: json(r, 3)?,
        directory: r.get(4)?,
        chapters: json(r, 5)?,
        cover: r.get(6)?,
        comic_type: r.get(7)?,
        downloaded_chapters: json(r, 8)?,
        created_at: r.get::<_, Option<i64>>(9)?.unwrap_or(0),
    })
}

pub(crate) fn chapter_ids(chapters: &Value) -> Vec<String> {
    let Some(object) = chapters.as_object() else {
        return Vec::new();
    };
    if object.values().any(|v| !v.is_object()) {
        object
            .iter()
            .filter(|(_, v)| !v.is_object())
            .map(|(k, _)| k.clone())
            .collect()
    } else {
        object
            .values()
            .filter_map(Value::as_object)
            .flat_map(|v| v.keys().cloned())
            .collect()
    }
}

impl LocalLibrary {
    pub fn open(path: &str) -> Result<Self> {
        let db = Connection::open(path)?;
        db.execute_batch("CREATE TABLE IF NOT EXISTS comics (id TEXT NOT NULL,title TEXT NOT NULL,subtitle TEXT NOT NULL,
            tags TEXT NOT NULL,directory TEXT NOT NULL,chapters TEXT NOT NULL,cover TEXT NOT NULL,comic_type INTEGER NOT NULL,
            downloadedChapters TEXT NOT NULL,created_at INTEGER,PRIMARY KEY(id,comic_type))")?;
        Ok(Self { db })
    }
    pub fn find(&self, id: &str, comic_type: i32) -> Result<Option<LocalComic>> {
        Ok(self
            .db
            .query_row(
                "SELECT * FROM comics WHERE id=? AND comic_type=?",
                params![id, comic_type],
                row,
            )
            .optional()?)
    }
    pub fn find_name(&self, name: &str) -> Result<Option<LocalComic>> {
        Ok(self
            .db
            .query_row(
                "SELECT * FROM comics WHERE title=? OR directory=?",
                params![name, name],
                row,
            )
            .optional()?)
    }
    pub fn next_id(&self, comic_type: i32) -> Result<String> {
        let id: i64 = self.db.query_row(
            "SELECT COALESCE(MAX(CAST(id AS INTEGER)),0)+1 FROM comics WHERE comic_type=?",
            [comic_type],
            |r| r.get(0),
        )?;
        Ok(id.to_string())
    }
    pub fn count(&self) -> Result<u32> {
        Ok(self
            .db
            .query_row("SELECT COUNT(*) FROM comics", [], |r| r.get(0))?)
    }
    pub fn save(&self, mut comic: LocalComic) -> Result<()> {
        let transaction = self.db.unchecked_transaction()?;
        if let Some(old) = self.find(&comic.id, comic.comic_type)? {
            comic.downloaded_chapters.extend(old.downloaded_chapters);
        }
        let mut seen = std::collections::HashSet::new();
        comic.downloaded_chapters.retain(|v| seen.insert(v.clone()));
        ensure!(
            comic.chapters.is_null() || comic.chapters.is_object(),
            "Invalid chapters"
        );
        self.db.execute(
            "INSERT OR REPLACE INTO comics VALUES (?,?,?,?,?,?,?,?,?,?)",
            params![
                comic.id,
                comic.title,
                comic.subtitle,
                serde_json::to_string(&comic.tags)?,
                comic.directory,
                serde_json::to_string(&comic.chapters)?,
                comic.cover,
                comic.comic_type,
                serde_json::to_string(&comic.downloaded_chapters)?,
                comic.created_at
            ],
        )?;
        transaction.commit()?;
        Ok(())
    }
    pub fn list(
        &self,
        sort: &str,
        keyword: Option<&str>,
        limit: Option<u32>,
    ) -> Result<Vec<LocalComic>> {
        let order = match sort {
            "name" => "title DESC",
            "time_asc" => "created_at ASC",
            _ => "created_at DESC",
        };
        let query=format!("SELECT * FROM comics WHERE ?1 IS NULL OR title LIKE ?1 OR tags LIKE ?1 OR subtitle LIKE ?1 ORDER BY {order} LIMIT ?2");
        Ok(self
            .db
            .prepare(&query)?
            .query_map(
                params![
                    keyword.map(|v| format!("%{v}%")),
                    limit.map(i64::from).unwrap_or(-1)
                ],
                row,
            )?
            .collect::<rusqlite::Result<_>>()?)
    }
    pub fn delete(&self, keys: &[crate::favorites::Key]) -> Result<()> {
        let transaction = self.db.unchecked_transaction()?;
        for key in keys {
            self.db.execute(
                "DELETE FROM comics WHERE id=? AND comic_type=?",
                params![key.id, key.comic_type],
            )?;
        }
        transaction.commit()?;
        Ok(())
    }
    pub fn delete_chapters(&self, id: &str, comic_type: i32, chapters: &[String]) -> Result<()> {
        if chapters.is_empty() {
            return Ok(());
        }
        let mut comic = self
            .find(id, comic_type)?
            .ok_or_else(|| anyhow!("Comic not found"))?;
        comic.downloaded_chapters.retain(|v| !chapters.contains(v));
        if comic.downloaded_chapters.is_empty() {
            self.db.execute(
                "DELETE FROM comics WHERE id=? AND comic_type=?",
                params![id, comic_type],
            )?;
        } else {
            self.db.execute(
                "UPDATE comics SET downloadedChapters=? WHERE id=? AND comic_type=?",
                params![
                    serde_json::to_string(&comic.downloaded_chapters)?,
                    id,
                    comic_type
                ],
            )?;
        }
        Ok(())
    }
    pub fn downloaded(
        &self,
        id: &str,
        comic_type: i32,
        episode: Option<u32>,
        chapters: Option<Value>,
    ) -> Result<bool> {
        let Some(mut comic) = self.find(id, comic_type)? else {
            return Ok(false);
        };
        let Some(episode) = episode else {
            return Ok(true);
        };
        if comic.chapters.is_null() {
            return Ok(true);
        }
        if let Some(chapters) = chapters {
            if chapter_ids(&chapters).len() != chapter_ids(&comic.chapters).len() {
                self.db.execute(
                    "UPDATE comics SET chapters=? WHERE id=? AND comic_type=?",
                    params![serde_json::to_string(&chapters)?, id, comic_type],
                )?;
            }
            comic.chapters = chapters;
        }
        Ok(episode
            .checked_sub(1)
            .and_then(|index| chapter_ids(&comic.chapters).get(index as usize).cloned())
            .is_some_and(|id| comic.downloaded_chapters.contains(&id)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn merges_progress_and_resolves_grouped_chapters() -> Result<()> {
        let db = LocalLibrary::open(":memory:")?;
        let comic = LocalComic {
            id: "2".into(),
            title: "title".into(),
            subtitle: String::new(),
            tags: vec![],
            directory: "folder".into(),
            chapters: serde_json::json!({"first":{"z":"First"},"second":{"a":"Second"}}),
            cover: "cover".into(),
            comic_type: 0,
            downloaded_chapters: vec!["z".into()],
            created_at: 10,
        };
        db.save(comic.clone())?;
        db.save(comic)?;
        assert_eq!(db.find("2", 0)?.unwrap().downloaded_chapters, vec!["z"]);
        assert_eq!(db.next_id(0)?, "3");
        assert!(db.downloaded("2", 0, Some(1), None)?);
        assert!(!db.downloaded("2", 0, Some(2), None)?);
        db.delete_chapters("2", 0, &["z".into()])?;
        assert_eq!(db.count()?, 0);
        Ok(())
    }
}
