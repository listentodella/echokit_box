pub mod app;
pub mod audio;
pub mod bt;
pub mod hal;
pub mod network;
pub mod protocol;
pub mod ui;
pub mod ws;

#[derive(Debug, Clone)]
pub struct Setting {
    pub ssid: String,
    pub pass: String,
    pub server_url: String,
    pub background_gif: (Vec<u8>, bool), // (data, ended)
}
