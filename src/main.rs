//! Mini Drive - 零认证简易云盘（Rust 后端 + Svelte 前端）
//!
//! 环境变量：
//!   HOST            监听地址，默认 127.0.0.1（局域网用 0.0.0.0）
//!   PORT            端口，默认 8787
//!   DATA_DIR        文件存储目录，默认 <工程>/storage
//!   RECYCLE_DIR     回收站目录，默认 <工程>/recycle
//!   PUBLIC_DIR      前端构建产物目录，默认 <工程>/public
//!   META_FILE       生命周期元数据，默认 <工程>/lifecycle.json
//!   MAX_UPLOAD_MB   单次请求体积上限，默认 200
//!   DEFAULT_TTL     全局默认过期时长（留空 = 仅显式指定 expires 才归档）
//!   RECYCLE_TTL     归档包保留时长，默认 7d
//!   SWEEP_INTERVAL  扫描间隔，默认 60s

mod help;
mod lifecycle;
mod util;
mod zip;

use serde_json::{json, Value};
use std::collections::HashMap;
use std::fs;
use std::io::Write;
use std::net::UdpSocket;
use std::path::PathBuf;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, UNIX_EPOCH};
use tiny_http::{Header, Request, Response, Server};

use util::{HttpError, Resp, Result};

pub struct Config {
    pub host: String,
    pub port: u16,
    pub root: PathBuf,
    pub storage: PathBuf,
    pub recycle: PathBuf,
    pub public: PathBuf,
    pub meta_file: PathBuf,
    pub max_bytes: usize,
    pub default_ttl_ms: Option<i64>,
    pub recycle_ttl_ms: i64,
    pub sweep_ms: u64,
}

struct State {
    cfg: Config,
    meta: Mutex<lifecycle::Meta>,
}

fn env(key: &str, default: &str) -> String {
    std::env::var(key).unwrap_or_else(|_| default.to_string())
}

fn resolve_dir(key: &str, default: &Path) -> PathBuf {
    match std::env::var(key) {
        Ok(v) if !v.trim().is_empty() => PathBuf::from(v),
        _ => default.to_path_buf(),
    }
}

fn duration_or(raw: &str, default_ms: i64) -> i64 {
    util::parse_duration_ms(raw).unwrap_or(default_ms)
}

fn build_config() -> Config {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let port: u16 = env("PORT", "8787").parse().unwrap_or(8787);
    let max_mb: usize = env("MAX_UPLOAD_MB", "200").parse().unwrap_or(200);
    let default_ttl = {
        let v = env("DEFAULT_TTL", "");
        if v.trim().is_empty() { None } else { util::parse_duration_ms(&v) }
    };
    Config {
        host: env("HOST", "127.0.0.1"),
        port,
        storage: resolve_dir("DATA_DIR", &root.join("storage")),
        recycle: resolve_dir("RECYCLE_DIR", &root.join("recycle")),
        public: resolve_dir("PUBLIC_DIR", &root.join("public")),
        meta_file: resolve_dir("META_FILE", &root.join("lifecycle.json")),
        max_bytes: max_mb.max(1) * 1024 * 1024,
        default_ttl_ms: default_ttl,
        recycle_ttl_ms: duration_or(&env("RECYCLE_TTL", "7d"), 7 * 86_400_000),
        sweep_ms: duration_or(&env("SWEEP_INTERVAL", "60s"), 60_000).max(100) as u64,
        root,
    }
}

fn local_ip() -> Option<String> {
    UdpSocket::bind("0.0.0.0:0").ok().and_then(|s| {
        s.connect("223.5.5.5:80").ok()?;
        s.local_addr().ok().map(|a| a.ip().to_string())
    })
}

/* ------------------------------------------------------------------ 请求信息 */

struct ReqInfo {
    method: String,
    pathname: String,
    query: HashMap<String, String>,
    headers: Vec<(String, String)>,
    body: Vec<u8>,
}

fn header<'a>(headers: &'a [(String, String)], name: &str) -> Option<&'a str> {
    headers.iter().find(|(k, _)| k.eq_ignore_ascii_case(name)).map(|(_, v)| v.as_str())
}

