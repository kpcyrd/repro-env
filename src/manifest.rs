use crate::errors::*;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Manifest {
    pub container: ContainerManifest,
}

impl Manifest {
    pub fn deserialize(buf: &str) -> Result<Self> {
        let manifest = toml::from_str(buf)?;
        Ok(manifest)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ContainerManifest {
    pub image: String,
}
