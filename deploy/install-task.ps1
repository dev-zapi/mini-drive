# 把 Mini Drive 注册为常驻服务（当前用户的计划任务，无需管理员权限）
# 用法: powershell -ExecutionPolicy Bypass -File deploy\install-task.ps1
param(
    [string]$TaskName = 'MiniDrive'
)

$ErrorActionPreference = 'Stop'

$deploy = $PSScriptRoot
$root = Split-Path -Parent $deploy
$runner = Join-Path $deploy 'run-service.ps1'
$exe = Join-Path $root 'target\release\mini-drive.exe'
$psExe = (Get-Process -Id $PID).Path
$user = "$env:USERDOMAIN\$env:USERNAME"

if (-not (Test-Path $exe)) { throw "未找到 $exe，请先执行 cargo build --release" }
if (-not (Test-Path $runner)) { throw "未找到 $runner" }

$action = New-ScheduledTaskAction `
    -Execute $psExe `
    -Argument ('-NoProfile -ExecutionPolicy Bypass -WindowStyle Hidden -File "{0}"' -f $runner) `
    -WorkingDirectory $root

# 登录即启动（该任务运行在当前用户会话中，无需存储密码）
$trigger = New-ScheduledTaskTrigger -AtLogOn -User $user

$settings = New-ScheduledTaskSettingsSet `
    -AllowStartIfOnBatteries `
    -DontStopIfGoingOnBatteries `
    -StartWhenAvailable `
    -RestartCount 999 `
    -RestartInterval (New-TimeSpan -Minutes 1) `
    -ExecutionTimeLimit ([TimeSpan]::Zero) `
    -MultipleInstances IgnoreNew `
    -Hidden

$principal = New-ScheduledTaskPrincipal -UserId $user -LogonType Interactive -RunLevel Limited

Register-ScheduledTask -TaskName $TaskName -Action $action -Trigger $trigger `
    -Settings $settings -Principal $principal -Force `
    -Description 'Mini Drive 零认证云盘：开机登录自启、崩溃自动重启、后台无窗口运行' | Out-Null

Write-Output "已注册计划任务: $TaskName"
Start-ScheduledTask -TaskName $TaskName
Write-Output "已触发启动，等待服务就绪..."

# 从配置文件读取端口，便于自检
$port = '8787'
$envFile = Join-Path $deploy 'mini-drive.env'
if (Test-Path $envFile) {
    Get-Content -Path $envFile -Encoding UTF8 | ForEach-Object {
        $line = $_.Trim()
        if ($line -match '^PORT\s*=\s*(\d+)$') { $port = $Matches[1] }
    }
}

$ok = $false
for ($i = 0; $i -lt 30; $i++) {
    Start-Sleep -Milliseconds 500
    try {
        $r = Invoke-WebRequest -Uri "http://127.0.0.1:$port/api/health" -UseBasicParsing -TimeoutSec 2
        if ($r.StatusCode -eq 200) { $ok = $true; break }
    } catch { }
}
if ($ok) { Write-Output "服务已就绪: http://127.0.0.1:$port/" } else { Write-Output "!! 服务未在 15 秒内就绪，请查看 logs\service.log" }
