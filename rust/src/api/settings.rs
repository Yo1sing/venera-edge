use anyhow::Result;

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
