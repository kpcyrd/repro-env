use crate::errors::*;
use crate::lockfile::Lockfile;
use indexmap::IndexSet;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::path::Path;
use tokio::fs;

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

    pub async fn read_from_file<P: AsRef<Path>>(path: P) -> Result<Self> {
        let path = path.as_ref();
        let buf = fs::read_to_string(&path)
            .await
            .with_context(|| anyhow!("Failed to read dependency manifest: {path:?}"))?;
        let manifest = Self::deserialize(&buf)?;
        debug!("Loaded manifest from file: {manifest:?}");
        Ok(manifest)
    }

    pub fn satisfied_by(&self, lockfile: &Lockfile) -> Result<()> {
        if let Some(packages) = &self.packages {
            let mut provided = HashSet::new();
            for package in &lockfile.packages {
                provided.insert(package.name.clone());
                provided.extend(package.provides.iter().cloned());
            }

            for dependency in &packages.dependencies {
                let (name, _) = dependency.split_once('=').unwrap_or((dependency, ""));
                if !provided.contains(name) {
                    bail!("Lockfile does not satisify dependency: {dependency:?}");
                }
            }
        }

        Ok(())
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
