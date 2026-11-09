//! 给 AI Agent 的自助说明：/api/help (JSON)、/llms.txt (纯文本)、/help (HTML)。

use crate::util::VERSION;
use crate::Config;
use serde_json::json;

fn endpoints() -> Vec<(&'static str, &'static str, &'static str)> {
    vec![
        ("GET", "/api/help", "本说明（JSON，含 curl/Python 示例与 agent_hint）"),
        ("GET", "/llms.txt", "纯文本版使用说明，适合直接贴给 AI Agent"),
        ("GET", "/help", "浏览器可读的帮助页"),
        ("GET", "/api/health", "健康检查"),
        ("PUT", "/files/<相对路径>", "上传：请求体 = 文件原始字节，父目录自动创建，同名覆盖"),
        ("GET", "/files/<相对路径>", "下载文件"),
        ("DELETE", "/files/<相对路径>", "删除：打包成 ZIP 移入回收站并删除原文件/目录，可从回收站恢复"),
        ("GET", "/api/download?path=docs/a.txt", "下载文件（query 形式）"),
        ("HEAD", "/files/<相对路径>", "只取响应头"),
        ("POST", "/api/upload?path=<相对路径>", "上传原始字节；也可用 X-File-Path 头指定路径"),
        ("POST", "/api/upload", "multipart/form-data 多文件上传，字段名 file，可选 path 字段指定目录"),
        ("GET", "/api/list?path=&recursive=1", "列目录；recursive=1 递归列出全部文件"),
        ("POST", "/api/mkdir?path=docs/2026", "创建空目录（上传会自动建目录）"),
        ("GET", "/api/lifecycle", "待归档文件及到期时间、当前配置"),
        ("POST", "/api/sweep", "立即执行一次生命周期扫描"),
        ("GET", "/api/recycle", "回收站中的 ZIP 归档包列表（含到期时间）"),
        ("GET", "/recycle/<name>.zip", "下载归档包"),
        ("DELETE", "/recycle/<name>.zip", "彻底删除归档包：立即删除 ZIP 与记录，不可恢复"),
        ("GET", "/", "浏览器文件列表页"),
    ]
}

