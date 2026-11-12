use crate::history::History;
use anyhow::{ensure, Context, Result};
use rusqlite::{params, OptionalExtension, Row};
use serde_json::{json, Value};
use std::collections::{BTreeMap, HashSet};

fn row_value(r: &Row) -> rusqlite::Result<Value> {
    Ok(json!({
        "id": r.get::<_, String>("id")?, "title": r.get::<_, String>("title")?,
        "sub_title": r.get::<_, String>("sub_title")?, "author": r.get::<_, String>("author")?,
        "tags": r.get::<_, String>("tags")?, "translated_tags": r.get::<_, String>("translated_tags")?,
        "time": r.get::<_, i64>("time")?, "max_page": r.get::<_, i32>("max_page")?,
        "source_key": r.get::<_, String>("source_key")?,
        "image_favorites_ep": r.get::<_, String>("image_favorites_ep")?,
        "other": r.get::<_, String>("other")?,
    }))
}

fn string<'a>(value: &'a Value, key: &str) -> Result<&'a str> {
    value[key]
        .as_str()
        .with_context(|| format!("missing string: {key}"))
}

fn normalize_episodes(episodes: Vec<Value>) -> Result<Vec<Value>> {
    let mut seen = HashSet::new();
    let mut result = Vec::new();
    for mut episode in episodes {
        let number = episode["ep"].as_i64().context("missing episode number")?;
        if number < 1 || !seen.insert(number) {
            continue;
        }
        let mut pages = BTreeMap::new();
        for page in episode["imageFavorites"]
            .as_array()
            .context("missing image favorites")?
        {
            let number = page["page"].as_i64().context("missing page number")?;
            if number < 1 {
                continue;
            }
            let mut value = json!({ "page": number, "imageKey": string(page, "imageKey")? });
            if let Some(auto) = page["isAutoFavorite"].as_bool() {
                value["isAutoFavorite"] = auto.into();
            }
            pages.entry(number).or_insert(value);
        }
        episode["imageFavorites"] = pages.into_values().collect::<Vec<_>>().into();
        result.push(episode);
    }
    result.sort_by_key(|e| e["ep"].as_i64().unwrap_or_default());
    Ok(result)
}

impl History {
    pub fn init_image_favorites(&self) -> Result<()> {
        self.db.execute_batch("CREATE TABLE IF NOT EXISTS image_favorites (id TEXT, title TEXT NOT NULL, sub_title TEXT, author TEXT, tags TEXT, translated_tags TEXT, time INT, max_page INT, source_key TEXT NOT NULL, image_favorites_ep TEXT NOT NULL, other TEXT NOT NULL, PRIMARY KEY(id,source_key));")?;
        Ok(())
    }

    pub fn image_save(&self, comic: Value) -> Result<()> {
        let id = string(&comic, "id")?;
        let source = string(&comic, "source_key")?;
        let episodes: Vec<Value> = serde_json::from_str(string(&comic, "image_favorites_ep")?)?;
        if episodes.is_empty() {
            self.db.execute(
                "DELETE FROM image_favorites WHERE id=? AND source_key=?",
                params![id, source],
            )?;
            return Ok(());
        }
        let episodes = normalize_episodes(episodes)?;
        ensure!(!episodes.is_empty(), "No valid image favorite chapters");
        self.db.execute("INSERT OR REPLACE INTO image_favorites (id,title,sub_title,author,tags,translated_tags,time,max_page,source_key,image_favorites_ep,other) VALUES (?,?,?,?,?,?,?,?,?,?,?)",
            params![id, string(&comic,"title")?,string(&comic,"sub_title")?,string(&comic,"author")?,string(&comic,"tags")?,string(&comic,"translated_tags")?,comic["time"].as_i64().context("invalid time")?,comic["max_page"].as_i64().context("invalid max_page")?,source,serde_json::to_string(&episodes)?,string(&comic,"other")?])?;
        Ok(())
    }

    pub fn image_find(&self, id: &str, source: &str) -> Result<Option<Value>> {
        Ok(self
            .db
            .query_row(
                "SELECT * FROM image_favorites WHERE id=? AND source_key=?",
                params![id, source],
                row_value,
            )
            .optional()?)
    }

