<#
.SYNOPSIS
  Gleicht einen lokalen Modpack-Ordner per SSH-Tunnel mit dem eigenen Server ab und veröffentlicht ihn.

.DESCRIPTION
  Ordneraufbau wie beim packtool:
    mein-pack\files\      immer synchron halten (mods\, config\, resourcepacks\ …)
    mein-pack\defaults\   nur anlegen, wenn beim Spieler noch nicht vorhanden (options.txt …)
  Ohne files\ und defaults\ wird der ganze Ordner als files\ behandelt – praktisch, um z. B.
  direkt den .minecraft-Ordner einer Prism-Instanz oder nur einen mods\-Ordner (-Prefix mods) hochzuladen.

  Nur geänderte Dateien werden übertragen (Vergleich per SHA1). Dateien, die lokal fehlen,
  fliegen aus dem Pack. Danach wird der Entwurf als neue Version veröffentlicht; ohne -Version
  wird die letzte Versionsnummer hochgezählt (1.4.2 → 1.4.3).

  # Neues Pack anlegen und veröffentlichen
  .\scripts\publish-pack.ps1 -Pack survival -Dir D:\Packs\survival -Name "Survival" `
      -Minecraft 1.21.4 -Loader fabric@0.16.10 -Server gaming.stephanides-it.at -Memory 6144 -Listed

  # Mods aktualisieren
  .\scripts\publish-pack.ps1 -Pack survival -Dir D:\Packs\survival -Notes "Sodium 0.6"

  # Nur zeigen, was passieren würde
  .\scripts\publish-pack.ps1 -Pack survival -Dir D:\Packs\survival -DryRun
#>
param(
    [Parameter(Mandatory)][string]$Pack,
    [Parameter(Mandatory)][string]$Dir,
    [string]$Version = '',
    [string]$Notes = '',
    # Unterordner im Spielordner, unter dem der ganze Ordner landet (nur ohne files\/defaults\).
    [string]$Prefix = '',
    # Metadaten – nur für neue Packs nötig, sonst überschreiben sie die vorhandenen Werte.
    [string]$Name = '',
    [string]$Description = '',
    [string]$Minecraft = '',
    [string]$Loader = '',
    [string]$Server = '',
    [int]$Memory = -1,
    [switch]$Listed,
    [switch]$Hidden,
    # Nur den Entwurf abgleichen, nicht veröffentlichen.
    [switch]$NoPublish,
    [switch]$DryRun
)

$ErrorActionPreference = 'Stop'
Import-Module (Join-Path $PSScriptRoot 'lib\MyLauncherServer.psm1') -Force

# Dateien, die in keinem Pack etwas verloren haben.
$Ignore = '(^|/)(\.git|\.DS_Store|Thumbs\.db|desktop\.ini)(/|$)|\.(tmp|part|bak)$'

function Get-LocalFiles([string]$Base, [string]$Into, [bool]$Overwrite) {
    $base = (Resolve-Path $Base).Path.TrimEnd('\')
    Get-ChildItem $base -Recurse -File -Force | ForEach-Object {
        $rel = $_.FullName.Substring($base.Length + 1).Replace('\', '/')
        if ($Into) { $rel = "$($Into.Trim('/'))/$rel" }
        if ($rel -notmatch $Ignore) {
            [pscustomobject]@{
                Path      = $rel
                File      = $_.FullName
                Size      = $_.Length
                Sha1      = (Get-FileHash $_.FullName -Algorithm SHA1).Hash.ToLower()
                Overwrite = $Overwrite
            }
        }
    }
}

function Step-Version([string]$Last) {
    if (-not $Last) { return '1.0.0' }
    if ($Last -match '^(.*?)(\d+)$') { return "$($Matches[1])$([int64]$Matches[2] + 1)" }
    return "$Last.1"
}

function Format-Size([double]$Bytes) {
    if ($Bytes -ge 1MB) { '{0:N1} MB' -f ($Bytes / 1MB) } else { '{0:N0} KB' -f ($Bytes / 1KB) }
}

if (-not (Test-Path $Dir -PathType Container)) { throw "Ordner nicht gefunden: $Dir" }
$filesDir = Join-Path $Dir 'files'
$defaultsDir = Join-Path $Dir 'defaults'
Write-Host "Lese $Dir …" -ForegroundColor Cyan
$local = @(
    if ((Test-Path $filesDir) -or (Test-Path $defaultsDir)) {
        if ($Prefix) { throw '-Prefix geht nur ohne files\/defaults\-Aufteilung.' }
        if (Test-Path $filesDir) { Get-LocalFiles $filesDir '' $true }
        if (Test-Path $defaultsDir) { Get-LocalFiles $defaultsDir '' $false }
    } else {
        Get-LocalFiles $Dir $Prefix $true
    }
)
$dupes = $local | Group-Object Path | Where-Object Count -gt 1
if ($dupes) { throw "Doppelt in files\ und defaults\: $(($dupes | ForEach-Object Name) -join ', ')" }
if (-not $local) { throw "Keine Dateien in $Dir" }

$cfg = Get-ServerConfig
Test-SshLogin $cfg
$tunnel = Open-AdminTunnel $cfg
try {
    $id = [uri]::EscapeDataString($Pack)
    $remote = Invoke-Admin $tunnel GET "/packs/$id" -AllowNotFound

    # --- Metadaten ---
    $loaderSpec = $null
    if ($Loader) {
        $kind, $ver = $Loader.Split('@', 2)
        $loaderSpec = [ordered]@{ type = $kind; version = [string]$ver }
    }
    if (-not $remote) {
        if (-not $Minecraft) { throw "Pack '$Pack' gibt es noch nicht – zum Anlegen mindestens -Minecraft angeben (und -Name, -Loader)." }
        $meta = [ordered]@{
            id = $Pack; name = $(if ($Name) { $Name } else { $Pack }); description = $Description
            minecraft = $Minecraft; loader = $(if ($loaderSpec) { $loaderSpec } else { @{ type = ''; version = '' } })
            server = $Server; memory = [math]::Max($Memory, 0); instanceId = ''; listed = $false
        }
        Write-Host "Lege Pack '$Pack' an ($Minecraft $Loader)" -ForegroundColor Yellow
        if (-not $DryRun) { $remote = Invoke-Admin $tunnel POST '/packs' $meta }
    } else {
        $changes = [ordered]@{}
        if ($Name) { $changes.name = $Name }
        if ($Description) { $changes.description = $Description }
        if ($Minecraft) { $changes.minecraft = $Minecraft }
        if ($loaderSpec) { $changes.loader = $loaderSpec }
        if ($Server) { $changes.server = $Server }
        if ($Memory -ge 0) { $changes.memory = $Memory }
        if ($changes.Count) {
            $meta = [ordered]@{}
            foreach ($k in 'name', 'description', 'minecraft', 'loader', 'server', 'memory', 'instanceId', 'listed') {
                $meta[$k] = if ($changes.Contains($k)) { $changes[$k] } else { $remote.$k }
            }
            Write-Host "Metadaten: $(($changes.Keys) -join ', ')" -ForegroundColor Yellow
            if (-not $DryRun) { $remote = Invoke-Admin $tunnel PUT "/packs/$id" $meta }
        }
    }

    # --- Dateien abgleichen ---
    $remoteFiles = @{}
    if ($remote) { foreach ($f in $remote.files) { $remoteFiles[$f.path] = $f } }
    $localPaths = @{}
    foreach ($f in $local) { $localPaths[$f.Path] = $true }

    $upload = @($local | Where-Object { -not $remoteFiles.ContainsKey($_.Path) -or $remoteFiles[$_.Path].sha1 -ne $_.Sha1 })
    $flags = @($local | Where-Object {
        $remoteFiles.ContainsKey($_.Path) -and $remoteFiles[$_.Path].sha1 -eq $_.Sha1 -and $remoteFiles[$_.Path].overwrite -ne $_.Overwrite })
    $remove = @($remoteFiles.Keys | Where-Object { -not $localPaths.ContainsKey($_) } | Sort-Object)

    $bytes = ($upload | Measure-Object Size -Sum).Sum
    Write-Host ("{0} Dateien lokal · hochladen: {1} ({2}) · entfernen: {3} · Vorgabe-Status ändern: {4}" -f
        $local.Count, $upload.Count, (Format-Size $bytes), $remove.Count, $flags.Count)
    foreach ($f in $upload) { Write-Host "  + $($f.Path)$(if (-not $f.Overwrite) { '  (Vorgabe)' })" -ForegroundColor Green }
    foreach ($p in $remove) { Write-Host "  - $p" -ForegroundColor Red }
    foreach ($f in $flags) { Write-Host "  ~ $($f.Path) → $(if ($f.Overwrite) { 'synchron' } else { 'Vorgabe' })" -ForegroundColor DarkYellow }

    if ($DryRun) {
        Write-Host "`nProbelauf – nichts geändert." -ForegroundColor DarkGray
        return
    }

    $i = 0
    foreach ($f in $upload) {
        $i++
        Write-Progress -Activity "Lade $Pack hoch" -Status "$i/$($upload.Count): $($f.Path)" -PercentComplete (100 * $i / $upload.Count)
        $q = ConvertTo-Query @{ path = $f.Path; overwrite = $f.Overwrite.ToString().ToLower() }
        $null = Invoke-Admin $tunnel PUT "/packs/$id/files?$q" -InFile $f.File
    }
    Write-Progress -Activity "Lade $Pack hoch" -Completed
    if ($remove) { $null = Invoke-Admin $tunnel POST "/packs/$id/files/delete" @{ paths = $remove } }
    foreach ($group in ($flags | Group-Object Overwrite)) {
        $null = Invoke-Admin $tunnel POST "/packs/$id/files/overwrite" @{
            paths = @($group.Group | ForEach-Object Path); overwrite = ($group.Name -eq 'True') }
    }

    # --- Veröffentlichen ---
    $detail = Invoke-Admin $tunnel GET "/packs/$id"
    if ($NoPublish) {
        Write-Host "`nEntwurf aktualisiert, nicht veröffentlicht." -ForegroundColor Green
    } elseif (-not $detail.draft.dirty) {
        Write-Host "`nKeine Änderungen – Version $($detail.current) bleibt." -ForegroundColor Green
    } else {
        if (-not $Version) {
            $last = @($detail.versions | Sort-Object publishedAt | Select-Object -Last 1).version
            $Version = Step-Version $last
        }
        $detail = Invoke-Admin $tunnel POST "/packs/$id/publish" @{ version = $Version; notes = $Notes }
        Write-Host "`n$Pack $Version veröffentlicht." -ForegroundColor Green
    }

    if ($Listed -or $Hidden) {
        $meta = [ordered]@{}
        foreach ($k in 'name', 'description', 'minecraft', 'loader', 'server', 'memory', 'instanceId') { $meta[$k] = $detail.$k }
        $meta.listed = [bool]$Listed
        $null = Invoke-Admin $tunnel PUT "/packs/$id" $meta
        Write-Host "Im Launcher $(if ($Listed) { 'sichtbar' } else { 'ausgeblendet' })."
    }
} finally {
    Close-AdminTunnel $tunnel
}
