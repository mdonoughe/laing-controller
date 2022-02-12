use anyhow::{Context, Result};
use serde::Deserialize;
use std::fs::File;

#[derive(Deserialize)]
pub struct Settings {
    pub serial_port: String,
    pub id: String,
    pub name: String,
    #[serde(default = "default_prefix")]
    pub prefix: String,
    #[serde(default = "default_hass_prefix")]
    pub hass_prefix: String,
    pub mqtt: MqttSettings,
}

#[derive(Deserialize)]
pub struct MqttSettings {
    pub host: String,
    #[serde(default)]
    pub port: Option<u16>,
    #[serde(default)]
    pub transport: MqttTransport,
    #[serde(default)]
    pub credentials: Option<MqttCredential>,
}

fn default_prefix() -> String {
    "desk".to_string()
}

fn default_hass_prefix() -> String {
    "homeassistant".into()
}

#[derive(Deserialize)]
pub enum MqttTransport {
    Tcp,
    Tls,
}

impl Default for MqttTransport {
    fn default() -> Self {
        MqttTransport::Tls
    }
}

#[derive(Deserialize)]
pub struct MqttCredential {
    pub username: String,
    pub password: String,
}

pub fn load_settings() -> Result<Settings> {
    let mut path = ::std::env::current_exe().context("Could not find installation directory")?;
    path.pop();
    path.push("laing-controller.yaml");
    let file = File::open(path).context("Failed to open settings")?;
    serde_yaml::from_reader(file).context("Failed to load settings")
}
