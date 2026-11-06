# Mini Drive 常驻服务启动器（由计划任务 MiniDrive 调用，也可手动前台运行调试）
# 自带守护循环：进程异常退出后自动重启；exit 0 视为正常停止，不再拉起。
$ErrorActionPreference = 'Stop'

$deploy = $PSScriptRoot
$root = Split-Path -Parent $deploy
$exe = Join-Path $root 'target\release\mini-drive.exe'
$envFile = Join-Path $deploy 'mini-drive.env'
$logDir = Join-Path $root 'logs'
$log = Join-Path $logDir 'service.log'
$enc = New-Object System.Text.UTF8Encoding($false)

function Write-Log([string]$text) {
    [System.IO.File]::AppendAllText($log, ('[{0}] {1}' -f (Get-Date -Format 'yyyy-MM-dd HH:mm:ss'), $text) + [Environment]::NewLine, $enc)
}

if (-not (Test-Path $exe)) { throw "未找到可执行文件: $exe (请先执行 cargo build --release)" }
if (-not (Test-Path $logDir)) { New-Item -ItemType Directory -Path $logDir -Force | Out-Null }

# 读取配置写入当前进程环境，子进程继承
if (Test-Path $envFile) {
    Get-Content -Path $envFile -Encoding UTF8 | ForEach-Object {
        $line = $_.Trim()
        if ($line -and -not $line.StartsWith('#')) {
            $i = $line.IndexOf('=')
            if ($i -gt 0) {
                Set-Item -Path ("env:" + $line.Substring(0, $i).Trim()) -Value $line.Substring($i + 1).Trim()
            }
        }
    }
}

# 日志轮转：超过 5 MB 归档为 .1
if ((Test-Path $log) -and ((Get-Item $log).Length -gt 5MB)) { Move-Item -Path $log -Destination "$log.1" -Force }

Write-Log ("===== 守护启动 {0} =====" -f $exe)
Write-Log ("HOST={0} PORT={1} MAX_UPLOAD_MB={2} DEFAULT_TTL={3} RECYCLE_TTL={4} SWEEP_INTERVAL={5}" -f `
        $env:HOST, $env:PORT, $env:MAX_UPLOAD_MB, $env:DEFAULT_TTL, $env:RECYCLE_TTL, $env:SWEEP_INTERVAL)

# 用 ProcessStartInfo 原样传参给 cmd 做追加重定向，避免 PowerShell 二次加引号
function Start-Drive {
    $psi = New-Object System.Diagnostics.ProcessStartInfo
    $psi.FileName = "$env:SystemRoot\System32\cmd.exe"
    $psi.Arguments = '/c ""' + $exe + '" >> "' + $log + '" 2>&1"'
    $psi.UseShellExecute = $false
    $psi.CreateNoWindow = $true
    $psi.WorkingDirectory = $root
    $p = [System.Diagnostics.Process]::Start($psi)
    $p.WaitForExit()
    return $p.ExitCode
}

$failures = 0
while ($true) {
    Write-Log '----- 启动进程 -----'
    $code = Start-Drive

    if ($code -eq 0) {
        Write-Log '进程正常退出（code=0），守护循环结束'
        break
    }

    $failures++
    $delay = [Math]::Min(60, 5 * $failures)
    Write-Log ("进程异常退出（code={0}），第 {1} 次重试，{2} 秒后重启" -f $code, $failures, $delay)
    Start-Sleep -Seconds $delay
}
