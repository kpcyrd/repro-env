use crate::args;
use crate::container::{self, Container};
use crate::errors::*;
use crate::lockfile::{ContainerLock, PackageLock};
use crate::manifest::PackagesManifest;
use flate2::read::GzDecoder;
use std::collections::{HashMap, HashSet};
use std::io::Read;
use tokio::signal;

#[derive(Debug, Default, PartialEq)]
pub struct Package {
    pub values: HashMap<String, Vec<String>>,
}

impl Package {
    pub fn parse(buf: &str) -> Result<Self> {
        let mut pkg = Self::default();

        let mut lines = buf.lines();
        while let Some(section) = lines.next() {
            let mut values = Vec::new();
            for line in &mut lines {
                if line.is_empty() {
                    break;
                }
                values.push(line.to_string());
            }
            pkg.values.insert(section.to_string(), values);
        }

        Ok(pkg)
    }

    pub fn add_values(&mut self, key: &str, values: &[&str]) {
        let values = values.iter().map(|s| s.to_string()).collect();
        self.values.insert(key.to_string(), values);
    }

    pub fn single_value(&self, key: &str) -> Result<&str> {
        let values = self
            .values
            .get(key)
            .with_context(|| anyhow!("Failed to find key in package metadata: {key:?}"))?;
        let mut values = values.iter();

        let value = values
            .next()
            .with_context(|| anyhow!("No value available for {key:?}"))?;

        if let Some(trailing) = values.next() {
            bail!("Unexpected trailing value in {key:?}: {trailing:?}");
        }

        Ok(value)
    }

    pub fn name(&self) -> Result<&str> {
        self.single_value("%NAME%")
    }

    pub fn archive_url(&self) -> Result<String> {
        let filename = self.single_value("%FILENAME%")?;
        let pkgname = self.name()?;
        let idx = pkgname
            .chars()
            .next()
            .context("Name for package is empty")?;
        Ok(format!(
            "https://archive.archlinux.org/packages/{idx}/{pkgname}/{filename}"
        ))
    }

    pub fn sha256(&self) -> Result<&str> {
        self.single_value("%SHA256SUM%")
    }

    pub fn signature(&self) -> Result<&str> {
        self.single_value("%PGPSIG%")
    }
}

#[derive(Debug, Default)]
pub struct DatabaseCache {
    imported_repositories: HashSet<String>,
    packages: HashMap<String, Package>,
}

impl DatabaseCache {
    pub fn has_repo(&self, repo: &str) -> bool {
        self.imported_repositories.contains(repo)
    }

    pub fn import_repo(&mut self, repo: &str, buf: &[u8]) -> Result<()> {
        let d = GzDecoder::new(buf);
        let mut tar = tar::Archive::new(d);

        for entry in tar.entries()? {
            let mut entry = entry?;
            if entry.header().entry_type() == tar::EntryType::Regular {
                let mut buf = String::new();
                trace!("Reading package from archive: {:?}", entry.path());
                entry
                    .read_to_string(&mut buf)
                    .context("Failed to read database entry")?;

                let pkg =
                    Package::parse(&buf).context("Failed to parse database entry as package")?;

                self.packages.insert(pkg.name()?.to_string(), pkg);
            }
        }

        self.imported_repositories.insert(repo.to_string());
        Ok(())
    }

    pub fn get_package(&self, name: &str) -> Result<&Package> {
        self.packages
            .get(name)
            .context("Failed to find package in any database: {name:?}")
    }
}

pub async fn resolve_dependencies(
    container: &Container,
    manifest: &PackagesManifest,
    dependencies: &mut Vec<PackageLock>,
    keep: bool,
) -> Result<()> {
    info!("Syncing package datatabase...");
    container
        .exec(&["pacman", "-Sy"], container::Exec::default())
        .await?;

    info!("Resolving dependencies...");
    let mut cmd = vec!["pacman", "-Sup", "--print-format", "%r %n %v", "--"];
    for dep in &manifest.dependencies {
        cmd.push(dep.as_str());
    }
    let buf = container
        .exec(
            &cmd,
            container::Exec {
                capture_stdout: true,
                ..Default::default()
            },
        )
        .await?;
    let buf = String::from_utf8(buf).context("Failed to decode pacman output as utf8")?;

    let mut dbs = DatabaseCache::default();
    for line in buf.lines() {
        let mut line = line.split(' ');
        let repo = line.next().context("Missing repo in pacman output")?;
        let name = line.next().context("Missing pkg name in pacman output")?;
        let version = line.next().context("Missing version in pacman output")?;
        if let Some(trailing) = line.next() {
            bail!("Trailing data in pacman output: {trailing:?}");
        }

        debug!("Detected dependency name={name:?} version={version:?} repo={repo:?}");
        if !dbs.has_repo(repo) {
            let buf = container
                .cat(&format!("/var/lib/pacman/sync/{repo}.db"))
                .await?;
            dbs.import_repo(repo, &buf)?;
        }

        let pkg = dbs.get_package(name)?;

        dependencies.push(PackageLock {
            name: name.to_string(),
            version: version.to_string(),
            system: "archlinux".to_string(),
            url: pkg.archive_url()?,
            sha256: pkg.sha256()?.to_string(),
            signature: Some(pkg.signature()?.to_string()),
        });
    }

    if keep {
        info!("Keeping container around until ^C...");
        futures::future::pending().await
    } else {
        Ok(())
    }
}

