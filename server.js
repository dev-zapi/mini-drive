'use strict';

/**
 * 简易云盘 Mini Drive —— 零依赖 Node.js HTTP 服务
 * 无认证、无数据库。文件直接存放在 storage/ 目录，目录结构与 API 路径一一对应。
 *
 * 环境变量：
 *   PORT          监听端口，默认 8787
 *   HOST          监听地址，默认 127.0.0.1（仅本机）；局域网访问设 0.0.0.0
 *   DATA_DIR      文件存储目录，默认 <工程目录>/storage
 *   MAX_UPLOAD_MB 单个上传体积上限（MB），默认 200
 *   DEFAULT_TTL   全局默认过期时长（如 24h / 7d）；留空则只有上传时显式指定 expires 的文件才归档
 *   RECYCLE_TTL   回收站压缩包保留时长，默认 7d
 *   SWEEP_INTERVAL 生命周期扫描周期，默认 60s（最小 1s）
 *   RECYCLE_DIR   回收站目录，默认 <工程目录>/recycle
 *   META_FILE     生命周期元数据文件，默认 <工程目录>/lifecycle.json
 */

const http = require('node:http');
const fs = require('node:fs');
const fsp = require('node:fs/promises');
const path = require('node:path');
const os = require('node:os');
const zlib = require('node:zlib');

const ROOT = __dirname;
const PUBLIC_DIR = path.join(ROOT, 'public');
const STORAGE_DIR = path.resolve(process.env.DATA_DIR || path.join(ROOT, 'storage'));
const RECYCLE_DIR = path.resolve(process.env.RECYCLE_DIR || path.join(ROOT, 'recycle'));
const META_FILE = path.resolve(process.env.META_FILE || path.join(ROOT, 'lifecycle.json'));
const PORT = Number(process.env.PORT || 8787);
const HOST = process.env.HOST || '127.0.0.1';
const MAX_BYTES = Math.max(1, Number(process.env.MAX_UPLOAD_MB || 200)) * 1024 * 1024;
const DEFAULT_TTL = (process.env.DEFAULT_TTL || '').trim();     // 空 = 未显式指定时间的文件不自动过期
const RECYCLE_TTL = (process.env.RECYCLE_TTL || '7d').trim();   // 回收站压缩包保留时长
const SWEEP_MS = Math.max(1000, msFromDuration(process.env.SWEEP_INTERVAL || '60s') || 60000);
const VERSION = '1.1.0';

const CORS = {
  'Access-Control-Allow-Origin': '*',
  'Access-Control-Allow-Methods': 'GET, HEAD, PUT, POST, OPTIONS',
  'Access-Control-Allow-Headers': 'Content-Type, X-File-Path, X-File-Name',
  'Access-Control-Expose-Headers': 'Content-Disposition',
};

class HttpError extends Error {
  constructor(status, message) {
    super(message);
    this.status = status;
  }
}
const err = (status, message) => new HttpError(status, message);

/* ------------------------------------------------------------------ 路径安全 */

const WIN_BAD_CHARS = /[<>:"|?*]/;
const WIN_RESERVED = /^(con|prn|aux|nul|com[1-9]|lpt[1-9])(\.|$)/i;

/** 把用户给的路径规范化为 storage 内的相对路径（用 / 分隔） */
function safeRel(input) {
  if (typeof input !== 'string') throw err(400, 'path 参数无效');
  let p = input.replace(/\\/g, '/').replace(/^\/+/, '').trim();
  if (!p) return '';
  const out = [];
  for (const seg of p.split('/')) {
    if (!seg || seg === '.') continue;
    if (seg === '..') throw err(400, '路径不允许包含 ..');
    if (seg.startsWith('.')) throw err(400, '不允许访问隐藏文件或目录（以 . 开头）');
    if (/^[a-zA-Z]:/.test(seg)) throw err(400, '不允许绝对路径或盘符');
    if (/[\u0000-\u001f\u007f]/.test(seg)) throw err(400, '路径包含控制字符');
    if (WIN_BAD_CHARS.test(seg) || WIN_RESERVED.test(seg)) throw err(400, `文件名含非法字符: ${seg}`);
    out.push(seg);
  }
  return out.join('/');
}

function isInside(base, target) {
  return target === base || target.startsWith(base + path.sep);
}

/** 解析成绝对路径，并阻止越出 storage 根目录 */
function absOf(rel) {
  const abs = path.resolve(STORAGE_DIR, rel || '.');
  if (!isInside(STORAGE_DIR, abs)) throw err(400, '路径越界');
  return abs;
}

/** 校验真实路径（含符号链接）没有逃逸出 storage；不存在的路径向上找最近的现存祖先 */
async function assertInside(abs) {
  let cur = abs;
  for (;;) {
    try {
      const real = await fsp.realpath(cur);
      const realBase = await fsp.realpath(STORAGE_DIR).catch(() => STORAGE_DIR);
      if (!isInside(realBase, real)) throw err(400, '路径越界（符号链接指向了存储目录之外）');
      return;
    } catch (e) {
      if (e instanceof HttpError) throw e;
      if (e.code !== 'ENOENT' && e.code !== 'ENOTDIR') throw e;
      const parent = path.dirname(cur);
      if (parent === cur) return;
      cur = parent;
    }
  }
}

function contentDisposition(name) {
  const ascii = name.replace(/[^\x20-\x7e]/g, '_').replace(/["\\]/g, '_');
  return `attachment; filename="${ascii}"; filename*=UTF-8''${encodeURIComponent(name)}`;
}

/* ------------------------------------------------------------------ HTTP 工具 */

function sendJson(res, status, obj) {
  const body = Buffer.from(JSON.stringify(obj, null, 2) + '\n', 'utf8');
  res.writeHead(status, { 'Content-Type': 'application/json; charset=utf-8', 'Content-Length': body.length, 'Cache-Control': 'no-store', ...CORS });
  res.end(reqMethodHead(res) ? undefined : body);
}
// HEAD 请求不发送 body（用 res 上的标记，见 route）
function reqMethodHead(res) { return res._headOnly === true; }

function sendText(res, status, text, type = 'text/plain; charset=utf-8') {
  const body = Buffer.from(text, 'utf8');
  res.writeHead(status, { 'Content-Type': type, 'Content-Length': body.length, 'Cache-Control': 'no-store', ...CORS });
  res.end(res._headOnly ? undefined : body);
}

function readBody(req) {
  return new Promise((resolve, reject) => {
    const declared = Number(req.headers['content-length'] || 0);
    if (declared > MAX_BYTES) {
      req.resume();
      reject(err(413, `文件超过上限 ${Math.round(MAX_BYTES / 1024 / 1024)} MB`));
      return;
    }
    const chunks = [];
    let size = 0;
    let done = false;
    req.on('data', (chunk) => {
      if (done) return;
      size += chunk.length;
      if (size > MAX_BYTES) {
        done = true;
        req.resume();
        reject(err(413, `文件超过上限 ${Math.round(MAX_BYTES / 1024 / 1024)} MB`));
        return;
      }
      chunks.push(chunk);
    });
    req.on('end', () => { if (!done) { done = true; resolve(Buffer.concat(chunks)); } });
    req.on('error', (e) => { if (!done) { done = true; reject(e); } });
  });
}

/** 轻量 multipart/form-data 解析（二进制安全，只依赖 boundary） */
function parseMultipart(body, contentType) {
  const m = /boundary=(?:"([^"]+)"|([^;]+))/i.exec(contentType || '');
  const boundary = (m && (m[1] || m[2]) || '').trim();
  if (!boundary || boundary.length > 200 || !/^[A-Za-z0-9'()+_,\-./:=? ]+$/.test(boundary)) {
    throw err(400, 'multipart boundary 无效');
  }
  const marker = Buffer.from('\r\n--' + boundary);
  const first = Buffer.from('--' + boundary);
  let pos = body.indexOf(first);
  if (pos < 0) throw err(400, 'multipart 数据格式错误');
  pos += first.length;
  if (body[pos] === 0x0d && body[pos + 1] === 0x0a) pos += 2;

  const parts = [];
  while (pos < body.length) {
    const next = body.indexOf(marker, pos);
    if (next < 0) break;
    const section = body.subarray(pos, next);
    const headerEnd = section.indexOf('\r\n\r\n');
    if (headerEnd >= 0) {
      const headers = section.subarray(0, headerEnd).toString('utf8');
      const nameM = /\bname="([^"]*)"/i.exec(headers);
      const fileM = /\bfilename="([^"]*)"/i.exec(headers);
      parts.push({
        name: nameM ? nameM[1] : '',
        filename: fileM ? fileM[1] : undefined,
        data: section.subarray(headerEnd + 4),
      });
    }
    pos = next + marker.length;
    if (body[pos] === 0x2d && body[pos + 1] === 0x2d) break; // 结束标记 --
    if (body[pos] === 0x0d && body[pos + 1] === 0x0a) pos += 2;
  }
  return parts;
}

