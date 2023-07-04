use crate::errors::*;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Lockfile {
    pub container: ContainerLockfile,
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
pub struct ContainerLockfile {
    pub image: String,
}