pub async fn resolve(
    update: &args::Update,
    manifest: &PackagesManifest,
    container: &ContainerLock,
    dependencies: &mut Vec<PackageLock>,
) -> Result<()> {
    debug!("Creating container...");
    let init = &["/__".to_string(), "-P".to_string()];
    let container = Container::create(
        &container.image,
        container::Config {
            init,
            mounts: &[],
            expose_fuse: false,
        },
    )
    .await?;
    let container_id = container.id.clone();
    let result = tokio::select! {
        result = resolve_dependencies(&container, manifest, dependencies, update.keep) => result,
        _ = signal::ctrl_c() => Err(anyhow!("Ctrl-c received")),
    };
    debug!("Removing container...");
    if let Err(err) = container.kill().await {
        warn!("Failed to kill container {:?}: {:#}", container_id, err);
    }
    debug!("Container cleanup complete");
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use flate2::write::GzEncoder;

    #[test]
    fn parse_pkg_entry() -> Result<()> {
        let buf = r#"%FILENAME%
zstd-1.5.5-1-x86_64.pkg.tar.zst

%NAME%
zstd

%BASE%
zstd

%VERSION%
1.5.5-1

%DESC%
Zstandard - Fast real-time compression algorithm

%CSIZE%
493009

%ISIZE%
1500453

%MD5SUM%
2ba620ed7816b97bcad1a721a2a9f6c4

%SHA256SUM%
1891970afabc725e72c6a9bb2c127d906c1d3cc70309336fbe87adbd460c05b8

%PGPSIG%
iQEzBAABCgAdFiEE5JnHn1PJalTlcv7hwGCGM3xQdz4FAmQ79ZMACgkQwGCGM3xQdz4V+Qf/Yz7Y+3WwSDKtspwcaEr3j95n1nN5+SAThl/OHe94WwmInDWV09GwM+Lrw6Y1RFDK1PI1ZLON3hOo/81udW0uCHJ4n0bnU/2x3B4UW82dcBqFBjiEqNEF1x6KcQGf9PE9seZndsiAxVzrbEH9u48RIHx0SuwWnzlryCoHPYTgYsPrpkH0IzLUerP2Lc8rjUR2eAKn6zoomb3mR74dPNMn2yx9gS0l+79EshQR8kWtOVvTv7xgRriWeJMBNoTTvDfiDq5B8395vPaBmSfrU0O3tvVF3eDAGtpxIb8hqfhtRqy3XqTcRrYaoj44KtJraGCbq5DrsImEdx5byS7qBhoheQ==

%URL%
https://facebook.github.io/zstd/

%LICENSE%
BSD
GPL2

%ARCH%
x86_64

%BUILDDATE%
1681646714

%PACKAGER%
Jelle van der Waa <jelle@archlinux.org>

%PROVIDES%
libzstd.so=1-64

%DEPENDS%
glibc
gcc-libs
zlib
xz
lz4

%MAKEDEPENDS%
cmake
gtest
ninja
"#;
        let pkg = Package::parse(buf)?;
        assert_eq!(pkg.name()?, "zstd");
        assert_eq!(
            pkg.archive_url()?,
            "https://archive.archlinux.org/packages/z/zstd/zstd-1.5.5-1-x86_64.pkg.tar.zst"
        );
        assert_eq!(
            pkg.sha256()?,
            "1891970afabc725e72c6a9bb2c127d906c1d3cc70309336fbe87adbd460c05b8"
        );
        assert_eq!(pkg.signature()?, "iQEzBAABCgAdFiEE5JnHn1PJalTlcv7hwGCGM3xQdz4FAmQ79ZMACgkQwGCGM3xQdz4V+Qf/Yz7Y+3WwSDKtspwcaEr3j95n1nN5+SAThl/OHe94WwmInDWV09GwM+Lrw6Y1RFDK1PI1ZLON3hOo/81udW0uCHJ4n0bnU/2x3B4UW82dcBqFBjiEqNEF1x6KcQGf9PE9seZndsiAxVzrbEH9u48RIHx0SuwWnzlryCoHPYTgYsPrpkH0IzLUerP2Lc8rjUR2eAKn6zoomb3mR74dPNMn2yx9gS0l+79EshQR8kWtOVvTv7xgRriWeJMBNoTTvDfiDq5B8395vPaBmSfrU0O3tvVF3eDAGtpxIb8hqfhtRqy3XqTcRrYaoj44KtJraGCbq5DrsImEdx5byS7qBhoheQ==");
        assert!(pkg.single_value("%DEPENDS%").is_err());

        let mut expected = Package::default();
        expected.add_values("%FILENAME%", &["zstd-1.5.5-1-x86_64.pkg.tar.zst"]);
        expected.add_values("%NAME%", &["zstd"]);
        expected.add_values("%BASE%", &["zstd"]);
        expected.add_values("%VERSION%", &["1.5.5-1"]);
        expected.add_values(
            "%DESC%",
            &["Zstandard - Fast real-time compression algorithm"],
        );

        expected.add_values("%CSIZE%", &["493009"]);
        expected.add_values("%ISIZE%", &["1500453"]);
        expected.add_values("%MD5SUM%", &["2ba620ed7816b97bcad1a721a2a9f6c4"]);
        expected.add_values(
            "%SHA256SUM%",
            &["1891970afabc725e72c6a9bb2c127d906c1d3cc70309336fbe87adbd460c05b8"],
        );
        expected.add_values("%PGPSIG%", &[
"iQEzBAABCgAdFiEE5JnHn1PJalTlcv7hwGCGM3xQdz4FAmQ79ZMACgkQwGCGM3xQdz4V+Qf/Yz7Y+3WwSDKtspwcaEr3j95n1nN5+SAThl/OHe94WwmInDWV09GwM+Lrw6Y1RFDK1PI1ZLON3hOo/81udW0uCHJ4n0bnU/2x3B4UW82dcBqFBjiEqNEF1x6KcQGf9PE9seZndsiAxVzrbEH9u48RIHx0SuwWnzlryCoHPYTgYsPrpkH0IzLUerP2Lc8rjUR2eAKn6zoomb3mR74dPNMn2yx9gS0l+79EshQR8kWtOVvTv7xgRriWeJMBNoTTvDfiDq5B8395vPaBmSfrU0O3tvVF3eDAGtpxIb8hqfhtRqy3XqTcRrYaoj44KtJraGCbq5DrsImEdx5byS7qBhoheQ=="]);
        expected.add_values("%URL%", &["https://facebook.github.io/zstd/"]);
        expected.add_values("%LICENSE%", &["BSD", "GPL2"]);
        expected.add_values("%ARCH%", &["x86_64"]);
        expected.add_values("%BUILDDATE%", &["1681646714"]);
        expected.add_values("%PACKAGER%", &["Jelle van der Waa <jelle@archlinux.org>"]);
        expected.add_values("%PROVIDES%", &["libzstd.so=1-64"]);
        expected.add_values("%DEPENDS%", &["glibc", "gcc-libs", "zlib", "xz", "lz4"]);
        expected.add_values("%MAKEDEPENDS%", &["cmake", "gtest", "ninja"]);

        assert_eq!(pkg, expected);
        Ok(())
    }

    #[test]
    fn test_database_cache_import() -> Result<()> {
        let mut db = DatabaseCache::default();
        assert_eq!(db.has_repo("core"), false);

        let data = {
            let mut tar =
                tar::Builder::new(GzEncoder::new(Vec::new(), flate2::Compression::default()));

            let data = br#"%FILENAME%
rust-1:1.70.0-1-x86_64.pkg.tar.zst

%NAME%
rust

%BASE%
rust

%VERSION%
1:1.70.0-1

%DESC%
Systems programming language focused on safety, speed and concurrency

%CSIZE%
90509601

%ISIZE%
483950051

%MD5SUM%
a8498a6e40c64d7b08d493133941e918

%SHA256SUM%
8d018b14d2226d76ee46ecd6e28f51ddfa7bfd930463e517eabd5d86f8a17851

%PGPSIG%
iIsEABYIADMWIQQGaHodnU+rCLUP2Ss7lKgOUKR3xwUCZHkDRRUcaGVmdGlnQGFyY2hsaW51eC5vcmcACgkQO5SoDlCkd8eCrQEA8y2X/SVbHhchDdfBUp+KBOFoqN63haT6TNq7MIFDvXoA/AwzQe1rwL0RfvxMh130A2wzrid77YXTOjk36QHPmGIL

%URL%
https://www.rust-lang.org/

%LICENSE%
Apache
MIT

%ARCH%
x86_64

%BUILDDATE%
1685646983

%PACKAGER%
Jan Alexander Steffens (heftig) <heftig@archlinux.org>

%REPLACES%
cargo
cargo-tree
rust-docs<1:1.56.1-3
rustfmt

%CONFLICTS%
cargo
rust-docs<1:1.56.1-3
rustfmt

%PROVIDES%
cargo
rustfmt

%DEPENDS%
curl
gcc
gcc-libs
libssh2
llvm-libs

%OPTDEPENDS%
gdb: rust-gdb script
lldb: rust-lldb script

%MAKEDEPENDS%
cmake
lib32-gcc-libs
libffi
lld
llvm
musl
ninja
perl
python
rust
wasi-libc

%CHECKDEPENDS%
gdb
procps-ng

"#;

            let mut header = tar::Header::new_gnu();
            header.set_path("rust-1:1.70.0-1/desc")?;
            header.set_size(data.len() as u64);
            header.set_cksum();
            tar.append(&header, &data[..])?;

            tar.into_inner()?.finish()?
        };
        db.import_repo("core", &data)?;
        assert_eq!(db.has_repo("core"), true);

        let pkg = db.get_package("rust")?;
        let mut expected = Package::default();
        expected.add_values("%FILENAME%", &["rust-1:1.70.0-1-x86_64.pkg.tar.zst"]);
        expected.add_values("%NAME%", &["rust"]);
        expected.add_values("%BASE%", &["rust"]);
        expected.add_values("%VERSION%", &["1:1.70.0-1"]);
        expected.add_values(
            "%DESC%",
            &["Systems programming language focused on safety, speed and concurrency"],
        );
        expected.add_values("%CSIZE%", &["90509601"]);
        expected.add_values("%ISIZE%", &["483950051"]);
        expected.add_values("%MD5SUM%", &["a8498a6e40c64d7b08d493133941e918"]);
        expected.add_values(
            "%SHA256SUM%",
            &["8d018b14d2226d76ee46ecd6e28f51ddfa7bfd930463e517eabd5d86f8a17851"],
        );
        expected.add_values("%PGPSIG%", &["iIsEABYIADMWIQQGaHodnU+rCLUP2Ss7lKgOUKR3xwUCZHkDRRUcaGVmdGlnQGFyY2hsaW51eC5vcmcACgkQO5SoDlCkd8eCrQEA8y2X/SVbHhchDdfBUp+KBOFoqN63haT6TNq7MIFDvXoA/AwzQe1rwL0RfvxMh130A2wzrid77YXTOjk36QHPmGIL"]);
        expected.add_values("%URL%", &["https://www.rust-lang.org/"]);
        expected.add_values("%LICENSE%", &["Apache", "MIT"]);
        expected.add_values("%ARCH%", &["x86_64"]);
        expected.add_values("%BUILDDATE%", &["1685646983"]);
        expected.add_values(
            "%PACKAGER%",
            &["Jan Alexander Steffens (heftig) <heftig@archlinux.org>"],
        );
        expected.add_values(
            "%REPLACES%",
            &["cargo", "cargo-tree", "rust-docs<1:1.56.1-3", "rustfmt"],
        );
        expected.add_values("%CONFLICTS%", &["cargo", "rust-docs<1:1.56.1-3", "rustfmt"]);
        expected.add_values("%PROVIDES%", &["cargo", "rustfmt"]);
        expected.add_values(
            "%DEPENDS%",
            &["curl", "gcc", "gcc-libs", "libssh2", "llvm-libs"],
        );
        expected.add_values(
            "%OPTDEPENDS%",
            &["gdb: rust-gdb script", "lldb: rust-lldb script"],
        );
        expected.add_values(
            "%MAKEDEPENDS%",
            &[
                "cmake",
                "lib32-gcc-libs",
                "libffi",
                "lld",
                "llvm",
                "musl",
                "ninja",
                "perl",
                "python",
                "rust",
                "wasi-libc",
            ],
        );
        expected.add_values("%CHECKDEPENDS%", &["gdb", "procps-ng"]);
        assert_eq!(pkg, &expected);

        Ok(())
    }
}
