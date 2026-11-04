'use strict';

const $ = (id) => document.getElementById(id);
const tbody = $('tbody');
const crumbs = $('crumbs');
const stats = $('stats');
const empty = $('empty');
const errorBox = $('error');
const fileInput = $('fileInput');
const targetDir = $('targetDir');
const uploadBtn = $('uploadBtn');
const selectedInfo = $('selectedInfo');
const uploadInfo = $('uploadInfo');
const dropzone = $('dropzone');
const ZIP_ICON = '<svg viewBox="0 0 24 24" width="18" height="18" fill="none" stroke="currentColor" stroke-width="1.7" stroke-linecap="round" stroke-linejoin="round"><path d="M5 3h11l3 3v15H5V3Z"/><path d="M12 3v3M12 8v2M12 12v2M12 16v2"/></svg>';

let currentPath = '';
let pendingFiles = [];
let view = 'main';

function encPath(p) {
  return p.split('/').map(encodeURIComponent).join('/');
}
function humanSize(n) {
  if (n === null || n === undefined) return '—';
  if (n < 1024) return n + ' B';
  if (n < 1024 * 1024) return (n / 1024).toFixed(1) + ' KB';
  if (n < 1024 * 1024 * 1024) return (n / 1024 / 1024).toFixed(1) + ' MB';
  return (n / 1024 / 1024 / 1024).toFixed(2) + ' GB';
}
function humanTime(iso) {
  if (!iso) return '—';
  const d = new Date(iso);
  const p = (x) => String(x).padStart(2, '0');
  return `${d.getFullYear()}-${p(d.getMonth() + 1)}-${p(d.getDate())} ${p(d.getHours())}:${p(d.getMinutes())}`;
}
function readHash() {
  const h = decodeURIComponent(location.hash.replace(/^#\/?/, '') || '');
  return h.replace(/\\/g, '/').replace(/^\/+|\/+$/g, '');
}
function goto(p) {
  location.hash = '#/' + encPath(p);
}

function renderCrumbs(p) {
  const parts = p ? p.split('/') : [];
  let acc = '';
  let html = '<button class="crumb" data-path="">根目录</button>';
  for (const seg of parts) {
    acc = acc ? acc + '/' + seg : seg;
    html += `<span class="sep">/</span><button class="crumb" data-path="${encodeURIComponent(acc)}">${escapeHtml(seg)}</button>`;
  }
  crumbs.innerHTML = html;
  crumbs.querySelectorAll('.crumb').forEach((b) => {
    b.addEventListener('click', () => goto(decodeURIComponent(b.dataset.path)));
  });
}
function escapeHtml(s) {
  return String(s).replace(/[&<>"']/g, (c) => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;' }[c]));
}

function render(data) {
  const items = data.items || [];
  tbody.innerHTML = items.map((it) => {
    const isDir = it.type === 'dir';
    const icon = isDir
      ? '<svg viewBox="0 0 24 24" width="18" height="18" fill="none" stroke="currentColor" stroke-width="1.7" stroke-linecap="round" stroke-linejoin="round"><path d="M3 7a2 2 0 0 1 2-2h4l2 2h8a2 2 0 0 1 2 2v9a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2V7Z"/></svg>'
      : '<svg viewBox="0 0 24 24" width="18" height="18" fill="none" stroke="currentColor" stroke-width="1.7" stroke-linecap="round" stroke-linejoin="round"><path d="M14 3H7a2 2 0 0 0-2 2v14a2 2 0 0 0 2 2h10a2 2 0 0 0 2-2V8l-5-5Z"/><path d="M14 3v5h5"/></svg>';
    const href = isDir ? '#' : '/files/' + encPath(it.path);
    const nameCell = isDir
      ? `<button class="name dir" data-path="${encodeURIComponent(it.path)}"><span class="icon dir-icon">${icon}</span><span>${escapeHtml(it.name)}</span></button>`
      : `<a class="name file" href="${href}" download="${escapeHtml(it.name)}"><span class="icon">${icon}</span><span>${escapeHtml(it.name)}</span></a>`;
    const action = isDir
      ? '<span class="muted">打开</span>'
      : `<a class="dl" href="${href}" download="${escapeHtml(it.name)}">下载</a>`;
    return `<tr><td>${nameCell}</td><td class="col-size muted">${isDir ? '—' : humanSize(it.size)}</td><td class="col-time muted">${humanTime(it.mtime)}</td><td class="col-act">${action}</td></tr>`;
  }).join('');

  tbody.querySelectorAll('.name.dir').forEach((b) => {
    b.addEventListener('click', () => goto(decodeURIComponent(b.dataset.path)));
  });
  const dirs = items.filter((i) => i.type === 'dir').length;
  const files = items.filter((i) => i.type === 'file').length;
  const bytes = items.reduce((s, i) => s + (i.size || 0), 0);
  stats.textContent = `${dirs} 个目录 · ${files} 个文件 · ${humanSize(bytes)}`;
  empty.hidden = items.length > 0;
  errorBox.hidden = true;
}

function humanLeft(s) {
  if (s <= 0) return '即将删除';
  if (s < 60) return `剩 ${s}s`;
  if (s < 3600) return `剩 ${Math.floor(s / 60)}m`;
  if (s < 86400) return `剩 ${Math.floor(s / 3600)}h`;
  return `剩 ${Math.floor(s / 86400)}d`;
}

function setView(v) {
  view = v;
  const recycle = v === 'recycle';
  dropzone.hidden = recycle;
  $('listCard').hidden = recycle;
  $('recycleCard').hidden = !recycle;
  $('recycleBtn').textContent = recycle ? '← 文件列表' : '回收站';
  if (recycle) loadRecycle();
  else load();
  loadLifecycleInfo();
}

async function loadRecycle() {
  try {
    const r = await fetch('/api/recycle', { cache: 'no-store' });
    const data = await r.json();
    if (!r.ok) throw new Error(data.error || '读取失败');
    const items = data.items || [];
    $('recycleBody').innerHTML = items.map((it) => {
      const name = escapeHtml(it.name);
      const href = it.download_url;
      return `<tr>
        <td><a class="name file" href="${href}" download="${name}"><span class="icon">${ZIP_ICON}</span><span>${name}</span></a></td>
        <td class="muted">${it.source ? escapeHtml(it.source) : '—'}</td>
        <td class="col-size muted">${humanSize(it.size)}</td>
        <td class="col-time muted">${humanTime(it.expires_at)} <span class="left">${humanLeft(it.seconds_left)}</span></td>
        <td class="col-act"><a class="dl" href="${href}" download="${name}">下载</a></td>
      </tr>`;
    }).join('');
    $('recycleEmpty').hidden = items.length > 0;
    $('recycleStats').textContent = `${items.length} 个压缩包 · 保留 ${data.ttl}`;
  } catch (e) {
    $('recycleBody').innerHTML = '';
    $('recycleEmpty').hidden = false;
    $('recycleStats').textContent = '加载失败：' + e.message;
  }
}

async function loadLifecycleInfo() {
  try {
    const r = await fetch('/api/lifecycle', { cache: 'no-store' });
    const d = await r.json();
    if (!r.ok) return;
    $('lifecycleInfo').textContent = `自动过期 ${d.config.default_ttl || '仅显式指定'} · 回收站保留 ${d.config.recycle_ttl} · 待归档 ${d.pending_count}`;
  } catch { /* 状态条不影响主流程 */ }
}

async function load() {
  currentPath = readHash();
  targetDir.value = currentPath;
  renderCrumbs(currentPath);
  try {
    const r = await fetch('/api/list?path=' + encodeURIComponent(currentPath), { cache: 'no-store' });
    const data = await r.json();
    if (!r.ok) throw new Error(data.error || '读取失败');
    render(data);
  } catch (e) {
    tbody.innerHTML = '';
    empty.hidden = true;
    errorBox.hidden = false;
    errorBox.textContent = '加载失败：' + e.message;
  }
}

function setPending(files) {
  pendingFiles = Array.from(files || []);
  uploadBtn.disabled = pendingFiles.length === 0;
  selectedInfo.textContent = pendingFiles.length
    ? `已选择 ${pendingFiles.length} 个文件：${pendingFiles.slice(0, 3).map((f) => f.name).join('、')}${pendingFiles.length > 3 ? ' …' : ''}`
    : '未选择文件 · 也可直接把文件拖到这里';
}

async function upload() {
  if (!pendingFiles.length) return;
  uploadBtn.disabled = true;
  const dir = targetDir.value.trim().replace(/\\/g, '/').replace(/^\/+|\/+$/g, '');
  const query = new URLSearchParams();
  const exp = $('expiresInput').value.trim();
  const rec = $('recycleInput').value.trim();
  if (exp) query.set('expires', exp);
  if (rec) query.set('recycle', rec);
  const qs = query.toString() ? '?' + query.toString() : '';
  let ok = 0;
  const failed = [];
  for (const f of pendingFiles) {
    const rel = (dir ? dir + '/' : '') + f.name;
    uploadInfo.textContent = `上传中 ${ok + 1}/${pendingFiles.length}：${f.name}`;
    try {
      const r = await fetch('/files/' + encPath(rel) + qs, { method: 'PUT', body: f });
      const data = await r.json();
      if (!r.ok) throw new Error(data.error || r.statusText);
      ok++;
    } catch (e) {
      failed.push(`${f.name}：${e.message}`);
    }
  }
  uploadInfo.textContent = ok ? `已上传 ${ok} 个文件${failed.length ? '，失败 ' + failed.length + ' 个' : ''}` : '上传失败';
  uploadInfo.title = failed.join('\n');
  if (failed.length) uploadInfo.classList.add('bad');
  else uploadInfo.classList.remove('bad');
  setPending([]);
  fileInput.value = '';
  if (ok) {
    if (dir && dir !== currentPath) goto(dir);
    else await load();
  }
}

fileInput.addEventListener('change', () => setPending(fileInput.files));
uploadBtn.addEventListener('click', upload);
$('refresh').addEventListener('click', () => view === 'recycle' ? loadRecycle() : load());
$('recycleBtn').addEventListener('click', () => setView(view === 'recycle' ? 'main' : 'recycle'));
$('backBtn').addEventListener('click', () => setView('main'));
window.addEventListener('hashchange', () => { if (view === 'main') load(); });

['dragenter', 'dragover'].forEach((ev) => dropzone.addEventListener(ev, (e) => {
  e.preventDefault(); dropzone.classList.add('drag');
}));
['dragleave', 'drop'].forEach((ev) => dropzone.addEventListener(ev, (e) => {
  e.preventDefault(); dropzone.classList.remove('drag');
}));
dropzone.addEventListener('drop', (e) => setPending(e.dataTransfer.files));

load();
loadLifecycleInfo();
