# 简易云盘 Mini Drive

零依赖、无认证的本地文件服务：给 AI Agent / 脚本用的极简上传下载 API，加一个浏览器文件列表页。

- **无登录**：没有账号、token、cookie、数据库。
- **文件即目录**：所有文件直接落在 `storage/`，子目录与 API 路径一一对应，可直接用资源管理器查看。
- **无预览 / 无改名 / 无删除**：页面只做「列目录 + 点击下载 + 简单上传 + 只读回收站」。
- **文件生命周期**：上传可设过期时间与回收站保留时长；到期文件自动打成 ZIP 移入回收站，压缩包超期自动删除。
- **Agent 自助说明**：`GET /api/help`（JSON）、`GET /llms.txt`（纯文本）、`GET /help`（HTML 帮助页）。

## 启动

需要 Node.js 18+（无需 `npm install`，零第三方依赖）。

```bash
cd mini-drive
npm start
```

或 Windows 双击 `start.bat`；macOS / Linux 执行 `./start.sh`。

默认地址：<http://127.0.0.1:8787/>

```bash
PORT=9000 HOST=0.0.0.0 DATA_DIR=/data/drive MAX_UPLOAD_MB=500 npm start
```

| 变量 | 默认值 | 说明 |
| --- | --- | --- |
| `PORT` | `8787` | 监听端口 |
| `HOST` | `127.0.0.1` | 仅本机；局域网/给别的机器调用时设 `0.0.0.0` |
| `DATA_DIR` | `<工程>/storage` | 文件实际存放目录 |
| `MAX_UPLOAD_MB` | `200` | 单次请求体积上限（MB） |
| `DEFAULT_TTL` | 空 | 全局默认过期时长（如 `24h`）；留空则**只有上传时显式指定 `expires` 的文件**才自动归档 |
| `RECYCLE_TTL` | `7d` | 回收站压缩包的保留时长，超期自动删除 |
| `SWEEP_INTERVAL` | `60s` | 生命周期扫描周期，最小 `1s` |
| `RECYCLE_DIR` | `<工程>/recycle` | 回收站目录 |
| `META_FILE` | `<工程>/lifecycle.json` | 到期时间等元数据（重启不丢） |

时长格式：`500ms` / `30s` / `10m` / `2h` / `7d` / `4w`，纯数字按秒；`expires` 另支持 ISO 8601 绝对时间。

启动后终端会打印页面地址、`/api/help`、`/llms.txt`、存储目录、回收站目录与生命周期配置；`HOST=0.0.0.0` 时还会打印局域网地址。

## 局域网 / 远程 Agent 访问

默认 `HOST=127.0.0.1`，**只有本机能访问**。要让局域网里的 AI Agent / 其他机器调用，把服务绑定到所有网卡：

- Windows：双击 `start-lan.bat`（内部设置 `HOST=0.0.0.0`）
- macOS / Linux：`./start-lan.sh`
- 或命令行：`HOST=0.0.0.0 npm start`（Windows cmd：`set HOST=0.0.0.0 && npm start`）

启动后终端会打印本机局域网地址，例如 `http://192.168.110.226:8787/`。给 Agent 的说明地址换成该地址即可：

```
先读取 http://192.168.110.226:8787/llms.txt ，按其说明上传和下载文件。
```

`/api/help` 与 `/llms.txt` 会依据请求的 Host 头生成 `base_url`，所以从哪台机器访问，示例里的地址就是哪台机器该用的地址。

**Windows 防火墙**：若别的机器连不上（本机用局域网 IP 能通、其他机器超时），多半是入站被拦。以管理员身份放行 TCP 8787：

```bat
netsh advfirewall firewall add rule name="Mini Drive 8787" dir=in action=allow protocol=TCP localport=8787
```

撤销：`netsh advfirewall firewall delete rule name="Mini Drive 8787"`。

> 服务无认证，开放后**同网段任何设备都能上传、下载、覆盖文件**，仅可用于可信内网。用完改用 `start.bat` / `npm start` 即恢复仅本机。

## 让 AI Agent 接入

把下面任意一条发给 Agent 即可，它会自己读到完整接口说明：

```
先读取 http://127.0.0.1:8787/llms.txt ，按其说明上传和下载文件。
```

