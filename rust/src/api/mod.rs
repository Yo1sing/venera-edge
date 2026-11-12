pub mod cache;
pub mod cookies;
pub mod history;
pub mod http;
pub mod settings;
pub mod tags;
pub mod text;

#[flutter_rust_bridge::frb(init)]
pub fn init_app() {
    flutter_rust_bridge::setup_default_user_utils();
}

pub mod download;
pub mod favorites;
pub mod local;