fn parse_query(raw: &str) -> HashMap<String, String> {
    let mut map = HashMap::new();
    for pair in raw.split('&') {
        if pair.is_empty() { continue; }
        let (k, v) = pair.split_once('=').unwrap_or((pair, ""));
        let key = urlencoding::decode(k).map(|s| s.into_owned()).unwrap_or_else(|_| k.to_string());
        let val = urlencoding::decode(v).map(|s| s.into_owned()).unwrap_or_else(|_| v.to_string());
        map.insert(key, val);
    }
    map
}

fn read_body(req: &mut Request, max: usize) -> Result<Vec<u8>> {
    if let Some(n) = req.body_length() {
        if n > max {
            return Err(HttpError::new(413, format!("请求体超过上限 {} MB", max / 1024 / 1024)));
        }
    }
    let mut buf = Vec::new();
    req.as_reader().read_to_end(&mut buf)
        .map_err(|e| HttpError::new(400, format!("读取请求体失败: {}", e)))?;
    if buf.len() > max {
        return Err(HttpError::new(413, "请求体超过上限"));
    }
    Ok(buf)
}

/* ------------------------------------------------------------------ 响应发送 */

fn send(req: Request, r: Resp) {
    let mut response = Response::from_data(r.body).with_status_code(r.status);
    macro_rules! hdr {
        ($n:expr, $v:expr) => {
            if let Ok(h) = Header::from_bytes($n.as_bytes(), $v.as_bytes()) {
                response.add_header(h);
            }
        };
    }
    hdr!("Content-Type", &r.content_type);
    for (k, v) in &r.headers { hdr!(k, v); }
    hdr!("Access-Control-Allow-Origin", "*");
    hdr!("Access-Control-Allow-Methods", "GET, POST, PUT, HEAD, OPTIONS");
    hdr!("Access-Control-Allow-Headers", "Content-Type, X-File-Path, X-File-Expires, X-File-Recycle");
    hdr!("Cache-Control", "no-store");
    let _ = req.respond(response);
}

fn attachment(name: &str) -> String {
    let ascii: String = name.chars().map(|c| if c.is_ascii() && c != '"' && c != '\\' { c } else { '_' }).collect();
    format!("attachment; filename=\"{}\"; filename*=UTF-8''{}", ascii, urlencoding::encode(name))
}

/* -------------------------------------------------------------------- 文件操作 */

fn mtime_iso(md: &fs::Metadata) -> String {
    md.modified().ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map(|d| util::fmt_iso(d.as_millis() as i64))
        .unwrap_or_default()
}

fn item_value(name: String, is_dir: bool, rel: String, size: u64, mtime: String) -> Value {
    json!({
        "name": name,
        "type": if is_dir { "dir" } else { "file" },
        "path": rel,
        "size": size,
        "mtime": mtime,
    })
}

fn walk(dir: &Path, prefix: &str, out: &mut Vec<Value>) -> Result<()> {
    let rd = fs::read_dir(dir).map_err(|e| HttpError::new(500, e.to_string()))?;
    for entry in rd {
        let entry = entry.map_err(|e| HttpError::new(500, e.to_string()))?;
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.starts_with('.') { continue; }
        let rel = if prefix.is_empty() { name.clone() } else { format!("{}/{}", prefix, name) };
        let meta = entry.metadata().map_err(|e| HttpError::new(500, e.to_string()))?;
        if meta.is_dir() {
            out.push(item_value(name, true, rel.clone(), 0, mtime_iso(&meta)));
            walk(&entry.path(), &rel, out)?;
        } else if meta.is_file() {
            out.push(item_value(name, false, rel, meta.len(), mtime_iso(&meta)));
        }
    }
    Ok(())
}