```
先读取 http://127.0.0.1:8787/api/help （JSON）了解这个云盘的用法。
```

也可以直接把项目根目录的 `llms.txt` 作为上下文贴给 Agent。运行时访问 `/llms.txt` 与 `/api/help` 会**自动带上当前实际 host 和端口**，比静态文件更准确。

## API 一览

所有 `path` 都是相对 `storage/` 的路径，用 `/` 分隔，支持多级子目录。上传时父目录自动创建，同名文件覆盖。下载统一返回 `attachment`，不预览。

| 方法 | 路径 | 说明 |
| --- | --- | --- |
| GET | `/api/help` | 完整接口说明（JSON，给 Agent） |
| GET | `/llms.txt` | 完整接口说明（纯文本，给 Agent） |
| GET | `/help` | HTML 帮助页（给人） |
| GET | `/api/health` | 健康检查 |
| GET | `/api/list?path=docs&recursive=1` | 列目录；`path` 省略为根，`recursive=1` 扁平列出所有子目录文件 |
| GET | `/files/<path>` | **下载**（推荐） |
| GET | `/api/download?path=docs/a.txt` | 下载（query 形式） |
| PUT | `/files/<path>` | **上传**：请求体就是文件原始字节（等价 `curl -T`） |
| PUT/POST | `/api/upload?path=docs/a.txt` | 上传原始字节，也可用 `X-File-Path` 头指定路径 |
| POST | `/api/upload` | multipart 上传：`file` 字段（可多个）+ 可选 `path`（目标目录） |
| POST | `/api/mkdir?path=docs/2026` | 建空目录（一般不用，上传会自动建目录） |
| GET | `/api/lifecycle` | 生命周期状态：配置、待归档文件与到期时间、上次扫描结果 |
| GET | `/api/recycle` | 回收站压缩包列表（含到期删除时间） |
| GET | `/recycle/<name>.zip` | 下载回收站压缩包 |
| POST | `/api/sweep` | 立即执行一次扫描（正常由定时器自动执行） |
| GET | `/` | 浏览器文件列表页，右上角可切换回收站 |

### 常用示例

```bash
B=http://127.0.0.1:8787

# 1) Agent 自助获取用法
curl $B/llms.txt
curl $B/api/help

# 2) 上传到子目录（最简，父目录自动创建）
curl -T ./report.pdf "$B/files/reports/2026/report.pdf"

# 3) 等价写法
curl --data-binary @./data.csv "$B/api/upload?path=data/data.csv"

# 4) multipart 多文件（path 是目标目录）
curl -F "path=reports/2026" -F "file=@./a.txt" -F "file=@./b.txt" "$B/api/upload"

# 5) 列出全部文件
curl "$B/api/list?recursive=1"

# 6) 下载（-O -J 让 curl 使用服务端返回的文件名）
curl -O -J "$B/files/reports/2026/report.pdf"
```

Python：

```python
import requests
B = "http://127.0.0.1:8787"
requests.put(B + "/files/reports/a.txt", data=open("a.txt", "rb").read())
print(requests.get(B + "/files/reports/a.txt").content)
print(requests.get(B + "/api/list?recursive=1").json())
```

响应格式：

```json
// 成功上传（未设过期时间）
{"ok": true, "path": "docs/a.txt", "size": 123, "download_url": "/files/docs/a.txt", "lifecycle": "none", "expires_at": null}
// 设定过期时间时：lifecycle="scheduled"，并返回 expires_at(ISO) 与 recycle_ttl_ms
// 列目录
{"ok": true, "path": "docs", "count": 2, "items": [
  {"name": "2026", "type": "dir", "path": "docs/2026", "size": null, "mtime": null},
  {"name": "a.txt", "type": "file", "path": "docs/a.txt", "size": 123, "mtime": "2026-11-04T08:00:00.000Z"}
]}
// 失败
{"ok": false, "error": "原因"}   // HTTP 4xx / 5xx
```

## 文件生命周期（过期归档 → 回收站 → 自动删除）

**两个阶段、两个时间**

