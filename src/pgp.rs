use crate::errors::*;
use crate::lockfile::PackageLock;
use data_encoding::BASE64;
use sequoia_openpgp::parse::{PacketParser, PacketParserResult, Parse};
use sequoia_openpgp::Packet;
use std::cmp;
use std::time;
use std::time::SystemTime;

pub fn parse_timestamp_from_sig(buf: &[u8]) -> Result<Option<time::SystemTime>> {
    let mut ppr = PacketParser::from_bytes(buf)?;

    while let PacketParserResult::Some(pp) = ppr {
        let (packet, next_ppr) = pp.recurse()?;
        ppr = next_ppr;
        debug!("Found packet in pgp data: {packet:?}");
        let Packet::Signature(sig) = &packet else {
            continue;
        };
        let Some(time) = sig.signature_creation_time() else {
            continue;
        };
        return Ok(Some(time));
    }

    Ok(None)
}

pub fn find_max_signature_time<'a, I: Iterator<Item = &'a PackageLock>>(
    pkgs: I,
) -> Result<Option<SystemTime>> {
    let mut current_max = None;

    for pkg in pkgs {
        let base64 = pkg
            .signature
            .as_ref()
            .context("Package in dependency lockfile is missing signature")?;
        let signature = BASE64
            .decode(base64.as_bytes())
            .with_context(|| anyhow!("Failed to decode signature as base64: {base64:?}"))?;

        match (parse_timestamp_from_sig(&signature), &mut current_max) {
            (Ok(Some(time)), Some(max)) => {
                *max = cmp::max(*max, time);
            }
            (Ok(Some(time)), max) => *max = Some(time),
            (Ok(None), _) => (),
            (Err(err), _) => {
                warn!("Failed to parse timestamp from signature {base64:?}: {err:#?}")
            }
        }
    }

    Ok(current_max)
}

#[cfg(test)]
mod tests {
    use super::*;
    use data_encoding::BASE64;

    #[test]
    fn test_parse_sig() {
        let buf = BASE64.decode(b"iHUEABYKAB0WIQQEKYl95fO9rFN6MGltQr3RFuAGjwUCZcU7FAAKCRBtQr3RFuAGj4Y4AQCKsihdyJWyNGBwQ9Kd5AmenehuvR4xfFOCjIOndQCYhwD+NFzEjbwraHHVtEjQh4HtrnZPc0JplQvM5zRT3gDCawE=").unwrap();
        let time = parse_timestamp_from_sig(&buf).unwrap().unwrap();
        let expected = time::UNIX_EPOCH
            .checked_add(time::Duration::from_secs(1707424532))
            .unwrap();
        assert_eq!(time, expected);
    }

    #[test]
    fn test_max_signature_time() {
        let pkgs = [
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
        ];

        let time = find_max_signature_time(pkgs.iter()).unwrap();
        let expected = time::UNIX_EPOCH
            .checked_add(time::Duration::from_secs(1688457054))
            .unwrap();
        assert_eq!(time, Some(expected));
    }
}
