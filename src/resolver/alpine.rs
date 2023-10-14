use crate::args;
use crate::container::{self, Container};
use crate::errors::*;
use crate::http;
use crate::lockfile::{ContainerLock, PackageLock};
use crate::manifest::PackagesManifest;
use crate::paths;
use crate::utils;
use data_encoding::BASE64;
use flate2::bufread::GzDecoder;
use sha1::Sha1;
use sha2::{Digest, Sha256};
use std::collections::{HashMap, HashSet};
use std::io::{BufRead, BufReader, Read};
use std::rc::Rc;
use tokio::fs;

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
        utils::read_gzip_to_end(&mut r).context("Failed to strip signature")?;

        let gz = GzDecoder::new(r);
        let mut tar = tar::Archive::new(gz);

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
    utils::read_gzip_to_end(&mut r)?;
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
        let calculated = calculate_checksum_for_apk(crate::test_data::ALPINE_APK_EXAMPLE)?;
        assert_eq!(checksum, calculated);
        Ok(())
    }
}