/* ------------------------------------------------------------------ 文件操作 */

async function statMaybe(abs) {
  try { return await fsp.stat(abs); } catch (e) {
    if (e.code === 'ENOENT' || e.code === 'ENOTDIR') return null;
    throw e;
  }
}

async function saveFile(relPath, buffer, opts = {}) {
  const rel = safeRel(relPath);
  if (!rel) throw err(400, '缺少目标文件名，例如 ?path=docs/a.txt');
  if (!buffer.length) throw err(400, '上传内容为空');
  const abs = absOf(rel);
  await assertInside(abs);
  const parent = path.dirname(abs);
  await fsp.mkdir(parent, { recursive: true });
  await assertInside(parent);
  const existing = await statMaybe(abs);
  if (existing && existing.isDirectory()) throw err(400, '目标路径已存在且是目录');
  await fsp.writeFile(abs, buffer);
  const life = registerFile(rel, buffer.length, opts.expires, opts.recycle);
  await saveMeta();
  const out = {
    path: rel,
    size: buffer.length,
    download_url: `/files/${rel.split('/').map(encodeURIComponent).join('/')}`,
  };
  if (life) {
    out.lifecycle = 'scheduled';
    out.expires_at = new Date(life.expiresAt).toISOString();
    out.recycle_ttl_ms = life.recycleTtlMs;
  } else {
    out.lifecycle = 'none';
    out.expires_at = null;
  }
  return out;
}

async function listDir(relPath, recursive) {
  const abs = absOf(safeRel(relPath));
  await assertInside(abs);
  const st = await statMaybe(abs);
  if (!st) throw err(404, '目录不存在');
  if (!st.isDirectory()) throw err(400, '该路径是文件，不是目录');

  const items = [];
  const walk = async (dirAbs, dirRel, depth) => {
    if (depth > 20) return;
    const entries = await fsp.readdir(dirAbs, { withFileTypes: true });
    for (const ent of entries) {
      if (ent.name.startsWith('.')) continue;
      if (ent.isSymbolicLink()) continue;
      const childRel = dirRel ? `${dirRel}/${ent.name}` : ent.name;
      const childAbs = path.join(dirAbs, ent.name);
      if (ent.isDirectory()) {
        if (recursive) await walk(childAbs, childRel, depth + 1);
        else items.push({ name: ent.name, type: 'dir', path: childRel, size: null, mtime: null });
      } else if (ent.isFile()) {
        const s = await fsp.stat(childAbs);
        items.push({
          name: ent.name, type: 'file', path: childRel, size: s.size,
          mtime: s.mtime.toISOString(),
          download_url: `/files/${childRel.split('/').map(encodeURIComponent).join('/')}`,
        });
      }
    }
  };
  await walk(abs, safeRel(relPath), 0);
  items.sort((a, b) => {
    if (a.type !== b.type) return a.type === 'dir' ? -1 : 1;
    return a.name.localeCompare(b.name, 'zh-CN', { numeric: true });
  });
  return { path: safeRel(relPath), recursive: !!recursive, count: items.length, items };
}

