# Ludex installer for Windows (per-user, checksum-verified) -- issue #276.
#
# One-liner:
#   irm https://raw.githubusercontent.com/cleeryy/ludex/main/install.ps1 | iex
# (or download this file and run it; both modes behave the same).
#
# The NSIS bundle is built with `bundle.windows.nsis.installMode = currentUser`,
# so it installs under %LOCALAPPDATA% and never asks for elevation. This script
# does not elevate, does not change the execution policy and does not unblock
# files: it downloads the installer, checks it against the release's signed
# checksum list, runs it silently and reports the version the app itself
# reports.
#
# Verification (the method that actually ran is always printed):
#   * minisign.exe on PATH -> verify SHA256SUMS (or the installer's own .sig)
#     against the updater key pinned in
#     apps/desktop/src-tauri/tauri.conf.json (plugins.updater.pubkey);
#   * otherwise            -> SHA-256 of the installer against SHA256SUMS.
# A mismatch aborts before anything is installed.
#
# Ludex has no Authenticode certificate yet, so SmartScreen will show
# "unknown publisher" the first time the setup runs. That is a publisher
# reputation gap, not a failed signature; the script says so before installing.
#
#Requires -Version 5.1
[CmdletBinding(SupportsShouldProcess = $true)]
param(
  # Release channel used when -Version is not given.
  [ValidateSet('stable', 'edge')]
  [string]$Channel = 'stable',

  # Release tag to install, e.g. v0.4.22 (0.4.22 also works). Overrides -Channel.
  [string]$Version,

  # Install directory, passed to the NSIS installer as /D=<path>.
  [string]$Dir,

  # Remove Ludex instead of installing it.
  [switch]$Uninstall,

  # Reinstall even when the same version is already installed.
  [switch]$Force,

  # Release asset root: an https URL or a local directory. Used by
  # scripts/test-install.ps1 and by air-gapped mirrors; leave empty normally.
  [string]$BaseUri
)

$script:HostErrorActionPreference = $ErrorActionPreference
$script:HostProgressPreference = $ProgressPreference
$ErrorActionPreference = 'Stop'
$ProgressPreference = 'SilentlyContinue'

# Windows PowerShell 5.1 on older Windows 10 builds still offers TLS 1.0 first;
# github.com only speaks TLS 1.2+. PowerShell 7 ignores this and uses the OS
# default, which is fine.
try {
  [Net.ServicePointManager]::SecurityProtocol = [Net.ServicePointManager]::SecurityProtocol -bor [Net.SecurityProtocolType]::Tls12
} catch {
}

$script:Repo = 'cleeryy/ludex'
$script:ReleaseRoot = 'https://github.com/cleeryy/ludex/releases'
$script:ProductName = 'Ludex'
$script:UninstallRoot = 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Uninstall'
$script:ChecksumNames = @('SHA256SUMS', 'edge-checksums-Windows-x64.txt')

# Pinned from apps/desktop/src-tauri/tauri.conf.json -> plugins.updater.pubkey
# (base64 of a minisign public key file, id 67DF1F7D6F8425D8). Update both
# places together if the updater key is ever rotated.
$script:MinisignPublicKey = 'dW50cnVzdGVkIGNvbW1lbnQ6IG1pbmlzaWduIHB1YmxpYyBrZXk6IDY3REYxRjdENkY4NDI1RDgKUldUWUpZUnZmUi9mWjdndnFKc3dvVkJSak9HeWdIYlJDZ0NreVVFRjVwUUZMWXBpRW53RHRoc0MK'
$script:MinisignKeyId = '67DF1F7D6F8425D8'

# Running through `-File` can use exit codes; code piped into `iex` must throw
# instead, or `exit 1` would close the caller's PowerShell window.
$script:RunFromFile = -not [string]::IsNullOrEmpty($PSCommandPath)
$script:PlanOnly = [bool]$WhatIfPreference
$script:TempDir = $null

