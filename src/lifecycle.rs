//! 文件生命周期：到期打成 ZIP 移入回收站，回收站超期删除；元数据持久化。

use crate::util::{basename, enc_path, fmt_iso, now_ms, HttpError, Result};
use crate::zip::build_zip;
use crate::Config;
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::collections::BTreeMap;
use std::fs;

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
    // 顺带清理空掉的父目录（不删 storage 本身）
    let mut parent = abs.parent();
    while let Some(p) = parent {
        if p == cfg.storage || !p.starts_with(&cfg.storage) { break; }
        if fs::read_dir(p).map(|mut it| it.next().is_none()).unwrap_or(false) {
            let _ = fs::remove_dir(p);
            parent = p.parent();
        } else { break; }
    }
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
