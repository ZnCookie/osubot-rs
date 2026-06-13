use serde::Deserialize;

#[derive(Debug, Deserialize, Clone)]
pub struct PluginConfig {
    #[serde(default = "default_plugin_dir")]
    pub dir: String,
    #[serde(default)]
    pub instances: Vec<PluginInstanceConfig>,
    #[serde(default = "default_lost_instances_threshold")]
    pub lost_instances_threshold: u32,
    #[serde(default = "default_reload_failures_threshold")]
    pub reload_failures_threshold: u32,
}

impl Default for PluginConfig {
    fn default() -> Self {
        Self {
            dir: default_plugin_dir(),
            instances: Vec::new(),
            lost_instances_threshold: default_lost_instances_threshold(),
            reload_failures_threshold: default_reload_failures_threshold(),
        }
    }
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

fn default_lost_instances_threshold() -> u32 {
    5
}

fn default_reload_failures_threshold() -> u32 {
    3
}