fn list_dir(state: &State, raw_path: &str, recursive: bool) -> Result<Resp> {
    let rel = if raw_path.trim().is_empty() { String::new() } else { util::safe_rel(raw_path)? };
    let dir = if rel.is_empty() { state.cfg.storage.clone() } else { state.cfg.storage.join(&rel) };
    if !dir.is_dir() {
        return Err(HttpError::new(404, "目录不存在"));
    }
    util::ensure_inside(&state.cfg.storage, &dir)?;
    let mut items: Vec<Value> = Vec::new();
    if recursive {
        walk(&dir, &rel, &mut items)?;
    } else {
        let rd = fs::read_dir(&dir).map_err(|e| HttpError::new(500, e.to_string()))?;
        for entry in rd {
            let entry = entry.map_err(|e| HttpError::new(500, e.to_string()))?;
            let name = entry.file_name().to_string_lossy().into_owned();
            if name.starts_with('.') { continue; }
            let meta = entry.metadata().map_err(|e| HttpError::new(500, e.to_string()))?;
            let child_rel = if rel.is_empty() { name.clone() } else { format!("{}/{}", rel, name) };
            if meta.is_dir() {
                items.push(item_value(name, true, child_rel, 0, mtime_iso(&meta)));
            } else if meta.is_file() {
                items.push(item_value(name, false, child_rel, meta.len(), mtime_iso(&meta)));
            }
        }
    }
    items.sort_by_key(|v| (
        v["type"].as_str() != Some("dir"),
        v["name"].as_str().unwrap_or("").to_lowercase(),
    ));
    let count = items.len();
    Ok(Resp::ok(json!({ "ok": true, "path": rel, "count": count, "items": items })))
}

/// 解析上传时的时间参数，返回 (过期绝对毫秒, 回收站保留毫秒) 或 None。
fn time_options(state: &State, expires: Option<&str>, recycle: Option<&str>) -> Result<Option<(i64, i64)>> {
    let exp_raw = expires.map(str::trim).filter(|s| !s.is_empty());
    let effective = match exp_raw {
        Some(v) => Some(util::parse_when(v)?),
        None => state.cfg.default_ttl_ms.map(|d| util::now_ms() + d),
    };
    let Some(expires_at) = effective else { return Ok(None) };
    let recycle_ms = match recycle.map(str::trim).filter(|s| !s.is_empty()) {
        Some(v) => util::parse_duration_ms(v)
            .ok_or_else(|| HttpError::bad(format!("recycle 时长无效: {:?}", v)))?,
        None => state.cfg.recycle_ttl_ms,
    };
    Ok(Some((expires_at, recycle_ms)))
}

fn save_file(state: &State, rel_raw: &str, data: &[u8], expires: Option<&str>, recycle: Option<&str>) -> Result<Value> {
    let rel = util::safe_rel(rel_raw)?;
    if data.is_empty() { return Err(HttpError::bad("上传内容为空")); }
    let abs = util::abs_of(&state.cfg.storage, &rel);
    if let Some(parent) = abs.parent() {
        fs::create_dir_all(parent).map_err(|e| HttpError::new(500, e.to_string()))?;
        util::ensure_inside(&state.cfg.storage, parent)?;
    }
    util::ensure_inside(&state.cfg.storage, &abs)?;
    if abs.is_dir() { return Err(HttpError::bad("目标路径已存在且是目录")); }
    fs::write(&abs, data).map_err(|e| HttpError::new(500, format!("写入文件失败: {}", e)))?;

    let times = time_options(state, expires, recycle)?;
    {
        let mut meta = state.meta.lock().unwrap();
        match times {
            Some((at, ttl)) => lifecycle::register(&mut meta, &rel, at, ttl),
            None => lifecycle::unregister(&mut meta, &rel),
        }
        lifecycle::save(&state.cfg.meta_file, &meta)
            .map_err(|e| HttpError::new(500, format!("保存元数据失败: {}", e)))?;
    }
    Ok(json!({
        "ok": true,
        "path": rel,
        "size": data.len(),
        "download_url": format!("/files/{}", util::enc_path(&rel)),
        "lifecycle": if times.is_some() { "scheduled" } else { "none" },
        "expires_at": times.map(|(at, _)| util::fmt_iso(at)),
        "recycle_ttl": times.map(|(_, ttl)| ttl / 1000),
    }))
}

fn download_file(state: &State, rel_raw: &str, head: bool) -> Result<Resp> {
    let rel = util::decode_rel(rel_raw)?;
    download_rel(state, &rel, head)
}

fn download_rel(state: &State, rel: &str, head: bool) -> Result<Resp> {
    let abs = util::abs_of(&state.cfg.storage, rel);
    if !abs.is_file() { return Err(HttpError::new(404, "文件不存在")); }
    util::ensure_inside(&state.cfg.storage, &abs)?;
    let name = rel.rsplit('/').next().unwrap_or(rel).to_string();
    let md = fs::metadata(&abs).map_err(|e| HttpError::new(500, e.to_string()))?;
    let body = if head { Vec::new() } else {
        fs::read(&abs).map_err(|e| HttpError::new(500, e.to_string()))?
    };
    Ok(Resp {
        status: 200,
        content_type: util::mime_of(&name).into(),
        headers: vec![
            ("Content-Disposition".into(), attachment(&name)),
            ("X-File-Size".into(), md.len().to_string()),
        ],
        body,
    })
}

