use crate::args;
use crate::container;
use crate::container::ImageRef;
use crate::errors::*;
use crate::lockfile::ContainerLock;
use crate::manifest::Manifest;

pub async fn resolve(args: &args::Update, manifest: &Manifest) -> Result<ContainerLock> {
    let image = manifest.container.image.clone();

    if !args.no_pull {
        container::pull(&image).await?;
    }
    let resolved = container::inspect(&image).await?;
    let digest = &resolved.digest;
    let mut image_ref = image.parse::<ImageRef>()?;
    image_ref.tag = None;
    image_ref.digest = Some(digest.to_string());
    let pinned_image = image_ref.to_string();
    info!("Resolved image reference {:?} to {:?}", image, pinned_image);

    Ok(ContainerLock {
        image: pinned_image,
    })
}