# ---------------------------------------------------------------------------
# Host and resource helpers
# ---------------------------------------------------------------------------

function Test-IsWindows {
  return ($env:OS -eq 'Windows_NT')
}

function Fail {
  param([string]$Message)
  throw $Message
}

# ShouldProcess needs an advanced-script host. Under `irm ... | iex` there is no
# $PSCmdlet, so fall back to "yes" (the plan mode already returned before any
# mutation); when the script runs from disk, -Confirm still works.
function Confirm-Action {
  param([string]$Target, [string]$Action)
  if ($script:ShouldProcess) {
    return $script:ShouldProcess.ShouldProcess($Target, $Action)
  }
  return $true
}

function Join-ResourceUri {
  param([string]$Root, [string]$Name)
  if ($Root -match '^(?i)https?://') {
    return ($Root.TrimEnd('/') + '/' + $Name)
  }
  return (Join-Path $Root $Name)
}

function Get-ResourceText {
  param([string]$Uri)

  if ($Uri -match '^(?i)https?://') {
    $params = @{
      Uri         = $Uri
      UserAgent   = 'ludex-install'
      ErrorAction = 'Stop'
    }
    # Windows PowerShell 5.1 defaults to the IE parser unless told otherwise;
    # PowerShell 7 keeps the parameter as a no-op. Branch on presence
    # so the same code runs on both.
    if ((Get-Command Invoke-WebRequest).Parameters.ContainsKey('UseBasicParsing')) {
      $params['UseBasicParsing'] = $true
    }
    $response = Invoke-WebRequest @params
    if ($null -ne $response.Content -and $response.Content -isnot [byte[]]) {
      return [string]$response.Content
    }
    return [Text.Encoding]::UTF8.GetString($response.Content)
  }

  if (-not (Test-Path -LiteralPath $Uri)) {
    throw "Resource not found: $Uri"
  }
  return (Get-Content -LiteralPath $Uri -Raw)
}

function Get-ResourceTextOrNull {
  param([string]$Uri)
  try {
    return (Get-ResourceText -Uri $Uri)
  } catch {
    return $null
  }
}

function Get-ResourceJsonOrNull {
  param([string]$Uri)
  $text = Get-ResourceTextOrNull -Uri $Uri
  if ([string]::IsNullOrWhiteSpace($text)) { return $null }
  try {
    return ($text | ConvertFrom-Json)
  } catch {
    return $null
  }
}

function Save-Resource {
  param([string]$Uri, [string]$Path)

  if ($Uri -match '^(?i)https?://') {
    if (Test-Path -LiteralPath $Path) { Remove-Item -LiteralPath $Path -Force }
    $params = @{
      Uri         = $Uri
      OutFile     = $Path
      UserAgent   = 'ludex-install'
      ErrorAction = 'Stop'
    }
    if ((Get-Command Invoke-WebRequest).Parameters.ContainsKey('UseBasicParsing')) {
      $params['UseBasicParsing'] = $true
    }
    Invoke-WebRequest @params
    return
  }

  if (-not (Test-Path -LiteralPath $Uri)) {
    throw "Resource not found: $Uri"
  }
  Copy-Item -LiteralPath $Uri -Destination $Path -Force
}

function Resolve-LocalBaseUri {
  param([string]$Root)
  if ($Root -match '^file:(//)?') {
    return ([uri]$Root).LocalPath
  }
  return $Root
}

# ---------------------------------------------------------------------------
# Installed-app inspection (HKCU, per-user NSIS)
# ---------------------------------------------------------------------------

function Get-UninstallerPath {
  param([string]$Command)
  if ([string]::IsNullOrWhiteSpace($Command)) { return $null }
  $trimmed = $Command.Trim()
  if ($trimmed -match '^"([^"]+)"') { return $Matches[1] }
  $parts = $trimmed -split '\s+', 2
  if ($parts.Count -ge 1 -and $parts[0]) { return $parts[0] }
  return $null
}

