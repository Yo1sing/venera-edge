use anyhow::Result;
#[flutter_rust_bridge::frb(opaque)]
pub struct ComicDownloadQueue {
    inner: std::sync::Arc<crate::comic_queue::Queue>,
}
impl ComicDownloadQueue {
    pub fn open(
        runtime: &crate::api::script::ScriptRuntime,
        options_json: String,
        source_types_json: String,
    ) -> Result<Self> {
        Ok(Self {
            inner: crate::comic_queue::Queue::open(
                runtime.inner.clone(),
                serde_json::from_str(&options_json)?,
                serde_json::from_str(&source_types_json)?,
            )?,
        })
    }
    pub fn configure(&self, options_json: String) -> Result<()> {
        self.inner.configure(serde_json::from_str(&options_json)?)
    }
    pub fn add(&self, task_json: String) -> Result<()> {
        self.inner.add(serde_json::from_str(&task_json)?)
    }
    pub fn command(&self, key: String, action: String) -> Result<()> {
        self.inner.command(&key, &action)
    }
    #[flutter_rust_bridge::frb(sync)]
    pub fn snapshot(&self) -> String {
        self.inner.snapshot()
    }
    pub async fn next_snapshot(&self) -> Option<String> {
        let mut receiver = self.inner.snapshot_receiver.lock().await;
        receiver.changed().await.ok()?;
        {
            let value = receiver.borrow_and_update().clone();
            Some(value)
        }
    }
    pub async fn next_platform_event(&self) -> Option<String> {
        self.inner.platform_receiver.lock().await.recv().await
    }
    pub fn resolve_platform(&self, id: String, error: Option<String>) -> Result<()> {
        self.inner.resolve_platform(&id, error)
    }
}
