use crate::{api::http::HttpCancellation, script::Engine};
use anyhow::{anyhow, ensure, Context, Result};
use futures_util::{stream, StreamExt, TryStreamExt};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{
    collections::HashMap,
    fs,
    io::Write,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::Instant,
};

#[derive(Clone, Deserialize, Serialize)]
pub struct Options {
    pub data_path: String,
    pub cache_path: String,
    pub local_path: String,
    pub saf: bool,
    pub threads: usize,
    pub http: crate::api::http::HttpOptions,
}
struct State {
    items: Vec<Value>,
    running: bool,
    epoch: u64,
    cancel: Option<Arc<HttpCancellation>>,
    options: Options,
}
pub struct Queue {
    engine: Arc<Engine>,
    execution: tokio::sync::Mutex<()>,
    state: Mutex<State>,
    snapshots: tokio::sync::watch::Sender<String>,
    pub snapshot_receiver: tokio::sync::Mutex<tokio::sync::watch::Receiver<String>>,
    platform: tokio::sync::mpsc::Sender<String>,
    pub platform_receiver: tokio::sync::Mutex<tokio::sync::mpsc::Receiver<String>>,
    acknowledgements: Mutex<HashMap<String, tokio::sync::oneshot::Sender<Result<()>>>>,
}
fn key(value: &Value) -> String {
    format!(
        "{}@{}",
        value["source"].as_str().unwrap_or_default(),
        value["comicId"].as_str().unwrap_or_default()
    )
}
fn normalize(value: &mut Value, types: &Value) {
    if value["source"].is_null() {
        value["source"] = value["comic"]["sourceKey"].clone();
    }
    if value["comicId"].is_null() {
        value["comicId"] = value["comic"]["comicId"].clone();
    }
    if value["comicType"].is_null() {
        value["comicType"] = types[value["source"].as_str().unwrap_or_default()].clone();
    }
    value["_key"] = key(value).into();
    value["_running"] = false.into();
    value["_error"] = false.into();
    value["_speed"] = 0.into();
    value["message"] = "Paused".into();
}
impl Queue {
    pub fn open(engine: Arc<Engine>, options: Options, types: Value) -> Result<Arc<Self>> {
        let path = Path::new(&options.data_path).join("downloading_tasks.json");
        let mut items: Vec<Value> = if path.exists() {
            serde_json::from_slice(&fs::read(path)?)?
        } else {
            vec![]
        };
        for item in &mut items {
            normalize(item, &types);
        }
        let (snapshots, receiver) = tokio::sync::watch::channel(serde_json::to_string(&items)?);
        let (platform, platform_receiver) = tokio::sync::mpsc::channel(8);
        Ok(Arc::new(Self {
            engine,
            execution: tokio::sync::Mutex::new(()),
            state: Mutex::new(State {
                items,
                running: false,
                epoch: 0,
                cancel: None,
                options,
            }),
            snapshots,
            snapshot_receiver: tokio::sync::Mutex::new(receiver),
            platform,
            platform_receiver: tokio::sync::Mutex::new(platform_receiver),
            acknowledgements: Mutex::new(HashMap::new()),
        }))
    }
    fn publish(&self, state: &State, persist: bool) -> Result<()> {
        let json = serde_json::to_string(&state.items)?;
        if persist {
            let root = Path::new(&state.options.data_path);
            fs::create_dir_all(root)?;
            let mut file = tempfile::NamedTempFile::new_in(root)?;
            file.write_all(json.as_bytes())?;
            file.as_file().sync_all()?;
            file.persist(root.join("downloading_tasks.json"))?;
        }
        self.snapshots.send_replace(json);
        Ok(())
    }
    pub fn snapshot(&self) -> String {
        self.snapshots.borrow().clone()
    }
    pub fn configure(&self, options: Options) -> Result<()> {
        self.state
            .lock()
            .map_err(|_| anyhow!("Queue poisoned"))?
            .options = options;
        Ok(())
    }
    pub fn add(self: &Arc<Self>, mut item: Value) -> Result<()> {
        normalize(&mut item, &json!({}));
        ensure!(item["comicType"].is_i64(), "Missing comic type");
        {
            let mut state = self.state.lock().map_err(|_| anyhow!("Queue poisoned"))?;
            ensure!(
                !state.items.iter().any(|v| key(v) == key(&item)),
                "Comic is already queued"
            );
            state.items.push(item);
            state.running = true;
            self.publish(&state, true)?;
        }
        self.schedule();
        Ok(())
    }
    pub fn command(self: &Arc<Self>, id: &str, action: &str) -> Result<()> {
        ensure!(
            matches!(action, "pause" | "resume" | "first" | "cancel"),
            "Unknown queue command"
        );
        {
            let mut state = self.state.lock().map_err(|_| anyhow!("Queue poisoned"))?;
            let index = state
                .items
                .iter()
                .position(|v| key(v) == id)
                .context("Download task not found")?;
            // Stop the old generation before changing order; its future cannot update the new generation.
            state.epoch += 1;
            if let Some(cancel) = state.cancel.take() {
                cancel.cancel();
            }
            for item in &mut state.items {
                item["_running"] = false.into();
                item["_speed"] = 0.into();
            }
            match action {
                "pause" => {
                    state.running = false;
                    state.items[index]["message"] = "Paused".into();
                }
                "resume" => {
                    let item = state.items.remove(index);
                    state.items.insert(0, item);
                    state.running = true;
                }
                "first" => {
                    let item = state.items.remove(index);
                    state.items.insert(0, item);
                }
                "cancel" => {
                    state.items.remove(index);
                }
                _ => return Err(anyhow!("Unknown queue command")),
            }
            self.publish(&state, true)?;
        }
        self.schedule();
        Ok(())
    }
    fn schedule(self: &Arc<Self>) {
        let start = {
            let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
            if !state.running || state.items.is_empty() || state.cancel.is_some() {
                return;
            }
            let cancel = Arc::new(HttpCancellation::new());
            state.cancel = Some(cancel.clone());
            state.items[0]["_running"] = true.into();
            state.items[0]["_error"] = false.into();
            state.items[0]["message"] = "Downloading...".into();
            let _ = self.publish(&state, false);
            (
                state.items[0].clone(),
                state.options.clone(),
                state.epoch,
                cancel,
            )
        };
        let queue = self.clone();
        crate::script::executor().spawn(async move {
            let (item, options, epoch, cancel) = start;
            let id = key(&item);
            // A cancelled generation must finish dropping its futures before the next
            // generation can write to the same destination or copy through SAF.
            let work = async {
                let _execution = queue.execution.lock().await;
                queue.run(item, options, epoch).await
            };
            let result = tokio::select! {
                biased;
                _ = cancel.cancelled() => return,
                result = work => result,
            };
            {
                let mut state = queue.state.lock().unwrap_or_else(|e| e.into_inner());
                if state.epoch != epoch {
                    return;
                }
                state.cancel = None;
                if let Some(index) = state.items.iter().position(|v| key(v) == id) {
                    match result {
                        Ok(()) => {
                            state.items.remove(index);
                        }
                        Err(error) => {
                            state.running = false;
                            let item = &mut state.items[index];
                            item["_running"] = false.into();
                            item["_error"] = true.into();
                            item["_speed"] = 0.into();
                            item["message"] = format!("Error: {error:#}").into();
                        }
                    }
                }
                if let Err(error) = queue.publish(&state, true) {
                    state.running = false;
                    eprintln!("Download queue persistence failed: {error}");
                }
            }
            queue.schedule();
        });
    }
    fn update(&self, id: &str, epoch: u64, patch: Value, persist: bool) -> Result<()> {
        let mut state = self.state.lock().map_err(|_| anyhow!("Queue poisoned"))?;
        ensure!(state.epoch == epoch, "Download cancelled");
        let item = state
            .items
            .iter_mut()
            .find(|v| key(v) == id)
            .context("Download cancelled")?;
        for (k, v) in patch.as_object().context("Invalid update")? {
            item[k] = v.clone();
        }
        self.publish(&state, persist)
    }
    pub fn resolve_platform(&self, id: &str, error: Option<String>) -> Result<()> {
        if let Some(sender) = self
            .acknowledgements
            .lock()
            .map_err(|_| anyhow!("Queue poisoned"))?
            .remove(id)
        {
            let _ = sender.send(error.map_or(Ok(()), |e| Err(anyhow!(e))));
        }
        Ok(())
    }
    async fn platform_copy(&self, from: &str, to: &str) -> Result<()> {
        let id = uuid::Uuid::new_v4().to_string();
        let (sender, receiver) = tokio::sync::oneshot::channel();
        self.acknowledgements
            .lock()
            .map_err(|_| anyhow!("Queue poisoned"))?
            .insert(id.clone(), sender);
        self.platform
            .send(json!({"id":id,"operation":"copyDirectory","from":from,"to":to}).to_string())
            .await?;
        receiver.await?
    }
    #[allow(clippy::too_many_arguments)]
    async fn image(
        &self,
        options: &Options,
        source: &str,
        comic: &str,
        chapter: &str,
        image: &str,
        thumbnail: bool,
        id: &str,
        epoch: u64,
    ) -> Result<Vec<u8>> {
        for attempt in 0..3 {
            let task = crate::image_tasks::Task::start(
                self.engine.clone(),
                options.data_path.clone(),
                options.cache_path.clone(),
                image.into(),
                Some(source.into()),
                comic.into(),
                chapter.into(),
                thumbnail,
            )?;
            let result=async {
                let mut receiver=task.receiver.lock().await;let started=Instant::now();let mut last=Instant::now();
                while let Some(progress)=receiver.recv().await {
                    if let Some(error)=progress.error{return Err(anyhow!(error));}
                    if let Some(bytes)=progress.bytes{return Ok(bytes);}
                    if last.elapsed().as_millis()>300 {self.update(id,epoch,json!({"_speed":(progress.current as f64/started.elapsed().as_secs_f64().max(0.1)) as u64}),false)?;last=Instant::now();}
                } Err(anyhow!("Image stream ended without data"))
            }.await;
            match result {
                Ok(bytes) => return Ok(bytes),
                Err(error) if attempt == 2 => return Err(error),
                Err(_) => tokio::time::sleep(std::time::Duration::from_secs(attempt + 1)).await,
            }
        }
        unreachable!()
    }
    async fn run(&self, mut item: Value, options: Options, epoch: u64) -> Result<()> {
        let id = key(&item);
        let source = item["source"]
            .as_str()
            .context("Missing source")?
            .to_owned();
        let comic_id = item["comicId"]
            .as_str()
            .context("Missing comic id")?
            .to_owned();
        if item["comic"].is_null() {
            let mut comic = evaluate_with_retry(
                &self.engine,
                &format!(
                    "ComicSource.sources[{}].comic.loadInfo({})",
                    json!(source),
                    json!(comic_id)
                ),
            )
            .await?;
            comic["comicId"] = comic_id.clone().into();
            comic["sourceKey"] = source.clone().into();
            item["comic"] = comic;
            self.update(&id, epoch, json!({"comic":item["comic"]}), true)?;
        }
        let comic = item["comic"].clone();
        let title = comic["title"].as_str().context("Missing comic title")?;
        let comic_type = item["comicType"].as_i64().context("Missing comic type")? as i32;
        let library = crate::local::LocalLibrary::open(&format!("{}/local.db", options.data_path))?;
        let existing = library.find(&comic_id, comic_type)?;
        drop(library);
        let destination = if let Some(existing) = &existing {
            Path::new(&options.local_path).join(&existing.directory)
        } else if let Some(path) = item["path"].as_str() {
            PathBuf::from(path)
        } else {
            Path::new(&options.local_path).join(format!(
                "{}-{}",
                crate::api::local::chapter_directory_name(title.chars().take(65).collect()),
                &uuid::Uuid::new_v4().to_string()[..8]
            ))
        };
        let working = if options.saf {
            Path::new(&options.data_path)
                .join("download_work")
                .join(format!("{:x}", md5::compute(&id)))
        } else {
            destination.clone()
        };
        fs::create_dir_all(&working)?;
        self.update(
            &id,
            epoch,
            json!({"path":destination.to_string_lossy()}),
            true,
        )?;
        let cover = if item["type"] == "ArchiveDownloadTask" {
            let archive = Path::new(&options.data_path)
                .join("download_work")
                .join(format!("{:x}.zip", md5::compute(&id)));
            fs::create_dir_all(archive.parent().unwrap())?;
            let task = crate::api::download::FileDownload::start(
                item["archiveUrl"]
                    .as_str()
                    .context("Missing archive URL")?
                    .into(),
                archive.to_string_lossy().into(),
                options.threads.clamp(1, 32) as u32,
                options.http.clone(),
            )
            .await?;
            let mut finished = false;
            while let Some(progress) = task.next_status().await? {
                self.update(&id,epoch,json!({"_speed":progress.bytes_per_second,"downloadedCount":progress.downloaded_bytes,"totalCount":progress.total_bytes,"message":"Downloading archive..."}),false)?;
                finished = progress.finished;
            }
            ensure!(finished, "Archive download interrupted");
            self.update(
                &id,
                epoch,
                json!({"message":"Extracting archive..."}),
                false,
            )?;
            crate::archives::extract(&archive, &working)?;
            let _ = fs::remove_file(archive);
            let mut files = fs::read_dir(&working)?
                .filter_map(|e| e.ok())
                .filter(|e| e.path().is_file() && is_image(&e.path()))
                .map(|e| e.file_name().to_string_lossy().into_owned())
                .collect::<Vec<_>>();
            files.sort();
            files
                .iter()
                .find(|p| p.starts_with("cover."))
                .or(files.first())
                .context("Archive contains no images")?
                .clone()
        } else {
            let cover = fs::read_dir(&working)?
                .filter_map(|v| v.ok())
                .find(|v| v.file_name().to_string_lossy().starts_with("cover."));
            let cover = if let Some(cover) = cover {
                cover.file_name().to_string_lossy().into_owned()
            } else {
                let bytes = self
                    .image(
                        &options,
                        &source,
                        &comic_id,
                        "",
                        comic["cover"].as_str().context("Missing cover")?,
                        true,
                        &id,
                        epoch,
                    )
                    .await?;
                let name = format!("cover.{}", extension(&bytes));
                write_image(&working.join(&name), &bytes)?;
                name
            };
            self.update(
                &id,
                epoch,
                json!({"cover":format!("file://{}",working.join(&cover).display())}),
                true,
            )?;
            let chapter_ids = if comic["chapters"].is_null() {
                vec![String::new()]
            } else {
                crate::local::chapter_ids(&comic["chapters"])
            };
            let selected = item["chapters"].as_array();
            let mut images = item["images"].as_object().cloned().unwrap_or_default();
            for chapter in chapter_ids
                .iter()
                .filter(|c| selected.is_none_or(|s| s.contains(&json!(c))))
            {
                if !images.contains_key(chapter) {
                    self.update(
                        &id,
                        epoch,
                        json!({"message":"Fetching image list..."}),
                        false,
                    )?;
                    let pages = evaluate_with_retry(
                        &self.engine,
                        &format!(
                            "ComicSource.sources[{}].comic.loadEp({},{})",
                            json!(source),
                            json!(comic_id),
                            if chapter.is_empty() {
                                Value::Null
                            } else {
                                json!(chapter)
                            }
                        ),
                    )
                    .await?;
                    ensure!(pages["images"].is_array(), "Invalid image list");
                    images.insert(chapter.clone(), pages["images"].clone());
                    self.update(&id, epoch, json!({"images":images}), true)?;
                }
            }
            let total: usize = images
                .values()
                .filter_map(Value::as_array)
                .map(Vec::len)
                .sum();
            let completed = Arc::new(std::sync::atomic::AtomicUsize::new(0));
            self.update(
                &id,
                epoch,
                json!({"totalCount":total,"downloadedCount":0}),
                true,
            )?;
            for (chapter, pages) in &images {
                let directory = if comic["chapters"].is_null() {
                    working.clone()
                } else {
                    working.join(crate::api::local::chapter_directory_name(chapter.clone()))
                };
                fs::create_dir_all(&directory)?;
                let pages = pages.as_array().context("Invalid saved image list")?;
                stream::iter(pages.clone().into_iter().enumerate().map(|(index, image)| {
                    let directory = &directory;
                    let completed = &completed;
                    let id = &id;
                    let source = &source;
                    let comic_id = &comic_id;
                    let options = &options;
                    async move {
                        let exists = fs::read_dir(directory)?.filter_map(|e| e.ok()).any(|e| {
                            e.path().is_file()
                                && e.path()
                                    .file_stem()
                                    .is_some_and(|v| v == index.to_string().as_str())
                                && is_image(&e.path())
                        });
                        if !exists {
                            let bytes = self
                                .image(
                                    options,
                                    source,
                                    comic_id,
                                    chapter,
                                    image.as_str().context("Invalid image key")?,
                                    false,
                                    id,
                                    epoch,
                                )
                                .await?;
                            write_image(
                                &directory.join(format!("{index}.{}", extension(&bytes))),
                                &bytes,
                            )?;
                        }
                        let count =
                            completed.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1;
                        self.update(
                            id,
                            epoch,
                            json!({"downloadedCount":count,"message":format!("{count}/{total}")}),
                            true,
                        )
                    }
                }))
                .buffer_unordered(options.threads.clamp(1, 32))
                .try_collect::<Vec<_>>()
                .await?;
            }
            cover
        };
        if options.saf {
            self.update(&id, epoch, json!({"message":"Saving files..."}), false)?;
            self.platform_copy(&working.to_string_lossy(), &destination.to_string_lossy())
                .await?;
        }
        let tags = comic["tags"]
            .as_object()
            .map(|tags| {
                tags.iter()
                    .flat_map(|(k, v)| {
                        v.as_array()
                            .into_iter()
                            .flatten()
                            .filter_map(move |s| s.as_str().map(|s| format!("{k}:{s}")))
                    })
                    .collect()
            })
            .unwrap_or_default();
        let downloaded_chapters = item["chapters"]
            .as_array()
            .map(|v| {
                v.iter()
                    .filter_map(|v| v.as_str().map(str::to_owned))
                    .collect()
            })
            .unwrap_or_else(|| crate::local::chapter_ids(&comic["chapters"]));
        let record = crate::local::LocalComic {
            id: comic_id,
            title: title.into(),
            subtitle: comic["subTitle"]
                .as_str()
                .or(comic["subtitle"].as_str())
                .unwrap_or_default()
                .into(),
            tags,
            directory: destination
                .file_name()
                .context("Invalid comic directory")?
                .to_string_lossy()
                .into(),
            chapters: comic["chapters"].clone(),
            cover,
            comic_type,
            downloaded_chapters,
            created_at: crate::cache::now_ms(),
        };
        let state = self.state.lock().map_err(|_| anyhow!("Queue poisoned"))?;
        ensure!(state.epoch == epoch, "Download cancelled");
        crate::local::LocalLibrary::open(&format!("{}/local.db", options.data_path))?
            .save(record)?;
        drop(state);
        if options.saf {
            let _ = fs::remove_dir_all(working);
        }
        Ok(())
    }
}
async fn evaluate_with_retry(engine: &Engine, code: &str) -> Result<Value> {
    for attempt in 0..3 {
        match crate::image_tasks::evaluate(engine, code).await {
            Ok(value) => return Ok(value),
            Err(error) if attempt == 2 => return Err(error),
            Err(_) => tokio::time::sleep(std::time::Duration::from_secs(attempt + 1)).await,
        }
    }
    unreachable!()
}
fn extension(bytes: &[u8]) -> &'static str {
    match image::guess_format(bytes) {
        Ok(image::ImageFormat::Jpeg) => "jpg",
        Ok(image::ImageFormat::Gif) => "gif",
        Ok(image::ImageFormat::WebP) => "webp",
        Ok(image::ImageFormat::Bmp) => "bmp",
        _ => "png",
    }
}
pub(crate) fn is_image(path: &Path) -> bool {
    path.extension().and_then(|v| v.to_str()).is_some_and(|v| {
        ["jpg", "jpeg", "png", "gif", "webp", "bmp", "avif"]
            .contains(&v.to_ascii_lowercase().as_str())
    })
}
fn write_image(path: &Path, bytes: &[u8]) -> Result<()> {
    ensure!(!bytes.is_empty(), "Empty image");
    let mut file = tempfile::NamedTempFile::new_in(path.parent().context("Missing directory")?)?;
    file.write_all(bytes)?;
    file.as_file().sync_all()?;
    file.persist(path)?;
    Ok(())
}