fn download_archive(state: &State, raw_name: &str, head: bool) -> Result<Resp> {
    let name = urlencoding::decode(raw_name)
        .map_err(|_| HttpError::bad("文件名编码无效"))?
        .into_owned();
    if name.is_empty() || name.starts_with('.') || name.contains('/') || name.contains('\\') || name.contains("..") {
        return Err(HttpError::bad("非法的归档包名"));
    }
    let abs = state.cfg.recycle.join(&name);
    if !abs.is_file() { return Err(HttpError::new(404, "归档包不存在")); }
    util::ensure_inside(&state.cfg.recycle, &abs)?;
    let body = if head { Vec::new() } else { fs::read(&abs).map_err(|e| HttpError::new(500, e.to_string()))? };
    Ok(Resp {
        status: 200,
        content_type: "application/zip".into(),
        headers: vec![("Content-Disposition".into(), attachment(&name))],
        body,
    })
}

fn serve_static(state: &State, raw_path: &str, head: bool) -> Result<Resp> {
    let mut rel = raw_path.trim_start_matches('/').to_string();
    if rel.is_empty() { rel = "index.html".into(); }
    let rel = urlencoding::decode(&rel).map_err(|_| HttpError::bad("路径编码无效"))?.into_owned();
    let rel = util::safe_rel(&rel)?;
    let abs = util::abs_of(&state.cfg.public, &rel);
    util::ensure_inside(&state.cfg.public, &abs)?;
    if !abs.is_file() { return Err(HttpError::new(404, "页面资源不存在")); }
    let body = if head { Vec::new() } else { fs::read(&abs).map_err(|e| HttpError::new(500, e.to_string()))? };
    Ok(Resp {
        status: 200,
        content_type: util::mime_of(&rel).into(),
        headers: vec![],
        body,
    })
}

fn mkdir(state: &State, raw: &str) -> Result<Resp> {
    let rel = util::safe_rel(raw)?;
    let abs = util::abs_of(&state.cfg.storage, &rel);
    fs::create_dir_all(&abs).map_err(|e| HttpError::new(500, e.to_string()))?;
    util::ensure_inside(&state.cfg.storage, &abs)?;
    Ok(Resp::ok(json!({ "ok": true, "path": rel })))
}

/* ------------------------------------------------------------------- 上传分流 */

fn upload_multipart(state: &State, info: &ReqInfo, content_type: &str) -> Result<Resp> {
    let boundary = util::boundary_of(content_type)
        .ok_or_else(|| HttpError::bad("multipart 缺少 boundary"))?;
    let parts = util::parse_multipart(&info.body, &boundary)?;
    let field = |name: &str| -> Option<String> {
        parts.iter().find(|p| p.name == name && p.filename.is_none())
            .map(|p| String::from_utf8_lossy(&p.data).trim().to_string())
            .filter(|s| !s.is_empty())
    };
    let base_dir = field("path").unwrap_or_default();
    let expires = field("expires");
    let recycle = field("recycle");
    let files: Vec<&util::Part> = parts.iter()
        .filter(|p| p.filename.as_deref().map(|f| !f.is_empty()).unwrap_or(false))
        .collect();
    if files.is_empty() {
        return Err(HttpError::bad("没有收到文件字段，请使用 -F \"file=@./a.txt\""));
    }
    let mut saved = Vec::new();
    for p in files {
        let filename = p.filename.as_deref().unwrap_or("");
        let name = util::basename(filename);
        let rel = if base_dir.is_empty() { name } else { format!("{}/{}", base_dir, name) };
        saved.push(save_file(state, &rel, &p.data, expires.as_deref(), recycle.as_deref())?);
    }
    let count = saved.len();
    Ok(Resp::ok(json!({ "ok": true, "count": count, "files": saved })))
}

/* --------------------------------------------------------------------- 分派 */

fn base_url(info: &ReqInfo, state: &State) -> String {
    let host = header(&info.headers, "host")
        .map(|s| s.to_string())
        .unwrap_or_else(|| format!("127.0.0.1:{}", state.cfg.port));
    let scheme = header(&info.headers, "x-forwarded-proto").unwrap_or("http");
    format!("{}://{}", scheme, host)
}

