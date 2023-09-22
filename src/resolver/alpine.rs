use crate::args;
use crate::container::{self, Container};
use crate::errors::*;
use crate::http;
use crate::lockfile::{ContainerLock, PackageLock};
use crate::manifest::PackagesManifest;
use crate::paths;
use data_encoding::BASE64;
use flate2::bufread::GzDecoder;
use sha1::Sha1;
use sha2::{Digest, Sha256};
use std::collections::{HashMap, HashSet};
use std::io::{BufRead, BufReader, Read};
use std::rc::Rc;
use tokio::fs;

pub fn read_gzip_to_end<R: BufRead>(reader: &mut R) -> Result<Vec<u8>> {
    let mut buf = Vec::new();
    let mut gz = GzDecoder::new(reader);
    gz.read_to_end(&mut buf)?;
    Ok(buf)
}

pub fn decode_apk_checksum(checksum: &str) -> Result<Vec<u8>> {
    let checksum = checksum
        .strip_prefix("Q1")
        .with_context(|| anyhow!("Only checksums starting with Q1 are supported: {checksum:?}"))?;
    let checksum = BASE64
        .decode(checksum.as_bytes())
        .context("Failed to decode checksum as base64")?;
    Ok(checksum)
}

#[derive(Debug, Default)]
pub struct DatabaseCache {
    repos: HashMap<String, Rc<String>>,
    pkgs: HashMap<String, CacheEntry>,
}

#[derive(Debug)]
pub struct CacheEntry {
    name: String,
    version: String,
    arch: String,
    checksum: String,
    repo_url: Rc<String>,
}

pub struct CacheEntryDraft {
    pub name: Option<String>,
    pub version: Option<String>,
    pub arch: Option<String>,
    pub checksum: Option<String>,
    pub repo_url: Rc<String>,
}

impl TryFrom<CacheEntryDraft> for CacheEntry {
    type Error = Error;

    fn try_from(draft: CacheEntryDraft) -> Result<Self> {
        Ok(Self {
            name: draft.name.context("Missing name field")?,
            version: draft.version.context("Missing version field")?,
            arch: draft.arch.context("Missing arch field")?,
            checksum: draft.checksum.context("Missing checksum field")?,
            repo_url: draft.repo_url,
        })
    }
}

impl CacheEntryDraft {
    pub fn new(repo_url: Rc<String>) -> Self {
        CacheEntryDraft {
            name: None,
            version: None,
            arch: None,
            checksum: None,
            repo_url,
        }
    }
}

impl DatabaseCache {
    pub fn get(&self, id: &str) -> Result<&CacheEntry> {
        let entry = self
            .pkgs
            .get(id)
            .context("Failed to find package database entry for: {id:?}")?;
        Ok(entry)
    }

    pub fn read_apkindex_text<R: Read>(&mut self, r: R, repo_url: &Rc<String>) -> Result<()> {
        let reader = BufReader::new(r);
        let mut draft = CacheEntryDraft::new(repo_url.clone());
        for line in reader.lines() {
            let line = line?;
            if line.is_empty() {
                let mut new = CacheEntryDraft::new(repo_url.clone());
                (new, draft) = (draft, new);
                let pkg = CacheEntry::try_from(new)?;
                let id = format!("{}-{}", pkg.name, pkg.version);
                trace!("Inserting pkg into lookup table: {id:?} => {pkg:?}");
                self.pkgs.insert(id, pkg);
            } else if let Some((key, value)) = line.split_once(':') {
                match key {
                    "P" => {
                        trace!("Package name: {value:?}");
                        draft.name = Some(value.to_string());
                    }
                    "V" => {
                        trace!("Package version: {value:?}");
                        draft.version = Some(value.to_string());
                    }
                    "C" => {
                        trace!("Package checksum: {value:?}");
                        let checksum = decode_apk_checksum(value)?;
                        draft.checksum = Some(hex::encode(checksum));
                    }
                    "A" => {
                        trace!("Package architecture: {value:?}");
                        draft.arch = Some(value.to_string());
                    }
                    _ => trace!("Ignoring APKINDEX value key={key:?}, value={value:?}"),
                }
            } else {
                bail!("Invalid line in index: {line:?}");
            }
        }
        Ok(())
    }