async function serveFile(req, res, relPath) {
  const rel = safeRel(relPath);
  if (!rel) throw err(400, '缺少文件路径');
  const abs = absOf(rel);
  await assertInside(abs);
  const st = await statMaybe(abs);
  if (!st || !st.isFile()) throw err(404, '文件不存在');
  const name = path.basename(abs);
  res.writeHead(200, {
    'Content-Type': 'application/octet-stream',
    'Content-Length': st.size,
    'Content-Disposition': contentDisposition(name),
    'Cache-Control': 'no-store',
    ...CORS,
  });
  if (req.method === 'HEAD' || res._headOnly) { res.end(); return; }
  const stream = fs.createReadStream(abs);
  stream.on('error', () => res.destroy());
  stream.pipe(res);
}

async function serveStatic(res, file, type) {
  const body = await fsp.readFile(path.join(PUBLIC_DIR, file));
  res.writeHead(200, { 'Content-Type': type, 'Content-Length': body.length, 'Cache-Control': 'no-cache', ...CORS });
  res.end(res._headOnly ? undefined : body);
}

/* ------------------------------------------------------------------ 文件生命周期 */

/** 相对时长 -> 毫秒。支持 500ms / 30s / 10m / 2h / 7d / 4w，纯数字按秒 */
function msFromDuration(spec) {
  if (spec === undefined || spec === null) return null;
  const m = /^(\d+(?:\.\d+)?)\s*(ms|s|m|min|h|d|w)?$/i.exec(String(spec).trim());
  if (!m) return null;
  const mult = { ms: 1, s: 1000, m: 60000, min: 60000, h: 3600000, d: 86400000, w: 604800000 }[(m[2] || 's').toLowerCase()];
  return Number(m[1]) * mult;
}

/** 过期设定：相对时长或绝对时间（ISO 8601）-> epoch ms */
function parseExpires(spec, now = Date.now()) {
  if (spec === undefined || spec === null || String(spec).trim() === '') return null;
  const s = String(spec).trim();
  const rel = msFromDuration(s);
  if (rel !== null) return now + rel;
  const abs = Date.parse(s);
  if (!Number.isNaN(abs)) return abs;
  throw err(400, 'expires 时间格式无效：可用 30s / 10m / 2h / 7d / 90（秒）或 ISO 8601 绝对时间');
}

/** 回收站保留时长（毫秒），上传时可覆盖 */
function archiveTtlMs(spec) {
  if (spec === undefined || spec === null || String(spec).trim() === '') {
    return msFromDuration(RECYCLE_TTL) || 7 * 86400000;
  }
  const ms = msFromDuration(spec);
  if (ms === null || ms <= 0) throw err(400, 'recycle 保留时长无效：可用 30m / 7d / 3600（秒）');
  return ms;
}

/* 标准 ZIP（单文件，deflate），零第三方依赖 */
const CRC_TABLE = (() => {
  const t = new Uint32Array(256);
  for (let n = 0; n < 256; n++) {
    let c = n;
    for (let k = 0; k < 8; k++) c = (c & 1) ? (0xEDB88320 ^ (c >>> 1)) : (c >>> 1);
    t[n] = c >>> 0;
  }
  return t;
})();
function crc32(buf) {
  let c = 0xFFFFFFFF;
  for (let i = 0; i < buf.length; i++) c = CRC_TABLE[(c ^ buf[i]) & 0xFF] ^ (c >>> 8);
  return (c ^ 0xFFFFFFFF) >>> 0;
}
function dosDateTime(d = new Date()) {
  const year = Math.max(1980, d.getFullYear());
  return {
    date: (((year - 1980) << 9) | ((d.getMonth() + 1) << 5) | d.getDate()) & 0xFFFF,
    time: ((d.getHours() << 11) | (d.getMinutes() << 5) | (d.getSeconds() >> 1)) & 0xFFFF,
  };
}
/** entryName 用 / 分隔，压缩包内保留原目录结构 */
function makeZip(entryName, data) {
  const name = Buffer.from(entryName, 'utf8');
  const packed = zlib.deflateRawSync(data, { level: 6 });
  const crc = crc32(data);
  const { date, time } = dosDateTime();
  const flags = 0x0800; // UTF-8 文件名

  const local = Buffer.alloc(30);
  local.writeUInt32LE(0x04034b50, 0);
  local.writeUInt16LE(20, 4);
  local.writeUInt16LE(flags, 6);
  local.writeUInt16LE(8, 8);         // deflate
  local.writeUInt16LE(time, 10);
  local.writeUInt16LE(date, 12);
  local.writeUInt32LE(crc, 14);
  local.writeUInt32LE(packed.length, 18);
  local.writeUInt32LE(data.length, 22);
  local.writeUInt16LE(name.length, 26);
  local.writeUInt16LE(0, 28);

  const central = Buffer.alloc(46);
  central.writeUInt32LE(0x02014b50, 0);
  central.writeUInt16LE(20, 4);
  central.writeUInt16LE(20, 6);
  central.writeUInt16LE(flags, 8);
  central.writeUInt16LE(8, 10);
  central.writeUInt16LE(time, 12);
  central.writeUInt16LE(date, 14);
  central.writeUInt32LE(crc, 16);
  central.writeUInt32LE(packed.length, 20);
  central.writeUInt32LE(data.length, 24);
  central.writeUInt16LE(name.length, 28);
  central.writeUInt16LE(0, 30);
  central.writeUInt16LE(0, 32);
  central.writeUInt16LE(0, 34);
  central.writeUInt16LE(0, 36);
  central.writeUInt32LE(0, 38);
  central.writeUInt32LE(0, 42);

  const eocd = Buffer.alloc(22);
  eocd.writeUInt32LE(0x06054b50, 0);
  eocd.writeUInt16LE(0, 4);
  eocd.writeUInt16LE(0, 6);
  eocd.writeUInt16LE(1, 8);
  eocd.writeUInt16LE(1, 10);
  eocd.writeUInt32LE(central.length + name.length, 12);
  eocd.writeUInt32LE(local.length + name.length + packed.length, 16);
  eocd.writeUInt16LE(0, 20);

  return Buffer.concat([local, name, packed, central, name, eocd]);
}

/* 元数据：重启后不丢失到期时间与回收站到期时间 */
let meta = { version: 1, files: {}, archives: {} };
let saveChain = Promise.resolve();
let sweeping = false;
let lastSweep = null;