fn dispatch(state: &State, info: &ReqInfo) -> Result<Resp> {
    let m = info.method.as_str();
    let p = info.pathname.as_str();
    let q = |k: &str| -> Option<&str> { info.query.get(k).map(String::as_str) };
    let base = base_url(info, state);

    if m == "OPTIONS" {
        return Ok(Resp { status: 204, content_type: "text/plain".into(), headers: vec![], body: vec![] });
    }

    // ---- 帮助 / 状态 ----
    if m == "GET" || m == "HEAD" {
        match p {
            "/api/health" => return Ok(Resp::ok(json!({
                "ok": true, "service": "mini-drive", "version": util::VERSION, "runtime": "Rust + Svelte",
                "storage": state.cfg.storage.to_string_lossy(),
                "recycle_dir": state.cfg.recycle.to_string_lossy(),
                "max_upload_bytes": state.cfg.max_bytes,
            }))),
            "/api/help" => return Ok(Resp::ok(help::help_json(&base, &state.cfg))),
            "/llms.txt" => return Ok(Resp::text(200, help::llms_txt(&base, &state.cfg), "text/plain; charset=utf-8")),
            "/help" => return Ok(Resp::text(200, help::help_html(&base, &state.cfg), "text/html; charset=utf-8")),
            _ => {}
        }
    }

    if (m == "GET" || m == "HEAD") && p == "/api/list" {
        return list_dir(state, q("path").unwrap_or(""), q("recursive") == Some("1"));
    }
    if p == "/api/download" && (m == "GET" || m == "HEAD") {
        let raw = q("path").ok_or_else(|| HttpError::bad("缺少 path 参数"))?;
        let rel = util::safe_rel(raw)?;
        return download_rel(state, &rel, m == "HEAD");
    }
    if p == "/api/lifecycle" && m == "GET" {
        let meta = state.meta.lock().unwrap();
        return Ok(Resp::ok(lifecycle::lifecycle_status(&state.cfg, &meta)));
    }
    if p == "/api/recycle" && m == "GET" {
        let meta = state.meta.lock().unwrap();
        return Ok(Resp::ok(lifecycle::recycle_status(&state.cfg, &meta)));
    }
    if p == "/api/sweep" && m == "POST" {
        let mut meta = state.meta.lock().unwrap();
        let r = lifecycle::sweep(&state.cfg, &mut meta);
        let _ = lifecycle::save(&state.cfg.meta_file, &meta);
        return Ok(Resp::ok(json!({
            "ok": true, "archived": r.archived, "purged": r.purged,
            "missing": r.missing, "errors": r.errors,
        })));
    }
    if p == "/api/mkdir" && m == "POST" {
        let path = q("path").ok_or_else(|| HttpError::bad("缺少 path 参数"))?;
        return mkdir(state, path);
    }

    // ---- 上传 ----
    if let Some(raw) = p.strip_prefix("/files/") {
        if m == "PUT" {
            let rel = util::decode_rel(raw)?;
            let exp = q("expires").or_else(|| header(&info.headers, "x-file-expires"));
            let rec = q("recycle").or_else(|| header(&info.headers, "x-file-recycle"));
            let saved = save_file(state, &rel, &info.body, exp, rec)?;
            return Ok(Resp::ok(saved));
        }
        if m == "GET" || m == "HEAD" {
            return download_file(state, raw, m == "HEAD");
        }
    }
    if p == "/api/upload" && (m == "POST" || m == "PUT") {
        let ct = header(&info.headers, "content-type").unwrap_or("");
        if ct.to_ascii_lowercase().starts_with("multipart/form-data") {
            return upload_multipart(state, info, ct);
        }
        let target = q("path").or_else(|| header(&info.headers, "x-file-path"))
            .ok_or_else(|| HttpError::bad("缺少 path 参数，例如 ?path=docs/a.txt（或 X-File-Path 头）"))?;
        let exp = q("expires").or_else(|| header(&info.headers, "x-file-expires"));
        let rec = q("recycle").or_else(|| header(&info.headers, "x-file-recycle"));
        let saved = save_file(state, target, &info.body, exp, rec)?;
        return Ok(Resp::ok(saved));
    }

    // ---- 回收站归档包 ----
    if let Some(name) = p.strip_prefix("/recycle/") {
        if m == "GET" || m == "HEAD" {
            return download_archive(state, name, m == "HEAD");
        }
    }

    // ---- 前端静态资源 ----
    if m == "GET" || m == "HEAD" {
        return serve_static(state, p, m == "HEAD");
    }

    Err(HttpError::new(405, format!("不支持 {} {}", m, p)))
}

