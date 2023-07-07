use crate::errors::*;
use crate::pkgs::Pkg;
use std::io::BufReader;
use std::io::Read;

pub fn parse_control(control: &str) -> Result<Pkg> {
    let mut name = None;
    let mut version = None;

    for line in control.lines() {
        if let Some(value) = line.strip_prefix("Package: ") {
            name = Some(value.to_string());
        }

        if let Some(value) = line.strip_prefix("Version: ") {
            version = Some(value.to_string());
        }
    }

    Ok(Pkg {
        name: name.context("Failed to find package name in deb control data")?,
        version: version.context("Failed to find package version in deb control data")?,
    })
}

pub fn parse_control_tar<R: Read>(filename: &[u8], reader: R) -> Result<Pkg> {
    let mut buf = Vec::new();
    let mut reader = BufReader::new(reader);
    match filename {
        b"control.tar.xz" => lzma_rs::xz_decompress(&mut reader, &mut buf)?,
        _ => bail!("Unsupported compression for control.tar: {filename:?}"),
    }

    let mut tar = tar::Archive::new(&buf[..]);
    for entry in tar.entries()? {
        let mut entry = entry?;
        let path = entry.path()?;
        let filename = path
            .to_str()
            .with_context(|| anyhow!("Package contains paths with invalid encoding: {:?}", path))?;

        if filename == "./control" {
            let mut buf = String::new();
            entry.read_to_string(&mut buf)?;
            return parse_control(&buf);
        }
    }

    bail!("Failed to find control data in control.tar")
}

pub fn parse<R: Read>(reader: R) -> Result<Pkg> {
    let mut archive = ar::Archive::new(reader);
    while let Some(entry) = archive.next_entry() {
        let mut entry = entry?;
        let filename = entry.header().identifier();
        if !filename.starts_with(b"control.tar") {
            continue;
        }
        let filename = filename.to_owned();
        return parse_control_tar(&filename, &mut entry);
    }

    bail!("Failed to find control data")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_control_data() -> Result<()> {
        let data = "Package: binutils-common\nSource: binutils\nVersion: 2.40-2\nArchitecture: amd64\nMaintainer: Matthias Klose <doko@debian.org>\nInstalled-Size: 15021\nBreaks: binutils (<< 2.38.50.20220527-2), binutils-multiarch (<< 2.38.50.20220527-2)\nReplaces: binutils (<< 2.38.50.20220527-2), binutils-multiarch (<< 2.38.50.20220527-2)\nSection: devel\nPriority: optional\nMulti-Arch: same\nHomepage: https://www.gnu.org/software/binutils/\nDescription: Common files for the GNU assembler, linker and binary utilities\n This package contains the localization files used by binutils packages for\n various target architectures and parts of the binutils documentation. It is\n not useful on its own.\n";
        let data = parse_control(data)?;
        assert_eq!(
            data,
            Pkg {
                name: "binutils-common".to_string(),
                version: "2.40-2".to_string(),
            }
        );
        Ok(())
    }
}