async function loadMeta() {
  try {
    const obj = JSON.parse(await fsp.readFile(META_FILE, 'utf8'));
    meta = { version: 1, files: obj.files || {}, archives: obj.archives || {} };
    console.log(`  生命周期   已恢复 ${Object.keys(meta.files).length} 个待归档 / ${Object.keys(meta.archives).length} 个回收站记录`);
  } catch (e) {
    if (e.code !== 'ENOENT') console.error('读取生命周期元数据失败:', e.message);
  }
}
async function saveMeta() {
  const snapshot = JSON.stringify(meta, null, 2) + '\n';
  saveChain = saveChain.catch(() => {}).then(() => fsp.writeFile(META_FILE, snapshot, 'utf8'));
  try { await saveChain; }
  catch (e) { console.error('保存生命周期元数据失败:', e.message); }
}
function lifecycleConfig() {
  return {
    default_ttl: DEFAULT_TTL || null,
    recycle_ttl: RECYCLE_TTL,
    sweep_interval_ms: SWEEP_MS,
    storage_dir: STORAGE_DIR,
    recycle_dir: RECYCLE_DIR,
    meta_file: META_FILE,
    note: DEFAULT_TTL ? '所有上传文件默认到期归档' : '仅上传时显式指定 expires 的文件到期归档',
  };
}
/** 上传成功后登记（或清除）生命周期记录 */
function registerFile(rel, size, expiresSpec, recycleSpec) {
  const now = Date.now();
  const spec = (expiresSpec !== undefined && expiresSpec !== null && String(expiresSpec).trim() !== '')
    ? expiresSpec : DEFAULT_TTL;
  const expiresAt = parseExpires(spec, now);
  const recycleMs = archiveTtlMs(recycleSpec);
  if (!expiresAt) { delete meta.files[rel]; return null; }
  meta.files[rel] = { uploadedAt: now, expiresAt, recycleTtlMs: recycleMs, size };
  return meta.files[rel];
}
function makeArchiveName(rel, now) {
  const d = new Date(now);
  const p = (x) => String(x).padStart(2, '0');
  const stamp = `${d.getFullYear()}${p(d.getMonth() + 1)}${p(d.getDate())}-${p(d.getHours())}${p(d.getMinutes())}${p(d.getSeconds())}`;
  const base = path.posix.basename(rel).replace(/[^\w\u4e00-\u9fa5.\-]/g, '_').slice(0, 60);
  return `${stamp}_${base}_${Math.random().toString(36).slice(2, 8)}.zip`;
}
async function listArchives() {
  await fsp.mkdir(RECYCLE_DIR, { recursive: true });
  const names = await fsp.readdir(RECYCLE_DIR);
  const items = [];
  for (const name of names) {
    if (!name.toLowerCase().endsWith('.zip')) continue;
    const st = await statMaybe(path.join(RECYCLE_DIR, name));
    if (!st || !st.isFile()) continue;
    const info = meta.archives[name];
    const expiresAt = Number(info && info.expiresAt) || st.mtimeMs + (msFromDuration(RECYCLE_TTL) || 7 * 86400000);
    items.push({
      name,
      size: st.size,
      source: (info && info.source) || null,
      created_at: new Date((info && info.createdAt) || st.birthtimeMs || st.mtimeMs).toISOString(),
      expires_at: new Date(expiresAt).toISOString(),
      seconds_left: Math.max(0, Math.round((expiresAt - Date.now()) / 1000)),
      download_url: '/recycle/' + encodeURIComponent(name),
    });
  }
  items.sort((a, b) => b.created_at.localeCompare(a.created_at));
  return items;
}

/** 一次扫描：过期文件 -> ZIP -> 回收站；回收站超期 -> 删除压缩包 */
async function sweep() {
  if (sweeping) return { skipped: true, reason: '上一次扫描仍在进行' };
  sweeping = true;
  const result = { at: new Date().toISOString(), archived: [], purged: [], missing: [], errors: [] };
  try {
    await fsp.mkdir(RECYCLE_DIR, { recursive: true });
    const now = Date.now();

    for (const [rel, info] of Object.entries(meta.files)) {
      if (!info) { delete meta.files[rel]; continue; }
      let zipPath = null;
      try {
        const abs = absOf(safeRel(rel));
        await assertInside(abs);
        const st = await statMaybe(abs);
        if (!st || !st.isFile()) {
          // 被外部删除/覆盖成目录的记录直接清掉，避免「待归档」里留幽灵条目
          delete meta.files[rel];
          result.missing.push(rel);
          continue;
        }
        if (!(Number(info.expiresAt) <= now)) continue;
        const data = await fsp.readFile(abs);
        const name = makeArchiveName(rel, now);
        zipPath = path.join(RECYCLE_DIR, name);
        await fsp.writeFile(zipPath, makeZip(rel, data));
        await fsp.unlink(abs);
        const ttl = Number(info.recycleTtlMs) || msFromDuration(RECYCLE_TTL) || 7 * 86400000;
        meta.archives[name] = {
          createdAt: now,
          expiresAt: now + ttl,
          source: rel,
          size: (await fsp.stat(zipPath)).size,
        };
        delete meta.files[rel];
        result.archived.push({ path: rel, archive: name, expires_at: new Date(now + ttl).toISOString() });
      } catch (e) {
        if (zipPath) await fsp.unlink(zipPath).catch(() => {});
        result.errors.push({ path: rel, error: e.message });
      }
    }

    for (const name of await fsp.readdir(RECYCLE_DIR)) {
      if (!name.toLowerCase().endsWith('.zip')) continue;
      const abs = path.join(RECYCLE_DIR, name);
      const st = await statMaybe(abs);
      if (!st || !st.isFile()) { delete meta.archives[name]; continue; }
      const info = meta.archives[name];
      const expiresAt = Number(info && info.expiresAt) || st.mtimeMs + (msFromDuration(RECYCLE_TTL) || 7 * 86400000);
      if (!info) meta.archives[name] = { createdAt: st.birthtimeMs || st.mtimeMs, expiresAt, source: null, size: st.size };
      if (expiresAt <= now) {
        try { await fsp.unlink(abs); delete meta.archives[name]; result.purged.push(name); }
        catch (e) { result.errors.push({ path: name, error: e.message }); }
      }
    }
    for (const name of Object.keys(meta.archives)) {
      if (!(await statMaybe(path.join(RECYCLE_DIR, name)))) delete meta.archives[name];
    }
    await saveMeta();
    lastSweep = result;
    if (result.archived.length || result.purged.length) {
      console.log(`[生命周期] 归档 ${result.archived.length} 个文件，清理 ${result.purged.length} 个压缩包`);
    }
    return result;
  } finally {
    sweeping = false;
  }
}

