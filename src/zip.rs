//! 零第三方依赖的 ZIP 打包（deflate + CRC32 + UTF-8 文件名标志）。
//! 归档包只包含一个文件，包内保留其原始相对目录结构。

use crate::util::{crc32, fmt_iso, now_ms};
use miniz_oxide::deflate::compress_to_vec;

fn u16le(out: &mut Vec<u8>, v: u16) { out.extend_from_slice(&v.to_le_bytes()); }
fn u32le(out: &mut Vec<u8>, v: u32) { out.extend_from_slice(&v.to_le_bytes()); }

/// DOS 日期/时间，取自 UTC 时间戳。
fn dos_datetime(ms: i64) -> (u16, u16) {
    let iso = fmt_iso(ms);
    let num = |a: usize, b: usize, d: u16| -> u16 {
        iso.get(a..b).and_then(|s| s.parse::<u16>().ok()).unwrap_or(d)
    };
    let y = num(0, 4, 1980).max(1980).min(2107);
    let mo = num(5, 7, 1).max(1).min(12);
    let d = num(8, 10, 1).max(1).min(31);
    let h = num(11, 13, 0).min(23);
    let mi = num(14, 16, 0).min(59);
    let s = num(17, 19, 0).min(59);
    (((y - 1980) << 9) | (mo << 5) | d, (h << 11) | (mi << 5) | (s / 2))
}

/// entries: (包内相对路径, 文件原始字节)。返回完整 ZIP 字节。
pub fn build_zip(entries: &[(&str, &[u8])]) -> Vec<u8> {
    let (time, date) = dos_datetime(now_ms());
    let mut out: Vec<u8> = Vec::new();
    let mut central: Vec<u8> = Vec::new();
    let mut count: u16 = 0;

    for &(name, data) in entries {
        let name_bytes = name.as_bytes();
        let compressed = compress_to_vec(data, 6);
        let crc = crc32(data);
        let offset = out.len() as u32;

        u32le(&mut out, 0x0403_4B50);
        u16le(&mut out, 20);            // version needed
        u16le(&mut out, 0x0800);        // UTF-8 filename
        u16le(&mut out, 8);             // deflate
        u16le(&mut out, time);
        u16le(&mut out, date);
        u32le(&mut out, crc);
        u32le(&mut out, compressed.len() as u32);
        u32le(&mut out, data.len() as u32);
        u16le(&mut out, name_bytes.len() as u16);
        u16le(&mut out, 0);
        out.extend_from_slice(name_bytes);
        out.extend_from_slice(&compressed);

        u32le(&mut central, 0x0201_4B50);
        u16le(&mut central, 20);        // version made by
        u16le(&mut central, 20);        // version needed
        u16le(&mut central, 0x0800);
        u16le(&mut central, 8);
        u16le(&mut central, time);
        u16le(&mut central, date);
        u32le(&mut central, crc);
        u32le(&mut central, compressed.len() as u32);
        u32le(&mut central, data.len() as u32);
        u16le(&mut central, name_bytes.len() as u16);
        u16le(&mut central, 0);         // extra len
        u16le(&mut central, 0);         // comment len
        u16le(&mut central, 0);         // disk start
        u16le(&mut central, 0);         // internal attrs
        u32le(&mut central, 0);         // external attrs
        u32le(&mut central, offset);
        central.extend_from_slice(name_bytes);
        count += 1;
    }

    let cd_offset = out.len() as u32;
    let cd_size = central.len() as u32;
    out.extend_from_slice(&central);

    u32le(&mut out, 0x0605_4B50);
    u16le(&mut out, 0);
    u16le(&mut out, 0);
    u16le(&mut out, count);
    u16le(&mut out, count);
    u32le(&mut out, cd_size);
    u32le(&mut out, cd_offset);
    u16le(&mut out, 0);
    out
}
