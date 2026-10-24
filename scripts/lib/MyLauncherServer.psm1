<#
  Gemeinsame Helfer für die Skripte, die mit dem eigenen mylauncher-server reden.

  Alles läuft über SSH: Programm und Konfiguration kommen per scp auf den Server, die Admin-API
  wird über einen SSH-Tunnel angesprochen (lokal 127.0.0.1:<tunnelPort> → Server 127.0.0.1:8787).
  Die Admin-Oberfläche muss dafür nicht aus dem Internet erreichbar sein.

  Einstellungen: scripts\server.config.json (Vorlage: server.config.example.json, nicht einchecken).
#>

$ErrorActionPreference = 'Stop'
$script:Root = Split-Path -Parent (Split-Path -Parent $PSScriptRoot)
$script:ConfigPath = Join-Path $script:Root 'scripts\server.config.json'
# Port, auf dem mylauncher-server auf dem Server lauscht (nur lokal, davor sitzt der Reverse-Proxy).
$script:RemotePort = 8787

function Get-RepoRoot { $script:Root }

function Get-ServerConfig {
    if (-not (Test-Path $script:ConfigPath)) {
        throw "scripts\server.config.json fehlt – Vorlage kopieren und anpassen:`n  Copy-Item scripts\server.config.example.json scripts\server.config.json"
    }
    $cfg = Get-Content $script:ConfigPath -Raw -Encoding UTF8 | ConvertFrom-Json
    foreach ($key in 'host', 'user', 'publicUrl') {
        if (-not $cfg.$key) { throw "server.config.json: '$key' ist leer." }
    }
    $cfg.publicUrl = $cfg.publicUrl.TrimEnd('/')
    if ($cfg.publicUrl -notmatch '^https?://[^/]+$') {
        throw "server.config.json: publicUrl muss wie https://launcher.example.net aussehen (ohne Pfad)."
    }
    $defaults = [ordered]@{ port = 22; identityFile = ''; adminPassword = ''; waypointTokens = @();
        proxy = 'caddy'; exposeAdmin = $false; tunnelPort = 18787 }
    foreach ($key in $defaults.Keys) {
        if ($null -eq $cfg.$key) { $cfg | Add-Member -NotePropertyName $key -NotePropertyValue $defaults[$key] }
    }
    if ($env:MYLAUNCHER_ADMIN_PASSWORD) { $cfg.adminPassword = $env:MYLAUNCHER_ADMIN_PASSWORD }
    return $cfg
}

function Save-ServerConfig($Config) {
    Write-Utf8 $script:ConfigPath ($Config | ConvertTo-Json -Depth 5)
}

# Ohne BOM und mit LF – die Dateien landen auf Linux bzw. bei serde_json.
function Write-Utf8([string]$Path, [string]$Text, [switch]$Lf) {
    if ($Lf) { $Text = $Text -replace "`r`n", "`n" }
    [System.IO.File]::WriteAllText($Path, $Text, (New-Object System.Text.UTF8Encoding $false))
}

function New-Secret([int]$Length = 32) {
    $chars = [char[]]'abcdefghijkmnopqrstuvwxyzABCDEFGHJKLMNPQRSTUVWXYZ23456789'
    $bytes = New-Object byte[] $Length
    [System.Security.Cryptography.RandomNumberGenerator]::Create().GetBytes($bytes)
    -join ($bytes | ForEach-Object { $chars[$_ % $chars.Length] })
}

function Get-SshTarget($Config) { "$($Config.user)@$($Config.host)" }

function Get-SshOptions($Config, [switch]$Scp) {
    $opts = @(if ($Scp) { '-P' } else { '-p' }), "$($Config.port)", '-o', 'StrictHostKeyChecking=accept-new'
    if ($Config.identityFile) { $opts += @('-i', $Config.identityFile) }
    return $opts
}

function Invoke-Ssh($Config, [string]$Command, [switch]$Tty) {
    $opts = Get-SshOptions $Config
    if ($Tty) { $opts += '-t' }
    & ssh @opts (Get-SshTarget $Config) $Command
    if ($LASTEXITCODE -ne 0) { throw "SSH-Befehl fehlgeschlagen ($LASTEXITCODE): $Command" }
}

function Copy-ToServer($Config, [string[]]$Files, [string]$RemoteDir) {
    $opts = Get-SshOptions $Config -Scp
    & scp -q @opts @Files "$(Get-SshTarget $Config):$RemoteDir/"
    if ($LASTEXITCODE -ne 0) { throw "scp fehlgeschlagen ($LASTEXITCODE)" }
}

