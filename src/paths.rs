use crate::errors::*;
use std::env;
use std::io::ErrorKind;
use std::path::{Component, Path, PathBuf};
use tokio::fs;

static SHARD_SIZE: usize = 2;

pub fn repro_env_dir() -> Result<PathBuf> {
    if let Some(path) = env::var_os("REPRO_ENV_HOME") {
        Ok(path.into())
    } else {
        let mut cache = dirs::cache_dir().context("Failed to detect cache directory")?;
        cache.push("repro-env");
        Ok(cache)
    }
}

pub fn cache_dir() -> Result<PathBuf> {
    if let Some(path) = env::var_os("REPRO_ENV_CACHE") {
        Ok(path.into())
    } else {
        repro_env_dir()
    }
}

pub fn pkgs_cache_dir() -> Result<PkgsCacheDir> {
    let mut path = cache_dir()?;
    path.push("pkgs");
    Ok(PkgsCacheDir { path })
}

pub fn alpine_cache_dir() -> Result<PkgsCacheDir> {
    let mut path = cache_dir()?;
    path.push("alpine");
    Ok(PkgsCacheDir { path })
}

#[derive(Debug)]
pub struct PkgsCacheDir {
    path: PathBuf,
}

impl PkgsCacheDir {
    fn shard<'a>(hash: &'a str, algo: &'static str, len: usize) -> Result<(&'a str, &'a str)> {
        if hash.len() != len {
            bail!("Unexpected {algo} checksum length: {:?}", hash.len());
        }
        if !hash.chars().all(char::is_alphanumeric) {
            bail!("Unexpected characters in {algo}: {hash:?}");
        }

        let shard = &hash[..SHARD_SIZE];
        let suffix = &hash[SHARD_SIZE..];
        Ok((shard, suffix))
    }

    fn shard_sha256(sha256: &str) -> Result<(&str, &str)> {
        Self::shard(sha256, "sha256", 64)
    }

    fn shard_sha1(sha1: &str) -> Result<(&str, &str)> {
        Self::shard(sha1, "sha1", 40)
    }

    pub fn sha256_path(&self, sha256: &str) -> Result<PathBuf> {
        let (shard, suffix) = Self::shard_sha256(sha256)?;

        let mut path = self.path.clone();
        path.push(shard);
        path.push(suffix);

        Ok(path)
    }

    fn sha1_path(&self, sha1: &str) -> Result<PathBuf> {
        let (shard, suffix) = Self::shard_sha1(sha1)?;

        let mut path = self.path.clone();
        path.push(shard);
        path.push(suffix);

        Ok(path)
    }

    pub async fn sha1_read_link(&self, sha1: &str) -> Result<Option<String>> {
        let path = self.sha1_path(sha1)?;
        match fs::read_link(&path).await {
            Ok(path) => {
                trace!("Found symlink in cache: {path:?}");
                let sha256 = Self::link_to_sha256(&path)?;
                Ok(Some(sha256))
            }
            Err(err) if err.kind() == ErrorKind::NotFound => {
                trace!("Did not find symlink in cache: {path:?}");
                Ok(None)
            }
            Err(err) => Err(err.into()),
        }
    }

    pub fn sha1_to_sha256(&self, sha1: &str, sha256: &str) -> Result<(PathBuf, PathBuf)> {
        let sha1_path = self.sha1_path(sha1)?;
        let (shard, suffix) = Self::shard_sha256(sha256)?;

        let mut sha256_path = PathBuf::from("../../pkgs");
        sha256_path.push(shard);
        sha256_path.push(suffix);

        Ok((sha1_path, sha256_path))
    }

    fn link_to_sha256(path: &Path) -> Result<String> {
        let mut components = path.components().rev();

        let tail = components.next().context("Link is missing filename")?;
        let shard = components.next().context("Link is missing shard")?;

        let tail = Self::component_to_name(&tail)?;
        let shard = Self::component_to_name(&shard)?;

        Ok(format!("{shard}{tail}"))
    }

    fn component_to_name<'a>(comp: &'a Component) -> Result<&'a str> {
        let Component::Normal(comp) = comp else {
            bail!("Component has reserved name")
        };
        let Some(comp) = comp.to_str() else {
            bail!("Component is invalid utf8")
        };
        Ok(comp)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn test_sha256_path() {
        let dir = PkgsCacheDir {
            path: PathBuf::from("/cache"),
        };
        assert!(dir.sha256_path("").is_err());
        assert!(dir.sha256_path("ffff").is_err());
        assert!(dir
            .sha256_path("////////////////////////////////////////////////////////////////")
            .is_err());

        let path = dir
            .sha256_path("ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff")
            .unwrap();
        assert_eq!(
            path,
            Path::new("/cache/ff/ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff")
        );
    }

    #[test]
    fn test_sha1_read_link() -> Result<()> {
        let path = PkgsCacheDir::link_to_sha256(Path::new(
            "../../../pkgs/ff/7951b5950a3a0319e86988041db4438b31a6ee4c7a36c64bd6c0c4607e40c9",
        ))?;
        assert_eq!(
            path,
            "ff7951b5950a3a0319e86988041db4438b31a6ee4c7a36c64bd6c0c4607e40c9"
        );
        Ok(())
    }

    #[test]
    fn test_sha1_to_sha256() -> Result<()> {
        let dir = PkgsCacheDir {
            path: PathBuf::from("/cache"),
        };

        let (sha1, sha256) = dir.sha1_to_sha256(
            "83d8ab27f4fd4725a147245f89d076aa96b52262",
            "ff7951b5950a3a0319e86988041db4438b31a6ee4c7a36c64bd6c0c4607e40c9",
        )?;
        assert_eq!(
            sha1,
            Path::new("/cache/83/d8ab27f4fd4725a147245f89d076aa96b52262")
        );
        assert_eq!(
            sha256,
            Path::new(
                "../../pkgs/ff/7951b5950a3a0319e86988041db4438b31a6ee4c7a36c64bd6c0c4607e40c9"
            )
        );

        Ok(())
    }
}
