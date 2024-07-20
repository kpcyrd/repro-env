use crate::args;
use crate::container;
use crate::errors::*;
use crate::http;
use crate::lockfile::{Lockfile, PackageLock};
use crate::paths;
use crate::pkgs;
use sha2::{Digest, Sha256};
use std::path::Path;
use tokio::fs;
use tokio::io::{AsyncSeekExt, AsyncWriteExt};

pub async fn download_dependencies(dependencies: &[PackageLock]) -> Result<()> {
    let client = http::Client::new()?;
    let pkgs_cache_dir = paths::pkgs_cache_dir()?;

    for package in dependencies {
        trace!("Found dependencies: {package:?}");
        let path = pkgs_cache_dir.sha256_path(&package.sha256)?;
        if path.exists() {
            debug!(
                "Package already in cache: {:?} {:?}",
                package.name, package.version
            );
        } else {
            let parent = path
                .parent()
                .context("Failed to determine parent directory")?;
            fs::create_dir_all(parent).await.with_context(|| {
                anyhow!("Failed to create parent directories for file: {path:?}")
            })?;

            let mut dl_path = path.clone();
            dl_path.as_mut_os_string().push(".tmp");

            let file = fs::OpenOptions::new()
                .write(true)
                .create(true)
                .truncate(false)
                .open(&dl_path)
                .await?;

            let mut lock = fd_lock::RwLock::new(file);
            debug!("Trying to acquire write lock for file: {path:?}");
            let mut lock = lock
                .write()
                .with_context(|| anyhow!("Failed to acquire lock for {dl_path:?}"))?;

            // check if file became available in meantime
            if path.exists() {
                debug!("File became available in the meantime, nothing to do");
            } else {
                debug!(
                    "Downloading package into cache: {:?} {:?}",
                    package.name, package.version
                );
                lock.set_len(0).await.context("Failed to truncate file")?;
                lock.rewind()
                    .await
                    .context("Failed to rewind file to beginning")?;

                let mut response = client.request(&package.url).await.with_context(|| {
                    anyhow!("Failed to download package from url: {:?}", package.url)
                })?;

                let mut hasher = Sha256::new();
                while let Some(chunk) = response
                    .chunk()
                    .await
                    .context("Failed to read from download stream")?
                {
                    lock.write_all(&chunk)
                        .await
                        .context("Failed to write to downloaded data to disk")?;
                    hasher.update(&chunk);
                }
                let result = hex::encode(hasher.finalize());

                if package.sha256 != result {
                    lock.set_len(0)
                        .await
                        .context("Mismatch of sha256, failed to truncate file")?;
                    bail!(
                        "Mismatch of sha256, expected={:?}, downloaded={:?}",
                        package.sha256,
                        result
                    );
                }

                lock.sync_all()
                    .await
                    .context("Failed to sync downloaded data to disk")?;
                fs::rename(&dl_path, &path)
                    .await
                    .with_context(|| anyhow!("Failed to rename {dl_path:?} to {path:?}"))?;
            }
        }
    }

    Ok(())
}

pub fn verify_pin_metadata(pkg: &[u8], pin: &PackageLock) -> Result<()> {
    let pkg = match pin.system.as_str() {
        "alpine" => pkgs::alpine::parse(pkg).context("Failed to parse data as alpine package")?,
        "archlinux" => {
            pkgs::archlinux::parse(pkg).context("Failed to parse data as archlinux package")?
        }
        "debian" => pkgs::debian::parse(pkg).context("Failed to parse data as debian package")?,
        system => bail!("Unknown package system: {system:?}"),
    };

    debug!("Parsed embedded metadata from package: {pkg:?}");

    if pin.name != pkg.name {
        bail!(
            "Package name in metadata doesn't match lockfile: expected={:?}, embedded={:?}",
            pin.name,
            pkg.name
        );
    }

    if pin.version != pkg.version {
        bail!(
            "Package version in metadata doesn't match lockfile: expected={:?}, embedded={:?}",
            pin.version,
            pkg.version
        );
    }

    Ok(())
}

pub async fn fetch(fetch: &args::Fetch) -> Result<()> {
    // load lockfile
    let path = fetch.file.as_deref().unwrap_or(Path::new("repro-env.lock"));
    let buf = fs::read_to_string(path)
        .await
        .with_context(|| anyhow!("Failed to read dependency lockfile: {path:?}"))?;

    let lockfile = Lockfile::deserialize(&buf)?;
    trace!("Loaded dependency lockfile from file: {lockfile:?}");

    if !fetch.no_pull {
        let image = &lockfile.container.image;
        if let Err(err) = container::inspect(image).await {
            debug!("Could not find image in cache: {err:#}");
            container::pull(image).await?;
        } else {
            info!("Found container image in local cache: {image:?}");
        }
    }

    // ignore packages that are already present in the container
    let dependencies = lockfile
        .packages
        .into_iter()
        .filter(|p| !p.installed)
        .collect::<Vec<_>>();

    if !dependencies.is_empty() {
        download_dependencies(&dependencies).await?;
    }

    Ok(())
}
