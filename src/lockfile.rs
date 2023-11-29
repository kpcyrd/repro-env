use crate::errors::*;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Lockfile {
    pub container: ContainerLock,
    #[serde(default, rename = "package", skip_serializing_if = "Vec::is_empty")]
    pub packages: Vec<PackageLock>,
}

impl Lockfile {
    pub fn deserialize(buf: &str) -> Result<Self> {
        let lockfile = toml::from_str(buf)?;
        Ok(lockfile)
    }

    pub fn serialize(&self) -> Result<String> {
        let toml = toml::to_string_pretty(self)?;
        Ok(toml)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ContainerLock {
    pub image: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PackageLock {
    pub name: String,
    pub version: String,
    pub system: String,
    pub url: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub provides: Vec<String>,
    pub sha256: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub signature: Option<String>,
    /// If true, this package is already present in the container and does not
    /// need to be installed. It's only in the lockfile to make the
    /// repro-env.lock diff easier to read and help git's delta-compression.
    #[serde(default, skip_serializing_if = "is_false")]
    pub installed: bool,
}

fn is_false(value: &bool) -> bool {
    !value
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    pub fn test_serialize_archlinux() -> Result<()> {
        let lockfile = Lockfile {
            container: ContainerLock {
                image:
                    "docker.io/library/archlinux@sha256:6568d3f1f278827a4a7d8537f80c2ae36982829a0c6bccff4cec081774025472"
                        .to_string(),
            },
            packages: vec![
                PackageLock {
                    name: "archlinux-keyring".to_string(),
                    version: "20230704-1".to_string(),
                    system: "archlinux".to_string(),
                    url: "https://archive.archlinux.org/packages/a/archlinux-keyring/archlinux-keyring-20230704-1-any.pkg.tar.zst".to_string(),
                    provides: vec![],
                    sha256: "6a3d2acaa396c4bd72fe3f61a3256d881e3fc2cf326113cf331f168e36dd9a3c".to_string(),
                    signature: Some(
"iHUEABYIAB0WIQQEKYl95fO9rFN6MGltQr3RFuAGjwUCZKPPXgAKCRBtQr3RFuAGj9oXAP94RQ1sKD53/RxVYlVEEOjKHvOmrWvDkt1veMYygnlnIgD+MLg/TT6d71kE8F08+JH+EcnG7wQow5Xr/qBo1VPLdgQ=".to_string()),
                    installed: false,
                },
                PackageLock {
                    name: "binutils".to_string(),
                    version: "2.40-6".to_string(),
                    system: "archlinux".to_string(),
                    url: "https://archive.archlinux.org/packages/b/binutils/binutils-2.40-6-x86_64.pkg.tar.zst".to_string(),
                    provides: vec![],
                    sha256: "b65fd16001578e10b602e577a8031cbfffc1164caf47ed9ba00c60d804519430".to_string(),
                    signature: Some(
"iNUEABYKAH0WIQQFx3danouXdAf+COadTFqhVCbaCgUCZG6Rg18UgAAAAAAuAChpc3N1ZXItZnByQG5vdGF0aW9ucy5vcGVucGdwLmZpZnRoaG9yc2VtYW4ubmV0MDVDNzc3NUE5RThCOTc3NDA3RkUwOEU2OUQ0QzVBQTE1NDI2REEwQQAKCRCdTFqhVCbaCge2AQD/LGBeHRaeO8xh4E/bAYfqd1O/OFqk2DrQBJ73cdKl2gD9EC8p4U/cXQK8V774m6LSS50usH5pxcQWEq/H0SF+FgM=".to_string()),
                    installed: false,
                }
            ],
        };

        let toml = lockfile.serialize()?;

        assert_eq!(
            toml,
            r#"[container]
image = "docker.io/library/archlinux@sha256:6568d3f1f278827a4a7d8537f80c2ae36982829a0c6bccff4cec081774025472"

[[package]]
name = "archlinux-keyring"
version = "20230704-1"
system = "archlinux"
url = "https://archive.archlinux.org/packages/a/archlinux-keyring/archlinux-keyring-20230704-1-any.pkg.tar.zst"
sha256 = "6a3d2acaa396c4bd72fe3f61a3256d881e3fc2cf326113cf331f168e36dd9a3c"
signature = "iHUEABYIAB0WIQQEKYl95fO9rFN6MGltQr3RFuAGjwUCZKPPXgAKCRBtQr3RFuAGj9oXAP94RQ1sKD53/RxVYlVEEOjKHvOmrWvDkt1veMYygnlnIgD+MLg/TT6d71kE8F08+JH+EcnG7wQow5Xr/qBo1VPLdgQ="

[[package]]
name = "binutils"
version = "2.40-6"
system = "archlinux"
url = "https://archive.archlinux.org/packages/b/binutils/binutils-2.40-6-x86_64.pkg.tar.zst"
sha256 = "b65fd16001578e10b602e577a8031cbfffc1164caf47ed9ba00c60d804519430"
signature = "iNUEABYKAH0WIQQFx3danouXdAf+COadTFqhVCbaCgUCZG6Rg18UgAAAAAAuAChpc3N1ZXItZnByQG5vdGF0aW9ucy5vcGVucGdwLmZpZnRoaG9yc2VtYW4ubmV0MDVDNzc3NUE5RThCOTc3NDA3RkUwOEU2OUQ0QzVBQTE1NDI2REEwQQAKCRCdTFqhVCbaCge2AQD/LGBeHRaeO8xh4E/bAYfqd1O/OFqk2DrQBJ73cdKl2gD9EC8p4U/cXQK8V774m6LSS50usH5pxcQWEq/H0SF+FgM="
"#
        );

        let deserialized = Lockfile::deserialize(&toml)?;
        assert_eq!(deserialized, lockfile);

        Ok(())
    }

    #[test]
    pub fn test_serialize_debian() -> Result<()> {
        let lockfile = Lockfile {
            container: ContainerLock {
                image:
                    "debian@sha256:3d868b5eb908155f3784317b3dda2941df87bbbbaa4608f84881de66d9bb297b"
                        .to_string(),
            },
            packages: vec![
                PackageLock {
                    name: "binutils".to_string(),
                    version: "2.40-2".to_string(),
                    system: "debian".to_string(),
                    url: "https://snapshot.debian.org/archive/debian/20230115T211934Z/pool/main/b/binutils/binutils_2.40-2_amd64.deb".to_string(),
                    provides: vec![],
                    sha256: "83c3e20b53e1fbd84d764c3ba27d26a0376e361ae5d7fb37120196934dd87424".to_string(),
                    signature: None,
                    installed: false,
                },
                PackageLock {
                    name: "binutils-common".to_string(),
                    version: "2.40-2".to_string(),
                    system: "debian".to_string(),
                    url: "https://snapshot.debian.org/archive/debian/20230115T211934Z/pool/main/b/binutils/binutils-common_2.40-2_amd64.deb".to_string(),
                    provides: vec![],
                    sha256: "ab314134f43a0891a48f69a9bc33d825da748fa5e0ba2bebb7a5c491b026f1a0".to_string(),
                    signature: None,
                    installed: false,
                }
            ],
        };

        let toml = lockfile.serialize()?;

        assert_eq!(
            toml,
            r#"[container]
image = "debian@sha256:3d868b5eb908155f3784317b3dda2941df87bbbbaa4608f84881de66d9bb297b"

[[package]]
name = "binutils"
version = "2.40-2"
system = "debian"
url = "https://snapshot.debian.org/archive/debian/20230115T211934Z/pool/main/b/binutils/binutils_2.40-2_amd64.deb"
sha256 = "83c3e20b53e1fbd84d764c3ba27d26a0376e361ae5d7fb37120196934dd87424"

[[package]]
name = "binutils-common"
version = "2.40-2"
system = "debian"
url = "https://snapshot.debian.org/archive/debian/20230115T211934Z/pool/main/b/binutils/binutils-common_2.40-2_amd64.deb"
sha256 = "ab314134f43a0891a48f69a9bc33d825da748fa5e0ba2bebb7a5c491b026f1a0"
"#
        );

        let deserialized = Lockfile::deserialize(&toml)?;
        assert_eq!(deserialized, lockfile);

        Ok(())
    }
}
