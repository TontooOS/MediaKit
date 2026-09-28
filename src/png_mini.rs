//! Minimal PNG writer (8-bit RGB, non-interlaced), pure Rust.
//!
//! Uses zlib stored blocks (no compression) so no DEFLATE tables are
//! needed; output is valid PNG readable by any decoder. Built for
//! native thumbnails without third-party image code.

/// CRC-32 (IEEE) over `data`.
pub fn crc32(data: &[u8]) -> u32 {
    let mut crc = 0xFFFF_FFFFu32;
    for &b in data {
        crc ^= b as u32;
        for _ in 0..8 {
            crc = if crc & 1 != 0 {
                (crc >> 1) ^ 0xEDB8_8320
            } else {
                crc >> 1
            };
        }
    }
    crc ^ 0xFFFF_FFFF
}

/// Adler-32 checksum for the zlib stream.
pub fn adler32(data: &[u8]) -> u32 {
    const MOD: u32 = 65521;
    let (mut a, mut b) = (1u32, 0u32);
    for &byte in data {
        a = (a + byte as u32) % MOD;
        b = (b + a) % MOD;
    }
    (b << 16) | a
}

fn chunk(typ: &[u8; 4], payload: &[u8], out: &mut Vec<u8>) {
    out.extend_from_slice(&(payload.len() as u32).to_be_bytes());
    out.extend_from_slice(typ);
    out.extend_from_slice(payload);
    let mut mac = Vec::with_capacity(4 + payload.len());
    mac.extend_from_slice(typ);
    mac.extend_from_slice(payload);
    out.extend_from_slice(&crc32(&mac).to_be_bytes());
}

fn zlib_stored(raw: &[u8]) -> Vec<u8> {
    let mut out = vec![0x78, 0x01];
    let mut off = 0usize;
    while off < raw.len() || raw.is_empty() && off == 0 {
        let take = (raw.len() - off).min(65535);
        let last = off + take >= raw.len();
        out.push(if last { 0x01 } else { 0x00 });
        out.extend_from_slice(&(take as u16).to_le_bytes());
        out.extend_from_slice(&(!(take as u16)).to_le_bytes());
        out.extend_from_slice(&raw[off..off + take]);
        off += take;
        if raw.is_empty() {
            break;
        }
    }
    out.extend_from_slice(&adler32(raw).to_be_bytes());
    out
}

/// Encodes RGB24 into a PNG byte buffer.
pub fn encode_png_rgb(width: u32, height: u32, rgb: &[u8]) -> Result<Vec<u8>, String> {
    if width == 0 || height == 0 || width > 16384 || height > 16384 {
        return Err("bad png dimensions".into());
    }
    if rgb.len() != (width * height * 3) as usize {
        return Err("rgb size mismatch".into());
    }
    let stride = (width * 3) as usize;
    let mut raw = Vec::with_capacity(((width * 3 + 1) * height) as usize);
    for y in 0..height as usize {
        raw.push(0); // filter: none
        raw.extend_from_slice(&rgb[y * stride..(y + 1) * stride]);
    }
    let mut out = vec![137, 80, 78, 71, 13, 10, 26, 10];
    let mut ihdr = Vec::with_capacity(13);
    ihdr.extend_from_slice(&width.to_be_bytes());
    ihdr.extend_from_slice(&height.to_be_bytes());
    ihdr.extend_from_slice(&[8, 2, 0, 0, 0]);
    chunk(b"IHDR", &ihdr, &mut out);
    chunk(b"IDAT", &zlib_stored(&raw), &mut out);
    chunk(b"IEND", &[], &mut out);
    Ok(out)
}