function Get-ExeVersion {
  param([string]$Path)
  if ([string]::IsNullOrWhiteSpace($Path) -or -not (Test-Path -LiteralPath $Path)) { return $null }
  try {
    $info = [System.Diagnostics.FileVersionInfo]::GetVersionInfo($Path)
    if ($info.ProductVersion) { return ([string]$info.ProductVersion).Trim() }
    if ($info.FileVersion) { return ([string]$info.FileVersion).Trim() }
  } catch {
    return $null
  }
  return $null
}

function Get-LudexInstallation {
  if (-not (Test-IsWindows)) { return }
  if (-not (Test-Path -LiteralPath $script:UninstallRoot)) { return }

  foreach ($key in @(Get-ChildItem -LiteralPath $script:UninstallRoot -ErrorAction SilentlyContinue)) {
    $props = $null
    try {
      $props = Get-ItemProperty -LiteralPath $key.PSPath -ErrorAction Stop
    } catch {
      continue
    }
    if ($null -eq $props) { continue }

    $displayName = [string]$props.DisplayName
    if ([string]::IsNullOrWhiteSpace($displayName)) { continue }
    if ($displayName -notmatch '^(?i)Ludex(\s|$)') { continue }

    $uninstallString = [string]$props.UninstallString
    $quietString = [string]$props.QuietUninstallString
    $uninstaller = Get-UninstallerPath -Command $(if ($quietString) { $quietString } else { $uninstallString })

    $location = [string]$props.InstallLocation
    if ([string]::IsNullOrWhiteSpace($location) -and $uninstaller) {
      $location = Split-Path -Parent $uninstaller
    }

    $candidates = @()
    if ($location) { $candidates += (Join-Path $location 'Ludex.exe') }
    if ($env:LOCALAPPDATA) { $candidates += (Join-Path $env:LOCALAPPDATA 'Ludex\Ludex.exe') }
    $exe = $candidates | Where-Object { Test-Path -LiteralPath $_ } | Select-Object -First 1

    $displayVersion = [string]$props.DisplayVersion
    $fileVersion = Get-ExeVersion -Path $exe
    $version = $displayVersion
    if ([string]::IsNullOrWhiteSpace($version)) { $version = $fileVersion }

    [pscustomobject]@{
      DisplayName          = $displayName
      DisplayVersion       = $displayVersion
      FileVersion          = $fileVersion
      Version              = $version
      Location             = $location
      ExePath              = $exe
      UninstallString      = $uninstallString
      QuietUninstallString = $quietString
      UninstallerPath      = $uninstaller
      KeyPath              = $key.PSPath
    }
  }
}

# ---------------------------------------------------------------------------
# Release resolution
# ---------------------------------------------------------------------------

function Format-Tag {
  param([string]$Value)
  $tag = $Value.Trim()
  if ($tag -match '^(?i)v\d') { return $tag }
  if ($tag -match '^\d+\.\d+') { return "v$tag" }
  return $tag
}

function Get-ReleaseTag {
  if ($Version) { return (Format-Tag -Value $Version) }
  if ($Channel -eq 'edge') { return 'edge' }

  # Stable: latest.json is the updater manifest and is fetched by redirect, so
  # no GitHub API quota is involved. The API is only a fallback.
  $meta = Get-ResourceJsonOrNull -Uri "$script:ReleaseRoot/latest/download/latest.json"
  if ($meta -and $meta.version) {
    return "v$($meta.version)"
  }

  $release = Get-ResourceJsonOrNull -Uri "https://api.github.com/repos/$script:Repo/releases/latest"
  if ($release -and $release.tag_name) {
    return [string]$release.tag_name
  }

  Fail "Could not resolve the latest stable release (latest.json and the GitHub API both failed). Pass -Version <tag> or -Channel edge."
}

