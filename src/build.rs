use crate::args;
use crate::container::{self, Container};
use crate::errors::*;
use crate::http;
use crate::lockfile::{Lockfile, PackageLock};
use crate::paths;
use crate::pkgs;
use data_encoding::BASE64;
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::env;
use std::path::Path;
use tempfile::TempDir;
use tokio::fs;
use tokio::io::{AsyncSeekExt, AsyncWriteExt};

async fn download_dependencies(dependencies: &[PackageLock]) -> Result<()> {
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

pub async fn setup_extra_folder(
    path: &Path,
    dependencies: &[PackageLock],
) -> Result<HashMap<String, Vec<String>>> {
    let pkgs_cache_dir = paths::pkgs_cache_dir()?;

    let mut install = HashMap::<_, Vec<_>>::new();
    for package in dependencies {
        // determine filename
        let url = package
            .url
            .parse::<reqwest::Url>()
            .with_context(|| anyhow!("Failed to parse string as url: {:?}", package.url))?;
        let filename = url
            .path_segments()
            .context("Failed to get path from url")?
            .last()
            .context("Failed to find filename from url")?;
        if filename.is_empty() {
            bail!("Filename from url is empty");
        }

        // setup /extra/ directory
        let source = pkgs_cache_dir.sha256_path(&package.sha256)?;
        let dest = path.join(filename);
        let dest_sig = path.join(filename.to_owned() + ".sig");

        debug!("Trying to reflink {source:?} -> {dest:?}...");
        if let Err(err) = clone_file::clone_file(&source, &dest) {
            debug!("Failed to reflink, trying traditional copy: {err:#}");
            fs::copy(&source, &dest)
                .await
                .context("Failed to copy package from cache to temporary folder")?;
        }

        // setup extra data
        match package.system.as_str() {
            "archlinux" => {
                let signature = package
                    .signature
                    .as_ref()
                    .context("Package in dependency lockfile is missing signature")?;
                let signature = BASE64
                    .decode(signature.as_bytes())
                    .context("Failed to decode signature as base64")?;
                debug!(
                    "Writing signature ({} bytes) to {dest_sig:?}...",
                    signature.len()
                );
                fs::write(dest_sig, signature).await?;
            }
            "debian" => (),
            system => bail!("Unknown package system: {system:?}"),
        }

        // verify pkg content matches pin metadata
        let pkg = fs::read(&dest).await?;
        verify_pin_metadata(&pkg, package)
            .with_context(|| anyhow!("Failed to verify metadata for {filename:?}"))?;

        install
            .entry(package.system.clone())
            .or_default()
            .push(filename.to_string());
    }

    Ok(install)
}

pub async fn run_build(
    container: &Container,
    build: &args::Build,
    extra: Option<&(TempDir, HashMap<String, Vec<String>>)>,
) -> Result<()> {
    if let Some((_, pkgs)) = extra {
        for (system, pkgs) in pkgs {
            match system.as_str() {
                "archlinux" => {
                    let mut cmd = vec![
                        "pacman".to_string(),
                        "-U".to_string(),
                        "--noconfirm".to_string(),
                        "--".to_string(),
                    ];
                    for pkg in pkgs {
                        cmd.push(format!("/extra/{pkg}"));
                    }

                    info!("Installing dependencies...");
                    container.exec(&cmd, container::Exec::default()).await?;
                }
                "debian" => {
                    let mut cmd = vec![
                        "apt-get".to_string(),
                        "install".to_string(),
                        "--".to_string(),
                    ];
                    for pkg in pkgs {
                        cmd.push(format!("/extra/{pkg}"));
                    }

                    info!("Installing dependencies...");
                    container.exec(&cmd, container::Exec::default()).await?;
                }
                system => bail!("Unknown package system: {system:?}"),
            }
        }
    }

    info!("Running build...");
    container
        .exec(
            &build.cmd,
            container::Exec {
                cwd: Some("/build"),
                ..Default::default()
            },
        )
        .await?;

    Ok(())
}

pub async fn build(build: &args::Build) -> Result<()> {
    container::test_for_unprivileged_userns_clone().await?;

    let path = build.file.as_deref().unwrap_or(Path::new("repro-env.lock"));

    let buf = fs::read_to_string(path)
        .await
        .with_context(|| anyhow!("Failed to read dependency lockfile: {path:?}"))?;

    let lockfile = Lockfile::deserialize(&buf)?;
    trace!("Loaded dependency lockfile from file: {lockfile:?}");

    let pwd = env::current_dir()?;
    let pwd = pwd
        .into_os_string()
        .into_string()
        .map_err(|_| anyhow!("Failed to convert current path to utf-8"))?;

    let mut mounts = vec![(pwd, "/build".to_string())];

    // ignore packages that are already present in the container
    let dependencies = lockfile
        .packages
        .into_iter()
        .filter(|p| !p.installed)
        .collect::<Vec<_>>();

    let extra = if !dependencies.is_empty() {
        download_dependencies(&dependencies).await?;

        let path = paths::repro_env_dir()?;
        let temp_dir = tempfile::Builder::new().prefix("env.").tempdir_in(path)?;
        let pkgs = setup_extra_folder(temp_dir.path(), &dependencies).await?;

        let path = temp_dir
            .path()
            .to_owned()
            .into_os_string()
            .into_string()
            .map_err(|_| anyhow!("Failed to convert temporary path to utf-8"))?;
        mounts.push((path, "/extra".to_string()));

        Some((temp_dir, pkgs))
    } else {
        None
    };

    let container = Container::create(
        &lockfile.container.image,
        container::Config {
            mounts: &mounts,
            expose_fuse: false,
        },
    )
    .await?;
    container
        .run(run_build(&container, build, extra.as_ref()), build.keep)
        .await
}
