use anyhow::{Context, Result};
use rusqlite::{Connection, OpenFlags};
use serde_json::{json, Value};
use std::path::Path;

fn tables(db: &Connection) -> Result<Vec<String>> {
    Ok(db
        .prepare("SELECT name FROM sqlite_master WHERE type='table'")?
        .query_map([], |r| r.get(0))?
        .collect::<rusqlite::Result<_>>()?)
}
fn type_id(value: i32, history: bool, types: &Value) -> i32 {
    let source = match value {
        0 => "picacg",
        1 => "ehentai",
        2 => "jm",
        3 => "hitomi",
        4 => "wnacg",
        5 if history => "nhentai",
        6 if !history => "nhentai",
        _ => return value,
    };
    types[source].as_i64().unwrap_or(i64::from(value)) as i32
}
pub fn import(
    input: &str,
    cache: &str,
    directory: &str,
    types: Value,
    installed: Vec<String>,
    traditional: bool,
    at_end: bool,
) -> Result<()> {
    std::fs::create_dir_all(cache)?;
    let temp = tempfile::tempdir_in(cache)?;
    crate::archives::extract(Path::new(input), temp.path())?;
    let favorite_file = temp.path().join("local_favorite.db");
    if favorite_file.exists() {
        let source = Connection::open_with_flags(favorite_file, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
        let target = crate::favorites::Favorites::open(
            &format!("{directory}/local_favorite.db"),
            traditional,
        )?;
        let names = tables(&source)?;
        for name in names.iter().filter(|n| {
            !["folder_order", "folder_sync"].contains(&n.as_str()) && !n.starts_with("sqlite_")
        }) {
            target.create(name, false)?;
            let query = format!("SELECT * FROM \"{}\"", name.replace('"', "\"\""));
            let mut statement = source.prepare(&query)?;
            let mut rows = statement.query([])?;
            while let Some(row) = rows.next()? {
                let favorite = crate::favorites::Favorite {
                    id: row.get("target")?,
                    name: row.get("name")?,
                    author: row.get("author")?,
                    cover: row.get("cover_path")?,
                    comic_type: type_id(row.get("type")?, false, &types),
                    tags: row
                        .get::<_, String>("tags")?
                        .split(',')
                        .map(str::to_owned)
                        .collect(),
                    time: String::new(),
                    folder: name.clone(),
                    translated_tags: String::new(),
                    last_update_time: None,
                    has_new_update: false,
                    last_check_time: None,
                };
                target.add(name, &favorite, None, at_end, traditional)?;
            }
        }
        if names.iter().any(|n| n == "folder_sync") {
            let mut stmt = source.prepare("SELECT * FROM folder_sync")?;
            let mut rows = stmt.query([])?;
            while let Some(row) = rows.next()? {
                let folder: String = row.get("folder_name")?;
                if !target.linked(&folder)?.is_empty() {
                    continue;
                }
                let mut key: String = row.get("key")?;
                if key.eq_ignore_ascii_case("htmanga") {
                    key = "wnacg".into();
                }
                let data: Value = serde_json::from_str(&row.get::<_, String>("sync_data")?)?;
                if let Some(id) = data["folderId"].as_str() {
                    target.link(&folder, &key, id)?;
                }
            }
        }
    }
    let history_file = temp.path().join("history.db");
    if history_file.exists() {
        let source = Connection::open_with_flags(history_file, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
        let names = tables(&source)?;
        let mut target = crate::history::History::open(&format!("{directory}/history.db"))?;
        if names.iter().any(|n| n == "history") {
            let mut statement = source.prepare("SELECT * FROM history")?;
            let mut rows = statement.query([])?;
            while let Some(row) = rows.next()? {
                let episode: i32 = row.get("ep")?;
                target.save(&crate::api::history::HistoryEntry {
                    id: row.get("target")?,
                    title: row.get("title")?,
                    subtitle: row.get("subtitle")?,
                    cover: row.get("cover")?,
                    time_ms: row.get("time")?,
                    comic_type: type_id(row.get("type")?, true, &types),
                    episode,
                    page: row.get("page")?,
                    max_page: row.get("max_page")?,
                    read_episodes: vec![episode.to_string()],
                    chapter_group: None,
                })?;
            }
        }
        if names.iter().any(|n| n == "image_favorites") {
            let mut statement = source.prepare("SELECT * FROM image_favorites")?;
            let mut rows = statement.query([])?;
            while let Some(row) = rows.next()? {
                let key: String = row.get("id")?;
                let (source, id) = key
                    .split_once('-')
                    .context("Invalid legacy image favorite id")?;
                let source = if source.eq_ignore_ascii_case("htmanga") {
                    "wnacg"
                } else {
                    source
                };
                if !installed.iter().any(|s| s == source) {
                    continue;
                }
                let ep = row.get::<_, i64>("ep")?.max(1);
                let page: i64 = row.get("page")?;
                let mut comic=target.image_find(id,source)?.unwrap_or_else(||json!({"id":id,"title":row.get::<_,String>("title").unwrap_or_default(),"sub_title":"","author":"","tags":"","translated_tags":"","time":crate::cache::now_ms(),"max_page":1,"source_key":source,"image_favorites_ep":"[]","other":"{}"}));
                let mut episodes: Vec<Value> = serde_json::from_str(
                    comic["image_favorites_ep"]
                        .as_str()
                        .context("Invalid image favorites")?,
                )?;
                if let Some(chapter) = episodes.iter_mut().find(|e| e["ep"] == ep) {
                    let pages = chapter["imageFavorites"]
                        .as_array_mut()
                        .context("Invalid image favorites")?;
                    if !pages.iter().any(|p| p["page"] == page) {
                        pages.push(json!({"page":page,"imageKey":""}));
                    }
                } else {
                    episodes.push(json!({"eid":"","ep":ep,"epName":"","maxPage":1,"imageFavorites":[{"page":page,"imageKey":""}]}));
                }
                comic["image_favorites_ep"] = serde_json::to_string(&episodes)?.into();
                target.image_save(comic)?;
            }
        }
    }
    Ok(())
}