function Get-ChecksumFile {
  param([string]$Base)
  foreach ($name in $script:ChecksumNames) {
    $uri = Join-ResourceUri -Root $Base -Name $name
    $text = Get-ResourceTextOrNull -Uri $uri
    if ($null -ne $text -and -not [string]::IsNullOrWhiteSpace($text)) {
      return [pscustomobject]@{ Name = $name; Uri = $uri; Text = $text }
    }
  }
  Fail "No checksum file next to the release ($Base): expected one of $($script:ChecksumNames -join ', ')."
}

function ConvertFrom-ChecksumFile {
  param([string]$Text)
  $map = @{}
  foreach ($line in ($Text -split "`r?`n")) {
    $entry = $line.Trim()
    if ([string]::IsNullOrWhiteSpace($entry) -or $entry.StartsWith('#')) { continue }
    if ($entry -match '^([0-9a-fA-F]{64})\s+\*?(.+)$') {
      $hash = $Matches[1].ToLowerInvariant()
      $name = $Matches[2].Trim()
      if ($name) { $map[$name] = $hash }
    }
  }
  return $map
}

function Get-VersionSortKey {
  param([string]$Value)
  $numbers = [regex]::Matches([string]$Value, '\d+')
  return (($numbers | ForEach-Object { $_.Value.PadLeft(8, '0') }) -join '.')
}

function Select-Installer {
  param([hashtable]$Checksums)

  $names = @($Checksums.Keys | Where-Object { $_ -match '(?i)^Ludex_(.+)_x64-setup\.exe$' })
  if ($names.Count -eq 0) {
    $available = ($Checksums.Keys | Sort-Object) -join ', '
    Fail "The checksum file has no Ludex_<version>_x64-setup.exe entry. Files listed: $available"
  }

  $sorted = $names | Sort-Object -Descending -Property @{ Expression = {
      $stem = $_ -replace '(?i)^Ludex_', '' -replace '(?i)_x64-setup\.exe$', ''
      Get-VersionSortKey -Value $stem
    }
  }
  $setupName = @($sorted)[0]
  $targetVersion = $setupName -replace '(?i)^Ludex_', '' -replace '(?i)_x64-setup\.exe$', ''

  [pscustomobject]@{
    Name    = $setupName
    Version = $targetVersion
    Sha256  = $Checksums[$setupName]
  }
}

# ---------------------------------------------------------------------------
# Verification
# ---------------------------------------------------------------------------

function Get-Sha256 {
  param([string]$Path)
  return (Get-FileHash -LiteralPath $Path -Algorithm SHA256).Hash.ToLowerInvariant()
}

function Get-MinisignPath {
  foreach ($name in @('minisign.exe', 'minisign')) {
    $command = Get-Command -Name $name -CommandType Application -ErrorAction SilentlyContinue |
      Select-Object -First 1
    if ($command) { return $command.Source }
  }
  return $null
}

function Write-PinnedPublicKey {
  param([string]$Path)
  try {
    $bytes = [Convert]::FromBase64String($script:MinisignPublicKey)
  } catch {
    Fail "The pinned minisign public key is not valid base64."
  }
  [IO.File]::WriteAllBytes($Path, $bytes)
}

function Assert-MinisignSignature {
  param(
    [string]$Minisign,
    [string]$MessagePath,
    [string]$SignatureText,
    [string]$Label
  )

  $signaturePath = Join-Path $script:TempDir ("minisign-" + [guid]::NewGuid().ToString('N') + '.minisig')
  try {
    $bytes = [Convert]::FromBase64String(($SignatureText -replace '\s', ''))
  } catch {
    Fail "The signature for $Label is not valid base64 (expected a minisign .sig from the release)."
  }
  [IO.File]::WriteAllBytes($signaturePath, $bytes)

  $publicKeyPath = Join-Path $script:TempDir 'ludex-updater.pub'
  if (-not (Test-Path -LiteralPath $publicKeyPath)) {
    Write-PinnedPublicKey -Path $publicKeyPath
  }

  $output = & $Minisign -V -m $MessagePath -p $publicKeyPath -x $signaturePath 2>&1
  $code = $LASTEXITCODE
  if ($code -ne 0) {
    Fail "minisign rejected the $Label signature (exit $code): $($output -join ' ')"
  }
}

