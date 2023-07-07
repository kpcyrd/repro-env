use crate::args;
use crate::container::{self, Container};
use crate::errors::*;
use crate::http;
use crate::lockfile::{ContainerLock, PackageLock};
use crate::manifest::PackagesManifest;
use crate::paths;
use serde::Deserialize;
use sha1::Sha1;
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::io::BufRead;
use std::io::Read;
use tokio::fs;
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

#[derive(Debug)]
pub struct PkgEntry {
    name: String,
    version: String,
    sha256: String,
}

#[derive(Debug, Default)]
pub struct PkgDatabase {
    pkgs: HashMap<String, PkgEntry>,
}

impl PkgDatabase {
    pub fn import_lz4<R: Read>(&mut self, reader: R) -> Result<()> {
        let rdr = lz4_flex::frame::FrameDecoder::new(reader);
        let mut lines = rdr.lines();

        while let Some(line) = lines.next() {
            let line = line?;
            trace!("Found line in debian package database: {line:?}");
            let Some(name) = line.strip_prefix("Package: ") else { bail!("Unexpected line in database (expected `Package: `): {line:?}") };
            let mut version = None;
            let mut filename = None;
            let mut sha256 = None;

            for line in &mut lines {
                let line = line?;
                trace!("Found line in debian package database: {line:?}");

                if line.is_empty() {
                    break;
                } else if let Some(value) = line.strip_prefix("Version: ") {
                    version = Some(value.to_string());
                } else if let Some(value) = line.strip_prefix("Filename: ") {
                    let value = value
                        .rsplit_once('/')
                        .map(|(_, filename)| filename)
                        .unwrap_or(value);
                    filename = Some(value.to_string());
                } else if let Some(value) = line.strip_prefix("SHA256: ") {
                    sha256 = Some(value.to_string());
                }
            }

            let filename = filename.context("Package database entry is missing filename")?;
            let old = self.pkgs.insert(
                filename.to_string(),
                PkgEntry {
                    name: name.to_string(),
                    version: version.context("Package database entry is missing version")?,
                    sha256: sha256.context("Package database entry is missing sha256")?,
                },
            );

            if let Some(old) = old {
                bail!("Filename is not unique in package database: filename={filename:?}, {old:?}");
            }
        }

        Ok(())
    }

    pub fn import_tar(buf: &[u8]) -> Result<Self> {
        let mut tar = tar::Archive::new(buf);

        let mut db = Self::default();
        for entry in tar.entries()? {
            let entry = entry?;
            let path = entry
                .header()
                .path()
                .context("Filename was not valid utf-8")?;
            let Some(extension) = path.extension() else { continue };

            if extension.to_str() == Some("lz4") {
                db.import_lz4(entry)?;
            }
        }

        Ok(db)
    }

    pub fn find_by_filename(&self, filename: &str) -> Result<&PkgEntry> {
        let entry = self
            .pkgs
            .get(filename)
            .context("Failed to find package database entry for: {filename:?}")?;
        Ok(entry)
    }

    pub fn find_by_apt_output(&self, line: &str) -> Result<(String, &PkgEntry)> {
        let mut line = line.split(' ');
        let url = line.next().context("Missing url in apt output")?;
        let filename = line.next().context("Missing filename in apt output")?;
        let _size = line.next().context("Missing size in apt output")?;
        let _md5sum = line.next().context("Missing md5sum in apt output")?;

        if let Some(trailing) = line.next() {
            bail!("Trailing data in apt output: {trailing:?}");
        }

        let url = url.strip_prefix('\'').unwrap_or(url);
        let url = url.strip_suffix('\'').unwrap_or(url);
        debug!("Detected dependency filename={filename:?} url={url:?}");

        let package = {
            let url = url
                .parse::<reqwest::Url>()
                .context("Failed to parse as url")?;
            let filename = url
                .path_segments()
                .context("Failed to get path from url")?
                .last()
                .context("Failed to get filename from url")?;
            let filename =
                urlencoding::decode(filename).context("Failed to url decode filename")?;
            self.find_by_filename(&filename).with_context(|| {
                anyhow!("Failed to find package database entry for file: {filename:?}")
            })?
        };

        Ok((url.to_string(), package))
    }
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

    info!("Importing package database...");
    let tar = container.tar("/var/lib/apt/lists").await?;
    let db = PkgDatabase::import_tar(&tar)?;

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
    let pkgs_cache_dir = paths::pkgs_cache_dir()?;
    for line in buf.lines() {
        let (url, package) = db.find_by_apt_output(line)?;

        let path = pkgs_cache_dir.sha256_path(&package.sha256)?;
        let buf = if path.exists() {
            fs::read(path).await?
        } else {
            let buf = client.fetch(&url).await?.to_vec();

            let mut hasher = Sha256::new();
            hasher.update(&buf);
            let result = hex::encode(hasher.finalize());

            if result != package.sha256 {
                bail!(
                    "Mismatch of sha256 checksum, expected={}, downloaded={}",
                    package.sha256,
                    result
                );
            }

            buf
        };

        let mut hasher = Sha1::new();
        hasher.update(&buf);
        let sha1 = hex::encode(hasher.finalize());

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
            name: package.name.to_string(),
            version: package.version.to_string(),
            system: "debian".to_string(),
            url,
            sha256: package.sha256.to_string(),
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