pub fn help_json(base: &str, cfg: &Config) -> serde_json::Value {
    let eps: Vec<serde_json::Value> = endpoints().iter()
        .map(|(m, p, d)| json!({"method": m, "path": p, "description": d}))
        .collect();
    let default_ttl = cfg.default_ttl_ms
        .map(|v| format!("{}s", v / 1000))
        .unwrap_or_else(|| "未设置（仅显式指定 expires 才归档）".into());
    json!({
        "service": "mini-drive",
        "version": VERSION,
        "runtime": "Rust + Svelte",
        "base_url": base,
        "authentication": "none",
        "authentication_note": "无需任何账号、token、cookie 或登录",
        "endpoints": eps,
        "upload_time_options": {
            "query": "?expires=2h&recycle=3d",
            "headers": ["X-File-Expires: 2h", "X-File-Recycle: 3d", "X-File-Path: docs/a.txt"],
            "multipart_fields": ["expires", "recycle", "path", "file"],
            "formats": "相对时长 30s/10m/2h/7d/4w、纯数字秒，或 ISO 8601 绝对时间",
        },
        "lifecycle": {
            "summary": "上传时可设置过期时间；到期后文件被打包成 ZIP 移入回收站并从原位置删除；也可 DELETE /files/<路径> 立即删除并归档（可恢复）；回收站中的 ZIP 超过保留时间后自动删除，或 DELETE /recycle/<name>.zip 立即彻底删除（不可恢复）。",
            "defaults": {
                "default_ttl": default_ttl,
                "recycle_ttl": format!("{}s", cfg.recycle_ttl_ms / 1000),
                "sweep_interval_ms": cfg.sweep_ms,
            },
            "zip": "归档包内保留原相对目录结构，UTF-8 文件名",
        },
        "examples": {
            "curl": [
                format!("# 上传（父目录自动创建，设 2 小时后归档、归档包保留 3 天）\ncurl -T ./report.pdf \"{}/files/reports/2026/report.pdf?expires=2h&recycle=3d\"", base),
                format!("# 不设过期时间，永久保留\ncurl -T ./a.txt \"{}/files/docs/a.txt\"", base),
                format!("# multipart 多文件\ncurl -F \"path=docs/2026\" -F \"file=@./a.txt\" -F \"file=@./b.txt\" {}/api/upload", base),
                format!("# 列出全部文件\ncurl \"{}/api/list?recursive=1\"", base),
                format!("# 下载\ncurl -O -J \"{}/files/reports/2026/report.pdf\"", base),
                format!("# 删除（原文件打包进回收站，可下载恢复）\ncurl -X DELETE \"{}/files/docs/a.txt\"", base),
                format!("# 查看回收站 / 彻底删除某个压缩包 / 立即扫描\ncurl \"{}/api/recycle\" ; curl -X DELETE \"{}/recycle/<name>.zip\" ; curl -X POST \"{}/api/sweep\"", base, base, base),
            ],
            "python": [
                "import urllib.request",
                format!("B = \"{}\"", base),
                "data = open('a.txt','rb').read()",
                "req = urllib.request.Request(B + '/files/docs/a.txt?expires=2h', data=data, method='PUT')",
                "print(urllib.request.urlopen(req).read().decode())   # {\"ok\":true,\"path\":...}",
                "print(urllib.request.urlopen(B + '/files/docs/a.txt').read())  # 下载",
            ],
        },
        "response_shapes": {
            "upload": "{\"ok\":true,\"path\":\"docs/a.txt\",\"size\":123,\"download_url\":\"/files/docs/a.txt\",\"lifecycle\":\"scheduled\",\"expires_at\":\"...\"}",
            "list": "{\"ok\":true,\"path\":\"docs\",\"count\":2,\"items\":[{\"name\":\"a.txt\",\"type\":\"file\",\"path\":\"docs/a.txt\",\"size\":123,\"mtime\":\"...\"}]}",
            "delete": "{\"ok\":true,\"path\":\"docs/a.txt\",\"kind\":\"file\",\"file_count\":1,\"archive\":\"20261109-...-a.txt-1a2b3c4d.zip\",\"size\":321,\"expires_at\":\"...\",\"download_url\":\"/recycle/20261109-...-a.txt-1a2b3c4d.zip\"}",
            "purge": "{\"ok\":true,\"deleted\":\"20261109-...-a.txt-1a2b3c4d.zip\"}",
            "error": "{\"ok\":false,\"error\":\"...\"}",
        },
        "agent_hint": format!("你是文件存取助手。先 GET {}/api/help 或 {}/llms.txt 了解接口；上传用 PUT {}/files/<相对路径>（body 为文件字节，可加 ?expires=2h&recycle=3d），下载用 GET 同一路径；列目录用 GET {}/api/list?recursive=1；回收站用 GET {}/api/recycle。无需任何认证信息。", base, base, base, base, base),
        "notes": [
            format!("无认证；单次请求体积上限 {} MB。", cfg.max_bytes / 1024 / 1024),
            "路径中的中文、空格需按 URL 规则编码；目录层级用 / 分隔。",
            "同名文件直接覆盖；DELETE /files/<路径> 会把内容归档进回收站（可下载恢复），DELETE /recycle/<name>.zip 与回收站到期清除均不可恢复。",
            "服务默认只监听 127.0.0.1；无认证，请勿直接暴露公网，必要时置于内网并加反向代理鉴权。",
        ],
    })
}

