use crate::errors::*;
use crate::pkgs::Pkg;
use std::io::{BufRead, BufReader, Read};

pub enum Compression {
    Xz,
    Zstd,
    None,
}

pub fn detect_compression(bytes: &[u8]) -> Compression {
    let mime = tree_magic_mini::from_u8(bytes);
    debug!("Detected mimetype for possibly compressed data: {:?}", mime);

    match mime {
        "application/x-xz" => Compression::Xz,
        "application/zstd" => Compression::Zstd,
        _ => Compression::None,
    }
}

pub fn parse_pkginfo<R: Read>(reader: R) -> Result<Pkg> {
    let reader = BufReader::new(reader);

    let mut name = None;
    let mut version = None;

    for line in reader.lines() {
        let line = line?;

        if let Some(value) = line.strip_prefix("pkgname = ") {
            name = Some(value.to_string());
        } else if let Some(value) = line.strip_prefix("pkgver = ") {
            version = Some(value.to_string());
        }
    }

    Ok(Pkg {
        name: name.context("")?,
        version: version.context("versioN")?,
    })
}

pub fn parse_tar<R: Read>(reader: R) -> Result<Pkg> {
    let mut tar = tar::Archive::new(reader);
    for entry in tar.entries()? {
        let entry = entry?;
        let path = entry.path()?;
        if path.to_str() == Some(".PKGINFO") {
            return parse_pkginfo(entry);
        }
    }
    bail!("Failed to find .PKGINFO in package file")
}

pub fn parse(reader: &[u8]) -> Result<Pkg> {
    match detect_compression(reader) {
        Compression::Xz => {
            let mut buf = Vec::new();
            lzma_rs::xz_decompress(&mut &reader[..], &mut buf)?;
            parse_tar(&buf[..])
        }
        Compression::Zstd => {
            let decoder = ruzstd::StreamingDecoder::new(reader)?;
            parse_tar(decoder)
        }
        Compression::None => parse_tar(reader),
    }
}
