use crate::args;
use crate::container::{self, Container};
use crate::errors::*;
use crate::http;
use crate::lockfile::{ContainerLock, PackageLock};
use crate::manifest::PackagesManifest;
use md5::Md5;
use serde::Deserialize;
use sha1::Sha1;
use sha2::{Digest, Sha256};
use tokio::signal;

#[derive(Debug, Deserialize)]
pub struct JsonSnapshotInfo {
    pub result: Vec<JsonSnapshotPkg>,
}

#[derive(Debug, Deserialize)]
pub struct JsonSnapshotPkg {
    pub archive_name: String,
    pub first_seen: String,
    pub name: String,
    pub path: String,
    pub size: i64,
}

pub async fn resolve_dependencies(
    container: &Container,
    manifest: &PackagesManifest,
    dependencies: &mut Vec<PackageLock>,
    keep: bool,
) -> Result<()> {
    info!("Update package datatabase...");
    container
        .exec(&["apt-get", "update"], container::Exec::default())
        .await?;

    info!("Resolving dependencies...");
    let mut cmd = vec![
        "apt-get",
        "-qq",
        "--print-uris",
        "--no-install-recommends",
        "upgrade",
        "--",
    ];
    for dep in &manifest.dependencies {
        cmd.push(dep.as_str());
    }
    let buf = container
        .exec(
            &cmd,
            container::Exec {
                capture_stdout: true,
                ..Default::default()
            },
        )
        .await?;
    let buf = String::from_utf8(buf).context("Failed to decode pacman output as utf8")?;

    let client = http::Client::new()?;
    for line in buf.lines() {
        let mut line = line.split(' ');
        let url = line.next().context("Missing url in apt output")?;
        let filename = line.next().context("Missing filename in apt output")?;
        let _size = line.next().context("Missing size in apt output")?;
        let md5sum = line.next().context("Missing md5sum in apt output")?;

        if let Some(trailing) = line.next() {
            bail!("Trailing data in apt output: {trailing:?}");
        }

        let url = url.strip_prefix('\'').unwrap_or(url);
        let url = url.strip_suffix('\'').unwrap_or(url);
        debug!("Detected dependency filename={filename:?} url={url:?} md5sum={md5sum:?}");

        let buf = client.fetch(url).await?;

        // TODO: we fail-open here because for debian-security it just prints nothing 🤷
        if !md5sum.is_empty() {
            // TODO: calculate hash during download
            let mut hasher = Md5::new();
            hasher.update(&buf);
            let md5 = hex::encode(hasher.finalize());

            if Some(md5.as_str()) != md5sum.strip_prefix("MD5Sum:") {
                // TODO: md5 should not be used here
                bail!("md5 checkout does not match");
            }
        }

        let mut hasher = Sha1::new();
        hasher.update(&buf);
        let sha1 = hex::encode(hasher.finalize());

        let mut hasher = Sha256::new();
        hasher.update(&buf);
        let sha256 = hex::encode(hasher.finalize());

        let url = format!("https://snapshot.debian.org/mr/file/{sha1}/info");
        let buf = client
            .fetch(&url)
            .await
            .context("Failed to lookup pkg hash on snapshot.debian.org")?;

        let info = serde_json::from_slice::<JsonSnapshotInfo>(&buf)
            .context("Failed to decode snapshot.debian.org json response")?;

        let pkg = info
            .result
            .first()
            .context("Could not find package in any snapshots")?;

        let archive_name = &pkg.archive_name;
        let first_seen = &pkg.first_seen;
        let path = &pkg.path;
        let name = &pkg.name;

        let url =
            format!("https://snapshot.debian.org/archive/{archive_name}/{first_seen}{path}/{name}");

        dependencies.push(PackageLock {
            name: filename.to_string(),
            version: "???".to_string(),
            system: "debian".to_string(),
            url,
            sha256,
            signature: None,
        });
    }

    if keep {
        info!("Keeping container around until ^C...");
        futures::future::pending().await
    } else {
        Ok(())
    }
}

pub async fn resolve(
    update: &args::Update,
    manifest: &PackagesManifest,
    container: &ContainerLock,
    dependencies: &mut Vec<PackageLock>,
) -> Result<()> {
    debug!("Creating container...");
    let init = &["/__".to_string(), "-P".to_string()];
    let container = Container::create(
        &container.image,
        container::Config {
            init,
            mounts: &[],
            expose_fuse: false,
        },
    )
    .await?;
    let container_id = container.id.clone();
    let result = tokio::select! {
        result = resolve_dependencies(&container, manifest, dependencies, update.keep) => result,
        _ = signal::ctrl_c() => Err(anyhow!("Ctrl-c received")),
    };
    debug!("Removing container...");
    if let Err(err) = container.kill().await {
        warn!("Failed to kill container {:?}: {:#}", container_id, err);
    }
    debug!("Container cleanup complete");
    result
}
