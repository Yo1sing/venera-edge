use anyhow::{ensure, Context, Result};
use serde::Deserialize;
use std::path::Path;
use url::Url;

#[derive(Deserialize)]
pub struct Options {
    pub directory: String,
    pub cache: String,
    pub http: crate::api::http::HttpOptions,
}
struct Dav {
    client: reqwest::Client,
    base: Url,
    user: String,
    password: String,
}
impl Dav {
    fn new(
        settings: &serde_json::Value,
        http: &crate::api::http::HttpOptions,
    ) -> Result<Option<Self>> {
        let config = settings["webdav"]
            .as_array()
            .context("Invalid WebDAV configuration")?;
        if config.is_empty() {
            return Ok(None);
        }
        ensure!(
            config.len() == 3 && config.iter().all(|v| v.is_string()),
            "Invalid WebDAV configuration"
        );
        let mut base = Url::parse(config[0].as_str().unwrap())?;
        ensure!(
            matches!(base.scheme(), "http" | "https"),
            "Invalid WebDAV URL"
        );
        if !base.path().ends_with('/') {
            base.set_path(&format!("{}/", base.path()));
        }
        Ok(Some(Self {
            client: crate::api::http::client(http)?,
            base,
            user: config[1].as_str().unwrap().into(),
            password: config[2].as_str().unwrap().into(),
        }))
    }
    fn request(&self, method: &str, name: &str) -> Result<reqwest::RequestBuilder> {
        let mut url = self.base.clone();
        if !name.is_empty() {
            url.path_segments_mut()
                .map_err(|_| anyhow::anyhow!("Invalid WebDAV URL"))?
                .pop_if_empty()
                .push(name);
        }
        Ok(self
            .client
            .request(method.parse()?, url)
            .basic_auth(&self.user, Some(&self.password)))
    }
    async fn list(&self) -> Result<Vec<(String, u64, u64)>> {
        let xml=self.request("PROPFIND", "")?.header("Depth","1").header("Content-Type","application/xml")
            .body(r#"<?xml version="1.0"?><d:propfind xmlns:d="DAV:"><d:prop><d:resourcetype/></d:prop></d:propfind>"#)
            .send().await?.error_for_status()?.text().await?;
        let doc = roxmltree::Document::parse(&xml)?;
        let mut files = Vec::new();
        for response in doc
            .descendants()
            .filter(|n| n.tag_name().name() == "response")
        {
            if response
                .descendants()
                .any(|n| n.tag_name().name() == "collection")
            {
                continue;
            }
            if let Some(href) = response
                .descendants()
                .find(|n| n.tag_name().name() == "href")
                .and_then(|n| n.text())
            {
                let url = self.base.join(href)?;
                ensure!(
                    url.origin() == self.base.origin(),
                    "WebDAV response points to another origin"
                );
                let Some(name) = url.path_segments().and_then(|mut p| p.next_back()) else {
                    continue;
                };
                if let Some((day, version)) = backup_name(name) {
                    files.push((name.into(), day, version));
                }
            }
        }
        files.sort_by_key(|v| (v.1, v.2));
        files.dedup_by(|a, b| a.0 == b.0);
        Ok(files)
    }
}
fn backup_name(name: &str) -> Option<(u64, u64)> {
    let (day, version) = name.strip_suffix(".venera")?.split_once('-')?;
    Some((day.parse().ok()?, version.parse().ok()?))
}
static TRANSFER: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());
pub async fn upload(options: Options) -> Result<String> {
    let _guard = TRANSFER.lock().await;
    let mut data = crate::settings::load(&options.directory)?;
    let Some(dav) = Dav::new(&data["settings"], &options.http)? else {
        return Ok(data.to_string());
    };
    let files = dav.list().await?;
    let version = data["settings"]["dataVersion"]
        .as_u64()
        .unwrap_or(0)
        .checked_add(1)
        .context("Data version overflow")?;
    data["settings"]["dataVersion"] = version.into();
    crate::settings::save(&options.directory, data.clone())?;
    let output = crate::backup::export(&options.directory, &options.cache, true)?;
    let day = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)?
        .as_secs()
        / 86400;
    let name = format!("{day}-{version}.venera");
    let result = async {
        // Existing backups remain intact if upload fails.
        dav.request("PUT", &name)?
            .header("Content-Type", "application/zip")
            .body(std::fs::read(&output)?)
            .send()
            .await?
            .error_for_status()?;
        let mut remaining = files
            .into_iter()
            .filter(|f| f.0 != name)
            .collect::<Vec<_>>();
        let mut obsolete = Vec::new();
        remaining.retain(|f| {
            if f.1 == day {
                obsolete.push(f.0.clone());
                false
            } else {
                true
            }
        });
        while remaining.len() >= 10 {
            obsolete.push(remaining.remove(0).0);
        }
        for old in obsolete {
            dav.request("DELETE", &old)?
                .send()
                .await?
                .error_for_status()?;
        }
        Ok::<_, anyhow::Error>(data.to_string())
    }
    .await;
    let _ = std::fs::remove_file(output);
    result
}
pub async fn download(options: Options) -> Result<Option<String>> {
    let _guard = TRANSFER.lock().await;
    let data = crate::settings::load(&options.directory)?;
    let Some(dav) = Dav::new(&data["settings"], &options.http)? else {
        return Ok(None);
    };
    let files = dav.list().await?;
    let (name, _, version) = files.last().context("No data file found")?;
    if *version <= data["settings"]["dataVersion"].as_u64().unwrap_or(0) {
        return Ok(None);
    }
    let mut response = dav.request("GET", name)?.send().await?.error_for_status()?;
    std::fs::create_dir_all(&options.cache)?;
    let mut output = tempfile::NamedTempFile::new_in(&options.cache)?;
    while let Some(bytes) = response.chunk().await? {
        use std::io::Write;
        output.write_all(&bytes)?;
    }
    output.as_file().sync_all()?;
    let path = Path::new(&options.cache).join(format!("{}.venera", uuid::Uuid::new_v4()));
    output.persist(&path)?;
    Ok(Some(path.to_string_lossy().into_owned()))
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn numeric_versions_and_invalid_names() {
        assert_eq!(backup_name("20000-100.venera"), Some((20000, 100)));
        assert_eq!(backup_name("../x.venera"), None);
        assert_eq!(backup_name("x-y.venera"), None);
    }
}
