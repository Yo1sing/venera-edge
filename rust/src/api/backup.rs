use anyhow::Result;

pub fn import_pica_data(
    input: String,
    cache: String,
    directory: String,
    source_types_json: String,
    installed: Vec<String>,
    traditional: bool,
    at_end: bool,
) -> Result<()> {
    crate::legacy_import::import(
        &input,
        &cache,
        &directory,
        serde_json::from_str(&source_types_json)?,
        installed,
        traditional,
        at_end,
    )
}

pub async fn sync_upload(options_json: String) -> Result<String> {
    crate::sync::upload(serde_json::from_str(&options_json)?).await
}
pub async fn sync_download(options_json: String) -> Result<Option<String>> {
    crate::sync::download(serde_json::from_str(&options_json)?).await
}

pub fn export_app_data(directory: String, cache: String, sync: bool) -> Result<String> {
    crate::backup::export(&directory, &cache, sync)
}

#[flutter_rust_bridge::frb(opaque)]
pub struct PreparedBackup {
    inner: crate::backup::Prepared,
}
impl PreparedBackup {
    pub fn open(
        input: String,
        cache: String,
        current_version: Option<i64>,
    ) -> Result<Option<Self>> {
        Ok(crate::backup::prepare(&input, &cache, current_version)?.map(|inner| Self { inner }))
    }
    pub fn apply(&self, directory: String) -> Result<String> {
        self.inner.apply(&directory)
    }
}