    pub fn read_apkindex_container<R: Read>(&mut self, r: R, repo_url: &Rc<String>) -> Result<()> {
        let mut r = BufReader::new(r);
        read_gzip_to_end(&mut r).context("Failed to strip signature")?;

        let d = GzDecoder::new(r);
        let mut tar = tar::Archive::new(d);

        for entry in tar.entries()? {
            let entry = entry?;
            if entry.header().entry_type() == tar::EntryType::Regular {
                let path = entry.path()?;
                if path.to_str() == Some("APKINDEX") {
                    self.read_apkindex_text(entry, repo_url)?;
                }
            }
        }

        Ok(())
    }

    pub fn import_from_container(&mut self, buf: &[u8]) -> Result<()> {
        let mut tar = tar::Archive::new(buf);

        for entry in tar.entries()? {
            let entry = entry?;
            if entry.header().entry_type() == tar::EntryType::Regular {
                let path = entry.path()?;
                let file_name = path
                    .file_name()
                    .context("Failed to detect filename")?
                    .to_str()
                    .unwrap_or("");
                if let Some(repo_url) = self.repos.get(file_name).cloned() {
                    debug!("Reading package index for repository: {repo_url:?} ({file_name:?})");
                    self.read_apkindex_container(entry, &repo_url)?;
                }
            }
        }

        Ok(())
    }

    pub fn register_repo(&mut self, repo: String) {
        let mut hasher = Sha1::new();
        hasher.update(&repo);
        let hash = hasher.finalize();
        let sha1 = hex::encode(&hash[..4]);
        self.repos
            .insert(format!("APKINDEX.{sha1}.tar.gz"), Rc::new(repo));
    }

    pub fn init_repos_from_container(&mut self, buf: &[u8]) -> Result<()> {
        let mut tar = tar::Archive::new(buf);
        for entry in tar.entries()? {
            let entry = entry?;
            if entry.header().entry_type() == tar::EntryType::Regular {
                let reader = BufReader::new(entry);
                for repo in reader.lines() {
                    let repo = repo?;
                    debug!("Found repository in /etc/apk/repositories: {repo:?}");
                    self.register_repo(repo);
                }
            }
        }
        Ok(())
    }
}

pub fn calculate_checksum_for_apk(apk: &[u8]) -> Result<Vec<u8>> {
    // the first gzip has no end-of-stream marker, only read one file from tar
    let remaining = {
        let gz = GzDecoder::new(apk);
        let mut tar = tar::Archive::new(gz);
        tar.entries()?.next();
        tar.into_inner().into_inner()
    };

    // this is slightly chaotic, there's some over-read by GzDecoder that we need to correct
    let sig = apk.len() - remaining.len() + 8;

    // locate the start of the 3rd gzip stream
    let mut r = &apk[sig..];
    read_gzip_to_end(&mut r)?;
    let content = r.len();

    // cut at the location of the 2nd gzip stream
    let control_data = &apk[sig..(apk.len() - content)];

    let mut sha1 = Sha1::new();
    sha1.update(control_data);
    let sha1 = sha1.finalize();
    Ok(sha1.to_vec())
}

pub async fn detect_installed(container: &Container) -> Result<HashSet<String>> {
    let buf = container
        .exec(
            &["apk", "info", "-v"],
            container::Exec {
                capture_stdout: true,
                ..Default::default()
            },
        )
        .await?;
    let buf = String::from_utf8(buf).context("Failed to decode apk output as utf8")?;

    let installed = buf.lines().map(String::from).collect();
    Ok(installed)
}

