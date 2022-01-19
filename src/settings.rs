use anyhow::{Context, Result};
use serde::Deserialize;
use tokio::{fs::File, io::AsyncReadExt};

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

pub async fn load_settings() -> Result<Settings> {
    let mut path = ::std::env::current_exe().context("Could not find installation directory")?;
    path.pop();
    path.push("laing-controller.yaml");
    let mut file = File::open(path).await.context("Failed to open settings")?;
    let mut settings = String::new();
    file.read_to_string(&mut settings)
        .await
        .context("Failed to read settings")?;
    serde_yaml::from_str(&settings).context("Failed to parse settings")
}
