<#
.SYNOPSIS
  Öffnet die Admin-Oberfläche des eigenen Servers über einen SSH-Tunnel.

.DESCRIPTION
  Der Tunnel bleibt offen, bis Enter gedrückt wird. Das Admin-Passwort steht in
  scripts\server.config.json und wird hier zum Kopieren angezeigt.

  .\scripts\server-admin.ps1
#>
$ErrorActionPreference = 'Stop'
Import-Module (Join-Path $PSScriptRoot 'lib\MyLauncherServer.psm1') -Force

$cfg = Get-ServerConfig
Test-SshLogin $cfg
$tunnel = Open-AdminTunnel $cfg
try {
    $url = "$($tunnel.Url)/admin/"
    Write-Host "Admin-Oberfläche: $url" -ForegroundColor Green
    Write-Host "Passwort:         $($cfg.adminPassword)"
    Start-Process $url
    Read-Host "`nEnter schließt den Tunnel"
} finally {
    Close-AdminTunnel $tunnel
}
