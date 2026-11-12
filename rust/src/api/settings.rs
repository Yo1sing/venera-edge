use anyhow::Result;

#[flutter_rust_bridge::frb(sync)]
pub fn mutate_settings(
    settings_json: String,
    scope: String,
    owner: Option<String>,
    key: Option<String>,
    value_json: Option<String>,
) -> Result<String> {
    use anyhow::Context;
    use serde_json::{json, Value};
    let mut settings: Value = serde_json::from_str(&settings_json)?;
    let value = value_json.map(|v| serde_json::from_str(&v)).transpose()?;
    let root = settings.as_object_mut().context("Invalid settings")?;
    if scope == "global" {
        let key = key.context("Missing setting key")?;
        if let Some(value) = value {
            root.insert(key, value);
        } else {
            root.remove(&key);
        }
    } else {
        let (group, owner) = match scope.as_str() {
            "comic" => ("comicSpecificSettings", owner.context("Missing comic key")?),
            "device" => {
                let device = root
                    .get("deviceId")
                    .and_then(Value::as_str)
                    .filter(|s| !s.is_empty())
                    .map(str::to_owned)
                    .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
                root.insert("deviceId".into(), device.clone().into());
                ("deviceSpecificSettings", device)
            }
            _ => anyhow::bail!("Invalid setting scope"),
        };
        let group = root
            .entry(group)
            .or_insert_with(|| json!({}))
            .as_object_mut()
            .context("Invalid reader settings")?;
        if let Some(key) = key {
            let record = group
                .entry(owner)
                .or_insert_with(|| json!({}))
                .as_object_mut()
                .context("Invalid reader settings record")?;
            if let Some(value) = value {
                record.insert(key, value);
            } else {
                record.remove(&key);
            }
        } else {
            group.remove(&owner);
        }
    }
    Ok(settings.to_string())
}

#[flutter_rust_bridge::frb(sync)]
pub fn default_settings_json() -> String {
    include_str!("../../settings_defaults.json").to_owned()
}

pub fn load_app_data(directory: String) -> Result<String> {
    Ok(serde_json::to_string(&crate::settings::load(&directory)?)?)
}

pub fn save_app_data(directory: String, data_json: String) -> Result<()> {
    crate::settings::save(&directory, serde_json::from_str(&data_json)?)
}

pub fn save_implicit_data(directory: String, data_json: String) -> Result<()> {
    crate::settings::save_implicit(&directory, serde_json::from_str(&data_json)?)
}

#[flutter_rust_bridge::frb(sync)]
pub fn merge_synced_data(current_json: String, incoming_json: String) -> Result<String> {
    Ok(serde_json::to_string(&crate::settings::merge(
        serde_json::from_str(&current_json)?,
        serde_json::from_str(&incoming_json)?,
    )?)?)
}

#[flutter_rust_bridge::frb(sync)]
pub fn update_search_history(
    history: Vec<String>,
    keyword: Option<String>,
    remove: bool,
) -> Vec<String> {
    crate::settings::search_history(history, keyword, remove)
}

#[flutter_rust_bridge::frb(sync)]
pub fn reader_setting(
    settings_json: String,
    comic_key: Option<String>,
    key: String,
) -> Result<String> {
    let settings = serde_json::from_str(&settings_json)?;
    Ok(serde_json::to_string(crate::settings::reader_setting(
        &settings,
        comic_key.as_deref(),
        &key,
    ))?)
}
