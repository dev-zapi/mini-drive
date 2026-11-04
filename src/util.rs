//! 通用工具：错误类型、路径安全校验、时间解析、CRC32、multipart 解析。

use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

pub const VERSION: &str = "2.0.0";

#[derive(Debug)]
pub struct HttpError {
    pub status: u16,
    pub message: String,
}

impl HttpError {
    pub fn new(status: u16, message: impl Into<String>) -> Self {
        Self { status, message: message.into() }
    }
    pub fn bad(message: impl Into<String>) -> Self {
        Self::new(400, message)
    }
}

pub type Result<T> = std::result::Result<T, HttpError>;

/// 统一的响应描述，由 main.rs 转成 tiny_http::Response。
pub struct Resp {
    pub status: u16,
    pub content_type: String,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

impl Resp {
    pub fn json(status: u16, value: serde_json::Value) -> Self {
        Self {
            status,
            content_type: "application/json; charset=utf-8".into(),
            headers: vec![],
            body: serde_json::to_vec(&value).unwrap_or_else(|_| b"{}".to_vec()),
        }
    }
    pub fn ok(value: serde_json::Value) -> Self {
        Self::json(200, value)
    }
    pub fn text(status: u16, body: impl Into<Vec<u8>>, mime: &str) -> Self {
        Self { status, content_type: mime.into(), headers: vec![], body: body.into() }
    }
    pub fn error(e: &HttpError) -> Self {
        Self::json(e.status, serde_json::json!({ "ok": false, "error": e.message }))
    }
}

/* ----------------------------------------------------------------- 时间处理 */

pub fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// 公历日期 -> 距 1970-01-01 的天数（Howard Hinnant 算法）。
fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let mp = m + if m > 2 { -3 } else { 9 };
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146097 + doe - 719468
}

/// 距 1970-01-01 的天数 -> (年, 月, 日)。
fn civil_from_days(z: i64) -> (i64, i64, i64) {
    let z = z + 719468;
    let era = if z >= 0 { z } else { z - 146096 } / 146097;
    let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = mp + if mp < 10 { 3 } else { -9 };
    (if m <= 2 { y + 1 } else { y }, m, d)
}

/// UTC 毫秒时间戳 -> ISO 8601 字符串。
pub fn fmt_iso(ms: i64) -> String {
    let secs = ms.div_euclid(1000);
    let millis = ms.rem_euclid(1000);
    let days = secs.div_euclid(86400);
    let sod = secs.rem_euclid(86400);
    let (y, m, d) = civil_from_days(days);
    let hh = sod / 3600;
    let mm = (sod % 3600) / 60;
    let ss = sod % 60;
    format!("{:04}-{:02}-{:02}T{:02}:{:02}:{:02}.{:03}Z", y, m, d, hh, mm, ss, millis)
}

/// 解析 ISO 8601（支持 Z / ±HH:MM / 无时区按 UTC / 仅日期）。
pub fn parse_iso_millis(s: &str) -> Option<i64> {
    let s = s.trim();
    if s.len() < 10 { return None; }
    let mut offset_ms: i64 = 0;
    let mut core = s;
    if let Some(rest) = s.strip_suffix('Z').or_else(|| s.strip_suffix('z')) {
        core = rest;
    } else {
        // 形如 +08:00 / -05:30
        let bytes = s.as_bytes();
        for i in (10..s.len()).rev() {
            let c = bytes[i];
            if c == b'+' || c == b'-' {
                let tz = &s[i..];
                let t: Vec<&str> = tz[1..].split(':').collect();
                if t.len() != 2 { return None; }
                let h: i64 = t[0].parse().ok()?;
                let m: i64 = t[1].parse().ok()?;
                let sign = if c == b'-' { -1 } else { 1 };
                offset_ms = sign * (h * 3600 + m * 60) * 1000;
                core = &s[..i];
                break;
            }
        }
    }
    let core = core.replace(' ', "T");
    let mut it = core.splitn(2, 'T');
    let date = it.next()?;
    let time = it.next().unwrap_or("00:00:00");
    let dp: Vec<&str> = date.split('-').collect();
    if dp.len() != 3 { return None; }
    let y: i64 = dp[0].parse().ok()?;
    let mo: i64 = dp[1].parse().ok()?;
    let da: i64 = dp[2].parse().ok()?;
    let tp: Vec<&str> = time.split(':').collect();
    if tp.len() < 2 || tp.len() > 3 { return None; }
    let h: i64 = tp[0].parse().ok()?;
    let mi: i64 = tp[1].parse().ok()?;
    let (sec, frac) = if tp.len() == 3 {
        let sp: Vec<&str> = tp[2].split('.').collect();
        let se: i64 = sp[0].parse().ok()?;
        let fr: i64 = if sp.len() > 1 {
            let mut f = sp[1].to_string();
            while f.len() < 3 { f.push('0'); }
            f[..3].parse().ok()?
        } else { 0 };
        (se, fr)
    } else { (0, 0) };
    let days = days_from_civil(y, mo, da);
    Some((days * 86400 + h * 3600 + mi * 60 + sec) * 1000 + frac - offset_ms)
}