function Get-VerificationPlan {
  param([string]$ChecksumsName, [bool]$HasChecksumsSignature, [bool]$HasInstallerSignature)

  $minisign = Get-MinisignPath
  $methods = @()
  if ($minisign -and $HasChecksumsSignature) {
    $methods += "minisign signature over $ChecksumsName (key $script:MinisignKeyId)"
  }
  if ($minisign -and $HasInstallerSignature) {
    $methods += 'minisign signature over the installer'
  }
  if ($methods.Count -gt 0) {
    $methods += 'SHA-256 against the checksum list'
  } elseif ($minisign) {
    $methods = @("SHA-256 against $ChecksumsName (minisign.exe found, but this release carries no usable signature)")
  } else {
    $methods = @("SHA-256 against $ChecksumsName (minisign.exe not on PATH -- the publisher signature is NOT checked)")
  }

  [pscustomobject]@{
    Minisign = $minisign
    Methods  = $methods
  }
}

function Assert-Installer {
  param(
    [string]$InstallerPath,
    [string]$ChecksumsPath,
    [string]$ChecksumsName,
    [string]$ChecksumsSignature,
    [string]$InstallerSignature,
    [string]$ExpectedSha256,
    [string]$InstallerName
  )

  $verification = Get-VerificationPlan `
    -ChecksumsName $ChecksumsName `
    -HasChecksumsSignature ([bool]$ChecksumsSignature) `
    -HasInstallerSignature ([bool]$InstallerSignature)

  if ($verification.Minisign) {
    if ($ChecksumsSignature) {
      Assert-MinisignSignature -Minisign $verification.Minisign -MessagePath $ChecksumsPath `
        -SignatureText $ChecksumsSignature -Label $ChecksumsName
      Write-Host "minisign: $ChecksumsName signature is valid (key $script:MinisignKeyId)."
    }
    if ($InstallerSignature) {
      Assert-MinisignSignature -Minisign $verification.Minisign -MessagePath $InstallerPath `
        -SignatureText $InstallerSignature -Label $InstallerName
      Write-Host "minisign: $InstallerName signature is valid."
    }
  }

  $actual = Get-Sha256 -Path $InstallerPath
  if ($actual -ne $ExpectedSha256.ToLowerInvariant()) {
    Fail "SHA-256 mismatch for $InstallerName`n  expected: $($ExpectedSha256.ToLowerInvariant())`n  actual:   $actual`nNothing was installed."
  }
  Write-Host "SHA-256: $actual matches $ChecksumsName."

  return ($verification.Methods -join ' + ')
}

# ---------------------------------------------------------------------------
# Actions
# ---------------------------------------------------------------------------

function Assert-NotRunning {
  if (-not (Test-IsWindows)) { return }
  $running = @(Get-Process -Name $script:ProductName -ErrorAction SilentlyContinue)
  if ($running.Count -gt 0) {
    $pids = ($running | ForEach-Object { $_.Id }) -join ', '
    Fail "Ludex is running (PID $pids). Close it fully, then run this script again."
  }
}

function Show-SmartScreenNote {
  Write-Host ''
  Write-Host 'Note: Ludex is not Authenticode-signed yet, so Windows SmartScreen may show' -ForegroundColor Yellow
  Write-Host '"Windows protected your PC" / an unknown publisher the first time the setup' -ForegroundColor Yellow
  Write-Host 'runs: choose "More info" then "Run anyway". This is a publisher reputation gap,' -ForegroundColor Yellow
  Write-Host 'not a signature failure -- the SHA-256/signature check above already passed.' -ForegroundColor Yellow
  Write-Host ''
}

