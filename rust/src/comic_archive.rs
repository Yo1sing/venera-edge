use anyhow::{ensure, Context, Result};
use serde_json::{json, Value};
use std::{fs, path::Path};

pub fn import(input: &str, cache: &str, local: &str, database: &str) -> Result<String> {
    fs::create_dir_all(cache)?;
    let temp = tempfile::tempdir_in(cache)?;
    crate::archives::extract(Path::new(input), temp.path())?;
    let entries = fs::read_dir(temp.path())?.collect::<std::io::Result<Vec<_>>>()?;
    let root = if entries.len() == 1 && entries[0].file_type()?.is_dir() {
        entries[0].path()
    } else {
        temp.path().to_owned()
    };
    let default_title = Path::new(input)
        .file_stem()
        .context("Invalid archive name")?
        .to_string_lossy();
    let metadata = fs::read(root.join("metadata.json"))
        .ok()
        .and_then(|b| serde_json::from_slice::<Value>(&b).ok())
        .unwrap_or_else(|| json!({"title":default_title,"author":"","tags":[]}));
    let title = metadata["title"].as_str().context("Invalid comic title")?;
    let library = crate::local::LocalLibrary::open(database)?;
    ensure!(
        library.find_name(title)?.is_none(),
        "Comic with name {title} already exists"
    );
    let mut images = fs::read_dir(&root)?
        .filter_map(|e| e.ok())
        .filter(|e| {
            e.file_type().is_ok_and(|k| k.is_file()) && crate::comic_queue::is_image(&e.path())
        })
        .map(|e| e.path().to_string_lossy().into_owned())
        .collect::<Vec<_>>();
    let cover = images
        .iter()
        .find(|p| Path::new(p).file_stem().is_some_and(|n| n == "cover"))
        .cloned();
    images = crate::api::local::sort_comic_images(images)
        .into_iter()
        .map(|p| p.trim_start_matches("file://").to_owned())
        .collect();
    ensure!(!images.is_empty(), "No images found in archive");
    let cover = cover.unwrap_or_else(|| images[0].clone());
    let name = format!(
        "{}-{}",
        crate::api::local::chapter_directory_name(title.chars().take(65).collect()),
        &uuid::Uuid::new_v4().to_string()[..8]
    );
    fs::create_dir_all(local)?;
    let staging = tempfile::tempdir_in(local)?;
    let cover_name = format!(
        "cover.{}",
        Path::new(&cover)
            .extension()
            .and_then(|v| v.to_str())
            .unwrap_or("png")
    );
    fs::copy(&cover, staging.path().join(&cover_name))?;
    let mut chapters = Value::Null;
    if let Some(list) = metadata["chapters"].as_array() {
        let mut map = serde_json::Map::new();
        for (i, chapter) in list.iter().enumerate() {
            let start = chapter["start"].as_u64().context("Invalid chapter start")? as usize;
            let end = chapter["end"].as_u64().context("Invalid chapter end")? as usize;
            ensure!(
                start > 0 && end >= start && end <= images.len(),
                "Chapter page range is invalid"
            );
            let folder = staging.path().join(i.to_string());
            fs::create_dir(&folder)?;
            copy_images(&images[start - 1..end], &folder)?;
            map.insert(i.to_string(), chapter["title"].clone());
        }
        chapters = map.into();
    } else {
        copy_images(&images, staging.path())?;
    }
    let record = crate::local::LocalComic {
        id: library.next_id(0)?,
        title: title.into(),
        subtitle: metadata["author"].as_str().unwrap_or_default().into(),
        tags: metadata["tags"]
            .as_array()
            .map(|v| {
                v.iter()
                    .filter_map(|v| v.as_str().map(str::to_owned))
                    .collect()
            })
            .unwrap_or_default(),
        directory: name.clone(),
        downloaded_chapters: crate::local::chapter_ids(&chapters),
        chapters,
        cover: cover_name,
        comic_type: 0,
        created_at: crate::cache::now_ms(),
    };
    fs::rename(staging.path(), Path::new(local).join(name))?;
    Ok(serde_json::to_string(&record)?)
}
fn copy_images(images: &[String], output: &Path) -> Result<()> {
    for (i, image) in images.iter().enumerate() {
        let ext = Path::new(image)
            .extension()
            .and_then(|v| v.to_str())
            .unwrap_or("png");
        fs::copy(image, output.join(format!("{}.{ext}", i + 1)))?;
    }
    Ok(())
}
