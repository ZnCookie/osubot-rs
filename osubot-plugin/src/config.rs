use serde::Deserialize;

#[derive(Debug, Deserialize, Clone, Default)]
pub struct PluginConfig {
    #[serde(default = "default_plugin_dir")]
    pub dir: String,
    #[serde(default)]
    pub instances: Vec<PluginInstanceConfig>,
}

fn default_plugin_dir() -> String {
    "./plugins".to_string()
}

#[derive(Debug, Deserialize, Clone, PartialEq)]
pub struct PluginInstanceConfig {
    pub name: String,
    pub path: String,
    #[serde(default = "default_enabled")]
    pub enabled: bool,
    #[serde(default)]
    pub priority: u32,
    #[serde(default)]
    pub config: Option<serde_json::Value>,
}

fn default_enabled() -> bool {
    false
}
