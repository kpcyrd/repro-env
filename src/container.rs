use crate::errors::*;
use serde::{Deserialize, Serialize};
use std::ffi::OsStr;
use std::fmt;
use std::future::{self, Future};
use std::io::Read;
use std::process::Stdio;
use std::str::FromStr;
use tokio::io::AsyncWriteExt;
use tokio::process::Command;
use tokio::signal;

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

#[derive(Debug, Default)]
pub struct ExecConfig {
    pub capture_stdout: bool,
    pub silence_stderr: bool,
    pub stdin: Option<Vec<u8>>,
}

pub async fn podman<I, S>(args: I, config: &ExecConfig) -> Result<Vec<u8>>
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr> + fmt::Debug,
{
    let mut cmd = Command::new("podman");
    let args = args.into_iter().collect::<Vec<_>>();
    cmd.args(&args);
    if config.stdin.is_some() {
        cmd.stdin(Stdio::piped());
    }
    if config.capture_stdout {
        cmd.stdout(Stdio::piped());
    }
    if config.silence_stderr {
        cmd.stderr(Stdio::null());
    }
    debug!("Spawning child process: podman {:?}", args);
    let mut child = cmd.spawn().context("Failed to execute podman binary")?;

    // write to stdin (if configured)
    if let Some(buf) = &config.stdin {
        if let Some(mut stdin) = child.stdin.take() {
            stdin.write_all(buf).await?;
        }
    }

    // wait for the process to exit
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
    podman(&["image", "pull", "--", image], &ExecConfig::default()).await?;
    Ok(())
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct Image {
    pub digest: String,
}

pub async fn inspect(image: &str) -> Result<Image> {
    let inspect = podman(
        &["image", "inspect", "--", image],
        &ExecConfig {
            capture_stdout: true,
            silence_stderr: true,
            ..Default::default()
        },
    )
    .await?;
    let mut list = serde_json::from_slice::<Vec<Image>>(&inspect)?;
    debug!("Image inspect result: {list:?}");

    let inspect = list
        .pop()
        .with_context(|| anyhow!("Could not find any matching image: {image:?}"))?;

    match list.len() {
        0 => Ok(inspect),
        len => bail!(
            "The specified image is not canonical, inspect returned {}, expected 1",
            len + 1
        ),
    }
}

#[derive(Debug)]
pub struct Config<'a> {
    pub mounts: &'a [(String, String)],
    pub expose_fuse: bool,
}

#[derive(Debug, Default)]
pub struct Exec<'a> {
    pub capture_stdout: bool,
    pub cwd: Option<&'a str>,
    pub user: Option<&'a str>,
    pub env: &'a [String],
}

#[derive(Debug)]
pub struct Container {
    pub id: String,
}

impl Container {
    pub async fn create(image: &str, config: Config<'_>) -> Result<Container> {
        let mut podman_args = vec![
            "container".to_string(),
            "run".to_string(),
            "--detach".to_string(),
            "--rm".to_string(),
            "--network=host".to_string(),
            "-v=/usr/bin/catatonit:/__:ro".to_string(),
            "--entrypoint=/__".to_string(),
        ];

        for (src, dest) in config.mounts {
            podman_args.push(format!("-v={src}:{dest}"));
        }

        if config.expose_fuse {
            debug!("Mapping /dev/fuse into the container");
            podman_args.push("--device=/dev/fuse".to_string());
        }

        podman_args.extend(["--".to_string(), image.to_string(), "-P".to_string()]);

        debug!("Creating container...");
        let mut out = podman(
            &podman_args,
            &ExecConfig {
                capture_stdout: true,
                ..Default::default()
            },
        )
        .await?;
        if let Some(idx) = memchr::memchr(b'\n', &out) {
            out.truncate(idx);
        }
        let id = String::from_utf8(out)?;
        Ok(Container { id })
    }

