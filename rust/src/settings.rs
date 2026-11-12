use anyhow::{ensure, Context, Result};
use serde_json::{json, Value};
use std::{fs, io::Write, path::Path, sync::Mutex};

// Serializes writes within this process, including callers on FRB's worker pool.
static WRITES: Mutex<()> = Mutex::new(());
const DEVICE_ONLY: &[&str] = &[
    "proxy",
    "authorizationRequired",
    "customImageProcessing",
    "webdav",
    "disableSyncFields",
    "deviceId",
];

fn atomic_json(path: &Path, data: &Value) -> Result<()> {
    let parent = path.parent().context("settings path has no parent")?;
    fs::create_dir_all(parent)?;
    let mut file = tempfile::NamedTempFile::new_in(parent)?;
    serde_json::to_writer(file.as_file_mut(), data)?;
    file.as_file_mut().flush()?;
    file.as_file_mut().sync_all()?;
    file.persist(path)?;
    Ok(())
}

fn read_optional(path: &Path) -> Result<Option<Value>> {
    match fs::read(path) {
        Ok(bytes) => {
            Ok(Some(serde_json::from_slice(&bytes).with_context(|| {
                format!("Invalid JSON in {}", path.display())
            })?))
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e.into()),
    }
}

pub fn load(directory: &str) -> Result<Value> {
    let mut settings: Value = serde_json::from_str(include_str!("../settings_defaults.json"))?;
    let data =
        read_optional(&Path::new(directory).join("appdata.json"))?.unwrap_or_else(|| json!({}));
    if let Some(saved) = data["settings"].as_object() {
        for (key, value) in saved {
            if !value.is_null() {
                settings[key] = value.clone();
            }
        }
    }
    if settings["deviceId"].as_str().unwrap_or_default().is_empty() {
        settings["deviceId"] = uuid::Uuid::new_v4().to_string().into();
    }
    let history = data["searchHistory"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    let implicit = read_optional(&Path::new(directory).join("implicitData.json"))?
        .unwrap_or_else(|| json!({}));
    Ok(json!({ "settings":settings, "searchHistory": history, "implicitData": implicit }))
}

fn excluded(settings: &Value) -> Vec<String> {
    settings["disableSyncFields"]
        .as_str()
        .unwrap_or_default()
        .split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
        .collect()
}

pub fn save(directory: &str, mut data: Value) -> Result<()> {
    ensure!(
        data["settings"].is_object() && data["searchHistory"].is_array(),
        "Invalid appdata snapshot"
    );
    let _guard = WRITES
        .lock()
        .map_err(|_| anyhow::anyhow!("settings lock poisoned"))?;
    // A UI snapshot taken during a WebDAV upload must not rewind its revision.
    if let Some(previous) = read_optional(&Path::new(directory).join("appdata.json"))? {
        if let Some(version) = previous["settings"]["dataVersion"].as_u64() {
            if version > data["settings"]["dataVersion"].as_u64().unwrap_or(0) {
                data["settings"]["dataVersion"] = version.into();
            }
        }
    }
    let mut sync = data.clone();
    for key in excluded(&data["settings"]) {
        sync["settings"].as_object_mut().unwrap().remove(&key);
    }
    // syncdata always exists, even with no custom exclusions.
    atomic_json(&Path::new(directory).join("syncdata.json"), &sync)?;
    atomic_json(&Path::new(directory).join("appdata.json"), &data)?;
    Ok(())
}

pub fn save_implicit(directory: &str, data: Value) -> Result<()> {
    ensure!(data.is_object(), "Invalid implicit data snapshot");
    let _guard = WRITES
        .lock()
        .map_err(|_| anyhow::anyhow!("settings lock poisoned"))?;
    atomic_json(&Path::new(directory).join("implicitData.json"), &data)
}

pub fn merge(mut current: Value, incoming: Value) -> Result<Value> {
    let excluded = excluded(&current["settings"]);
    let settings = current["settings"]
        .as_object_mut()
        .context("Invalid local settings")?;
    if let Some(incoming) = incoming["settings"].as_object() {
        for (key, value) in incoming {
            if !DEVICE_ONLY.contains(&key.as_str()) && !excluded.contains(key) {
                settings.insert(key.clone(), value.clone());
            }
        }
    }
    current["searchHistory"] = incoming["searchHistory"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .into();
    Ok(current)
}

pub fn search_history(
    mut history: Vec<String>,
    keyword: Option<String>,
    remove: bool,
) -> Vec<String> {
    let Some(keyword) = keyword else {
        return Vec::new();
    };
    history.retain(|item| item != &keyword);
    if !remove {
        history.insert(0, keyword);
        history.truncate(50);
    }
    history
}

pub fn reader_setting<'a>(settings: &'a Value, comic_key: Option<&str>, key: &str) -> &'a Value {
    if let Some(comic) = comic_key {
        let values = &settings["comicSpecificSettings"][comic];
        if values["enabled"] == true && !values[key].is_null() {
            return &values[key];
        }
    }
    if let Some(device) = settings["deviceId"].as_str().filter(|id| !id.is_empty()) {
        let values = &settings["deviceSpecificSettings"][device];
        if values["enabled"] == true && !values[key].is_null() {
            return &values[key];
        }
    }
    &settings[key]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn persisted_settings_and_sync_exclusions() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let directory = temp.path().to_str().unwrap();
        let loaded = load(directory)?;
        assert!(!loaded["settings"]["deviceId"].as_str().unwrap().is_empty());
        let local = json!({"settings":{"proxy":"local","deviceId":"device","theme_mode":"dark","disableSyncFields":"language, color","language":"zh-CN"},"searchHistory":["one"]});
        save(directory, local.clone())?;
        save(directory, local.clone())?;
        let sync = read_optional(&temp.path().join("syncdata.json"))?.unwrap();
        assert!(sync["settings"].get("language").is_none());
        let result = merge(
            local,
            json!({"settings":{"proxy":"remote","deviceId":"other","language":"en-US","theme_mode":"light"},"searchHistory":["two"]}),
        )?;
        assert_eq!(result["settings"]["proxy"], "local");
        assert_eq!(result["settings"]["language"], "zh-CN");
        assert_eq!(result["settings"]["theme_mode"], "light");
        assert_eq!(load(directory)?["settings"]["deviceId"], "device");
        Ok(())
    }

    #[test]
    fn reader_precedence_and_search_limit() {
        let settings = json!({"x":1,"deviceId":"d","deviceSpecificSettings":{"d":{"enabled":true,"x":2}},"comicSpecificSettings":{"c@s":{"enabled":true,"x":3}}});
        assert_eq!(reader_setting(&settings, Some("c@s"), "x"), 3);
        assert_eq!(reader_setting(&settings, Some("other"), "x"), 2);
        let history = search_history(
            (0..60).map(|i| i.to_string()).collect(),
            Some("10".into()),
            false,
        );
        assert_eq!(history.len(), 50);
        assert_eq!(history[0], "10");
        assert_eq!(history.iter().filter(|s| *s == "10").count(), 1);
    }
}
