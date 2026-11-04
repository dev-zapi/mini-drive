<script>
  import { onMount } from 'svelte';

  let view = 'files';
  let currentPath = '';
  let items = [];
  let recycleItems = [];
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
        recycleItems = (await api('/api/recycle')).items || [];
        items = [];
      } else {
        const data = await api('/api/list?path=' + encodeURIComponent(currentPath));
        items = data.items || [];
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

      {#if items.length === 0 && !loading}
        <div class="empty">这里还没有文件，上传一个试试。</div>
      {:else}
        <table>
          <thead>
            <tr><th>名称</th><th>类型</th><th>修改时间</th><th style="text-align:right">大小</th><th></th></tr>
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
                <td class="num muted">{it.type === 'dir' ? '-' : fmtSize(it.size)}</td>
                <td class="num">
                  {#if it.type === 'dir'}
                    <button class="btn" on:click={() => goto(it.path)}>进入</button>
                  {:else}
                    <a class="btn" href={'/files/' + encPath(it.path)} download={it.name}>下载</a>
                  {/if}
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
        <table>
          <thead>
            <tr><th>压缩包</th><th>原文件</th><th>到期时间</th><th style="text-align:right">大小</th><th></th></tr>
          </thead>
          <tbody>
            {#each recycleItems as it (it.name)}
              <tr>
                <td class="name"><span class="icon">🗜️</span>{it.name}</td>
                <td class="muted">{it.source}</td>
                <td class="muted">{fmtTime(it.expires_at)} <span class="tag">剩 {it.seconds_left}s</span></td>
                <td class="num muted">{fmtSize(it.size)}</td>
                <td class="num"><a class="btn" href={'/recycle/' + encodeURIComponent(it.name)} download={it.name}>下载</a></td>
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