pub fn llms_txt(base: &str, cfg: &Config) -> String {
    let mut s = String::new();
    s.push_str("# Mini Drive - 简易云盘 API（供 AI Agent 读取）\n\n");
    s.push_str(&format!("Base URL: {}\n运行时: Rust {} + Svelte\n认证: 无（不需要任何 token / 账号 / 登录）\n\n", base, VERSION));
    s.push_str("## 接口一览\n");
    for (m, p, d) in endpoints() {
        s.push_str(&format!("- {:5} {:42} {}\n", m, p, d));
    }
    s.push_str("\n## 最常用：上传 / 下载\n");
    s.push_str("上传（请求体 = 文件原始字节，父目录自动创建，同名覆盖）：\n");
    s.push_str(&format!("  curl -T ./a.txt \"{}/files/docs/a.txt\"\n", base));
    s.push_str(&format!("  curl -T ./tmp.log \"{}/files/temp/tmp.log?expires=2h&recycle=3d\"\n", base));
    s.push_str("下载：\n");
    s.push_str(&format!("  curl -O -J \"{}/files/docs/a.txt\"\n", base));
    s.push_str("列出全部文件：\n");
    s.push_str(&format!("  curl \"{}/api/list?recursive=1\"\n", base));
    s.push_str("删除（原文件打包进回收站，可下载恢复）：\n");
    s.push_str(&format!("  curl -X DELETE \"{}/files/docs/a.txt\"\n", base));
    s.push_str("彻底删除回收站压缩包（不可恢复）：\n");
    s.push_str(&format!("  curl -X DELETE \"{}/recycle/<name>.zip\"\n\n", base));
    s.push_str("## 文件生命周期\n");
    s.push_str("上传时可设过期时间；到期后文件打成 ZIP 移入回收站并从 storage/ 删除；\n");
    s.push_str("也可 DELETE /files/<路径>（文件或整个目录）立即归档，效果相同且可恢复。\n");
    s.push_str("回收站里的 ZIP 超过保留时间后自动删除，或 DELETE /recycle/<name>.zip 立即彻底删除（均不可恢复）。\n");
    s.push_str("设时间的方式（相对 30s/10m/2h/7d/4w、纯数字秒、ISO 绝对时间）：\n");
    s.push_str("  ?expires=2h&recycle=3d\n  X-File-Expires: 2h / X-File-Recycle: 3d\n  multipart 字段 expires / recycle\n");
    s.push_str(&format!("全局默认：DEFAULT_TTL={}，RECYCLE_TTL={}s，扫描间隔 {}ms。\n",
        cfg.default_ttl_ms.map(|v| format!("{}s", v / 1000)).unwrap_or_else(|| "未设置".into()),
        cfg.recycle_ttl_ms / 1000, cfg.sweep_ms));
    s.push_str("相关接口：GET /api/lifecycle、GET /api/recycle、GET/DELETE /recycle/<name>.zip、DELETE /files/<路径>、POST /api/sweep\n\n");
    s.push_str("## 响应示例\n");
    s.push_str("成功上传： {\"ok\":true,\"path\":\"docs/a.txt\",\"size\":123,\"download_url\":\"/files/docs/a.txt\",\"lifecycle\":\"scheduled\",\"expires_at\":\"...\"}\n");
    s.push_str("列目录：   {\"ok\":true,\"path\":\"docs\",\"count\":2,\"items\":[{\"name\":\"a.txt\",\"type\":\"file\",\"path\":\"docs/a.txt\",\"size\":123,\"mtime\":\"...\"}]}\n");
    s.push_str("错误：     {\"ok\":false,\"error\":\"...\"}\n\n");
    s.push_str("## 提示\n");
    s.push_str(&format!("1. 完整 JSON 说明见 {}/api/help，网页版见 {}/help。\n", base, base));
    s.push_str("2. 无需任何认证；路径中的中文/空格需 URL 编码。\n");
    s.push_str(&format!("3. 单次请求上限 {} MB；同名文件直接覆盖。\n", cfg.max_bytes / 1024 / 1024));
    s.push_str("4. 归档会删除原文件但可从回收站下载恢复；彻底删除或回收站到期清除后不可恢复，重要文件请提前下载。\n");
    s.push_str("5. 服务默认只监听本机；无认证，勿直接暴露公网。\n");
    s
}