/// Decodes our own stored-block PNGs back to pixels (test helper).
pub fn decode_png_stored(data: &[u8]) -> Result<(u32, u32, Vec<u8>), String> {
    if data.len() < 8 || &data[0..8] != &[137, 80, 78, 71, 13, 10, 26, 10] {
        return Err("bad signature".into());
    }
    let mut off = 8usize;
    let (mut w, mut h) = (0u32, 0u32);
    let mut idat = Vec::new();
    while off + 8 <= data.len() {
        let len = u32::from_be_bytes([data[off], data[off + 1], data[off + 2], data[off + 3]])
            as usize;
        let typ = &data[off + 4..off + 8];
        let payload = data
            .get(off + 8..off + 8 + len)
            .ok_or("truncated chunk")?;
        if typ == b"IHDR" {
            w = u32::from_be_bytes([payload[0], payload[1], payload[2], payload[3]]);
            h = u32::from_be_bytes([payload[4], payload[5], payload[6], payload[7]]);
            if payload[8] != 8 || payload[9] != 2 {
                return Err("only 8-bit rgb supported".into());
            }
        } else if typ == b"IDAT" {
            idat.extend_from_slice(payload);
        } else if typ == b"IEND" {
            break;
        }
        off += 12 + len;
    }
    // zlib: 2-byte header, stored blocks, adler32.
    if idat.len() < 6 {
        return Err("bad idat".into());
    }
    let mut raw = Vec::new();
    let mut p = 2usize;
    loop {
        if p + 5 > idat.len() {
            return Err("bad block".into());
        }
        let last = idat[p] & 1 != 0;
        if idat[p] & 0x06 != 0 {
            return Err("only stored blocks supported".into());
        }
        let len = u16::from_le_bytes([idat[p + 1], idat[p + 2]]) as usize;
        let nlen = u16::from_le_bytes([idat[p + 3], idat[p + 4]]) as usize;
        if len ^ nlen != 0xFFFF {
            return Err("bad nlen".into());
        }
        p += 5;
        raw.extend_from_slice(
            idat.get(p..p + len).ok_or("block overrun")?,
        );
        p += len;
        if last {
            break;
        }
    }
    let stride = (w * 3) as usize;
    let mut rgb = vec![0u8; (w * h * 3) as usize];
    let mut rp = 0usize;
    for y in 0..h as usize {
        if raw[rp] != 0 {
            return Err("only filter 0 supported".into());
        }
        rp += 1;
        rgb[y * stride..(y + 1) * stride].copy_from_slice(&raw[rp..rp + stride]);
        rp += stride;
    }
    Ok((w, h, rgb))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn checksums_match_known_vectors() {
        assert_eq!(crc32(b""), 0);
        assert_eq!(crc32(b"123456789"), 0xCBF43926);
        assert_eq!(adler32(b""), 1);
        assert_eq!(adler32(b"123456789"), 0x091E01DE);
    }

    #[test]
    fn roundtrip_small() {
        let rgb: Vec<u8> = (0..48).collect();
        let png = encode_png_rgb(4, 4, &rgb).unwrap();
        assert_eq!(&png[0..8], &[137, 80, 78, 71, 13, 10, 26, 10]);
        let (w, h, back) = decode_png_stored(&png).unwrap();
        assert_eq!((w, h), (4, 4));
        assert_eq!(back, rgb);
    }

    #[test]
    fn roundtrip_large_spans_blocks() {
        // 300x200 RGB forces multiple stored blocks? No: one row is
        // 900 bytes, whole image 180300 bytes -> 3 blocks.
        let rgb = vec![0x7Fu8; 300 * 200 * 3];
        let png = encode_png_rgb(300, 200, &rgb).unwrap();
        let (w, h, back) = decode_png_stored(&png).unwrap();
        assert_eq!((w, h), (300, 200));
        assert_eq!(back, rgb);
    }

    #[test]
    fn rejects_bad_input() {
        assert!(encode_png_rgb(0, 4, &[]).is_err());
        assert!(encode_png_rgb(4, 4, &[0u8; 10]).is_err());
        assert!(decode_png_stored(b"nope").is_err());
    }
}
