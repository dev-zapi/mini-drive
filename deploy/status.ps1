# 查看 Mini Drive 常驻服务状态
param(
    [string]$TaskName = 'MiniDrive'
)

$ErrorActionPreference = 'Continue'
$root = Split-Path -Parent $PSScriptRoot
$log = Join-Path $root 'logs\service.log'

Write-Output "===== 计划任务 ====="
$task = Get-ScheduledTask -TaskName $TaskName -ErrorAction SilentlyContinue
if ($task) {
    $info = $task | Get-ScheduledTaskInfo
    [PSCustomObject]@{
        名称       = $task.TaskName
        状态       = $task.State
        上次运行   = $info.LastRunTime
        上次结果   = $info.LastTaskResult
        下次运行   = $info.NextRunTime
        触发器     = ($task.Triggers | ForEach-Object { $_.CimClass.CimClassName }) -join ', '
    } | Format-List
} else {
    Write-Output "未注册（$TaskName 不存在）"
}

Write-Output "===== 进程 ====="
$procs = Get-Process -Name 'mini-drive' -ErrorAction SilentlyContinue
if ($procs) {
    $procs | Select-Object Id, StartTime, @{n = '内存MB'; e = { [math]::Round($_.WorkingSet64 / 1MB, 1) } } | Format-Table -AutoSize
} else {
    Write-Output "无运行中的 mini-drive 进程"
}

Write-Output "===== 端口 ====="
$conn = Get-NetTCPConnection -LocalPort 8787 -State Listen -ErrorAction SilentlyContinue
if ($conn) {
    $conn | Select-Object LocalAddress, LocalPort, OwningProcess | Format-Table -AutoSize
} else {
    Write-Output "8787 未监听"
}

Write-Output "===== 健康检查 ====="
foreach ($url in 'http://127.0.0.1:8787/api/health', 'http://127.0.0.1:8787/api/lifecycle') {
    try {
        $r = Invoke-WebRequest -Uri $url -UseBasicParsing -TimeoutSec 3
        Write-Output ("  {0} -> HTTP {1}" -f $url, $r.StatusCode)
    } catch {
        Write-Output ("  {0} -> 失败: {1}" -f $url, $_.Exception.Message)
    }
}

Write-Output "===== 最近日志（10 行） ====="
if (Test-Path $log) {
    Get-Content -Path $log -Tail 10 -Encoding UTF8
} else {
    Write-Output "暂无日志"
}
