use crate::errors::*;
use std::path::PathBuf;

static SHARD_SIZE: usize = 2;

pub fn repro_env_dir() -> Result<PathBuf> {
    let mut cache = dirs::cache_dir().context("Failed to detect cache directory")?;
    cache.push("repro-env");
    Ok(cache)
}

pub fn pkgs_cache_dir() -> Result<PkgsCacheDir> {
    let mut path = repro_env_dir()?;
    path.push("pkgs");
    Ok(PkgsCacheDir { path })
}

#[derive(Debug)]
pub struct PkgsCacheDir {
    path: PathBuf,
}

impl PkgsCacheDir {
    pub fn sha256_path(&self, sha256: &str) -> Result<PathBuf> {
        if sha256.len() != 64 {
            bail!("Unexpected sha256 checksum length: {:?}", sha256.len());
        }
        if !sha256.chars().all(char::is_alphanumeric) {
            bail!("Unexpected characters in sha256: {sha256:?}");
        }

        let mut path = self.path.clone();

        let shard = &sha256[..SHARD_SIZE];
        path.push(shard);
        let suffix = &sha256[SHARD_SIZE..];
        path.push(suffix);

        Ok(path)
    }
}