/* ------------------------------------------------------------------ 帮助信息 */

function buildHelp(baseUrl) {
  const B = baseUrl.replace(/\/$/, '');
  return {
    service: '简易云盘 Mini Drive',
    version: VERSION,
    base_url: B,
    authentication: '无。不需要 token、cookie、账号或登录。',
    purpose: '让 AI Agent / 脚本上传、下载、浏览文件；浏览器页面只做目录列表与点击下载。',
    storage: '所有路径都相对服务端 storage/ 根目录，用 / 分隔，支持多级子目录。',
    rules: [
      '上传时父目录自动创建；同名文件直接覆盖（无版本管理）。',
      '下载一律返回 application/octet-stream + attachment，不做预览。',
      '路径禁止 .. 、绝对路径、盘符、隐藏文件（以 . 开头）和 Windows 非法字符。',
      '单个请求体积上限由 MAX_UPLOAD_MB 控制，默认 200 MB。',
      '上传可指定 expires（过期时间）与 recycle（回收站保留时长）；未指定 expires 且未配 DEFAULT_TTL 时文件永不自动归档。',
      '到期文件会被打成 ZIP 移入 recycle/ 并从原位置删除；回收站压缩包到期后自动删除，不可恢复，需提前下载。',
      '服务默认只监听 127.0.0.1；无认证，请勿直接暴露公网，必要时置于内网并加反向代理鉴权。',
    ],
    endpoints: [
      { method: 'GET', path: '/api/help', description: '本说明（JSON，给 Agent 读）' },
      { method: 'GET', path: '/llms.txt', description: '本说明（纯文本，给 Agent 读）' },
      { method: 'GET', path: '/help', description: '本说明（HTML 帮助页，给人读）' },
      { method: 'GET', path: '/api/health', description: '健康检查' },
      { method: 'GET', path: '/api/list?path=docs&recursive=1', description: '列出目录；path 省略为根目录；recursive=1 返回全部子目录文件（扁平）' },
      { method: 'GET', path: '/files/<path>', description: '下载文件（推荐，路径直接拼在 URL 上）' },
      { method: 'GET', path: '/api/download?path=docs/a.txt', description: '下载文件（query 形式）' },
      { method: 'PUT', path: '/files/<path>', description: '上传：请求体就是文件原始字节（推荐，等价于 curl -T）' },
      { method: 'PUT/POST', path: '/api/upload?path=docs/a.txt', description: '上传：请求体为文件原始字节；也可用 X-File-Path 头指定路径' },
      { method: 'POST', path: '/api/upload', description: '上传：multipart/form-data，字段 file（可多个）+ 可选 path（目标目录，默认根目录）' },
      { method: 'POST', path: '/api/mkdir?path=docs/2026', description: '创建空目录（通常不需要，上传会自动建目录）' },
      { method: 'GET', path: '/api/lifecycle', description: '生命周期状态：配置、待归档文件及到期时间、上次扫描结果' },
      { method: 'GET', path: '/api/recycle', description: '列出回收站压缩包（含到期删除时间）' },
      { method: 'GET', path: '/recycle/<name>.zip', description: '下载回收站里的压缩包' },
      { method: 'POST', path: '/api/sweep', description: '立即执行一次扫描（正常由定时器自动执行）' },
      { method: 'GET', path: '/', description: '浏览器文件列表页，点击文件名即可下载；右上角可切换回收站' },
    ],
    lifecycle: {
      summary: '上传时可设定过期时间；到期后文件被打成 ZIP 移入回收站；回收站压缩包再按保留时长到期自动删除。',
      set_time_on_upload: {
        query: '?expires=2h —— 相对时长 500ms/30s/10m/2h/7d/4w，纯数字按秒；也支持 ISO 8601 绝对时间',
        header: 'X-File-Expires: 2h',
        multipart: '-F "expires=2h"',
        recycle: '?recycle=3d / X-File-Recycle / -F "recycle=3d" —— 单独覆盖该文件归档包的回收站保留时长',
        default_ttl: 'DEFAULT_TTL 可给所有上传设默认过期时长；默认留空，即只有显式指定 expires 的文件才归档',
      },
      flow: [
        '上传指定 expires → 响应含 lifecycle=scheduled、expires_at；记录写入 lifecycle.json（重启不丢）',
        '到期扫描：原文件打成单个 ZIP（包内保留原相对目录结构），删除原文件，ZIP 放入 recycle/',
        '回收站 ZIP 超过 RECYCLE_TTL（默认 7d）自动删除；页面「回收站」可查看与下载',
      ],
      env: {
        DEFAULT_TTL: '全局默认过期时长，默认空（不自动过期）',
        RECYCLE_TTL: '回收站保留时长，默认 7d',
        SWEEP_INTERVAL: '扫描周期，默认 60s（最小 1s）',
        RECYCLE_DIR: '回收站目录，默认 <工程>/recycle',
        META_FILE: '元数据文件，默认 <工程>/lifecycle.json',
      },
    },
    examples: {
      curl: [
        `# 查看说明（Agent 第一步）\ncurl ${B}/api/help`,
        `# 上传文件到子目录（curl -T 走 PUT，最简）\ncurl -T ./report.pdf ${B}/files/reports/2026/report.pdf`,
        `# 上传（POST 原始字节）\ncurl --data-binary @./data.csv ${B}/api/upload?path=data/data.csv`,
        `# multipart 上传到指定目录\ncurl -F "path=reports/2026" -F "file=@./a.txt" -F "file=@./b.txt" ${B}/api/upload`,
        `# 列出全部文件\ncurl "${B}/api/list?recursive=1"`,
        `# 下载\ncurl -O -J ${B}/files/reports/2026/report.pdf`,
        `# 上传一个 2 小时后过期、归档包在回收站保留 3 天的文件\ncurl -T ./tmp.log "${B}/files/temp/tmp.log?expires=2h&recycle=3d"`,
        `# 查看生命周期状态 / 立即扫描 / 列出回收站\ncurl ${B}/api/lifecycle\ncurl -X POST ${B}/api/sweep\ncurl ${B}/api/recycle`,
      ],
      python: [
        `import requests\nB = "${B}"\nrequests.put(B + "/files/reports/a.txt", data=open("a.txt","rb").read())\nr = requests.get(B + "/files/reports/a.txt")\nprint(r.content)\nprint(requests.get(B + "/api/list?recursive=1").json())`,
      ],
    },
    agent_hint: `你是文件存取助手。先 GET ${B}/api/help 或 ${B}/llms.txt 了解接口；上传用 PUT ${B}/files/<相对路径>（body 为文件字节），下载用 GET 同一路径；列目录用 GET ${B}/api/list?recursive=1。无需任何认证信息。若文件只需临时存放，上传时在 URL 加 ?expires=2h&recycle=3d：到期会自动打成 ZIP 移入回收站、原文件删除，回收站 ZIP 到期自动删除；用 GET ${B}/api/recycle 查看归档包，GET ${B}/recycle/<name>.zip 下载。`,
  };
}

