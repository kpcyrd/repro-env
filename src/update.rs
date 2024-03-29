use crate::args;
use crate::container;
use crate::errors::*;
use crate::manifest::Manifest;
use crate::resolver;
use std::path::Path;
use tokio::fs;

pub async fn update(update: &args::Update) -> Result<()> {
    container::test_for_unprivileged_userns_clone().await?;

    let manifest_path = Path::new("repro-env.toml");
    let lockfile_path = Path::new("repro-env.lock");

    let manifest = Manifest::read_from_file(manifest_path).await?;

    let lockfile = resolver::resolve(update, &manifest).await?;
    trace!("Resolved manifest into lockfile: {lockfile:?}");

    debug!("Updating dependency lockfile: {lockfile_path:?}");
    let buf = lockfile.serialize()?;
    fs::write(lockfile_path, buf).await?;

    Ok(())
}
