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
use std::io::BufReader;
use std::io::Read;
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

#[derive(Debug, PartialEq)]
pub struct PkgInfo {
    pub name: String,
    pub version: String,
}

impl PkgInfo {
    pub fn parse_control(control: &str) -> Result<Self> {
        let mut name = None;
        let mut version = None;

        for line in control.lines() {
            if let Some(value) = line.strip_prefix("Package: ") {
                name = Some(value.to_string());
            }

            if let Some(value) = line.strip_prefix("Version: ") {
                version = Some(value.to_string());
            }
        }

        Ok(PkgInfo {
            name: name.context("Failed to find package name in deb control data")?,
            version: version.context("Failed to find package version in deb control data")?,
        })
    }

    pub fn parse_control_tar<R: Read>(filename: &[u8], reader: R) -> Result<Self> {
        let mut buf = Vec::new();
        let mut reader = BufReader::new(reader);
        match filename {
            b"control.tar.xz" => lzma_rs::xz_decompress(&mut reader, &mut buf)?,
            _ => bail!("Unsupported compression for control.tar: {filename:?}"),
        }

        let mut tar = tar::Archive::new(&buf[..]);
        for entry in tar.entries()? {
            let mut entry = entry?;
            let path = entry.path()?;
            let filename = path.to_str().with_context(|| {
                anyhow!("Package contains paths with invalid encoding: {:?}", path)
            })?;

            if filename == "./control" {
                let mut buf = String::new();
                entry.read_to_string(&mut buf)?;
                return Self::parse_control(&buf);
            }
        }

        bail!("Failed to find control data in control.tar")
    }

    pub fn parse_deb<R: Read>(reader: R) -> Result<Self> {
        let mut archive = ar::Archive::new(reader);
        while let Some(entry) = archive.next_entry() {
            let mut entry = entry?;
            let filename = entry.header().identifier();
            if !filename.starts_with(b"control.tar") {
                continue;
            }
            let filename = filename.to_owned();
            return Self::parse_control_tar(&filename, &mut entry);
        }

        bail!("Failed to find control data")
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

        let pkginfo = PkgInfo::parse_deb(&buf[..])?;

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
            name: pkginfo.name,
            version: pkginfo.version,
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_control_data() -> Result<()> {
        let data = "Package: binutils-common\nSource: binutils\nVersion: 2.40-2\nArchitecture: amd64\nMaintainer: Matthias Klose <doko@debian.org>\nInstalled-Size: 15021\nBreaks: binutils (<< 2.38.50.20220527-2), binutils-multiarch (<< 2.38.50.20220527-2)\nReplaces: binutils (<< 2.38.50.20220527-2), binutils-multiarch (<< 2.38.50.20220527-2)\nSection: devel\nPriority: optional\nMulti-Arch: same\nHomepage: https://www.gnu.org/software/binutils/\nDescription: Common files for the GNU assembler, linker and binary utilities\n This package contains the localization files used by binutils packages for\n various target architectures and parts of the binutils documentation. It is\n not useful on its own.\n";
        let data = PkgInfo::parse_control(data)?;
        assert_eq!(
            data,
            PkgInfo {
                name: "binutils-common".to_string(),
                version: "2.40-2".to_string(),
            }
        );
        Ok(())
    }
}
