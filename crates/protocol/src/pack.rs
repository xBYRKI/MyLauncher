//! Modpack-Manifest – vom Launcher gelesen, vom Server und von `packtool` geschrieben.

use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LoaderSpec {
    #[serde(rename = "type", default)]
    pub kind: String,
    #[serde(default)]
    pub version: String,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Memory {
    #[serde(default)]
    pub min: Option<u32>,
    pub max: u32,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PackFile {
    pub path: String,
    pub url: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sha1: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub size: Option<u64>,
    /// `false` = nur anlegen, wenn die Datei fehlt (z. B. options.txt)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub overwrite: Option<bool>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Manifest {
    #[serde(default = "one")]
    pub format_version: u32,
    pub id: String,
    pub name: String,
    pub version: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub description: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub instance_id: String,
    pub minecraft: String,
    #[serde(default)]
    pub loader: LoaderSpec,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub server: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub memory: Option<Memory>,
    #[serde(default)]
    pub files: Vec<PackFile>,
}

fn one() -> u32 {
    1
}

/// Bekannte Loader und ihre Prism-Komponente.
pub fn loader_uid(kind: &str) -> Option<&'static str> {
    match kind {
        "fabric" => Some("net.fabricmc.fabric-loader"),
        "quilt" => Some("org.quiltmc.quilt-loader"),
        "forge" => Some("net.minecraftforge"),
        "neoforge" => Some("net.neoforged"),
        _ => None,
    }
}
