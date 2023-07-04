use crate::args;
use crate::container;
use crate::container::ImageRef;
use crate::errors::*;
use crate::lockfile::{ContainerLockfile, Lockfile};
use crate::manifest::Manifest;

pub async fn resolve(args: &args::Update, manifest: &Manifest) -> Result<Lockfile> {
    let image = manifest.container.image.clone();

    if !args.no_pull {
        container::pull(&image).await?;
    }
    let images = container::inspect(&image).await?;
    if images.len() != 1 {
        bail!(
            "The specified image is not canonical, inspect returned {}, expected 1",
            images.len()
        );
    }
    let digest = &images[0].digest;
    let mut image_ref = image.parse::<ImageRef>()?;
    image_ref.tag = None;
    image_ref.digest = Some(digest.to_string());
    let pinned_image = image_ref.to_string();
    info!("Resolved image reference {:?} to {:?}", image, pinned_image);

    Ok(Lockfile {
        container: ContainerLockfile {
            image: pinned_image,
        },
    })
}