function buildLlmsTxt(h) {
  const B = h.base_url;
  return `# ${h.service} API 使用说明

> ${h.purpose}
> Base URL: ${B}
> 认证：${h.authentication}

## 规则
${h.rules.map((r, i) => `${i + 1}. ${r}`).join('\n')}

## 接口
${h.endpoints.map((e) => `- ${e.method} ${e.path} — ${e.description}`).join('\n')}

## 文件生命周期（过期归档 → 回收站 → 自动删除）
${h.lifecycle.summary}
设定方式（expires 支持 500ms/30s/10m/2h/7d/4w、纯数字秒、ISO 8601 绝对时间）：
  curl -T ./tmp.log "${B}/files/temp/tmp.log?expires=2h&recycle=3d"
  curl -H "X-File-Expires: 2h" --data-binary @./a.txt "${B}/api/upload?path=a.txt"
  curl -F "expires=2h" -F "recycle=3d" -F "file=@./a.txt" "${B}/api/upload"
未指定 expires 且未配 DEFAULT_TTL 的文件永不自动归档；recycle 可单独覆盖回收站保留时长。
流程：到期 → 打成单个 ZIP（包内保留原目录结构）→ 移入 recycle/ 并删除原文件 → ZIP 超过 RECYCLE_TTL（默认 7d）自动删除。元数据存 lifecycle.json，重启不丢。
查看与触发：GET ${B}/api/lifecycle（待归档与到期时间） · POST ${B}/api/sweep（立即扫描） · GET ${B}/api/recycle（压缩包列表） · GET ${B}/recycle/<name>.zip（下载归档包）
环境变量：DEFAULT_TTL（默认空）/ RECYCLE_TTL（7d）/ SWEEP_INTERVAL（60s）/ RECYCLE_DIR / META_FILE

## 最常用：上传 / 下载
上传（请求体 = 文件原始字节，父目录自动创建）：
  curl -T ./local.txt ${B}/files/docs/2026/local.txt

上传（multipart，可多文件，path = 目标目录）：
  curl -F "path=docs/2026" -F "file=@./a.txt" -F "file=@./b.txt" ${B}/api/upload

列出全部文件：
  curl "${B}/api/list?recursive=1"

下载：
  curl -O -J ${B}/files/docs/2026/local.txt

## 更多示例
${h.examples.curl.join('\n\n')}

## 响应格式
成功上传：{"ok":true,"path":"docs/a.txt","size":123,"download_url":"/files/docs/a.txt","lifecycle":"none","expires_at":null}
设定过期时返回 lifecycle="scheduled" + expires_at（ISO）+ recycle_ttl_ms；未设定返回 lifecycle="none"。
列目录：  {"ok":true,"path":"docs","count":2,"items":[{"name":"a.txt","type":"file","path":"docs/a.txt","size":123,"mtime":"..."}]}
失败：    {"ok":false,"error":"错误原因"}（HTTP 4xx/5xx）

## 给 AI Agent 的指令
${h.agent_hint}

人类可读帮助页：${B}/help  |  浏览器文件列表：${B}/
`;
}