fn handle(mut req: Request, state: &Arc<State>) {
    let method = req.method().as_str().to_string();
    let url = req.url().to_string();
    let headers: Vec<(String, String)> = req.headers().iter().map(|h| {
        (h.field.as_str().as_str().to_string(), h.value.as_str().to_string())
    }).collect();
    let (pathname, query_raw) = url.split_once('?').unwrap_or((&url, ""));
    let query = parse_query(query_raw);
    let needs_body = method == "POST" || method == "PUT";
    let body = if needs_body {
        match read_body(&mut req, state.cfg.max_bytes) {
            Ok(b) => b,
            Err(e) => { send(req, Resp::error(&e)); return; }
        }
    } else { Vec::new() };

    let info = ReqInfo {
        method,
        pathname: pathname.to_string(),
        query,
        headers,
        body,
    };
    let resp = match dispatch(state, &info) {
        Ok(r) => r,
        Err(e) => Resp::error(&e),
    };
    send(req, resp);
}

fn main() {
    let cfg = build_config();
    for dir in [&cfg.storage, &cfg.recycle, &cfg.public] {
        if let Err(e) = fs::create_dir_all(dir) {
            eprintln!("创建目录失败 {:?}: {}", dir, e);
        }
    }

    let mut meta = lifecycle::load(&cfg.meta_file);
    lifecycle::sweep(&cfg, &mut meta);
    let _ = lifecycle::save(&cfg.meta_file, &meta);
    let state = Arc::new(State { cfg, meta: Mutex::new(meta) });

    // 定时扫描线程
    {
        let st = state.clone();
        thread::spawn(move || loop {
            thread::sleep(Duration::from_millis(st.cfg.sweep_ms));
            let mut m = st.meta.lock().unwrap();
            let r = lifecycle::sweep(&st.cfg, &mut m);
            let _ = lifecycle::save(&st.cfg.meta_file, &m);
            if !r.archived.is_empty() || !r.purged.is_empty() || !r.errors.is_empty() {
                println!("[sweep] 归档 {} / 清理 {} / 错误 {}", r.archived.len(), r.purged.len(), r.errors.len());
            }
        });
    }

    let addr = format!("{}:{}", state.cfg.host, state.cfg.port);
    let server = match Server::http(&addr) {
        Ok(s) => Arc::new(s),
        Err(e) => { eprintln!("无法监听 {}: {}", addr, e); std::process::exit(1); }
    };

    let display = if state.cfg.host == "0.0.0.0" { "127.0.0.1" } else { state.cfg.host.as_str() };
    println!();
    println!("  Mini Drive {}  (Rust + Svelte) 已启动", util::VERSION);
    println!("  页面       http://{}:{}/", display, state.cfg.port);
    println!("  API 帮助   http://{}:{}/api/help   /llms.txt   /help", display, state.cfg.port);
    println!("  存储目录   {}", state.cfg.storage.display());
    println!("  回收站     {}", state.cfg.recycle.display());
    println!("  生命周期   默认TTL {:?} / 回收站 {}s / 扫描 {}ms / 上限 {} MB",
        state.cfg.default_ttl_ms, state.cfg.recycle_ttl_ms / 1000, state.cfg.sweep_ms, state.cfg.max_bytes / 1024 / 1024);
    if state.cfg.host == "0.0.0.0" {
        if let Some(ip) = local_ip() {
            println!("  局域网     http://{}:{}/", ip, state.cfg.port);
        }
        println!("  ⚠ 无认证服务已对所有网卡开放，请确认处于可信内网");
    }
    println!();
    let _ = std::io::stdout().flush();

    let mut handles = Vec::new();
    for _ in 0..4 {
        let srv = server.clone();
        let st = state.clone();
        handles.push(thread::spawn(move || {
            for req in srv.incoming_requests() {
                handle(req, &st);
            }
        }));
    }
    for h in handles { let _ = h.join(); }
}