fn esc(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

pub fn help_html(base: &str, cfg: &Config) -> String {
    let h = help_json(base, cfg);
    let eps = h["endpoints"].as_array().unwrap();
    let rows: String = eps.iter().map(|e| format!(
        "<tr><td><code>{}</code></td><td><code>{}</code></td><td>{}</td></tr>",
        esc(e["method"].as_str().unwrap()), esc(e["path"].as_str().unwrap()), esc(e["description"].as_str().unwrap()))).collect();
    let curl: String = h["examples"]["curl"].as_array().unwrap().iter()
        .map(|c| format!("<pre>{}</pre>", esc(c.as_str().unwrap()))).collect();
    let py: String = h["examples"]["python"].as_array().unwrap().iter()
        .map(|c| format!("<pre>{}</pre>", esc(c.as_str().unwrap()))).collect();
    format!(r#"<!doctype html><html lang="zh-CN"><head><meta charset="utf-8">
<meta name="viewport" content="width=device-width,initial-scale=1"><title>Mini Drive API 使用说明</title>
<style>
:root{{color-scheme:light}}body{{margin:0;background:#f6f7f9;color:#1f2328;font:15px/1.7 -apple-system,Segoe UI,Microsoft YaHei,sans-serif}}
.wrap{{max-width:1000px;margin:0 auto;padding:32px 20px 64px}}h1{{font-size:26px;margin:0 0 6px}}h2{{font-size:19px;margin:32px 0 12px;padding-bottom:8px;border-bottom:1px solid #e3e6ea}}
a{{color:#2563eb}}code{{background:#eef1f5;padding:2px 6px;border-radius:4px;font-size:13px}}
pre{{background:#fff;border:1px solid #e3e6ea;border-radius:8px;padding:12px 14px;overflow:auto;font-size:13px;line-height:1.5}}
table{{width:100%;border-collapse:collapse;background:#fff;border:1px solid #e3e6ea;border-radius:8px;overflow:hidden}}
td,th{{padding:9px 12px;border-bottom:1px solid #eef0f3;text-align:left;vertical-align:top}}th{{background:#fafbfc;font-weight:600}}
.card{{background:#fff;border:1px solid #e3e6ea;border-radius:8px;padding:14px 18px;margin:12px 0}}
.badge{{display:inline-block;background:#e8f5e9;color:#1b7f3b;border-radius:20px;padding:2px 12px;font-size:13px}}
</style></head><body><div class="wrap">
<h1>Mini Drive · API 使用说明</h1>
<p><span class="badge">无需认证</span> Base URL：<code>{base}</code> · 版本 {ver} · Rust + Svelte</p>
<div class="card">{agent_hint}</div>
<h2>接口一览</h2><table><tr><th>方法</th><th>路径</th><th>说明</th></tr>{rows}</table>
<h2>文件生命周期</h2><div class="card"><p>{lifecycle}</p><p>设时间：<code>?expires=2h&amp;recycle=3d</code>、请求头 <code>X-File-Expires</code> / <code>X-File-Recycle</code>、或 multipart 字段。支持 30s/10m/2h/7d/4w、纯数字秒、ISO 绝对时间。</p><p>默认 TTL：<code>{ttl}</code> · 回收站保留：<code>{rttl}s</code> · 扫描间隔：<code>{sweep}ms</code> · 单次上限 <code>{max} MB</code></p></div>
<h2>curl 示例</h2>{curl}
<h2>Python 示例</h2>{py}
<h2>返回结构</h2><div class="card"><pre>{shapes}</pre></div>
<h2>其他入口</h2><p><a href="/">文件列表页</a> · <a href="/api/help">/api/help (JSON)</a> · <a href="/llms.txt">/llms.txt (纯文本)</a> · <a href="/api/recycle">/api/recycle</a></p>
</div></body></html>"#,
        base = esc(base), ver = VERSION,
        agent_hint = esc(h["agent_hint"].as_str().unwrap()),
        lifecycle = esc(h["lifecycle"]["summary"].as_str().unwrap()),
        ttl = esc(&cfg.default_ttl_ms.map(|v| format!("{}s", v / 1000)).unwrap_or_else(|| "未设置".into())),
        rttl = cfg.recycle_ttl_ms / 1000, sweep = cfg.sweep_ms, max = cfg.max_bytes / 1024 / 1024,
        rows = rows, curl = curl, py = py,
        shapes = esc(&serde_json::to_string_pretty(&h["response_shapes"]).unwrap_or_default()),
    )
}
