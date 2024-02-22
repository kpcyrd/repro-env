use crate::errors::*;
use sequoia_openpgp::parse::{PacketParser, PacketParserResult, Parse};
use sequoia_openpgp::Packet;
use std::time;

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
}
