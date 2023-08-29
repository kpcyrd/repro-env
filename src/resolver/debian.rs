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
use std::io::prelude::*;
use tokio::fs;

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

#[derive(Debug, Clone, PartialEq)]
pub struct PkgEntry {
    name: String,
    version: String,
    sha256: String,
}

#[derive(Debug, Default, PartialEq)]
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
            let Some(name) = line.strip_prefix("Package: ") else {
                bail!("Unexpected line in database (expected `Package: `): {line:?}")
            };
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
            let new = PkgEntry {
                name: name.to_string(),
                version: version.context("Package database entry is missing version")?,
                sha256: sha256.context("Package database entry is missing sha256")?,
            };
            let old = self.pkgs.insert(filename.to_string(), new.clone());

            if let Some(old) = old {
                // it's only a problem if they differ
                if old != new {
                    bail!("Filename is not unique in package database: filename={filename:?}, old={old:?}, new={new:?}");
                }
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
            let Some(extension) = path.extension() else {
                continue;
            };

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

    Ok(())
}

pub async fn resolve(
    update: &args::Update,
    manifest: &PackagesManifest,
    container: &ContainerLock,
    dependencies: &mut Vec<PackageLock>,
) -> Result<()> {
    let container = Container::create(
        &container.image,
        container::Config {
            mounts: &[],
            expose_fuse: false,
        },
    )
    .await?;
    container
        .run(
            resolve_dependencies(&container, manifest, dependencies),
            update.keep,
        )
        .await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_pkg_database() -> Result<()> {
        let lz4 = {
            let mut w = lz4_flex::frame::FrameEncoder::new(Vec::new());
            w.write_all(br#"Package: binutils-aarch64-linux-gnu
Source: binutils
Version: 2.40-2
Installed-Size: 19242
Maintainer: Matthias Klose <doko@debian.org>
Architecture: amd64
Replaces: binutils (<< 2.29-6), binutils-dev (<< 2.38.50.20220609-2)
Depends: binutils-common (= 2.40-2), libbinutils (>= 2.39.50), libc6 (>= 2.36), libgcc-s1 (>= 4.2), libjansson4 (>= 2.14), libzstd1 (>= 1.5.2), zlib1g (>= 1:1.1.4)
Suggests: binutils-doc (= 2.40-2)
Breaks: binutils (<< 2.29-6), binutils-dev (<< 2.38.50.20220609-2)
Description: GNU binary utilities, for aarch64-linux-gnu target
Multi-Arch: allowed
Homepage: https://www.gnu.org/software/binutils/
Description-md5: 102820197d11c3672c0cd4ce0becb720
Section: devel
Priority: optional
Filename: pool/main/b/binutils/binutils-aarch64-linux-gnu_2.40-2_amd64.deb
Size: 3352924
MD5sum: 2c02fdb8d4455ace16be0bb922eb8502
SHA256: 3d6f64a7a4ed6d73719f8fa2e85fd896f58ff7f211a6683942ba93de690aaa66

Package: rustc
Version: 1.63.0+dfsg1-2
Installed-Size: 7753
Maintainer: Debian Rust Maintainers <pkg-rust-maintainers@alioth-lists.debian.net>
Architecture: amd64
Replaces: libstd-rust-dev (<< 1.25.0+dfsg1-2~~)
Depends: libc6 (>= 2.34), libgcc-s1 (>= 3.0), libstd-rust-dev (= 1.63.0+dfsg1-2), gcc, libc-dev, binutils (>= 2.26)
Recommends: cargo (>= 0.64.0~~), cargo (<< 0.65.0~~), llvm-14
Suggests: lld-14, clang-14
Breaks: libstd-rust-dev (<< 1.25.0+dfsg1-2~~)
Description: Rust systems programming language
Multi-Arch: allowed
Homepage: http://www.rust-lang.org/
Description-md5: 67ca6080eea53dc7f3cdf73bc6b8521e
Section: rust
Priority: optional
Filename: pool/main/r/rustc/rustc_1.63.0+dfsg1-2_amd64.deb
Size: 2612712
MD5sum: 5eaa6969388c512a206377bf813ab531
SHA256: 26dd439266153e38d3e6fbe0fe2dbbb41f20994afa688faa71f38427348589ed
"#)?;
            w.finish()?
        };

        let tar = {
            let mut tar = tar::Builder::new(Vec::new());
            let mut header = tar::Header::new_gnu();
            header.set_path("deb.debian.org_debian_dists_stable_main_binary-amd64_Packages.lz4")?;
            header.set_size(lz4.len() as u64);
            header.set_cksum();
            tar.append(&header, &lz4[..])?;
            tar.into_inner()?
        };

        let db = PkgDatabase::import_tar(&tar)?;
        let pkgs = {
            let mut pkgs = HashMap::new();
            pkgs.insert(
                "binutils-aarch64-linux-gnu_2.40-2_amd64.deb".to_string(),
                PkgEntry {
                    name: "binutils-aarch64-linux-gnu".to_string(),
                    version: "2.40-2".to_string(),
                    sha256: "3d6f64a7a4ed6d73719f8fa2e85fd896f58ff7f211a6683942ba93de690aaa66"
                        .to_string(),
                },
            );
            pkgs.insert(
                "rustc_1.63.0+dfsg1-2_amd64.deb".to_string(),
                PkgEntry {
                    name: "rustc".to_string(),
                    version: "1.63.0+dfsg1-2".to_string(),
                    sha256: "26dd439266153e38d3e6fbe0fe2dbbb41f20994afa688faa71f38427348589ed"
                        .to_string(),
                },
            );
            pkgs
        };
        assert_eq!(db, PkgDatabase { pkgs });

        Ok(())
    }

    #[test]
    fn test_pkg_database_apt_output_parser() -> Result<()> {
        let mut db = PkgDatabase::default();
        db.pkgs.insert(
            "rustc_1.63.0+dfsg1-2_amd64.deb".to_string(),
            PkgEntry {
                name: "rustc".to_string(),
                version: "1.63.0+dfsg1-2".to_string(),
                sha256: "26dd439266153e38d3e6fbe0fe2dbbb41f20994afa688faa71f38427348589ed"
                    .to_string(),
            },
        );

        let result = db.find_by_apt_output("'http://deb.debian.org/debian/pool/main/r/rustc/rustc_1.63.0%2bdfsg1-2_amd64.deb' rustc_1.63.0+dfsg1-2_amd64.deb 2612712 MD5Sum:5eaa6969388c512a206377bf813ab531")?;
        assert_eq!(
            result,
            (
                "http://deb.debian.org/debian/pool/main/r/rustc/rustc_1.63.0%2bdfsg1-2_amd64.deb"
                    .to_string(),
                &PkgEntry {
                    name: "rustc".to_string(),
                    version: "1.63.0+dfsg1-2".to_string(),
                    sha256: "26dd439266153e38d3e6fbe0fe2dbbb41f20994afa688faa71f38427348589ed"
                        .to_string(),
                }
            )
        );

        let result = db.find_by_apt_output("'http://deb.debian.org/debian/pool/main/n/non-existant/non-existant_1.2.3_amd64.deb' non-existant_1.2.3_amd64.deb 2612712 MD5Sum:5eaa6969388c512a206377bf813ab531");
        assert!(result.is_err());

        Ok(())
    }
}
