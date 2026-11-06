# 停止并移除 Mini Drive 常驻服务
param(
    [string]$TaskName = 'MiniDrive'
)

$ErrorActionPreference = 'Continue'
$root = Split-Path -Parent $PSScriptRoot

$task = Get-ScheduledTask -TaskName $TaskName -ErrorAction SilentlyContinue
if ($task) {
    Stop-ScheduledTask -TaskName $TaskName -ErrorAction SilentlyContinue
    Start-Sleep -Milliseconds 500
    Unregister-ScheduledTask -TaskName $TaskName -Confirm:$false
    Write-Output "已移除计划任务: $TaskName"
} else {
    Write-Output "计划任务 $TaskName 不存在"
}

# 兜底清理残留进程
$procs = Get-Process -Name 'mini-drive' -ErrorAction SilentlyContinue
if ($procs) {
    $procs | Stop-Process -Force
    Write-Output "已结束残留进程: $($procs.Id -join ', ')"
} else {
    Write-Output "无残留进程"
}
