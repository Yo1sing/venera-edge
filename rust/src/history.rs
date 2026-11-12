use crate::api::history::{ComicKey, HistoryEntry};
use anyhow::Result;
use rusqlite::{params, Connection, OptionalExtension, Row};

pub struct History {
    pub(crate) db: Connection,
}

fn entry(row: &Row) -> rusqlite::Result<HistoryEntry> {
    Ok(HistoryEntry {
        id: row.get("id")?,
        title: row.get("title")?,
        subtitle: row.get("subtitle")?,
        cover: row.get("cover")?,
        time_ms: row.get("time")?,
        comic_type: row.get("type")?,
        episode: row.get("ep")?,
        page: row.get("page")?,
        read_episodes: row
            .get::<_, String>("readEpisode")?
            .split(',')
            .filter(|s| !s.is_empty())
            .map(str::to_owned)
            .collect(),
        max_page: row.get("max_page")?,
        chapter_group: row.get("chapter_group")?,
    })
}

impl History {
    pub fn open(path: &str) -> Result<Self> {
        let mut db = Connection::open(path)?;
        db.busy_timeout(std::time::Duration::from_secs(5))?;
        db.execute_batch("CREATE TABLE IF NOT EXISTS history (id TEXT PRIMARY KEY, title TEXT, subtitle TEXT, cover TEXT, time INT, type INT, ep INT, page INT, readEpisode TEXT, max_page INT, chapter_group INT);")?;
        let columns = {
            let mut statement = db.prepare("PRAGMA table_info(history)")?;
            let rows = statement.query_map([], |r| r.get::<_, String>("name"))?;
            rows.collect::<rusqlite::Result<Vec<_>>>()?
        };
        if !columns.iter().any(|s| s == "chapter_group") {
            let transaction = db.transaction()?;
            transaction.execute_batch("ALTER TABLE history ADD COLUMN chapter_group INT;")?;
            transaction.commit()?;
        }
        let history = Self { db };
        history.init_image_favorites()?;
        Ok(history)
    }

    pub fn save(&mut self, item: &HistoryEntry) -> Result<()> {
        self.db.execute("INSERT OR REPLACE INTO history (id,title,subtitle,cover,time,type,ep,page,readEpisode,max_page,chapter_group) VALUES (?,?,?,?,?,?,?,?,?,?,?)",
            params![item.id,item.title,item.subtitle,item.cover,item.time_ms,item.comic_type,item.episode,item.page,item.read_episodes.join(","),item.max_page,item.chapter_group])?;
        Ok(())
    }

    pub fn find(&self, id: &str, comic_type: i32) -> Result<Option<HistoryEntry>> {
        Ok(self
            .db
            .query_row(
                "SELECT * FROM history WHERE id=? AND type=?",
                params![id, comic_type],
                entry,
            )
            .optional()?)
    }

    pub fn list(&self, limit: Option<u32>) -> Result<Vec<HistoryEntry>> {
        let mut statement = self
            .db
            .prepare("SELECT * FROM history ORDER BY time DESC LIMIT ?")?;
        let rows = statement.query_map([limit.map(i64::from).unwrap_or(-1)], entry)?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    pub fn count(&self) -> Result<u32> {
        Ok(self
            .db
            .query_row("SELECT count(*) FROM history", [], |r| r.get(0))?)
    }

    pub fn delete(&mut self, keys: Vec<ComicKey>) -> Result<()> {
        let transaction = self.db.transaction()?;
        for key in keys {
            transaction.execute(
                "DELETE FROM history WHERE id=? AND type=?",
                params![key.id, key.comic_type],
            )?;
        }
        transaction.commit()?;
        Ok(())
    }

    pub fn clear(&self) -> Result<()> {
        self.db.execute("DELETE FROM history", [])?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn migrates_legacy_database_and_preserves_progress() -> Result<()> {
        let directory = tempfile::tempdir()?;
        let path = directory.path().join("history.db");
        let path = path.to_str().unwrap();
        let db = Connection::open(path)?;
        db.execute_batch("CREATE TABLE history (id TEXT PRIMARY KEY, title TEXT, subtitle TEXT, cover TEXT, time INT, type INT, ep INT, page INT, readEpisode TEXT, max_page INT);
            INSERT INTO history VALUES ('id','title','author','cover',100,123,2,5,'1,2',30);")?;
        drop(db);
        let mut history = History::open(path)?;
        let mut record = history.find("id", 123)?.unwrap();
        assert_eq!(record.read_episodes, vec!["1", "2"]);
        assert_eq!(record.page, 5);
        assert_eq!(record.chapter_group, None);
        record.chapter_group = Some(2);
        record.page = 8;
        history.save(&record)?;
        assert_eq!(history.list(Some(20))?, vec![record]);
        history.delete(vec![ComicKey {
            id: "id".into(),
            comic_type: 124,
        }])?;
        assert_eq!(history.count()?, 1);
        history.delete(vec![ComicKey {
            id: "id".into(),
            comic_type: 123,
        }])?;
        assert_eq!(history.count()?, 0);
        Ok(())
    }
}
