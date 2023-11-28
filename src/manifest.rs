use crate::errors::*;
use indexmap::IndexSet;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Manifest {
    pub container: ContainerManifest,
    pub packages: Option<PackagesManifest>,
}

impl Manifest {
    pub fn deserialize(buf: &str) -> Result<Self> {
        let manifest = toml::from_str(buf).context("Failed to load manifest from toml")?;
        Ok(manifest)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ContainerManifest {
    pub image: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PackagesManifest {
    pub system: String,
    #[serde(default)]
    pub dependencies: IndexSet<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_manifest() -> Result<()> {
        let manifest = Manifest::deserialize(
            r#"[container]
image = "docker.io/library/rust:1-alpine"
"#,
        )?;

        assert_eq!(
            manifest,
            Manifest {
                container: ContainerManifest {
                    image: "docker.io/library/rust:1-alpine".to_string(),
                },
                packages: None
            }
        );

        Ok(())
    }
}