/// 相对时长：30s / 10m / 2h / 7d / 4w，纯数字按秒；返回毫秒。
pub fn parse_duration_ms(s: &str) -> Option<i64> {
    let s = s.trim();
    if s.is_empty() { return None; }
    let (num, unit) = match s.as_bytes().last()? {
        b's' | b'S' => (&s[..s.len() - 1], 1_000i64),
        b'm' | b'M' => (&s[..s.len() - 1], 60_000),
        b'h' | b'H' => (&s[..s.len() - 1], 3_600_000),
        b'd' | b'D' => (&s[..s.len() - 1], 86_400_000),
        b'w' | b'W' => (&s[..s.len() - 1], 604_800_000),
        b'0'..=b'9' => (s, 1_000),
        _ => return None,
    };
    let n: f64 = num.trim().parse().ok()?;
    if !n.is_finite() || n < 0.0 { return None; }
    Some((n * unit as f64) as i64)
}

/// 解析 expires/recycle：相对时长（相对当前时间）或 ISO 绝对时间。
pub fn parse_when(s: &str) -> Result<i64> {
    let s = s.trim();
    if s.is_empty() { return Err(HttpError::bad("时间值不能为空")); }
    if let Some(ms) = parse_duration_ms(s) {
        if ms == 0 { return Err(HttpError::bad("时长必须大于 0")); }
        return Ok(now_ms() + ms);
    }
    if let Some(ms) = parse_iso_millis(s) { return Ok(ms); }
    Err(HttpError::bad(format!("无法解析时间 {:?}，可用 30s/10m/2h/7d/4w、秒数或 ISO 时间", s)))
}

/* ----------------------------------------------------------------- 路径安全 */

/// 规范化并校验相对路径；返回以 / 分隔的安全相对路径。
pub fn safe_rel(raw: &str) -> Result<String> {
    let normalized = raw.replace('\\', "/");
    if normalized.starts_with('/') {
        return Err(HttpError::bad("不允许绝对路径"));
    }
    let bytes = normalized.as_bytes();
    if bytes.len() >= 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':' {
        return Err(HttpError::bad("不允许盘符路径"));
    }
    let mut out: Vec<&str> = Vec::new();
    for seg in normalized.split('/') {
        if seg.is_empty() { return Err(HttpError::bad("路径含空目录段")); }
        if seg == "." || seg == ".." { return Err(HttpError::bad("路径含 . 或 ..")); }
        if seg.starts_with('.') { return Err(HttpError::bad("不允许隐藏文件/目录")); }
        if seg.ends_with('.') || seg.ends_with(' ') {
            return Err(HttpError::bad("目录名不能以点或空格结尾"));
        }
        for c in seg.chars() {
            if matches!(c, '<' | '>' | ':' | '"' | '|' | '?' | '*') || (c as u32) < 32 {
                return Err(HttpError::bad(format!("路径含非法字符 {:?}", c)));
            }
        }
        out.push(seg);
    }
    if out.is_empty() { return Err(HttpError::bad("路径为空")); }
    Ok(out.join("/"))
}

/// URL 路径百分比解码 -> 安全相对路径。
pub fn decode_rel(raw: &str) -> Result<String> {
    let decoded = urlencoding::decode(raw)
        .map_err(|_| HttpError::bad("路径 URL 编码无效"))?;
    safe_rel(decoded.as_ref())
}

pub fn abs_of(root: &Path, rel: &str) -> PathBuf {
    root.join(rel)
}

/// 确保目标路径（含符号链接情况）不越出 root。
pub fn ensure_inside(root: &Path, abs: &Path) -> Result<()> {
    let canon_root = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
    // 目标可能还不存在（上传前 / 文件已被归档删除）：逐级向上找到最近存在的祖先，
    // 对它做 canonicalize 后再把剩余段拼回来，保证符号链接与前缀判定都正确。
    let mut probe = abs.to_path_buf();
    let mut suffix: Vec<std::ffi::OsString> = Vec::new();
    let mut canonical: Option<PathBuf> = None;
    loop {
        if let Ok(p) = probe.canonicalize() {
            let mut rebuilt = p;
            for seg in suffix.iter().rev() { rebuilt = rebuilt.join(seg); }
            canonical = Some(rebuilt);
            break;
        }
        match probe.file_name() {
            Some(name) => {
                suffix.push(name.to_os_string());
                match probe.parent() {
                    Some(parent) => probe = parent.to_path_buf(),
                    None => break,
                }
            }
            None => break,
        }
    }
    if canonical.map(|p| p.starts_with(&canon_root)).unwrap_or(false) {
        Ok(())
    } else {
        Err(HttpError::bad("路径越界"))
    }
}

