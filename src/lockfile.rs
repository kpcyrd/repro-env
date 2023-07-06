use crate::errors::*;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Lockfile {
    pub container: ContainerLock,
    #[serde(default, rename = "package", skip_serializing_if = "Vec::is_empty")]
    pub packages: Vec<PackageLock>,
}

impl Lockfile {
    pub fn deserialize(buf: &str) -> Result<Self> {
        let lockfile = toml::from_str(buf)?;
        Ok(lockfile)
    }

    pub fn serialize(&self) -> Result<String> {
        let toml = toml::to_string_pretty(self)?;
        Ok(toml)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ContainerLock {
    pub image: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PackageLock {
    pub name: String,
    pub version: String,
    pub system: String,
    pub url: String,
    pub sha256: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub signature: Option<String>,
}
