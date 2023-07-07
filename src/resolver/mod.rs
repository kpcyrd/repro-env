pub mod archlinux;
pub mod container;
pub mod debian;

use crate::args;
use crate::errors::*;
use crate::lockfile::Lockfile;
use crate::manifest::Manifest;

pub async fn resolve(args: &args::Update, manifest: &Manifest) -> Result<Lockfile> {
    let container = container::resolve(args, manifest).await?;

    let mut dependencies = Vec::new();
    if let Some(packages) = &manifest.packages {
        match packages.system.as_str() {
            "archlinux" => {
                archlinux::resolve(args, packages, &container, &mut dependencies).await?
            }
            "debian" => debian::resolve(args, packages, &container, &mut dependencies).await?,
            system => bail!("Unknown package system: {system:?}"),
        }
    }

    dependencies.sort_by(|a, b| {
        a.name
            .cmp(&b.name)
            .then(a.version.cmp(&b.version))
            .then(a.system.cmp(&b.system))
    });

    Ok(Lockfile {
        container,
        packages: dependencies,
    })
}
