use crate::api::{http::HttpCancellation, media::ImageProgress};
use anyhow::{Context, Result};
use serde_json::json;
use std::sync::{Arc, Mutex, OnceLock, Weak};

pub struct Request {
    pub cancellation: Arc<HttpCancellation>,
    pub sender: tokio::sync::mpsc::Sender<ImageProgress>,
}
impl Request {
    pub fn progress(&self, current: u64, total: Option<u64>) {
        let _ = self.sender.try_send(ImageProgress {
            current,
            total,
            bytes: None,
            error: None,
        });
    }
}
fn registry() -> &'static Mutex<std::collections::HashMap<String, Weak<Request>>> {
    static REGISTRY: OnceLock<Mutex<std::collections::HashMap<String, Weak<Request>>>> =
        OnceLock::new();
    REGISTRY.get_or_init(Default::default)
}
pub fn request(id: &str) -> Option<Arc<Request>> {
    registry().lock().ok()?.get(id)?.upgrade()
}

pub struct Task {
    pub state: Arc<Request>,
    engine: Arc<crate::script::Engine>,
    id: String,
    finished: Arc<std::sync::atomic::AtomicBool>,
    pub receiver: tokio::sync::Mutex<tokio::sync::mpsc::Receiver<ImageProgress>>,
}
impl Drop for Task {
    fn drop(&mut self) {
        self.cancel();
    }
}
pub async fn evaluate(engine: &crate::script::Engine, code: &str) -> Result<serde_json::Value> {
    let mut wire: serde_json::Value = serde_json::from_str(&engine.evaluate(code)?)?;
    if wire[0] == "promise" {
        wire = serde_json::from_str(
            &engine
                .promise(wire[1].as_u64().context("Invalid promise")?)
                .await?,
        )?;
    }
    crate::script::unpack(&wire)
}
impl Task {
    pub fn cancel(&self) {
        self.state.cancellation.cancel();
        if !self.finished.load(std::sync::atomic::Ordering::Relaxed) {
            let _ = self
                .engine
                .evaluate(&format!("__images.cancel({})", json!(self.id)));
        }
    }
    #[allow(clippy::too_many_arguments)]
    pub fn start(
        engine: Arc<crate::script::Engine>,
        data_path: String,
        cache_path: String,
        image: String,
        source: Option<String>,
        comic: String,
        chapter: String,
        thumbnail: bool,
    ) -> Result<Self> {
        let (sender, receiver) = tokio::sync::mpsc::channel(8);
        let state = Arc::new(Request {
            cancellation: Arc::new(HttpCancellation::new()),
            sender,
        });
        let worker = state.clone();
        let id = uuid::Uuid::new_v4().to_string();
        let task_engine = engine.clone();
        let task_id = id.clone();
        let finished = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let worker_finished = finished.clone();
        registry()
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(id.clone(), Arc::downgrade(&state));
        crate::script::executor().spawn(async move {
            let source_text=source.as_deref().unwrap_or("null");
            let key=if thumbnail {format!("{image}@{source_text}{}",if comic.is_empty(){String::new()}else{format!("@{comic}")})} else {format!("{image}@{source_text}@{comic}@{chapter}")};
            let work=async {
                let _lock=crate::http_policy::serial(&format!("image:{key}")).await;
                let cache=crate::cache::shared(&data_path,&cache_path)?;
                let hit=cache.lock().map_err(|_|anyhow::anyhow!("Cache poisoned"))?.find(&key)?;
                if let Some(path)=hit{return Ok::<_,anyhow::Error>(std::fs::read(path)?);}
                let code=format!("__images.fetch({},{},{},{},{},{})",json!(image),json!(source),json!(comic),json!(chapter),thumbnail,json!(id));
                let bytes=crate::script_crypto::bytes(&evaluate(&engine,&code).await?)?;
                cache.lock().map_err(|_|anyhow::anyhow!("Cache poisoned"))?.write(&key,&bytes,7*24*60*60*1000)?;
                Ok(bytes)
            };
            let result=tokio::select!{ _=worker.cancellation.cancelled()=>Err(anyhow::anyhow!("Image request cancelled")), result=work=>result };
            let progress=match result {
                Ok(bytes)=>ImageProgress{current:bytes.len() as u64,total:Some(bytes.len() as u64),bytes:Some(bytes),error:None},
                Err(error)=>ImageProgress{current:0,total:None,bytes:None,error:Some(format!("{error:#}"))},
            };
            worker_finished.store(true,std::sync::atomic::Ordering::Relaxed);
            let _=worker.sender.send(progress).await;
            registry().lock().unwrap_or_else(|e|e.into_inner()).remove(&id);
        });
        Ok(Self {
            state,
            engine: task_engine,
            id: task_id,
            finished,
            receiver: tokio::sync::Mutex::new(receiver),
        })
    }
}