function esc(s) {
  return String(s).replace(/[&<>"']/g, (c) => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;' })[c]);
}

function buildHelpHtml(h) {
  const B = h.base_url;
  return `<!doctype html>
<html lang="zh-CN"><head><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1">
<title>API 使用说明 · ${esc(h.service)}</title>
<link rel="stylesheet" href="/style.css"></head>
<body class="help-page"><main class="help-wrap">
<header class="help-head"><a href="/" class="back">← 返回文件列表</a>
<h1>${esc(h.service)} · API 使用说明</h1>
<p class="sub">${esc(h.purpose)}</p>
<div class="kv"><span>Base URL</span><code>${esc(B)}</code><span>认证</span><code>${esc(h.authentication)}</code></div>
<p class="tip">AI Agent 可直接读取机器可读版本：<a href="/api/help">/api/help</a>（JSON） · <a href="/llms.txt">/llms.txt</a>（纯文本）</p></header>

<section><h2>调用规则</h2><ol>${h.rules.map((r) => `<li>${esc(r)}</li>`).join('')}</ol></section>

<section><h2>接口一览</h2><div class="table-wrap"><table><thead><tr><th>方法</th><th>路径</th><th>说明</th></tr></thead><tbody>
${h.endpoints.map((e) => `<tr><td><code>${esc(e.method)}</code></td><td><code>${esc(e.path)}</code></td><td>${esc(e.description)}</td></tr>`).join('')}
</tbody></table></div></section>

<section><h2>文件生命周期（过期归档 → 回收站 → 自动删除）</h2>
<p>${esc(h.lifecycle.summary)}</p>
<h3>上传时怎么设时间</h3><div class="table-wrap"><table><thead><tr><th>方式</th><th>写法</th></tr></thead><tbody>
${Object.entries(h.lifecycle.set_time_on_upload).map(([k, v]) => `<tr><td><code>${esc(k)}</code></td><td><code>${esc(v)}</code></td></tr>`).join('')}
</tbody></table></div>
<h3>流程</h3><ol>${h.lifecycle.flow.map((f) => `<li>${esc(f)}</li>`).join('')}</ol>
<h3>环境变量</h3><div class="table-wrap"><table><thead><tr><th>变量</th><th>说明</th></tr></thead><tbody>
${Object.entries(h.lifecycle.env).map(([k, v]) => `<tr><td><code>${esc(k)}</code></td><td>${esc(v)}</td></tr>`).join('')}
</tbody></table></div>
<p class="tip">页面右上角「回收站」可查看并下载归档包；到期自动删除后不可恢复，重要文件请提前取回。</p></section>

<section><h2>curl 示例</h2>${h.examples.curl.map((c) => `<pre>${esc(c)}</pre>`).join('')}</section>

<section><h2>Python 示例</h2>${h.examples.python.map((c) => `<pre>${esc(c)}</pre>`).join('')}</section>

<section><h2>给 AI Agent 的提示词</h2><pre>${esc(h.agent_hint)}</pre></section>

<footer>Mini Drive v${esc(h.version)} · 无认证、零依赖 · <a href="/">文件列表</a></footer>
</main></body></html>`;
}

/* ------------------------------------------------------------------ 路由 */

async function handleUploadRaw(req, res, targetPath, opts = {}) {
  const body = await readBody(req);
  const saved = await saveFile(targetPath, body, opts);
  sendJson(res, 200, { ok: true, ...saved });
}

async function handleUploadMultipart(req, res, defaultDir, opts = {}) {
  const contentType = req.headers['content-type'] || '';
  const body = await readBody(req);
  const parts = parseMultipart(body, contentType);
  const field = (name) => {
    const p = parts.find((x) => x.name === name && x.filename === undefined);
    return p ? p.data.toString('utf8').trim() : undefined;
  };
  const dirPart = field('path');
  const baseDir = dirPart !== undefined ? safeRel(dirPart) : safeRel(defaultDir || '');
  const lifeOpts = {
    expires: opts.expires !== undefined ? opts.expires : field('expires'),
    recycle: opts.recycle !== undefined ? opts.recycle : field('recycle'),
  };
  const files = parts.filter((p) => p.filename !== undefined && p.filename !== '');
  if (!files.length) throw err(400, '没有收到文件字段，请使用 -F "file=@./a.txt"');
  const saved = [];
  for (const f of files) {
    const name = path.posix.basename(String(f.filename).replace(/\\/g, '/'));
    const rel = safeRel(baseDir ? `${baseDir}/${name}` : name);
    saved.push(await saveFile(rel, f.data, lifeOpts));
  }
  sendJson(res, 200, { ok: true, count: saved.length, files: saved });
}

async function route(req, res) {
  const u = new URL(req.url, 'http://localhost');
  const pathname = u.pathname;
  const method = req.method.toUpperCase();
  const baseUrl = `${req.headers['x-forwarded-proto'] || 'http'}://${req.headers.host || `127.0.0.1:${PORT}`}`;
  const lifeOpts = {
    expires: u.searchParams.get('expires') !== null ? u.searchParams.get('expires') : req.headers['x-file-expires'],
    recycle: u.searchParams.get('recycle') !== null ? u.searchParams.get('recycle') : req.headers['x-file-recycle'],
  };

  if (method === 'OPTIONS') { res.writeHead(204, CORS); res.end(); return; }
  if (pathname === '/favicon.ico') { res.writeHead(204, CORS); res.end(); return; }

  if (method === 'GET' || method === 'HEAD') {
    res._headOnly = method === 'HEAD';

    if (pathname === '/' || pathname === '/index.html') return serveStatic(res, 'index.html', 'text/html; charset=utf-8');
    if (pathname === '/app.js') return serveStatic(res, 'app.js', 'application/javascript; charset=utf-8');
    if (pathname === '/style.css') return serveStatic(res, 'style.css', 'text/css; charset=utf-8');
    if (pathname === '/api/health') return sendJson(res, 200, {
      ok: true, service: 'mini-drive', version: VERSION, storage: STORAGE_DIR, max_upload_bytes: MAX_BYTES,
      lifecycle: {
        default_ttl: DEFAULT_TTL || null, recycle_ttl: RECYCLE_TTL, sweep_interval_ms: SWEEP_MS,
        pending: Object.keys(meta.files).length, archives: Object.keys(meta.archives).length,
      },
    });
    if (pathname === '/api/lifecycle') {
      const pending = Object.entries(meta.files).map(([p, i]) => ({
        path: p,
        expires_at: new Date(i.expiresAt).toISOString(),
        seconds_left: Math.max(0, Math.round((i.expiresAt - Date.now()) / 1000)),
        recycle_ttl_ms: i.recycleTtlMs,
      }));
      return sendJson(res, 200, {
        ok: true, config: lifecycleConfig(),
        pending_count: pending.length, pending,
        archive_count: (await listArchives()).length, last_sweep: lastSweep,
      });
    }
    if (pathname === '/api/recycle') {
      const items = await listArchives();
      return sendJson(res, 200, { ok: true, ttl: RECYCLE_TTL, dir: RECYCLE_DIR, count: items.length, items });
    }
    if (pathname.startsWith('/recycle/')) {
      let name;
      try { name = decodeURIComponent(pathname.slice('/recycle/'.length)); }
      catch { throw err(400, '压缩包名编码无效'); }
      if (!/^[\w\u4e00-\u9fa5.\-]+\.zip$/i.test(name) || name.includes('..')) throw err(400, '非法的压缩包名');
      const abs = path.resolve(RECYCLE_DIR, name);
      if (!isInside(RECYCLE_DIR, abs)) throw err(400, '路径越界');
      const st = await statMaybe(abs);
      if (!st || !st.isFile()) throw err(404, '压缩包不存在或已被清理');
      res.writeHead(200, {
        'Content-Type': 'application/zip',
        'Content-Length': st.size,
        'Content-Disposition': contentDisposition(name),
        'Cache-Control': 'no-store', ...CORS,
      });
      if (res._headOnly) { res.end(); return; }
      const stream = fs.createReadStream(abs);
      stream.on('error', () => res.destroy());
      stream.pipe(res);
      return;
    }
    if (pathname === '/api/help') return sendJson(res, 200, { ok: true, ...buildHelp(baseUrl) });
    if (pathname === '/llms.txt') return sendText(res, 200, buildLlmsTxt(buildHelp(baseUrl)));
    if (pathname === '/help') return sendText(res, 200, buildHelpHtml(buildHelp(baseUrl)), 'text/html; charset=utf-8');

    if (pathname === '/api/list') {
      const result = await listDir(u.searchParams.get('path') || '', u.searchParams.get('recursive') === '1');
      return sendJson(res, 200, { ok: true, ...result });
    }
    if (pathname === '/api/download') return serveFile(req, res, u.searchParams.get('path') || '');
    if (pathname.startsWith('/files/')) {
      let rel;
      try { rel = decodeURIComponent(pathname.slice('/files/'.length)); }
      catch { throw err(400, '路径 URL 编码无效'); }
      return serveFile(req, res, rel);
    }
    throw err(404, '未找到该资源');
  }

  if (method === 'PUT' || method === 'POST') {
    const contentType = req.headers['content-type'] || '';
    const isMultipart = /^multipart\/form-data/i.test(contentType);

    if (method === 'POST' && pathname === '/api/sweep') {
      const report = await sweep();
      return sendJson(res, 200, { ok: true, ...report });
    }
    if (method === 'PUT' && pathname.startsWith('/files/')) {
      let rel;
      try { rel = decodeURIComponent(pathname.slice('/files/'.length)); }
      catch { throw err(400, '路径 URL 编码无效'); }
      return handleUploadRaw(req, res, rel, lifeOpts);
    }
    if (pathname === '/api/upload') {
      if (isMultipart) return handleUploadMultipart(req, res, u.searchParams.get('path') || '', lifeOpts);
      const target = u.searchParams.get('path') || req.headers['x-file-path'];
      if (!target) throw err(400, '缺少 path 参数，例如 ?path=docs/a.txt（或 X-File-Path 头）');
      return handleUploadRaw(req, res, target, lifeOpts);
    }
    if (method === 'POST' && pathname === '/api/mkdir') {
      const rel = safeRel(u.searchParams.get('path') || '');
      if (!rel) throw err(400, '缺少 path 参数');
      const abs = absOf(rel);
      await assertInside(abs);
      await fsp.mkdir(abs, { recursive: true });
      return sendJson(res, 200, { ok: true, path: rel });
    }
    throw err(404, '未找到该资源');
  }

  throw err(405, '不支持的 HTTP 方法');
}

const server = http.createServer((req, res) => {
  const started = Date.now();
  res.on('finish', () => {
    console.log(`[${new Date().toLocaleTimeString('zh-CN')}] ${req.method} ${decodeURI(req.url)} -> ${res.statusCode} (${Date.now() - started}ms)`);
  });
  route(req, res).catch((e) => {
    if (res.headersSent) { res.destroy(); return; }
    const status = e instanceof HttpError ? e.status : 500;
    if (!(e instanceof HttpError)) console.error(e);
    try {
      sendJson(res, status, { ok: false, error: e instanceof HttpError ? e.message : `服务器内部错误: ${e.message}` });
    } catch { /* 客户端已断开 */ }
  });
});

Promise.all([
  fsp.mkdir(STORAGE_DIR, { recursive: true }),
  fsp.mkdir(RECYCLE_DIR, { recursive: true }),
]).then(loadMeta).then(() => {
  server.listen(PORT, HOST, () => {
    const displayHost = HOST === '0.0.0.0' ? '0.0.0.0' : HOST;
    console.log('');
    console.log('  简易云盘 Mini Drive v' + VERSION);
    console.log('  ---------------------------------------------');
    console.log(`  页面       http://${HOST === '0.0.0.0' ? '127.0.0.1' : HOST}:${PORT}/`);
    console.log(`  API 帮助   http://${HOST === '0.0.0.0' ? '127.0.0.1' : HOST}:${PORT}/api/help`);
    console.log(`  llms.txt   http://${HOST === '0.0.0.0' ? '127.0.0.1' : HOST}:${PORT}/llms.txt`);
    console.log(`  存储目录   ${STORAGE_DIR}`);
    console.log(`  回收站     ${RECYCLE_DIR}`);
    console.log(`  生命周期   默认过期 ${DEFAULT_TTL || '未启用(仅显式指定)'} / 回收站保留 ${RECYCLE_TTL} / 扫描周期 ${SWEEP_MS / 1000}s`);
    console.log(`  监听       ${displayHost}:${PORT}  上限 ${Math.round(MAX_BYTES / 1024 / 1024)} MB/请求`);
    if (HOST === '0.0.0.0') {
      const ips = Object.values(os.networkInterfaces()).flat().filter((i) => i && i.family === 'IPv4' && !i.internal).map((i) => i.address);
      if (ips.length) console.log(`  局域网     ${ips.map((ip) => `http://${ip}:${PORT}/`).join('  ')}`);
      console.log('  ⚠ 无认证服务已对所有网卡开放，请确认处于可信内网');
    }
    console.log('');
  });

  sweep().catch((e) => console.error('[生命周期] 启动扫描失败:', e.message));
  setInterval(() => {
    sweep().catch((e) => console.error('[生命周期] 定时扫描失败:', e.message));
  }, SWEEP_MS).unref();
}).catch((e) => {
  console.error('启动失败:', e);
  process.exit(1);
});

server.on('error', (e) => {
  console.error('服务错误:', e.message);
  process.exit(1);
});