/// 相对路径 -> URL 编码后的路径（逐段编码，保留 /）。
pub fn enc_path(rel: &str) -> String {
    rel.split('/')
        .map(|s| urlencoding::encode(s).into_owned())
        .collect::<Vec<_>>()
        .join("/")
}

pub fn basename(name: &str) -> String {
    let n = name.replace('\\', "/");
    n.rsplit('/').next().unwrap_or("").to_string()
}

/* --------------------------------------------------------------------- CRC32 */

pub fn crc32(data: &[u8]) -> u32 {
    let mut table = [0u32; 256];
    for (i, item) in table.iter_mut().enumerate() {
        let mut c = i as u32;
        for _ in 0..8 {
            c = if c & 1 != 0 { 0xEDB8_8320 ^ (c >> 1) } else { c >> 1 };
        }
        *item = c;
    }
    let mut crc = 0xFFFF_FFFFu32;
    for &b in data {
        crc = table[((crc ^ b as u32) & 0xFF) as usize] ^ (crc >> 8);
    }
    crc ^ 0xFFFF_FFFF
}

/* ----------------------------------------------------------------- multipart */

pub struct Part {
    pub name: String,
    pub filename: Option<String>,
    pub data: Vec<u8>,
}

/// 从 Content-Type 中取 boundary。
pub fn boundary_of(content_type: &str) -> Option<String> {
    for piece in content_type.split(';') {
        let p = piece.trim();
        if let Some(v) = p.strip_prefix("boundary=") {
            let v = v.trim().trim_matches('"');
            if !v.is_empty() { return Some(v.to_string()); }
        }
    }
    None
}

/// 极简 multipart/form-data 解析（按 boundary 二进制切分，不破坏文件内容）。
pub fn parse_multipart(body: &[u8], boundary: &str) -> Result<Vec<Part>> {
    let delim = [b"--".as_slice(), boundary.as_bytes()].concat();
    let mut parts = Vec::new();
    for chunk in split_bytes(body, &delim) {
        let mut c = chunk;
        if c.starts_with(b"\r\n") { c = &c[2..]; }
        if c.starts_with(b"--") { break; } // 结束标记
        if c.ends_with(b"\r\n") { c = &c[..c.len() - 2]; }
        if c.is_empty() { continue; }
        let sep = find_bytes(c, b"\r\n\r\n").ok_or_else(|| HttpError::bad("multipart 格式错误"))?;
        let header = String::from_utf8_lossy(&c[..sep]).to_string();
        let data = c[sep + 4..].to_vec();
        let mut name = String::new();
        let mut filename = None;
        for line in header.lines() {
            if line.to_ascii_lowercase().starts_with("content-disposition:") {
                for field in line.split(';').skip(1) {
                    let f = field.trim();
                    if let Some(v) = f.strip_prefix("name=") {
                        name = v.trim().trim_matches('"').to_string();
                    } else if let Some(v) = f.strip_prefix("filename=") {
                        filename = Some(v.trim().trim_matches('"').to_string());
                    }
                }
            }
        }
        parts.push(Part { name, filename, data });
    }
    Ok(parts)
}

fn find_bytes(hay: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() || hay.len() < needle.len() { return None; }
    hay.windows(needle.len()).position(|w| w == needle)
}

fn split_bytes<'a>(hay: &'a [u8], needle: &[u8]) -> Vec<&'a [u8]> {
    let mut out = Vec::new();
    let mut pos = 0usize;
    while let Some(i) = find_bytes(&hay[pos..], needle) {
        let start = pos + i;
        out.push(&hay[pos..start]);
        pos = start + needle.len();
    }
    out.push(&hay[pos..]);
    out
}

/* ----------------------------------------------------------------- Content-Type */

pub fn mime_of(name: &str) -> &'static str {
    let lower = name.to_ascii_lowercase();
    let ext = lower.rsplit('.').next().unwrap_or("");
    match ext {
        "html" | "htm" => "text/html; charset=utf-8",
        "js" | "mjs" => "application/javascript; charset=utf-8",
        "css" => "text/css; charset=utf-8",
        "json" => "application/json; charset=utf-8",
        "txt" | "md" | "log" | "csv" => "text/plain; charset=utf-8",
        "svg" => "image/svg+xml",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "ico" => "image/x-icon",
        "woff2" => "font/woff2",
        "pdf" => "application/pdf",
        "zip" => "application/zip",
        "wasm" => "application/wasm",
        _ => "application/octet-stream",
    }
}
