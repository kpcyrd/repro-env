use crate::args;
use crate::container::{self, Container};
use crate::errors::*;
use crate::fetch;
use crate::lockfile::PackageLock;
use crate::paths;
use crate::pgp;
use crate::pkgs::archlinux;
use data_encoding::BASE64;
use std::env;
use std::path::Path;
use std::time::Duration;
use tempfile::TempDir;
use time::format_description::well_known;
use time::OffsetDateTime;
use tokio::fs;

#[derive(Debug, PartialEq, Default)]
pub struct Install {
    alpine: Vec<(PackageLock, String)>,
    archlinux: Vec<(PackageLock, String)>,
    debian: Vec<(PackageLock, String)>,
}

impl Install {
    fn add_pkg(&mut self, pkg: PackageLock, filename: String) -> Result<()> {
        let list = match pkg.system.as_str() {
            "alpine" => &mut self.alpine,
            "archlinux" => &mut self.archlinux,
            "debian" => &mut self.debian,
            system => bail!("Unknown package system: {system:?}"),
        };
        list.push((pkg, filename));
        Ok(())
    }
}

pub async fn setup_extra_folder(path: &Path, dependencies: Vec<PackageLock>) -> Result<Install> {
    let pkgs_cache_dir = paths::pkgs_cache_dir()?;

    let mut install = Install::default();
    for package in dependencies {
        // determine filename
        let url = package
            .url
            .parse::<reqwest::Url>()
            .with_context(|| anyhow!("Failed to parse string as url: {:?}", package.url))?;
        let filename = url
            .path_segments()
            .context("Failed to get path from url")?
            .next_back()
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
            "alpine" => (),
            "archlinux" => {
                let base64 = package
                    .signature
                    .as_ref()
                    .context("Package in dependency lockfile is missing signature")?;
                let signature = BASE64
                    .decode(base64.as_bytes())
                    .with_context(|| anyhow!("Failed to decode signature as base64: {base64:?}"))?;

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
        fetch::verify_pin_metadata(&pkg, &package)
            .with_context(|| anyhow!("Failed to verify metadata for {filename:?}"))?;

        install.add_pkg(package, filename.to_string())?;
    }

    Ok(install)
}

pub async fn run_build(
    container: &Container,
    build: &args::Build,
    extra: Option<&(TempDir, Install)>,
) -> Result<()> {
    if let Some((_, install)) = extra {
        if !install.alpine.is_empty() {
            let mut cmd = vec![
                "apk".to_string(),
                "add".to_string(),
                "--no-network".to_string(),
                "--".to_string(),
            ];
            for (_, filename) in &install.alpine {
                cmd.push(format!("/extra/{filename}"));
            }

            info!("Installing dependencies...");
            container.exec(&cmd, container::Exec::default()).await?;
        }

        if !install.archlinux.is_empty() {
            // determine verification timestamp and add it to gpg.conf
            let filename_iter = install.archlinux.iter().map(|(pkg, _)| pkg);
            if let Some(time) = pgp::find_max_signature_time(filename_iter)? {
                let time = time
                    .checked_add(Duration::from_secs(1))
                    .with_context(|| anyhow!("Failed to increase time by 1 second {time:?}"))?;
                let datetime = OffsetDateTime::from(time).format(&well_known::Rfc3339)?;

                info!("Derived signature verification timestamp: {datetime:?}");
                archlinux::set_pacman_verification_datetime(container, time).await?;
            }

            // prepare and execute the install command
            let mut cmd = vec![
                "pacman".to_string(),
                "-U".to_string(),
                "--noconfirm".to_string(),
                "--".to_string(),
            ];
            for (_, filename) in &install.archlinux {
                cmd.push(format!("/extra/{filename}"));
            }

            info!("Installing dependencies...");
            container.exec(&cmd, container::Exec::default()).await?;
        }

        if !install.debian.is_empty() {
            let mut cmd = vec![
                "apt-get".to_string(),
                "install".to_string(),
                "--".to_string(),
            ];
            for (_, filename) in &install.debian {
                cmd.push(format!("/extra/{filename}"));
            }

            info!("Installing dependencies...");
            container.exec(&cmd, container::Exec::default()).await?;
        }
    }

    info!("Running build...");
    container
        .exec(
            &build.cmd,
            container::Exec {
                cwd: Some("/build"),
                env: &build.env,
                ..Default::default()
            },
        )
        .await?;

    Ok(())
}

pub async fn build(build: &args::Build) -> Result<()> {
    container::test_for_unprivileged_userns_clone().await?;

    // ensure arguments make sense
    build.validate()?;

    // load lockfile
    let (manifest, lockfile) = build.load_files().await?;
    if let Some(manifest) = &manifest {
        if let Err(err) = manifest.satisfied_by(&lockfile) {
            warn!("Lockfile might be out-of-sync: {err:#}");
        }
    }

    // mount current directory into container
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
        fetch::download_dependencies(&dependencies).await?;

        let path = paths::repro_env_dir()?;
        let temp_dir = tempfile::Builder::new().prefix("env.").tempdir_in(path)?;
        let pkgs = setup_extra_folder(temp_dir.path(), dependencies).await?;

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