function Install-Ludex {
  param($Installer, [string]$InstallerPath)

  $existing = @(Get-LudexInstallation) | Select-Object -First 1
  if ($existing -and -not $Force -and $existing.Version -eq $Installer.Version) {
    Write-Host "Ludex $($existing.Version) is already installed at $($existing.Location)."
    Write-Host 'Use -Force to reinstall over it, or -Uninstall to remove it.'
    return
  }
  if ($existing) {
    Write-Host "Upgrading in place: $($existing.Version) -> $($Installer.Version) (same per-user install)."
  }

  Assert-NotRunning

  $argumentList = '/S'
  $installDir = $null
  if ($Dir) {
    $installDir = [Environment]::ExpandEnvironmentVariables($Dir)
    if (Test-IsWindows) { $installDir = [IO.Path]::GetFullPath($installDir) }
    if ($installDir -match '"') { Fail '-Dir cannot contain a double quote.' }
    # NSIS requires /D= to be the last argument and unquoted; it owns the rest
    # of the command line, so spaces are fine.
    $argumentList = "$argumentList /D=$installDir"
  }

  if ($script:PlanOnly) {
    Write-Host 'Would install silently (per-user, no UAC prompt):'
    Write-Host "  `"$InstallerPath`" $argumentList"
    return
  }

  if (-not (Test-IsWindows)) {
    Fail 'install.ps1 installs Windows software and only runs on Windows. Use -WhatIf to inspect the plan elsewhere.'
  }
  Show-SmartScreenNote
  if (-not (Confirm-Action -Target $InstallerPath -Action "Install Ludex $($Installer.Version) silently")) {
    return
  }

  Write-Host "Installing Ludex $($Installer.Version) silently (per-user, no UAC prompt)..."
  $process = Start-Process -FilePath $InstallerPath -ArgumentList $argumentList -Wait -PassThru
  if ($process.ExitCode -ne 0) {
    Fail "The installer exited with code $($process.ExitCode). Nothing was left half-installed by this script."
  }

  # The NSIS installer writes the HKCU uninstall entry at the end; wait for it
  # so the reported version really comes from the installed app. A forced
  # reinstall keeps the same version, so that also ends the wait.
  $previousVersion = $null
  if ($existing) { $previousVersion = $existing.Version }
  $deadline = (Get-Date).AddSeconds(90)
  $installed = $null
  while ((Get-Date) -lt $deadline) {
    $installed = @(Get-LudexInstallation) | Select-Object -First 1
    if ($installed -and $installed.Version) {
      if ($installed.Version -eq $Installer.Version) { break }
      if ($previousVersion -and $installed.Version -ne $previousVersion) { break }
    }
    Start-Sleep -Milliseconds 500
  }

  if (-not $installed) {
    $fallback = $null
    if ($env:LOCALAPPDATA) { $fallback = Join-Path $env:LOCALAPPDATA 'Ludex\Ludex.exe' }
    if ($installDir) { $fallback = Join-Path $installDir 'Ludex.exe' }
    if ($fallback -and (Test-Path -LiteralPath $fallback)) {
      $installed = [pscustomobject]@{
        Version    = (Get-ExeVersion -Path $fallback)
        Location   = (Split-Path -Parent $fallback)
        ExePath    = $fallback
        DisplayVersion = $null
        FileVersion    = (Get-ExeVersion -Path $fallback)
      }
    }
  }

  if (-not $installed) {
    Fail 'The installer finished but no Ludex entry appeared under HKCU\...\Uninstall and no Ludex.exe was found.'
  }

  $reported = $installed.FileVersion
  if ([string]::IsNullOrWhiteSpace($reported)) { $reported = $installed.DisplayVersion }
  if ([string]::IsNullOrWhiteSpace($reported)) { $reported = $installed.Version }

  Write-Host ''
  Write-Host "Installed: Ludex $reported" -ForegroundColor Green
  if ($installed.Location) { Write-Host "  Location: $($installed.Location)" }
  Write-Host "  DisplayVersion: $($installed.DisplayVersion)"
  Write-Host "  File version:   $($installed.FileVersion)"
  if ($reported -and $Installer.Version -and $reported -ne $Installer.Version) {
    Write-Warning "The app reports $reported while the downloaded installer is $($Installer.Version); check the release assets."
  }
  if ($existing -and $existing.ExePath -and $installed.ExePath -and $existing.ExePath -ne $installed.ExePath) {
    Write-Warning "The previous install lived at $($existing.ExePath); the new one is at $($installed.ExePath)."
  }
  Write-Host 'Run Ludex from the Start Menu, or start Ludex.exe from the location above.'
}

function Uninstall-Ludex {
  if (-not (Test-IsWindows) -and -not $script:PlanOnly) {
    Fail 'install.ps1 only uninstalls Ludex on Windows.'
  }

  $installations = @(Get-LudexInstallation)
  if ($installations.Count -eq 0) {
    Write-Host 'Ludex is not installed for the current user; nothing to uninstall.'
    return
  }

  Assert-NotRunning

  foreach ($installation in $installations) {
    $uninstaller = $installation.UninstallerPath
    $entryName = if ($installation.DisplayName) { $installation.DisplayName } else { 'Ludex' }

    if (-not $uninstaller -or -not (Test-Path -LiteralPath $uninstaller)) {
      Write-Warning "The uninstall entry '$entryName' has no usable uninstaller (UninstallString: $($installation.UninstallString)); skipping."
      continue
    }
    if ((Split-Path -Leaf $uninstaller) -match '^(?i)msiexec\.exe$') {
      Write-Warning "The uninstall entry '$entryName' is an MSI, not the per-user NSIS install; skipping."
      continue
    }

    if ($script:PlanOnly) {
      Write-Host "Would uninstall $entryName with: `"$uninstaller`" /S"
      continue
    }

    if (-not (Confirm-Action -Target $uninstaller -Action "Uninstall $entryName silently")) {
      continue
    }

    Write-Host "Uninstalling $entryName from $($installation.Location)..."
    $process = Start-Process -FilePath $uninstaller -ArgumentList '/S' -Wait -PassThru
    if ($process.ExitCode -ne 0) {
      Write-Warning "The uninstaller exited with code $($process.ExitCode)."
    }

    # The NSIS uninstaller relaunches itself from %TEMP%; the registry entry it
    # deletes is the reliable "done" signal, not the first process's exit.
    $deadline = (Get-Date).AddSeconds(120)
    while ((Get-Date) -lt $deadline) {
      if (-not (Test-Path -LiteralPath $installation.KeyPath)) { break }
      Start-Sleep -Milliseconds 500
    }
  }

  $remaining = @(Get-LudexInstallation)
  if ($remaining.Count -gt 0) {
    $names = ($remaining | ForEach-Object { $_.DisplayName }) -join ', '
    Fail "Ludex is still registered after uninstalling ($names)."
  }

  Write-Host 'Ludex was removed: app files, Start Menu shortcut and the HKCU uninstall entry.'
  Write-Host 'User data under %APPDATA% (settings, library cache) is left in place.'
}