    pub fn image_list(&self, keyword: Option<&str>) -> Result<Vec<Value>> {
        let mut stmt = self.db.prepare("SELECT * FROM image_favorites WHERE ? = '' OR title LIKE ? OR sub_title LIKE ? OR LOWER(tags) LIKE LOWER(?) OR LOWER(translated_tags) LIKE LOWER(?) OR author LIKE ?")?;
        let keyword = keyword.unwrap_or("");
        let pattern = format!("%{keyword}%");
        let rows = stmt.query_map(
            params![keyword, pattern, pattern, pattern, pattern, pattern],
            row_value,
        )?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    pub fn image_count(&self) -> Result<u32> {
        Ok(self
            .db
            .query_row("SELECT count(*) FROM image_favorites", [], |r| r.get(0))?)
    }

    pub fn image_has(
        &self,
        id: &str,
        source: &str,
        eid: &str,
        page: i32,
        episode: i32,
    ) -> Result<bool> {
        let Some(comic) = self.image_find(id, source)? else {
            return Ok(false);
        };
        let episodes: Vec<Value> = serde_json::from_str(string(&comic, "image_favorites_ep")?)?;
        Ok(episodes.iter().any(|e| {
            e["eid"] == eid
                && e["ep"] == episode
                && e["imageFavorites"]
                    .as_array()
                    .is_some_and(|pages| pages.iter().any(|p| p["page"] == page))
        }))
    }

    pub fn image_delete(&mut self, images: Vec<Value>) -> Result<()> {
        self.db.execute_batch("SAVEPOINT delete_images")?;
        let result = (|| -> Result<()> {
            for image in images {
                let Some(mut comic) =
                    self.image_find(string(&image, "id")?, string(&image, "sourceKey")?)?
                else {
                    continue;
                };
                let mut episodes: Vec<Value> =
                    serde_json::from_str(string(&comic, "image_favorites_ep")?)?;
                for episode in &mut episodes {
                    if episode["ep"] == image["ep"] && episode["eid"] == image["eid"] {
                        if let Some(pages) = episode["imageFavorites"].as_array_mut() {
                            pages.retain(|p| p["page"] != image["page"]);
                        }
                    }
                }
                episodes.retain(|e| {
                    e["imageFavorites"]
                        .as_array()
                        .is_some_and(|pages| !pages.is_empty())
                });
                comic["image_favorites_ep"] = serde_json::to_string(&episodes)?.into();
                self.image_save(comic)?;
            }
            Ok(())
        })();
        match result {
            Ok(()) => {
                self.db.execute_batch("RELEASE delete_images")?;
                Ok(())
            }
            Err(error) => {
                self.db
                    .execute_batch("ROLLBACK TO delete_images; RELEASE delete_images")?;
                Err(error)
            }
        }
    }

    pub fn image_statistics(&self) -> Result<Value> {
        let mut tags = BTreeMap::<String, u64>::new();
        let mut authors = BTreeMap::<String, u64>::new();
        let mut comics = Vec::new();
        let mut count = 0;
        const EXCLUDED: &[&str] = &[
            "連載中",
            "",
            "translated",
            "chinese",
            "sole male",
            "sole female",
            "original",
            "doujinshi",
            "manga",
            "multi-work series",
            "mosaic censorship",
            "dilf",
            "bbm",
            "uncensored",
            "full censorship",
        ];
        for comic in self.image_list(None)? {
            let episodes: Vec<Value> = serde_json::from_str(string(&comic, "image_favorites_ep")?)?;
            let images: u64 = episodes
                .iter()
                .map(|e| e["imageFavorites"].as_array().map_or(0, |p| p.len() as u64))
                .sum();
            let pages: u64 = episodes
                .iter()
                .map(|e| e["maxPage"].as_u64().unwrap_or(1))
                .sum();
            count += images;
            for tag in string(&comic, "tags")?.split(',') {
                let tag = tag.rsplit(':').next().unwrap_or(tag);
                if !EXCLUDED.contains(&tag.to_lowercase().as_str()) && tag.parse::<f64>().is_err() {
                    *tags.entry(tag.to_owned()).or_default() += 1;
                }
            }
            let author = string(&comic, "author")?;
            if !author.is_empty() {
                *authors.entry(author.to_owned()).or_default() += images;
            }
            if pages >= 10 {
                comics.push((string(&comic, "title")?.to_owned(), images));
            }
        }
        fn top(mut rows: Vec<(String, u64)>) -> Value {
            rows.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
            rows.into_iter()
                .take(20)
                .map(|(text, count)| json!({"text":text,"count":count}))
                .collect()
        }
        Ok(
            json!({ "tags": top(tags.into_iter().collect()), "authors":top(authors.into_iter().collect()), "comics":top(comics), "count":count }),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn normalizes_and_deletes_images_atomically() -> Result<()> {
        let mut history = History::open(":memory:")?;
        let episodes = json!([{"eid":"one","ep":1,"epName":"Chapter","maxPage":20,"imageFavorites":[{"page":2,"imageKey":"two"},{"page":1,"imageKey":"one"},{"page":1,"imageKey":"duplicate"}]}]);
        history.image_save(json!({"id":"comic","source_key":"source","title":"Title","sub_title":"","author":"Author","tags":"tag","translated_tags":"","time":100,"max_page":20,"other":"{}","image_favorites_ep":episodes.to_string()}))?;
        assert!(history.image_has("comic", "source", "one", 1, 1)?);
        assert_eq!(history.image_statistics()?["count"], 2);
        let invalid = vec![
            json!({"id":"comic","sourceKey":"source","eid":"one","ep":1,"page":1}),
            json!({}),
        ];
        assert!(history.image_delete(invalid).is_err());
        assert!(history.image_has("comic", "source", "one", 1, 1)?);
        history.image_delete(vec![
            json!({"id":"comic","sourceKey":"source","eid":"one","ep":1,"page":1}),
        ])?;
        assert!(!history.image_has("comic", "source", "one", 1, 1)?);
        assert_eq!(history.image_statistics()?["count"], 1);
        Ok(())
    }
}
