<script>
  import { onMount } from 'svelte';

  let view = 'files';
  let currentPath = '';
  let items = [];
  let recycleItems = [];
  let expiryByPath = new Map();
  let expiryLoaded = false;
  let loading = false;
  let error = '';
  let notice = '';
  let pendingFiles = [];
  let targetDir = '';
  let expires = '';
  let recycleTtl = '';
  let dragging = false;
  let uploading = false;
  let fileInput;
  let copiedPath = '';
  let manualPrompt = '';
  let copyTimer;

  const encPath = (p) => String(p).split('/').map(encodeURIComponent).join('/');

  function fmtSize(n) {
    if (n === null || n === undefined) return '-';
    if (n < 1024) return n + ' B';
    if (n < 1024 * 1024) return (n / 1024).toFixed(1) + ' KB';
    if (n < 1024 * 1024 * 1024) return (n / 1024 / 1024).toFixed(1) + ' MB';
    return (n / 1024 / 1024 / 1024).toFixed(2) + ' GB';
  }

  function fmtTime(s) {
    if (!s) return '-';
    const d = new Date(s);
    return isNaN(d.getTime()) ? String(s) : d.toLocaleString('zh-CN', { hour12: false });
  }

  // 剩余秒数压成短标签：90 -> 1m，7200 -> 2h，90000 -> 1d 1h
  function fmtLeft(s) {
    if (s < 60) return s + 's';
    if (s < 3600) return Math.floor(s / 60) + 'm';
    if (s < 86400) return Math.floor(s / 3600) + 'h ' + Math.floor((s % 3600) / 60) + 'm';
    return Math.floor(s / 86400) + 'd ' + Math.floor((s % 86400) / 3600) + 'h';
  }

  // 回收站压缩包的过期时间：到期后 ZIP 会被自动删除
  function recycleExpiry(it) {
    const left = Number(it.seconds_left) || 0;
    return {
      text: fmtTime(it.expires_at),
      tag: left <= 0 ? '已到期' : '剩 ' + fmtLeft(left),
      cls: left <= 0 ? 'due' : left < 600 ? 'soon' : '',
    };
  }

  // 过期信息来自 /api/lifecycle 的元数据，按 path 与列表项对齐
  function expiryInfo(it) {
    if (it.type === 'dir') return { text: '-' };
    if (!expiryLoaded) return { tag: '—', title: '过期信息暂不可用' };
    const info = expiryByPath.get(it.path);
    if (!info) return { tag: '永久', title: '未设置过期时间，不会自动归档' };
    const left = Number(info.seconds_left) || 0;
    const tail = info.recycle_ttl ? '；归档后在回收站保留 ' + fmtLeft(Number(info.recycle_ttl)) : '';
    const title = '到期归档并移入回收站' + tail;
    if (left <= 0) return { text: fmtTime(info.expires_at), tag: '已到期', cls: 'due', title: title + '（等待下次扫描）' };
    return {
      text: fmtTime(info.expires_at),
      tag: '剩 ' + fmtLeft(left),
      cls: left < 600 ? 'soon' : '',
      title: title,
    };
  }

  function pathFromHash() {
    const raw = decodeURIComponent(location.hash.replace(/^#/, ''));
    if (raw === '/recycle' || raw.startsWith('/recycle/')) return { view: 'recycle', path: '' };
    return { view: 'files', path: raw.replace(/^\/+|\/+$/g, '') };
  }

  async function api(url, opts) {
    const r = await fetch(url, opts);
    let data = null;
    try { data = await r.json(); } catch { /* noop */ }
    if (!r.ok) throw new Error((data && data.error) || ('HTTP ' + r.status));
    return data;
  }

  async function load() {
    const h = pathFromHash();
    view = h.view;
    currentPath = h.path;
    loading = true;
    error = '';
    try {
      if (view === 'recycle') {
        recycleItems = ((await api('/api/recycle')).items || [])
          .map((it) => ({ ...it, expiry: recycleExpiry(it) }));
        items = [];
        expiryByPath = new Map();
        expiryLoaded = false;
      } else {
        const [data, lifecycle] = await Promise.all([
          api('/api/list?path=' + encodeURIComponent(currentPath)),
          api('/api/lifecycle').catch(() => null),
        ]);
        expiryLoaded = !!lifecycle;
        expiryByPath = new Map(((lifecycle && lifecycle.pending) || []).map((x) => [x.path, x]));
        items = (data.items || []).map((it) => ({ ...it, expiry: expiryInfo(it) }));
        recycleItems = [];
      }
    } catch (e) {
      error = e.message;
    } finally {
      loading = false;
    }
  }

  function goto(path) {
    location.hash = '#/' + path;
    if (pathFromHash().path === currentPath) load();
  }

  function setPending(list) {
    pendingFiles = Array.from(list || []);
    notice = '';
  }

  async function upload() {
    if (!pendingFiles.length) { error = '请先选择要上传的文件'; return; }
    uploading = true;
    error = '';
    notice = '';
    const dir = targetDir.trim().replace(/\\/g, '/').replace(/^\/+|\/+$/g, '');
    const q = new URLSearchParams();
    if (expires.trim()) q.set('expires', expires.trim());
    if (recycleTtl.trim()) q.set('recycle', recycleTtl.trim());
    const qs = q.toString() ? '?' + q.toString() : '';
    let ok = 0;
    const failed = [];
    for (const f of pendingFiles) {
      const rel = (dir ? dir + '/' : '') + f.name;
      try {
        await api('/files/' + encPath(rel) + qs, { method: 'PUT', body: f });
        ok++;
      } catch (e) {
        failed.push(f.name + ': ' + e.message);
      }
    }
    uploading = false;
    if (ok) notice = '已上传 ' + ok + ' 个文件' + (qs ? '（已设置时间参数）' : '') + '。';
    if (failed.length) error = failed.join('；');
    pendingFiles = [];
    if (fileInput) fileInput.value = '';
    await load();
  }

  function onDrop(e) {
    dragging = false;
    setPending(e.dataTransfer.files);
  }

  // 生成一段可直接贴给 AI Agent 的提示词：自带服务地址、说明入口和目标路径
  function buildPrompt(it) {
    const base = location.origin;
    const p = it.path;
    const head = [
      `Mini Drive 云盘（无需认证）：${base}`,
      `接口说明：GET ${base}/llms.txt`,
    ];
    if (it.type === 'dir') {
      return head.concat([
        `目标目录：${p}`,
        `列出：GET ${base}/api/list?path=${encodeURIComponent(p)}&recursive=1`,
        '请先列出该目录，再按我的要求处理。',
      ]).join('\n');
    }
    return head.concat([
      `目标文件：${p}`,
      `下载：GET ${base}/files/${encPath(p)}`,
      '请读取该文件并按我的要求处理。',
    ]).join('\n');
  }

  async function copyText(text) {
    try {
      if (navigator.clipboard && window.isSecureContext) {
        await navigator.clipboard.writeText(text);
        return true;
      }
    } catch { /* 落到兜底方案 */ }
    // 局域网 http:// 不是安全上下文，clipboard API 不可用，用 execCommand 兜底
    const ta = document.createElement('textarea');
    ta.value = text;
    ta.setAttribute('readonly', '');
    ta.style.cssText = 'position:fixed;top:-1000px;left:0;opacity:0';
    document.body.appendChild(ta);
    ta.select();
    let ok = false;
    try { ok = document.execCommand('copy'); } catch { ok = false; }
    document.body.removeChild(ta);
    return ok;
  }

  async function share(it) {
    const text = buildPrompt(it);
    const ok = await copyText(text);
    error = '';
    notice = '';
    if (ok) {
      manualPrompt = '';
      copiedPath = it.path;
      notice = `已复制「${it.name}」的 AI 提示词，直接粘贴给 Agent 即可。`;
      clearTimeout(copyTimer);
      copyTimer = setTimeout(() => { copiedPath = ''; }, 2500);
    } else {
      copiedPath = '';
      manualPrompt = text;
      error = '浏览器阻止了自动复制，请手动选中下面文本框里的内容复制。';
    }
  }

  // 删除 = 归档进回收站，原文件不可直接访问，但仍可从压缩包恢复
  async function removeItem(it) {
    const isDir = it.type === 'dir';
    const what = isDir ? `目录「${it.name}」及其中全部文件` : `文件「${it.name}」`;
    const msg = `确定删除${what}吗？\n\n`
      + '删除后内容会打包成 ZIP 移入回收站，原位置不再可访问。\n'
      + '可在回收站下载恢复，也可彻底删除；超过保留时间后会自动清除。';
    if (!window.confirm(msg)) return;
    try {
      const r = await api('/files/' + encPath(it.path), { method: 'DELETE' });
      error = '';
      notice = `已把「${it.name}」移入回收站（归档包含 ${r.file_count} 个文件）。`;
      const parent = currentPath.split('/').slice(0, -1).join('/');
      await load();
      // 删掉最后一个文件后空目录会被服务端清理，此时退回上一级
      if (error === '目录不存在' && currentPath) {
        location.hash = '#/' + parent;
        await load();
      }
    } catch (e) {
      error = '删除失败：' + e.message;
      notice = '';
    }
  }

  // 彻底删除 = 立即删除回收站里的 ZIP 与元数据，不可恢复
  async function purgeArchive(it) {
    const msg = `确定彻底删除压缩包「${it.name}」吗？\n\n`
      + `对应原文件：${it.source}\n`
      + 'ZIP 文件与记录会被立即永久删除，无法恢复。';
    if (!window.confirm(msg)) return;
    try {
      await api('/recycle/' + encodeURIComponent(it.name), { method: 'DELETE' });
      error = '';
      notice = `已彻底删除「${it.name}」。`;
      await load();
    } catch (e) {
      error = '彻底删除失败：' + e.message;
      notice = '';
    }
  }

  onMount(() => {
    load();
    window.addEventListener('hashchange', load);
    return () => window.removeEventListener('hashchange', load);
  });

  $: crumbs = currentPath
    ? currentPath.split('/').map((name, i, arr) => ({ name, path: arr.slice(0, i + 1).join('/') }))
    : [];
</script>

<div class="wrap">
  <header class="top">
    <div>
      <h1>Mini Drive</h1>
      <p>无认证简易云盘 · Rust 后端 / Svelte 前端 · 点击文件名即可下载</p>
    </div>
    <nav class="actions">
      <button class="btn" class:active={view === 'files'} on:click={() => goto('')}>文件列表</button>
      <button class="btn" class:active={view === 'recycle'} on:click={() => { location.hash = '#/recycle'; if (view === 'recycle') load(); }}>回收站</button>
      <a class="btn" href="/help" target="_blank" rel="noreferrer">API 使用说明</a>
      <button class="btn" on:click={load} disabled={loading}>刷新</button>
    </nav>
  </header>

  {#if view === 'files'}
    <section class="card">
      <h2>上传文件</h2>
      <div class="upload-grid">
        <label class="field">
          <span>上传到目录（留空 = 根目录）</span>
          <input type="text" placeholder="例如 reports/2026" bind:value={targetDir} autocomplete="off" />
        </label>
        <label class="field">
          <span>过期时间（可选）</span>
          <input type="text" placeholder="例如 2h / 7d / 2026-12-01T00:00:00Z" bind:value={expires} autocomplete="off" />
        </label>
        <label class="field">
          <span>回收站保留（可选）</span>
          <input type="text" placeholder="例如 3d，默认 7d" bind:value={recycleTtl} autocomplete="off" />
        </label>
      </div>
      <div
        role="region"
        aria-label="文件拖拽上传区域"
        class="dropzone"
        class:over={dragging}
        on:dragover|preventDefault={() => (dragging = true)}
        on:dragleave|preventDefault={() => (dragging = false)}
        on:drop|preventDefault={onDrop}
      >
        把文件拖到这里，或<strong>点击下面按钮选择多个文件</strong>
      </div>
      <label class="btn primary file-btn">
        选择文件
        <input bind:this={fileInput} type="file" multiple hidden on:change={(e) => setPending(e.target.files)} />
      </label>
      <button class="btn primary file-btn" on:click={upload} disabled={uploading || !pendingFiles.length}>
        {uploading ? '上传中…' : '开始上传'}
      </button>
      {#if pendingFiles.length}
        <div class="pending">待上传 {pendingFiles.length} 个：{pendingFiles.map((f) => f.name).join('、')}</div>
      {/if}
    </section>

    <section class="card">
      <div class="bar">
        <div class="crumbs">
          <button on:click={() => goto('')}>根目录</button>
          {#each crumbs as c, i}
            <span class="sep">/</span>
            {#if i === crumbs.length - 1}
              <b>{c.name}</b>
            {:else}
              <button on:click={() => goto(c.path)}>{c.name}</button>
            {/if}
          {/each}
        </div>
        <span class="muted">{loading ? '加载中…' : items.length + ' 项'}</span>
      </div>

      {#if error}<div class="error">{error}</div>{/if}
      {#if notice}<div class="notice">{notice}</div>{/if}

      {#if manualPrompt}
        <div class="prompt-fallback">
          <textarea aria-label="可手动复制的 AI 提示词" readonly value={manualPrompt}></textarea>
          <button class="btn" on:click={() => { manualPrompt = ''; error = ''; }}>关闭</button>
        </div>
      {/if}

      {#if items.length === 0 && !loading}
        <div class="empty">这里还没有文件，上传一个试试。</div>
      {:else}
        <table class="files">
          <thead>
            <tr><th>名称</th><th>类型</th><th>修改时间</th><th>过期时间</th><th style="text-align:right">大小</th><th></th></tr>
          </thead>
          <tbody>
            {#each items as it (it.path)}
              <tr>
                <td class="name">
                  {#if it.type === 'dir'}
                    <span class="icon">📁</span><a href={'#/' + it.path}>{it.name}</a>
                  {:else}
                    <span class="icon">📄</span><a href={'/files/' + encPath(it.path)} download={it.name}>{it.name}</a>
                  {/if}
                </td>
                <td class="muted">{it.type === 'dir' ? '目录' : '文件'}</td>
                <td class="muted">{fmtTime(it.mtime)}</td>
                <td class="expiry muted" title={it.expiry.title || ''}>
                  {#if it.expiry.text}<span class="abs">{it.expiry.text}</span>{/if}
                  {#if it.expiry.tag}<span class={'tag ' + (it.expiry.cls || '')}>{it.expiry.tag}</span>{/if}
                </td>
                <td class="num muted">{it.type === 'dir' ? '-' : fmtSize(it.size)}</td>
                <td class="num">
                  <div class="row-actions">
                    {#if it.type === 'dir'}
                      <button class="btn" on:click={() => goto(it.path)}>进入</button>
                    {:else}
                      <a class="btn" href={'/files/' + encPath(it.path)} download={it.name}>下载</a>
                    {/if}
                    <button
                      class="btn share"
                      class:copied={copiedPath === it.path}
                      title="复制一段可直接贴给 AI 的提示词，让它访问这个{it.type === 'dir' ? '目录' : '文件'}"
                      on:click={() => share(it)}
                    >{copiedPath === it.path ? '已复制' : '分享'}</button>
                    <button
                      class="btn danger"
                      title="删除后打包进回收站，可在回收站恢复或彻底删除"
                      on:click={() => removeItem(it)}
                    >删除</button>
                  </div>
                </td>
              </tr>
            {/each}
          </tbody>
        </table>
      {/if}
    </section>
  {:else}
    <section class="card">
      <div class="bar">
        <h2 style="margin:0">回收站 · 归档压缩包</h2>
        <span class="muted">{loading ? '加载中…' : recycleItems.length + ' 个压缩包'}</span>
      </div>
      <p class="muted" style="margin-top:0;font-size:13px">
        文件到期后会被打包成 ZIP 移到这里并删除原文件；超过保留时间后压缩包会自动删除。此页面只读，支持下载。
      </p>
      {#if error}<div class="error">{error}</div>{/if}
      {#if recycleItems.length === 0 && !loading}
        <div class="empty">回收站是空的。</div>
      {:else}
        <table class="recycle">
          <thead>
            <tr><th>压缩包</th><th>原文件</th><th>过期时间</th><th style="text-align:right">大小</th><th></th></tr>
          </thead>
          <tbody>
            {#each recycleItems as it (it.name)}
              <tr>
                <td class="name"><span class="icon">🗜️</span>{it.name}</td>
                <td class="muted">{it.source}</td>
                <td class="expiry muted" title="到期后压缩包会被自动删除，无法恢复">
                  <span class="abs">{it.expiry.text}</span>
                  <span class={'tag ' + it.expiry.cls}>{it.expiry.tag}</span>
                </td>
                <td class="num muted">{fmtSize(it.size)}</td>
                <td class="num">
                  <div class="row-actions">
                    <a class="btn" href={'/recycle/' + encodeURIComponent(it.name)} download={it.name}>下载</a>
                    <button class="btn danger" title="永久删除该压缩包，不可恢复" on:click={() => purgeArchive(it)}>彻底删除</button>
                  </div>
                </td>
              </tr>
            {/each}
          </tbody>
        </table>
      {/if}
    </section>
  {/if}

  <footer class="foot">
    <span>Mini Drive · 文件存于服务端 storage/，归档包存于 recycle/</span>
    <span>给 AI Agent：<a href="/api/help">/api/help</a> · <a href="/llms.txt">/llms.txt</a> · <a href="/help">/help</a></span>
  </footer>
</div>
