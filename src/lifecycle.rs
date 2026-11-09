//! 文件生命周期：到期打成 ZIP 移入回收站，回收站超期删除；元数据持久化。

use crate::util::{basename, enc_path, fmt_iso, now_ms, HttpError, Result};
use crate::zip::build_zip;
use crate::Config;
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

#[derive(Clone, Serialize, Deserialize)]
pub struct FileMeta {
    #[serde(alias = "expiresAt")]
    pub expires_at: i64,
    #[serde(alias = "recycleTtlMs")]
    pub recycle_ttl_ms: i64,
    #[serde(default, alias = "uploadedAt")]
    pub uploaded_at: i64,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct ArchiveMeta {
    pub source: String,
    #[serde(alias = "createdAt")]
    pub created_at: i64,
    #[serde(alias = "expiresAt")]
    pub expires_at: i64,
    #[serde(default)]
    pub size: u64,
}

#[derive(Default, Serialize, Deserialize)]
pub struct Meta {
    #[serde(default)]
    pub files: BTreeMap<String, FileMeta>,
    #[serde(default)]
    pub archives: BTreeMap<String, ArchiveMeta>,
}

pub fn load(path: &std::path::Path) -> Meta {
    match fs::read(path) {
        Ok(bytes) => serde_json::from_slice(&bytes).unwrap_or_default(),
        Err(_) => Meta::default(),
    }
}

pub fn save(path: &std::path::Path, meta: &Meta) -> std::io::Result<()> {
    if let Some(parent) = path.parent() { fs::create_dir_all(parent)?; }
    let data = serde_json::to_vec_pretty(meta).unwrap_or_else(|_| b"{}".to_vec());
    fs::write(path, data)
}

/// 生成归档包名：时间戳 + 源文件名（去掉 Windows 非法字符）。
fn archive_name(source: &str, now: i64) -> String {
    let base = basename(source);
    let clean: String = base
        .chars()
        .map(|c| if matches!(c, '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*') { '_' } else { c })
        .take(60)
        .collect();
    let stamp = fmt_iso(now)
        .replace('-', "")
        .replace(':', "")
        .replace('.', "")
        .replace('T', "-")
        .replace('Z', "");
    // 附加源路径哈希，避免同一毫秒内不同目录下的同名文件归档时互相覆盖
    format!("{}-{}-{:08x}.zip", stamp, clean, crate::util::crc32(source.as_bytes()))
}

#[derive(Default, Serialize)]
pub struct SweepResult {
    pub archived: Vec<String>,
    pub purged: Vec<String>,
    pub missing: Vec<String>,
    pub errors: Vec<String>,
}

/// 原文件移走后，顺带清理空掉的父目录（不删 storage 本身）。
fn prune_parents(root: &Path, abs: &Path) {
    let mut parent = abs.parent();
    while let Some(p) = parent {
        if p == root || !p.starts_with(root) { break; }
        if fs::read_dir(p).map(|mut it| it.next().is_none()).unwrap_or(false) {
            let _ = fs::remove_dir(p);
            parent = p.parent();
        } else { break; }
    }
}

/// 递归收集目录内所有文件，包内路径保留原相对目录结构。
fn collect_dir(abs: &Path, rel: &str, out: &mut Vec<(String, Vec<u8>)>) -> Result<()> {
    let rd = fs::read_dir(abs).map_err(|e| HttpError::new(500, e.to_string()))?;
    for entry in rd {
        let entry = entry.map_err(|e| HttpError::new(500, e.to_string()))?;
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.starts_with('.') { continue; }
        let child_rel = format!("{}/{}", rel, name);
        let meta = entry.metadata().map_err(|e| HttpError::new(500, e.to_string()))?;
        if meta.is_dir() {
            collect_dir(&entry.path(), &child_rel, out)?;
        } else if meta.is_file() {
            let data = fs::read(entry.path())
                .map_err(|e| HttpError::new(500, format!("读取待归档文件失败: {}", e)))?;
            out.push((child_rel, data));
        }
    }
    Ok(())
}

fn archive_one(cfg: &Config, rel: &str, now: i64) -> Result<(String, u64)> {
    let abs = cfg.storage.join(rel);
    crate::util::ensure_inside(&cfg.storage, &abs)?;
    let data = fs::read(&abs).map_err(|e| HttpError::new(500, format!("读取待归档文件失败: {}", e)))?;
    let zip_bytes = build_zip(&[(rel, &data)]);
    let name = archive_name(rel, now);
    let dest = cfg.recycle.join(&name);
    fs::create_dir_all(&cfg.recycle).map_err(|e| HttpError::new(500, e.to_string()))?;
    fs::write(&dest, &zip_bytes).map_err(|e| HttpError::new(500, format!("写入归档包失败: {}", e)))?;
    fs::remove_file(&abs).map_err(|e| HttpError::new(500, format!("删除原文件失败: {}", e)))?;
    prune_parents(&cfg.storage, &abs);
    Ok((name, zip_bytes.len() as u64))
}

/// 扫描一次：核对登记项存在性、归档到期文件、清理超期压缩包。
pub fn sweep(cfg: &Config, meta: &mut Meta) -> SweepResult {
    let now = now_ms();
    let mut result = SweepResult::default();
    let mut dirty = false;

    let rels: Vec<String> = meta.files.keys().cloned().collect();
    for rel in rels {
        let Some(info) = meta.files.get(&rel).cloned() else { continue };
        let abs = cfg.storage.join(&rel);
        if !abs.is_file() {
            meta.files.remove(&rel);
            result.missing.push(rel);
            dirty = true;
            continue;
        }
        if info.expires_at > now { continue; }
        match archive_one(cfg, &rel, now) {
            Ok((name, size)) => {
                let ttl = if info.recycle_ttl_ms > 0 { info.recycle_ttl_ms } else { cfg.recycle_ttl_ms };
                meta.archives.insert(name, ArchiveMeta {
                    source: rel.clone(),
                    created_at: now,
                    expires_at: now + ttl,
                    size,
                });
                meta.files.remove(&rel);
                result.archived.push(rel);
                dirty = true;
            }
            Err(e) => result.errors.push(format!("{}: {}", rel, e.message)),
        }
    }

    let names: Vec<String> = meta.archives.keys().cloned().collect();
    for name in names {
        let Some(info) = meta.archives.get(&name).cloned() else { continue };
        let abs = cfg.recycle.join(&name);
        if !abs.is_file() {
            meta.archives.remove(&name);
            result.missing.push(name);
            dirty = true;
            continue;
        }
        if info.expires_at > now { continue; }
        match fs::remove_file(&abs) {
            Ok(_) => result.purged.push(name.clone()),
            Err(e) => result.errors.push(format!("{}: {}", name, e)),
        }
        meta.archives.remove(&name);
        dirty = true;
    }

    if dirty {
        if let Err(e) = save(&cfg.meta_file, meta) {
            result.errors.push(format!("保存元数据失败: {}", e));
        }
    }
    result
}

pub fn register(meta: &mut Meta, rel: &str, expires_at: i64, recycle_ttl_ms: i64) {
    meta.files.insert(rel.to_string(), FileMeta {
        expires_at,
        recycle_ttl_ms,
        uploaded_at: now_ms(),
    });
}

pub fn unregister(meta: &mut Meta, rel: &str) {
    meta.files.remove(rel);
}

pub fn lifecycle_status(cfg: &Config, meta: &Meta) -> serde_json::Value {
    let now = now_ms();
    let pending: Vec<serde_json::Value> = meta.files.iter().map(|(path, info)| json!({
        "path": path,
        "expires_at": fmt_iso(info.expires_at),
        "seconds_left": ((info.expires_at - now) / 1000).max(0),
        "recycle_ttl": info.recycle_ttl_ms / 1000,
    })).collect();
    json!({
        "ok": true,
        "pending_count": pending.len(),
        "pending": pending,
        "archive_count": meta.archives.len(),
        "config": {
            "default_ttl": cfg.default_ttl_ms.map(|v| format!("{}s", v / 1000)),
            "recycle_ttl": format!("{}s", cfg.recycle_ttl_ms / 1000),
            "sweep_interval_ms": cfg.sweep_ms,
            "note": "只有显式设置 expires 或配置 DEFAULT_TTL 的文件才会自动归档"
        }
    })
}

pub fn recycle_status(cfg: &Config, meta: &Meta) -> serde_json::Value {
    let now = now_ms();
    let mut items: Vec<serde_json::Value> = meta.archives.iter().filter_map(|(name, info)| {
        let abs = cfg.recycle.join(name);
        let size = fs::metadata(&abs).ok().map(|m| m.len()).unwrap_or(info.size);
        if !abs.is_file() { return None; }
        Some(json!({
            "name": name,
            "source": info.source,
            "size": size,
            "created_at": fmt_iso(info.created_at),
            "expires_at": fmt_iso(info.expires_at),
            "seconds_left": ((info.expires_at - now) / 1000).max(0),
            "download_url": format!("/recycle/{}", enc_path(name)),
        }))
    }).collect();
    items.sort_by(|a, b| b["created_at"].as_str().cmp(&a["created_at"].as_str()));
    json!({
        "ok": true,
        "ttl": format!("{}s", cfg.recycle_ttl_ms / 1000),
        "dir": cfg.recycle.to_string_lossy(),
        "count": items.len(),
        "items": items,
    })
}

/// 手动删除：把文件（或整个目录）打包成 ZIP 移入回收站，并删除原路径。
/// 与到期归档共用 archive_name / ZIP 格式 / 元数据，回收站保留时长沿用文件自身的设置或全局默认。
pub fn delete_to_recycle(cfg: &Config, meta: &mut Meta, raw: &str) -> Result<serde_json::Value> {
    let rel = crate::util::safe_rel(raw)?;
    let abs = cfg.storage.join(&rel);
    crate::util::ensure_inside(&cfg.storage, &abs)?;
    let is_dir = abs.is_dir();
    if !is_dir && !abs.is_file() {
        return Err(HttpError::new(404, "文件或目录不存在"));
    }

    let now = now_ms();
    let mut entries: Vec<(String, Vec<u8>)> = Vec::new();
    if is_dir {
        collect_dir(&abs, &rel, &mut entries)?;
    } else {
        let data = fs::read(&abs)
            .map_err(|e| HttpError::new(500, format!("读取待归档文件失败: {}", e)))?;
        entries.push((rel.clone(), data));
    }
    let refs: Vec<(&str, &[u8])> = entries.iter()
        .map(|(p, d)| (p.as_str(), d.as_slice()))
        .collect();
    let zip_bytes = build_zip(&refs);
    let name = archive_name(&rel, now);
    let dest = cfg.recycle.join(&name);
    fs::create_dir_all(&cfg.recycle).map_err(|e| HttpError::new(500, e.to_string()))?;
    fs::write(&dest, &zip_bytes)
        .map_err(|e| HttpError::new(500, format!("写入归档包失败: {}", e)))?;

    let ttl = if is_dir {
        cfg.recycle_ttl_ms
    } else {
        meta.files.get(&rel)
            .map(|f| if f.recycle_ttl_ms > 0 { f.recycle_ttl_ms } else { cfg.recycle_ttl_ms })
            .unwrap_or(cfg.recycle_ttl_ms)
    };

    if is_dir {
        fs::remove_dir_all(&abs)
            .map_err(|e| HttpError::new(500, format!("删除目录失败: {}", e)))?;
        let prefix = format!("{}/", rel);
        meta.files.retain(|k, _| k.as_str() != rel.as_str() && !k.starts_with(&prefix));
    } else {
        fs::remove_file(&abs)
            .map_err(|e| HttpError::new(500, format!("删除原文件失败: {}", e)))?;
        meta.files.remove(&rel);
    }
    prune_parents(&cfg.storage, &abs);

    let file_count = entries.len();
    let size = zip_bytes.len() as u64;
    let expires_at = now + ttl;
    meta.archives.insert(name.clone(), ArchiveMeta {
        source: rel.clone(),
        created_at: now,
        expires_at,
        size,
    });
    save(&cfg.meta_file, meta)
        .map_err(|e| HttpError::new(500, format!("保存元数据失败: {}", e)))?;
    Ok(json!({
        "ok": true,
        "path": rel,
        "kind": if is_dir { "dir" } else { "file" },
        "file_count": file_count,
        "archive": name,
        "size": size,
        "expires_at": fmt_iso(expires_at),
        "download_url": format!("/recycle/{}", enc_path(&name)),
    }))
}

/// 彻底删除回收站中的 ZIP（立即删除文件与元数据，不可恢复）。
pub fn purge_archive_now(cfg: &Config, meta: &mut Meta, name: &str) -> Result<serde_json::Value> {
    if name.is_empty() || name.starts_with('.')
        || name.contains('/') || name.contains('\\') || name.contains("..") {
        return Err(HttpError::bad("非法的归档包名"));
    }
    let abs = cfg.recycle.join(name);
    crate::util::ensure_inside(&cfg.recycle, &abs)?;
    if !abs.is_file() && !meta.archives.contains_key(name) {
        return Err(HttpError::new(404, "归档包不存在"));
    }
    if abs.is_file() {
        fs::remove_file(&abs)
            .map_err(|e| HttpError::new(500, format!("删除归档包失败: {}", e)))?;
    }
    meta.archives.remove(name);
    save(&cfg.meta_file, meta)
        .map_err(|e| HttpError::new(500, format!("保存元数据失败: {}", e)))?;
    Ok(json!({ "ok": true, "deleted": name }))
}