1. 上传时给文件设定过期时间 `expires`；到期后服务把该文件打成**一个 ZIP**（包内保留原相对目录结构），从 `storage/` 删除原文件，ZIP 放进 `recycle/`。
2. 给回收站压缩包设定保留时长 `recycle`（或全局 `RECYCLE_TTL`，默认 7 天）；超期后 ZIP 被自动删除，**不可恢复**。

| 场景 | 做法 |
| --- | --- |
| 单个文件临时存放 | 上传时带 `?expires=2h`（或 `X-File-Expires` 头、multipart 的 `expires` 字段） |
| 所有上传文件都自动过期 | 启动设 `DEFAULT_TTL=24h` |
| 调整回收站保留 | 启动设 `RECYCLE_TTL=3d`，或单文件带 `?recycle=3d` |
| 不自动过期 | 不设 `expires` 且 `DEFAULT_TTL` 留空（默认行为） |

```bash
B=http://127.0.0.1:8787

# 2 小时后归档；归档包在回收站保留 3 天
curl -T ./tmp.log "$B/files/temp/tmp.log?expires=2h&recycle=3d"

# 用绝对时间（+ 需编码为 %2B）
curl -T ./a.txt "$B/files/a.txt?expires=2026-11-05T20:00:00%2B08:00"

# 请求头方式
curl -H "X-File-Expires: 30m" --data-binary @./a.txt "$B/api/upload?path=a.txt"

# multipart 方式（可多文件）
curl -F "expires=2h" -F "recycle=3d" -F "file=@./a.txt" "$B/api/upload"

# 查看待归档 / 立即扫描 / 查看与取回归档包
curl $B/api/lifecycle
curl -X POST $B/api/sweep
curl $B/api/recycle
curl -O -J "$B/recycle/<name>.zip"
```

上传响应会返回 `lifecycle: "scheduled"`、`expires_at`、`recycle_ttl_ms`；未设时间返回 `lifecycle: "none"`。到期记录写在 `lifecycle.json`，重启后仍在；扫描周期由 `SWEEP_INTERVAL` 控制，也可随时 `POST /api/sweep` 手动触发。页面右上角「回收站」可查看并下载归档包（只读，不提供恢复 / 手动删除）。

页面上传区也提供「过期时间」与「回收站保留」两个输入框，留空即不自动过期 / 使用默认保留时长。

> 归档会**删除原文件**，回收站到期也会**删除压缩包**。重要文件请在到期前下载。

## 页面

打开 `/` 即可：面包屑进入子目录，点击文件名或「下载」保存文件，顶部可切换上传目录并批量选择/拖拽上传，还能给这次上传设定过期时间与回收站保留时长。右上角「回收站」列出归档包、到期时间和下载入口。页面不提供预览、改名、删除、恢复。

## 安全说明

- **没有任何认证**，默认只绑定 `127.0.0.1`。若改成 `0.0.0.0`，仅建议在可信内网使用；需要公网访问时请在外层加反向代理鉴权/白名单。
- 服务端拒绝 `..`、绝对路径、盘符、隐藏文件（以 `.` 开头）、Windows 非法字符，并校验符号链接不越出 `storage/`。
- 同名文件直接覆盖，无版本管理；生命周期归档会删除原文件、回收站压缩包到期删除且不可恢复，重要数据请自行备份 `storage/` 与 `recycle/`。
- CORS 开放为 `*`，方便不同来源的 Agent / 网页调用。

## 目录结构

```
mini-drive/
├── server.js        # 零依赖 HTTP 服务（全部逻辑）
├── public/
│   ├── index.html   # 文件列表页
│   ├── app.js       # 列表 / 上传 / 下载交互
│   └── style.css    # 页面与帮助页样式
├── storage/         # 实际文件（自动创建，可配置 DATA_DIR）
├── recycle/         # 回收站：到期文件打成的 ZIP（自动创建，可配置 RECYCLE_DIR）
├── lifecycle.json   # 到期时间等元数据（运行时生成，重启不丢）
├── llms.txt         # 静态版 Agent 使用说明
├── README.md
├── package.json
├── start.bat / start.sh          # 仅本机（默认 HOST=127.0.0.1）
├── start-lan.bat / start-lan.sh  # 局域网（HOST=0.0.0.0）
└── .gitignore
```
