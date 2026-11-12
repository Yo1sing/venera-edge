use anyhow::Result;

pub struct ImageProgress {
    pub current: u64,
    pub total: Option<u64>,
    pub bytes: Option<Vec<u8>>,
    pub error: Option<String>,
}
#[flutter_rust_bridge::frb(opaque)]
pub struct ImageTransfer {
    pub(crate) inner: crate::image_tasks::Task,
}
impl ImageTransfer {
    #[allow(clippy::too_many_arguments)]
    pub fn start(
        runtime: &crate::api::script::ScriptRuntime,
        data_path: String,
        cache_path: String,
        image: String,
        source: Option<String>,
        comic: String,
        chapter: String,
        thumbnail: bool,
    ) -> Result<Self> {
        Ok(Self {
            inner: crate::image_tasks::Task::start(
                runtime.inner.clone(),
                data_path,
                cache_path,
                image,
                source,
                comic,
                chapter,
                thumbnail,
            )?,
        })
    }
    pub async fn next(&self) -> Option<ImageProgress> {
        self.inner.receiver.lock().await.recv().await
    }
    #[flutter_rust_bridge::frb(sync)]
    pub fn cancel(&self) {
        self.inner.cancel();
    }
}

pub fn export_book(book_json: String, output: String, format: String) -> Result<()> {
    let book = serde_json::from_str(&book_json)?;
    match format.as_str() {
        "pdf" => crate::books::pdf(&book, output.as_ref()),
        "epub" => crate::books::epub(&book, output.as_ref()),
        "cbz" => crate::books::cbz(&book, output.as_ref()),
        _ => anyhow::bail!("Unsupported book format"),
    }
}

pub async fn modify_image(data: Vec<u8>, script: String) -> Result<Vec<u8>> {
    static LIMIT: tokio::sync::Semaphore = tokio::sync::Semaphore::const_new(4);
    let _permit = LIMIT.acquire().await?;
    tokio::task::spawn_blocking(move || crate::imaging::modify(data, script)).await?
}