pub async fn resolve_dependencies(
    container: &Container,
    manifest: &PackagesManifest,
    dependencies: &mut Vec<PackageLock>,
) -> Result<()> {
    info!("Syncing package datatabase...");
    container
        .exec(&["apk", "update"], container::Exec::default())
        .await?;

    let mut dbs = DatabaseCache::default();
    {
        // we only need these files briefly, declare them in a small scope so they get free'd early
        let repos = container.tar("/etc/apk/repositories").await?;
        dbs.init_repos_from_container(&repos)?;

        let tar = container.tar("/var/cache/apk").await?;
        dbs.import_from_container(&tar)?;
    }

    info!("Resolving dependencies...");
    let initial_packages = detect_installed(container).await?;

    // upgrade and install
    container
        .exec(&["apk", "upgrade"], container::Exec::default())
        .await?;

    let mut cmd = vec!["apk", "add", "--"];
    for dep in &manifest.dependencies {
        cmd.push(dep.as_str());
    }
    container.exec(&cmd, container::Exec::default()).await?;

    // detect dependencies
    let packages_afterwards = detect_installed(container).await?;
    let new_packages = packages_afterwards.difference(&initial_packages);

    info!("Calculating package checksums...");
    let client = http::Client::new()?;
    let alpine_cache_dir = paths::alpine_cache_dir()?;
    for pkg_identifier in new_packages {
        let pkg = dbs.get(pkg_identifier)?;
        debug!("Detected dependency: {pkg:?}");

        let url = format!(
            "{}/{}/{}-{}.apk",
            pkg.repo_url, pkg.arch, pkg.name, pkg.version
        );

        let sha256 = if let Some(sha256) = alpine_cache_dir.sha1_read_link(&pkg.checksum).await? {
            sha256
        } else {
            let mut buf = Vec::new();

            let mut response = client
                .request(&url)
                .await
                .with_context(|| anyhow!("Failed to download package from url: {:?}", url))?;

            let mut sha256 = Sha256::new();
            while let Some(chunk) = response
                .chunk()
                .await
                .context("Failed to read from download stream")?
            {
                buf.extend(&chunk);
                sha256.update(&chunk);
            }

            let sha256 = hex::encode(sha256.finalize());
            let sha1 = hex::encode(&calculate_checksum_for_apk(&buf)?);

            if sha1 != pkg.checksum {
                bail!("Downloaded package (checksum={sha1:?} does not match checksum in APKINDEX (checksum={:?})",
                    pkg.checksum
                );
            }

            let (sha1_path, sha256_path) =
                alpine_cache_dir.sha1_to_sha256(&pkg.checksum, &sha256)?;

            let parent = sha1_path
                .parent()
                .context("Failed to determine parent directory")?;
            fs::create_dir_all(parent).await.with_context(|| {
                anyhow!("Failed to create parent directories for file: {sha1_path:?}")
            })?;

            fs::symlink(sha256_path, sha1_path)
                .await
                .context("Failed to create sha1 symlink")?;

            sha256
        };

        dependencies.push(PackageLock {
            name: pkg.name.to_string(),
            version: pkg.version.to_string(),
            system: "alpine".to_string(),
            url,
            sha256,
            signature: None,
            installed: false,
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
    fn test_checksum_from_apk() -> Result<()> {
        let checksum = decode_apk_checksum("Q10cGs1h9J5440p6BRXhZC8FO7pVg=")?;
        let apk = &[
            0x1f, 0x8b, 0x08, 0x00, 0x00, 0x00, 0x00, 0x00, 0x02, 0x03, 0xd3, 0x0b, 0xf6, 0x74,
            0xf7, 0xd3, 0x0b, 0x0a, 0x76, 0xd4, 0x4b, 0xcc, 0x29, 0xc8, 0xcc, 0x4b, 0xd5, 0x4d,
            0x49, 0x2d, 0x4b, 0xcd, 0x71, 0xc8, 0xc9, 0x2c, 0x2e, 0x29, 0x86, 0x0a, 0xe5, 0x64,
            0xe6, 0x95, 0x56, 0xe8, 0xe5, 0x17, 0xa5, 0xeb, 0x9a, 0x19, 0x9a, 0x99, 0xa6, 0xa6,
            0x9a, 0x5a, 0xea, 0x15, 0x15, 0x27, 0xea, 0x15, 0x94, 0x26, 0x31, 0x10, 0x07, 0x0c,
            0x80, 0xc0, 0xcc, 0xc4, 0x04, 0x4c, 0x03, 0x01, 0x3a, 0x6d, 0x08, 0x62, 0x1b, 0x9a,
            0x98, 0x98, 0x99, 0x18, 0x9a, 0x99, 0x19, 0x9b, 0x03, 0xc5, 0x8d, 0x0c, 0x4c, 0x8c,
            0x8d, 0x19, 0x14, 0x0c, 0x18, 0xe8, 0x00, 0x4a, 0x8b, 0x4b, 0x12, 0x8b, 0x14, 0x14,
            0x18, 0x46, 0x28, 0xb0, 0x4e, 0xd3, 0xcb, 0x7b, 0x2e, 0x79, 0xed, 0xc7, 0x93, 0x73,
            0x9a, 0x61, 0xa7, 0xc5, 0x17, 0xa5, 0x86, 0xeb, 0x45, 0xee, 0x30, 0xad, 0xba, 0x72,
            0x73, 0xff, 0xdd, 0xee, 0xee, 0x87, 0xce, 0x57, 0xef, 0xd4, 0xa6, 0xcc, 0x48, 0x9b,
            0xc7, 0x27, 0x65, 0x7d, 0xf2, 0xc7, 0xdb, 0x07, 0xba, 0x1e, 0x57, 0xa7, 0x86, 0x49,
            0x57, 0xcd, 0xcf, 0x70, 0x55, 0xfb, 0xeb, 0x7e, 0x73, 0x55, 0x56, 0xca, 0x6d, 0x59,
            0x6f, 0x26, 0xfb, 0xeb, 0x11, 0x72, 0x5f, 0x65, 0xa3, 0xba, 0x22, 0x33, 0x0e, 0x14,
            0xbe, 0x4e, 0x3c, 0x68, 0xfd, 0x43, 0xed, 0x8c, 0x8b, 0x71, 0xf4, 0xaf, 0xb9, 0x0b,
            0xd7, 0x44, 0x95, 0x7a, 0x74, 0x1c, 0x9e, 0x39, 0x4b, 0x59, 0xfa, 0xf5, 0xa1, 0x3d,
            0xd3, 0x84, 0xf7, 0x5c, 0xcc, 0x95, 0x3d, 0xed, 0xfc, 0x61, 0x59, 0xe7, 0x99, 0x98,
            0x1d, 0x7c, 0x0f, 0xf5, 0xac, 0xa7, 0x9e, 0x35, 0x12, 0x79, 0xf2, 0x75, 0xd9, 0xe9,
            0xd9, 0x53, 0x1d, 0x2b, 0x2b, 0x4e, 0x4d, 0x8d, 0xd9, 0x59, 0x26, 0x53, 0xd6, 0x2b,
            0x7e, 0x62, 0xf5, 0xc1, 0xc9, 0x6a, 0x4f, 0xb3, 0xa7, 0x55, 0x5d, 0xdb, 0x39, 0xfb,
            0xbf, 0x6b, 0xb8, 0xf4, 0xa5, 0xa9, 0x0f, 0x66, 0x1c, 0x4c, 0x6b, 0xdb, 0x98, 0x90,
            0xeb, 0xbd, 0x82, 0x6d, 0x5e, 0x50, 0xd2, 0x85, 0x29, 0xcb, 0xe7, 0xcf, 0x79, 0x72,
            0x8b, 0x8f, 0x6d, 0xd9, 0x93, 0x03, 0xd6, 0x76, 0x47, 0xd9, 0x59, 0xd3, 0x74, 0x77,
            0x6d, 0x5d, 0x75, 0xf2, 0xda, 0x8b, 0xfd, 0x09, 0x52, 0xb7, 0xe2, 0x3b, 0x72, 0xe6,
            0x30, 0x9c, 0x5a, 0x58, 0xee, 0xe7, 0xd6, 0xf3, 0xf0, 0x81, 0xd8, 0x62, 0xf1, 0xaf,
            0xe2, 0x71, 0x79, 0xa7, 0x12, 0x4f, 0x37, 0xf6, 0x25, 0x16, 0xca, 0xd7, 0x64, 0xd7,
            0xd9, 0xfe, 0x5c, 0x73, 0xed, 0xca, 0x7c, 0x8d, 0x3f, 0x4f, 0xd6, 0x2d, 0xd8, 0x5d,
            0xe1, 0x96, 0x27, 0xda, 0x2c, 0xbb, 0xa8, 0x41, 0x2f, 0x77, 0x35, 0x83, 0xcf, 0xb7,
            0xad, 0x06, 0x0d, 0xc6, 0xab, 0xc2, 0x3a, 0x5f, 0xc5, 0x4a, 0xbb, 0x2f, 0xda, 0x55,
            0xba, 0x76, 0xcb, 0x22, 0xb5, 0x1f, 0x67, 0xdc, 0x2d, 0x66, 0xec, 0x9b, 0xe1, 0x16,
            0x70, 0xec, 0x43, 0xc2, 0xb2, 0xe7, 0xdb, 0x67, 0x71, 0xbf, 0xce, 0xd1, 0x3e, 0x70,
            0x9d, 0x8d, 0x6b, 0xfd, 0xd5, 0xfd, 0x6c, 0xa5, 0xad, 0xfe, 0x33, 0xbb, 0xd7, 0xfd,
            0x3b, 0x69, 0xa0, 0x53, 0x68, 0xdf, 0x7a, 0x6c, 0x86, 0xf8, 0xa1, 0xc2, 0xb8, 0xf9,
            0x05, 0xba, 0x47, 0xa7, 0xfc, 0x54, 0x3d, 0x75, 0xf5, 0xd2, 0xc9, 0xb5, 0xde, 0xc7,
            0x85, 0x67, 0x4c, 0x3b, 0xe2, 0x73, 0x38, 0xb5, 0xe5, 0xa5, 0x91, 0x4d, 0xc1, 0x19,
            0x81, 0x0d, 0xe7, 0xed, 0xf6, 0x26, 0x17, 0x2d, 0xbb, 0xab, 0x50, 0x18, 0x79, 0xf1,
            0x8a, 0x52, 0xed, 0xc2, 0xb2, 0x1b, 0xc6, 0x6b, 0xc5, 0x4f, 0xe6, 0x4d, 0xfe, 0x98,
            0xbe, 0xc1, 0xbf, 0x82, 0x6d, 0x3d, 0xf3, 0xc9, 0xe5, 0xcb, 0x0c, 0xac, 0xd9, 0x82,
            0x57, 0x46, 0x1c, 0x3c, 0x77, 0x9b, 0x5f, 0xe4, 0xb5, 0x28, 0xdb, 0x84, 0x05, 0xf3,
            0xbd, 0x67, 0xcd, 0xfb, 0xd7, 0x58, 0xee, 0x21, 0x60, 0x98, 0x9c, 0x6b, 0xc5, 0xf3,
            0x33, 0x20, 0x40, 0xf8, 0x25, 0x5f, 0x95, 0xfd, 0xca, 0x33, 0xa7, 0x76, 0xd9, 0x9d,
            0x89, 0x0f, 0xab, 0xc8, 0x6b, 0x3d, 0x55, 0xae, 0x7c, 0xe3, 0xd1, 0xbc, 0xb0, 0xfd,
            0xe6, 0x8b, 0xe6, 0x3e, 0xbc, 0xb5, 0x8f, 0x7f, 0x7b, 0xd8, 0x8a, 0x3b, 0xb7, 0x7c,
            0x6e, 0x4e, 0x37, 0xe1, 0x62, 0xfb, 0xf8, 0x8b, 0xe9, 0xd1, 0x9f, 0x2b, 0x8f, 0xca,
            0xa7, 0x7b, 0xca, 0xf7, 0x7c, 0x72, 0x78, 0xa0, 0xf7, 0x40, 0xbe, 0x67, 0x7e, 0x61,
            0xa9, 0xd2, 0xc2, 0x39, 0x6c, 0xdf, 0x4c, 0x66, 0xd5, 0x1c, 0x4c, 0xe3, 0x3f, 0x18,
            0xf6, 0xfb, 0x92, 0x9b, 0xfc, 0x5d, 0xcf, 0xe4, 0xd0, 0x69, 0x67, 0x1e, 0x2d, 0xdb,
            0x0c, 0x00, 0x91, 0x82, 0x74, 0x2c, 0x00, 0x04, 0x00, 0x00, 0x1f, 0x8b, 0x08, 0x00,
            0x00, 0x00, 0x00, 0x00, 0x02, 0x03, 0xed, 0x55, 0x6d, 0x6b, 0xdb, 0x30, 0x10, 0xce,
            0x67, 0xff, 0x0a, 0x41, 0x3f, 0xc7, 0x91, 0x64, 0x59, 0xb6, 0x43, 0x3b, 0xba, 0x15,
            0xd6, 0x95, 0xd1, 0xae, 0x1f, 0xba, 0x1f, 0x70, 0x96, 0xe5, 0x54, 0x44, 0x96, 0x8c,
            0x25, 0x97, 0xa4, 0xbf, 0x7e, 0xe7, 0x2c, 0xd0, 0x2d, 0x61, 0x6f, 0xb0, 0x15, 0xb6,
            0xee, 0x01, 0x73, 0xc7, 0x73, 0xa7, 0x7b, 0xd1, 0xf9, 0x50, 0xba, 0xb8, 0x85, 0xcd,
            0x3b, 0x0d, 0x8d, 0x1e, 0xc2, 0x22, 0xbd, 0x7d, 0x7f, 0x79, 0x75, 0xf3, 0xf6, 0xc3,
            0xec, 0xf7, 0x82, 0x22, 0xa4, 0x10, 0x3b, 0x89, 0x38, 0x94, 0x94, 0x72, 0x39, 0x63,
            0x42, 0x48, 0xc1, 0xa4, 0xcc, 0x0a, 0xe4, 0x19, 0x13, 0x54, 0xcc, 0xc8, 0x66, 0xf6,
            0x0c, 0x18, 0x43, 0x84, 0x01, 0x4b, 0x99, 0xbd, 0x4c, 0x30, 0x46, 0x54, 0x34, 0x9d,
            0x3e, 0xa3, 0x09, 0xaa, 0xb0, 0x57, 0x67, 0xff, 0xf1, 0x42, 0xf0, 0x87, 0x56, 0xfe,
            0x57, 0xf6, 0x1f, 0x57, 0x5f, 0x1c, 0xec, 0x3f, 0x67, 0x28, 0x08, 0x7d, 0xce, 0xfd,
            0x1f, 0xbc, 0x8f, 0xdf, 0xf3, 0xfb, 0x91, 0xfd, 0xb0, 0xb9, 0xbf, 0x04, 0x27, 0xe4,
            0x52, 0x3b, 0x3d, 0x40, 0xd4, 0x0d, 0xa9, 0xb7, 0x04, 0xea, 0xd1, 0xd8, 0x86, 0x64,
            0x29, 0x63, 0x29, 0x9b, 0x0f, 0x34, 0x39, 0x21, 0x63, 0x30, 0x6e, 0x45, 0x5a, 0x58,
            0xeb, 0xe9, 0x0a, 0xc8, 0x03, 0x3e, 0x14, 0xc6, 0x3b, 0xc2, 0xd2, 0x8c, 0xa1, 0xf5,
            0x1a, 0xd5, 0xd7, 0xe3, 0x8a, 0x90, 0x82, 0xb0, 0x6c, 0x49, 0xab, 0x25, 0xe3, 0xe4,
            0xe3, 0xdd, 0x05, 0xe1, 0x94, 0x67, 0x49, 0xbf, 0x5e, 0x39, 0xe8, 0x34, 0x39, 0x23,
            0x60, 0x7b, 0xe3, 0xf4, 0xbc, 0x86, 0xa0, 0x27, 0x16, 0x83, 0x20, 0x89, 0x59, 0xca,
            0x34, 0x9b, 0xb2, 0x20, 0xd5, 0xe8, 0xa0, 0x90, 0xbb, 0xd6, 0x11, 0x48, 0x0f, 0x6a,
            0x0d, 0x2b, 0x4d, 0x5a, 0x3f, 0x90, 0xce, 0x38, 0xd3, 0x81, 0xdd, 0x47, 0x20, 0xbb,
            0x08, 0xe3, 0x60, 0xd1, 0xf5, 0x3e, 0xc6, 0x3e, 0x2c, 0x17, 0x8b, 0xcf, 0x16, 0x6b,
            0xdc, 0xb8, 0x49, 0xfd, 0xb0, 0x4a, 0x76, 0x2d, 0x34, 0xd8, 0x11, 0xfa, 0x30, 0x59,
            0x31, 0xc1, 0xb2, 0x22, 0xe7, 0xc9, 0x3e, 0xe8, 0x94, 0xf8, 0xcd, 0xe4, 0xe1, 0x1f,
            0x51, 0x3f, 0xdd, 0x17, 0xd6, 0xe8, 0x07, 0x6d, 0xcf, 0xad, 0x09, 0x31, 0xa4, 0x07,
            0xf1, 0x5e, 0x25, 0xc1, 0x3c, 0x4e, 0xb1, 0x04, 0xad, 0x64, 0x02, 0x83, 0xba, 0x47,
            0xdd, 0xf9, 0x49, 0x49, 0xfc, 0x60, 0x56, 0xc6, 0x1d, 0xf4, 0xa7, 0x7c, 0xd7, 0x99,
            0x88, 0x64, 0x51, 0x8a, 0xac, 0xa5, 0xa5, 0x2c, 0xb3, 0xa6, 0x6d, 0xa9, 0xd2, 0xb2,
            0xc8, 0x2a, 0xa1, 0xeb, 0xb2, 0xd6, 0xbc, 0xcd, 0xcb, 0x36, 0xaf, 0x4a, 0x49, 0x25,
            0xab, 0xaa, 0xa4, 0x03, 0xe3, 0x22, 0x7e, 0xbb, 0xe2, 0x6e, 0x20, 0x82, 0x03, 0x6d,
            0xc9, 0x85, 0xef, 0x81, 0x9c, 0x3a, 0x85, 0xe2, 0xfc, 0xa8, 0x26, 0x6b, 0x94, 0x76,
            0x61, 0x2a, 0xeb, 0xfa, 0xea, 0x2e, 0x19, 0x74, 0x6f, 0x41, 0xe9, 0xf0, 0x75, 0x25,
            0x16, 0xb6, 0x7e, 0x8c, 0x49, 0xa3, 0x7b, 0xed, 0x9a, 0x9f, 0x32, 0x29, 0xef, 0xda,
            0x23, 0x72, 0xd0, 0x56, 0x4f, 0x7d, 0x3d, 0xf1, 0xfd, 0x7a, 0x1e, 0xbd, 0xb7, 0xe1,
            0x89, 0xaa, 0xc7, 0xb0, 0xad, 0xfd, 0xe6, 0x88, 0x98, 0x77, 0x78, 0xb3, 0x73, 0x8f,
            0xe4, 0xa0, 0x8e, 0x8d, 0xdf, 0xe2, 0xc3, 0x68, 0x9a, 0x27, 0xd6, 0x9a, 0x5a, 0xcd,
            0xc7, 0x68, 0xbe, 0xcc, 0xb7, 0x3f, 0x79, 0x42, 0x60, 0x8c, 0xbe, 0xc3, 0x67, 0x4b,
            0x81, 0xb5, 0x5b, 0xd2, 0xe8, 0xa8, 0x15, 0xfe, 0xca, 0xcb, 0x04, 0xe7, 0x0f, 0xf7,
            0x10, 0xa6, 0x59, 0xb5, 0x05, 0x15, 0x4d, 0x41, 0x33, 0xa8, 0x54, 0xc9, 0x4b, 0xc9,
            0x5b, 0x59, 0x32, 0x0a, 0x55, 0x55, 0x0b, 0xc9, 0xa5, 0xe0, 0x2c, 0x87, 0x8a, 0xe7,
            0x79, 0xce, 0x25, 0x17, 0x80, 0xd3, 0x51, 0x5c, 0x30, 0xa0, 0x54, 0x15, 0x80, 0x53,
            0xd3, 0xb5, 0xaa, 0xff, 0xe9, 0xd7, 0xf0, 0x13, 0xe6, 0x95, 0x32, 0x8e, 0x00, 0x0a,
            0x00, 0x00, 0x1f, 0x8b, 0x08, 0x00, 0x00, 0x00, 0x00, 0x00, 0x02, 0x03, 0xed, 0xd3,
            0x4d, 0x4a, 0x03, 0x31, 0x14, 0x07, 0xf0, 0xec, 0x8a, 0x05, 0x77, 0x2e, 0x5c, 0xce,
            0x05, 0x9c, 0xbe, 0x4c, 0x32, 0x99, 0xc9, 0xa2, 0x60, 0x77, 0x85, 0x0a, 0x2d, 0x54,
            0xb7, 0xc5, 0xcc, 0x24, 0x83, 0x52, 0x86, 0xc2, 0x7c, 0x40, 0xbd, 0x85, 0x1b, 0x4f,
            0xe0, 0x01, 0x3c, 0x82, 0xd7, 0x10, 0x0f, 0xa1, 0x57, 0x30, 0x15, 0x37, 0x76, 0xd1,
            0x22, 0xb4, 0x85, 0xe2, 0xff, 0xb7, 0x79, 0x2f, 0x1f, 0x8b, 0x17, 0xc8, 0x3f, 0xec,
            0x4d, 0xcc, 0x72, 0xe8, 0x8c, 0x75, 0x55, 0xdd, 0x0b, 0x6d, 0x5b, 0x96, 0x0f, 0x6c,
            0xd7, 0xc8, 0x53, 0x52, 0x7e, 0x57, 0x6f, 0xbd, 0x12, 0x17, 0x11, 0xe3, 0x52, 0x2a,
            0xc9, 0x95, 0x12, 0x89, 0xdf, 0xe7, 0x7e, 0x45, 0x2c, 0x58, 0xb2, 0x03, 0x68, 0xeb,
            0xc6, 0x54, 0x7e, 0x14, 0xf6, 0x3f, 0x71, 0x1e, 0xe4, 0xcd, 0x7d, 0xe9, 0xfa, 0xd4,
            0xf5, 0xad, 0xf9, 0x69, 0x55, 0x1a, 0x0c, 0x26, 0xa3, 0x8b, 0xeb, 0xf1, 0xf8, 0x6a,
            0x1a, 0xe6, 0x77, 0x2e, 0x9f, 0xd7, 0x6d, 0x19, 0x4e, 0x87, 0x03, 0xde, 0xb7, 0x46,
            0x68, 0x23, 0x9c, 0x8b, 0x9d, 0xca, 0x64, 0x46, 0x56, 0x44, 0x71, 0x9c, 0x15, 0xae,
            0xd0, 0xb1, 0x22, 0x9e, 0x6a, 0x32, 0x85, 0x4d, 0x29, 0x21, 0xdd, 0x65, 0x70, 0x04,
            0xf6, 0x14, 0xf9, 0x3f, 0xe5, 0x7f, 0xd5, 0xff, 0xce, 0x7f, 0x22, 0x05, 0x67, 0xc1,
            0x2a, 0x93, 0xfe, 0x1b, 0x46, 0x97, 0xec, 0x8c, 0xbd, 0xeb, 0xe7, 0x8f, 0xd9, 0x7c,
            0x74, 0x1a, 0xdd, 0xbc, 0x7e, 0x3e, 0xdd, 0x9e, 0x3f, 0xbe, 0xbc, 0x75, 0x4e, 0x76,
            0x9a, 0xff, 0x6a, 0xb1, 0x68, 0x36, 0xdd, 0xdb, 0x76, 0xbe, 0xfe, 0x38, 0x00, 0x00,
            0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
            0x80, 0x03, 0xf8, 0x02, 0xd7, 0x2b, 0xfd, 0xaf, 0x00, 0x28, 0x00, 0x00,
        ];

        let calculated = calculate_checksum_for_apk(&apk[..])?;
        assert_eq!(checksum, calculated);
        Ok(())
    }
}