    pub async fn exec<I, S>(&self, args: I, options: Exec<'_>) -> Result<Vec<u8>>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str> + fmt::Debug + Clone,
    {
        let args = args.into_iter().collect::<Vec<_>>();
        let mut a = vec!["container".to_string(), "exec".to_string()];

        if let Some(cwd) = options.cwd {
            a.extend(["-w".to_string(), cwd.to_string()]);
        }

        if let Some(user) = options.user {
            a.extend(["-u".to_string(), user.to_string()]);
        }

        for env in options.env {
            a.extend(["-e".to_string(), env.to_string()]);
        }

        a.extend(["--".to_string(), self.id.to_string()]);
        a.extend(args.iter().map(|x| x.as_ref().to_string()));
        let buf = podman(
            &a,
            &ExecConfig {
                capture_stdout: options.capture_stdout,
                ..Default::default()
            },
        )
        .await
        .with_context(|| anyhow!("Failed to execute in container: {:?}", args))?;
        Ok(buf)
    }

    pub async fn tar(&self, path: &str) -> Result<Vec<u8>> {
        let a = vec![
            "container".to_string(),
            "cp".to_string(),
            "--".to_string(),
            format!("{}:{}", self.id, path),
            "-".to_string(),
        ];
        let buf = podman(
            &a,
            &ExecConfig {
                capture_stdout: true,
                ..Default::default()
            },
        )
        .await
        .with_context(|| anyhow!("Failed to read from container: {:?}", path))?;

        Ok(buf)
    }

    pub async fn cat(&self, path: &str) -> Result<Vec<u8>> {
        let buf = self.tar(path).await?;

        let mut tar = tar::Archive::new(&buf[..]);
        let mut entries = tar.entries()?;
        let entry = entries
            .next()
            .context("Tar archive generated by podman cp is empty")?;
        let mut entry = entry?;

        let entry_type = entry.header().entry_type();
        if entry_type != tar::EntryType::Regular {
            bail!("Extracted file is not of type file: {entry_type:?}");
        }

        let mut buf = Vec::new();
        entry.read_to_end(&mut buf)?;

        Ok(buf)
    }

    pub async fn write_file(&self, directory: &str, filename: &str, content: &[u8]) -> Result<()> {
        // generate tar file
        let mut tar = tar::Builder::new(Vec::new());

        let mut header = tar::Header::new_gnu();
        header.set_size(content.len() as u64);
        header.set_mode(0o640);

        debug!(
            "Adding to archive: {:?} ({} bytes)",
            filename,
            content.len()
        );
        tar.append_data(&mut header, filename, content)?;
        let buf = tar.into_inner()?;

        // pass archive into container
        let a = vec![
            "container".to_string(),
            "cp".to_string(),
            "--".to_string(),
            "-".to_string(),
            format!("{}:{}", self.id, directory),
        ];
        podman(
            &a,
            &ExecConfig {
                stdin: Some(buf),
                ..Default::default()
            },
        )
        .await
        .with_context(|| {
            anyhow!("Failed to write container (directory={directory:?}, filename={filename:?}")
        })?;

        Ok(())
    }

    pub async fn kill(&self) -> Result<()> {
        podman(
            &["container", "kill", &self.id],
            &ExecConfig {
                capture_stdout: true,
                ..Default::default()
            },
        )
        .await
        .context("Failed to remove container")?;
        Ok(())
    }

    pub async fn run<F: Future<Output = Result<()>>>(&self, fut: F, keep: bool) -> Result<()> {
        let fut = async {
            fut.await?;
            if keep {
                info!("Keeping container around until ^C...");
                future::pending().await
            } else {
                Ok(())
            }
        };
        let result = tokio::select! {
            result = fut => result,
            _ = signal::ctrl_c() => Err(anyhow!("Ctrl-c received")),
        };
        debug!("Removing container...");
        if let Err(err) = self.kill().await {
            warn!("Failed to kill container {:?}: {:#}", self.id, err);
        }
        debug!("Container cleanup complete");
        result
    }
}

#[cfg(target_os = "linux")]
pub fn test_userns_clone() -> Result<()> {
    use nix::sched::CloneFlags;
    use nix::sys::wait::{WaitPidFlag, WaitStatus};

    let cb = Box::new(|| 0);
    let stack = &mut [0; 1024];
    let flags = CloneFlags::CLONE_NEWNS | CloneFlags::CLONE_NEWUSER;

    let pid = unsafe { nix::sched::clone(cb, stack, flags, None) }
        .context("Failed to create user namespace")?;
    let status = nix::sys::wait::waitpid(pid, Some(WaitPidFlag::__WCLONE))
        .context("Failed to reap child")?;

    if status != WaitStatus::Exited(pid, 0) {
        bail!("Unexpected wait result: {:?}", status);
    }

    Ok(())
}

#[cfg(target_os = "linux")]
pub async fn test_for_unprivileged_userns_clone() -> Result<()> {
    if std::env::var("REPRO_ENV_SKIP_CLONE_CHECK")
        .map(|x| x != "0")
        .unwrap_or(false)
    {
        debug!("Skipping test if user namespaces can be created");
        return Ok(());
    }

    debug!("Testing if user namespaces can be created");
    if let Err(err) = test_userns_clone() {
        match tokio::fs::read("/proc/sys/kernel/unprivileged_userns_clone").await {
            Ok(buf) => {
                if buf == b"0\n" {
                    warn!("User namespaces are not enabled in /proc/sys/kernel/unprivileged_userns_clone")
                }
            }
            Err(err) => warn!(
                "Failed to check if unprivileged_userns_clone are allowed: {:#}",
                err
            ),
        }

        Err(err)
    } else {
        debug!("Successfully tested for user namespaces");
        Ok(())
    }
}

#[cfg(not(target_os = "linux"))]
pub async fn test_for_unprivileged_userns_clone() -> Result<()> {
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
