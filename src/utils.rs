use crate::errors::*;
use flate2::bufread::GzDecoder;
use std::io::{BufRead, Read};

pub fn read_gzip_to_end<R: BufRead>(reader: &mut R) -> Result<Vec<u8>> {
    let mut buf = Vec::new();
    let mut gz = GzDecoder::new(reader);
    gz.read_to_end(&mut buf)?;
    Ok(buf)
}
