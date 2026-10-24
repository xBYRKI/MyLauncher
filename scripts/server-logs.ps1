<#
.SYNOPSIS
  Zeigt das Log von mylauncher-server auf dem eigenen Server (journalctl).

  .\scripts\server-logs.ps1            # laufend mitlesen (Strg+C beendet)
  .\scripts\server-logs.ps1 -Lines 200 # nur die letzten Zeilen
#>
param([int]$Lines = 0)
$ErrorActionPreference = 'Stop'
Import-Module (Join-Path $PSScriptRoot 'lib\MyLauncherServer.psm1') -Force

$cfg = Get-ServerConfig
$sudo = if ($cfg.user -eq 'root') { '' } else { 'sudo ' }
$cmd = if ($Lines -gt 0) { "-n $Lines --no-pager" } else { '-n 50 -f' }
Invoke-Ssh $cfg "${sudo}systemctl status mylauncher-server --no-pager -n 0; ${sudo}journalctl -u mylauncher-server $cmd" -Tty