# Prüft, ob SSH ohne Passwortabfrage klappt – der Tunnel läuft im Hintergrund und kann nicht nachfragen.
function Test-SshLogin($Config) {
    $opts = (Get-SshOptions $Config) + @('-o', 'BatchMode=yes', '-o', 'ConnectTimeout=10')
    & ssh @opts (Get-SshTarget $Config) 'true' 2>$null
    if ($LASTEXITCODE -ne 0) {
        throw "SSH-Anmeldung an $(Get-SshTarget $Config) ohne Passwort fehlgeschlagen. Schlüssel hinterlegen, z. B.:`n  type `$env:USERPROFILE\.ssh\id_ed25519.pub | ssh $(Get-SshTarget $Config) `"mkdir -p ~/.ssh && cat >> ~/.ssh/authorized_keys`""
    }
}

function Test-LocalPort([int]$Port) {
    $client = New-Object System.Net.Sockets.TcpClient
    try {
        $client.ConnectAsync('127.0.0.1', $Port).Wait(300) -and $client.Connected
    } catch { $false } finally { $client.Dispose() }
}

<#
  Öffnet den SSH-Tunnel zur Admin-API und wartet, bis der Server antwortet.
  Rückgabe: Objekt für Invoke-Admin / Close-AdminTunnel.
#>
function Open-AdminTunnel($Config) {
    if (-not $Config.adminPassword) {
        throw 'Kein Admin-Passwort – erst .\scripts\deploy-server.ps1 ausführen (legt eins an) oder MYLAUNCHER_ADMIN_PASSWORD setzen.'
    }
    $port = [int]$Config.tunnelPort
    if (Test-LocalPort $port) { throw "Port $port ist lokal belegt – tunnelPort in server.config.json ändern." }
    $opts = (Get-SshOptions $Config) + @('-N', '-o', 'BatchMode=yes', '-o', 'ExitOnForwardFailure=yes',
        '-o', 'ServerAliveInterval=30', '-L', "127.0.0.1:${port}:127.0.0.1:$($script:RemotePort)", (Get-SshTarget $Config))
    $quoted = $opts | ForEach-Object { if ($_ -match '\s') { "`"$_`"" } else { $_ } }
    $proc = Start-Process ssh -ArgumentList $quoted -NoNewWindow -PassThru
    $tunnel = [pscustomobject]@{ Process = $proc; Url = "http://127.0.0.1:$port"; Password = $Config.adminPassword }
    $deadline = (Get-Date).AddSeconds(20)
    while ((Get-Date) -lt $deadline) {
        if ($proc.HasExited) { throw "SSH-Tunnel beendet (Code $($proc.ExitCode)) – läuft SSH ohne Passwort? (Test-SshLogin)" }
        if (Test-LocalPort $port) {
            try {
                $null = Invoke-Admin $tunnel GET '/status'
                return $tunnel
            } catch {
                if ($_.Exception.Message -match 'HTTP 401') { Close-AdminTunnel $tunnel; throw 'Admin-Passwort wird vom Server abgelehnt.' }
                if ($_.Exception.Message -notmatch 'HTTP 000') { Close-AdminTunnel $tunnel; throw }
            }
        }
        Start-Sleep -Milliseconds 300
    }
    Close-AdminTunnel $tunnel
    throw "Server antwortet nicht über den Tunnel – läuft mylauncher-server? (.\scripts\server-logs.ps1)"
}

function Close-AdminTunnel($Tunnel) {
    if ($Tunnel -and $Tunnel.Process -and -not $Tunnel.Process.HasExited) {
        Stop-Process -Id $Tunnel.Process.Id -Force -ErrorAction SilentlyContinue
    }
}

<#
  Ruft die Admin-API auf (Pfad relativ zu /admin/api). -Body wird als JSON geschickt, -InFile als Rohdaten.
  Über curl.exe, weil es große Dateien streamt und Fehlertexte des Servers zurückgibt.
  Antwort und Header laufen über Dateien: Windows PowerShell verstümmelt sonst Umlaute und Anführungszeichen.
#>
function Invoke-Admin($Tunnel, [string]$Method, [string]$Path, $Body = $null, [string]$InFile = '', [switch]$AllowNotFound) {
    $tmp = [System.IO.Path]::GetTempPath()
    $id = [guid]::NewGuid().ToString('n')
    $hdr = Join-Path $tmp "mls-$id.hdr"
    $out = Join-Path $tmp "mls-$id.out"
    $json = Join-Path $tmp "mls-$id.json"
    try {
        Write-Utf8 $hdr "Authorization: Bearer $($Tunnel.Password)`n"
        $args = @('-sS', '-X', $Method, '-H', "@$hdr", '-o', $out, '-w', '%{http_code}')
        if ($null -ne $Body) {
            Write-Utf8 $json ($Body | ConvertTo-Json -Depth 8 -Compress)
            $args += @('-H', 'Content-Type: application/json', '--data-binary', "@$json")
        }
        if ($InFile) { $args += @('-T', $InFile) }
        $code = & curl.exe @args "$($Tunnel.Url)/admin/api$Path" 2>$null
        $text = if (Test-Path $out) { [System.IO.File]::ReadAllText($out, [System.Text.Encoding]::UTF8) } else { '' }
        if ($code -eq '404' -and $AllowNotFound) { return $null }
        if ($code -notmatch '^2') { throw "HTTP $code bei $Method $Path`: $text" }
        if ($text) { return $text | ConvertFrom-Json }
    } finally {
        Remove-Item $hdr, $out, $json -ErrorAction SilentlyContinue
    }
}

function ConvertTo-Query([hashtable]$Values) {
    ($Values.GetEnumerator() | ForEach-Object {
        "$($_.Key)=$([uri]::EscapeDataString([string]$_.Value))"
    }) -join '&'
}

Export-ModuleMember -Function Get-RepoRoot, Get-ServerConfig, Save-ServerConfig, Write-Utf8, New-Secret,
    Get-SshTarget, Invoke-Ssh, Copy-ToServer, Test-SshLogin, Open-AdminTunnel, Close-AdminTunnel,
    Invoke-Admin, ConvertTo-Query