# ---------------------------------------------------------------------------
# Main
# ---------------------------------------------------------------------------

function Invoke-Main {
  if ($Uninstall -and $Dir) {
    Fail '-Dir cannot be combined with -Uninstall.'
  }
  if ($Version -and $Version -ne 'edge' -and $Channel -eq 'edge') {
    Fail 'Use either -Channel edge or -Version <tag>, not both.'
  }

  $script:ShouldProcess = $PSCmdlet
  $script:TempDir = Join-Path ([IO.Path]::GetTempPath()) ('ludex-install-' + [guid]::NewGuid().ToString('N'))

  if ($Uninstall) {
    Write-Host "Ludex uninstaller (per-user)"
    Uninstall-Ludex
    return
  }

  $base = $BaseUri
  if ($base) { $base = Resolve-LocalBaseUri -Root $base }
  $tag = $null
  if (-not $base) {
    $tag = Get-ReleaseTag
    $base = "$script:ReleaseRoot/download/$tag"
    Write-Host "Ludex installer (Windows, per-user) -- channel $Channel, release $tag"
  } else {
    if ($Version) { $tag = Format-Tag -Value $Version }
    Write-Host "Ludex installer (Windows, per-user) -- assets: $base"
  }

  $checksums = Get-ChecksumFile -Base $base
  $map = ConvertFrom-ChecksumFile -Text $checksums.Text
  $installer = Select-Installer -Checksums $map

  $installerUri = Join-ResourceUri -Root $base -Name $installer.Name
  $checksumsSignature = $null
  if ($checksums.Name -eq 'SHA256SUMS') {
    $checksumsSignature = Get-ResourceTextOrNull -Uri (Join-ResourceUri -Root $base -Name 'SHA256SUMS.sig')
  }
  $installerSignature = Get-ResourceTextOrNull -Uri (Join-ResourceUri -Root $base -Name ($installer.Name + '.sig'))

  $verification = Get-VerificationPlan `
    -ChecksumsName $checksums.Name `
    -HasChecksumsSignature ([bool]$checksumsSignature) `
    -HasInstallerSignature ([bool]$installerSignature)

  Write-Host "  Installer:  $($installer.Name)"
  Write-Host "  Download:   $installerUri"
  Write-Host "  Checksums:  $($checksums.Name)"
  Write-Host "  SHA-256:    $($installer.Sha256)"
  Write-Host "  Verification: $($verification.Methods -join ' + ')"

  if ($script:PlanOnly) {
    Write-Host ''
    Write-Host "Would download $($installer.Name) to a temporary folder"
    Write-Host 'Would verify it before running it.'
    Install-Ludex -Installer $installer -InstallerPath (Join-Path $script:TempDir $installer.Name)
    Write-Host 'WhatIf: nothing was downloaded, installed or removed.'
    return
  }

  New-Item -ItemType Directory -Path $script:TempDir -Force | Out-Null
  $installerPath = Join-Path $script:TempDir $installer.Name
  if (-not (Confirm-Action -Target $installerUri -Action "Download $($installer.Name)")) {
    return
  }

  Write-Host "Downloading $($installer.Name)..."
  Save-Resource -Uri $installerUri -Path $installerPath

  $checksumsPath = Join-Path $script:TempDir $checksums.Name
  [IO.File]::WriteAllText($checksumsPath, $checksums.Text)

  $method = Assert-Installer `
    -InstallerPath $installerPath `
    -ChecksumsPath $checksumsPath `
    -ChecksumsName $checksums.Name `
    -ChecksumsSignature $checksumsSignature `
    -InstallerSignature $installerSignature `
    -ExpectedSha256 $installer.Sha256 `
    -InstallerName $installer.Name
  Write-Host "Verified with: $method"

  Install-Ludex -Installer $installer -InstallerPath $installerPath
}

try {
  Invoke-Main
} catch {
  $message = $_.Exception.Message
  $ErrorActionPreference = $script:HostErrorActionPreference
  $ProgressPreference = $script:HostProgressPreference
  Write-Host ''
  Write-Host "FAILED: $message" -ForegroundColor Red
  if ($script:RunFromFile) {
    exit 1
  }
  throw
} finally {
  if ($script:TempDir -and (Test-Path -LiteralPath $script:TempDir)) {
    Remove-Item -LiteralPath $script:TempDir -Recurse -Force -ErrorAction SilentlyContinue
  }
  if ($null -ne $script:HostErrorActionPreference) {
    $ErrorActionPreference = $script:HostErrorActionPreference
  }
  if ($null -ne $script:HostProgressPreference) {
    $ProgressPreference = $script:HostProgressPreference
  }
}
