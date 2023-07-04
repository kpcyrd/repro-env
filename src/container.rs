use crate::errors::*;
use serde::{Deserialize, Serialize};
use std::ffi::OsStr;
use std::fmt;
use std::process::Stdio;
use std::str::FromStr;
use tokio::process::Command;

#[derive(Debug, PartialEq, Clone)]
pub struct ImageRef {
    pub repo: String,
    pub tag: Option<String>,
    pub digest: Option<String>,
}

impl FromStr for ImageRef {
    type Err = Error;

    fn from_str(s: &str) -> Result<Self> {
        if let Some((repo, digest)) = s.split_once('@') {
            Ok(ImageRef {
                repo: repo.to_string(),
                tag: None,
                digest: Some(digest.to_string()),
            })
        } else if let Some((repo, tag)) = s.split_once(':') {
            Ok(ImageRef {
                repo: repo.to_string(),
                tag: Some(tag.to_string()),
                digest: None,
            })
        } else {
            Ok(ImageRef {
                repo: s.to_string(),
                tag: None,
                digest: None,
            })
        }
    }
}

impl ToString for ImageRef {
    fn to_string(&self) -> String {
        let repo = &self.repo;
        if let Some(digest) = &self.digest {
            format!("{repo}@{digest}")
        } else if let Some(tag) = &self.tag {
            format!("{repo}:{tag}")
        } else {
            repo.to_string()
        }
    }
}

pub async fn podman<I, S>(args: I, capture_stdout: bool) -> Result<Vec<u8>>
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr> + fmt::Debug,
{
    let mut cmd = Command::new("podman");
    let args = args.into_iter().collect::<Vec<_>>();
    cmd.args(&args);
    if capture_stdout {
        cmd.stdout(Stdio::piped());
    }
    debug!("Spawning child process: podman {:?}", args);
    let child = cmd.spawn().context("Failed to execute podman binary")?;

    let out = child.wait_with_output().await?;
    debug!("Podman command exited: {:?}", out.status);
    if !out.status.success() {
        bail!(
            "Podman command ({:?}) failed to execute: {:?}",
            args,
            out.status
        );
    }
    Ok(out.stdout)
}

pub async fn pull(image: &str) -> Result<()> {
    podman(&["image", "pull", "--", image], false).await?;
    Ok(())
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct Image {
    pub digest: String,
}

pub async fn inspect(image: &str) -> Result<Vec<Image>> {
    let inspect = podman(&["image", "inspect", "--", image], true).await?;
    let inspect = serde_json::from_slice::<Vec<Image>>(&inspect)?;
    debug!("Image inspect result: {inspect:?}");
    Ok(inspect)
}

pub async fn run(image: &str, cmd: &[&str], mounts: &[(&str, &str)]) -> Result<()> {
    let mut args = vec![
        "run".to_string(),
        "--init".to_string(),
        "--rm".to_string(),
        "-w".to_string(),
        "/build".to_string(),
    ];

    for (src, dest) in mounts {
        args.extend(["-v".to_string(), format!("{src}:{dest}")]);
    }

    args.extend(["--".to_string(), image.to_string()]);
    for arg in cmd {
        args.push(arg.to_string());
    }

    podman(&args, false).await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_image_ref() -> Result<()> {
        let image_ref = ImageRef::from_str("rust")?;
        assert_eq!(
            image_ref,
            ImageRef {
                repo: "rust".to_string(),
                tag: None,
                digest: None,
            }
        );
        Ok(())
    }

    #[test]
    fn test_parse_image_ref_digest() -> Result<()> {
        let image_ref = ImageRef::from_str(
            "rust@sha256:28ee8822965a932e229599b59928f8c2655b2a198af30568acf63e8aff0e8a3a",
        )?;
        assert_eq!(
            image_ref,
            ImageRef {
                repo: "rust".to_string(),
                tag: None,
                digest: Some(
                    "sha256:28ee8822965a932e229599b59928f8c2655b2a198af30568acf63e8aff0e8a3a"
                        .to_string()
                ),
            }
        );
        Ok(())
    }

    #[test]
    fn test_parse_image_ref_tag() -> Result<()> {
        let image_ref = ImageRef::from_str("rust:1-alpine3.18")?;
        assert_eq!(
            image_ref,
            ImageRef {
                repo: "rust".to_string(),
                tag: Some("1-alpine3.18".to_string()),
                digest: None,
            }
        );
        Ok(())
    }
}
