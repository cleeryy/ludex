# Tests for install.ps1 (#276). Pester-free and self-contained so it runs the
# same on a windows-latest runner and on a developer machine:
#
#   pwsh -NoProfile -File scripts/test-install.ps1
#
# It never touches the network and never runs a real installer: every case uses
# a local fixture directory (fake SHA256SUMS + a tiny fake setup) through
# install.ps1 -BaseUri.
#
# Covered:
#   * install.ps1 parses with the PowerShell AST parser;
#   * it does not change the execution policy or unblock downloaded files;
#   * -WhatIf plans the install (and /D=) without installing anything;
#   * a tampered SHA256SUMS aborts with a non-zero exit code and no install;
#   * -Uninstall -WhatIf is a no-op when Ludex is not installed.
#
# Deliberately skipped: anything that needs github.com (stable tag resolution,
# the real download), and a real install/uninstall.

$ErrorActionPreference = 'Stop'
$ProgressPreference = 'SilentlyContinue'

$script:RepoRoot = Split-Path -Parent $PSScriptRoot
$script:InstallScript = Join-Path $script:RepoRoot 'install.ps1'
$script:HostExe = (Get-Process -Id $PID).Path
$script:Checks = 0

function Assert-True {
  param([bool]$Condition, [string]$Message)
  $script:Checks++
  if (-not $Condition) {
    throw "FAIL: $Message"
  }
  Write-Host ("  ok {0,2} - {1}" -f $script:Checks, $Message)
}

function Test-Contains {
  param([string]$Text, [string]$Needle)
  return ($Text.IndexOf($Needle, [StringComparison]::OrdinalIgnoreCase) -ge 0)
}

function Invoke-InstallScript {
  param([string[]]$ScriptArguments)
  $output = & $script:HostExe -NoProfile -File $script:InstallScript @ScriptArguments 2>&1
  $code = $LASTEXITCODE
  return [pscustomobject]@{
    ExitCode = $code
    Output   = (($output | ForEach-Object { [string]$_ }) -join "`n")
  }
}

function New-Fixture {
  param([string]$Name, [string]$Hash = $null)

  $dir = Join-Path $script:WorkDir $Name
  New-Item -ItemType Directory -Path $dir -Force | Out-Null
  $setupName = 'Ludex_9.9.9_x64-setup.exe'
  $setupPath = Join-Path $dir $setupName
  [IO.File]::WriteAllText($setupPath, "not a real installer: fixture for scripts/test-install.ps1`n")

  if (-not $Hash) {
    $Hash = (Get-FileHash -LiteralPath $setupPath -Algorithm SHA256).Hash.ToLowerInvariant()
  }
  $lines = @(
    "$Hash  $setupName",
    "$(('0' * 64))  Ludex_9.9.9_x64-setup.exe.sig"
  )
  [IO.File]::WriteAllText((Join-Path $dir 'SHA256SUMS'), (($lines -join "`n") + "`n"))

  return [pscustomobject]@{ Dir = $dir; SetupName = $setupName; Sha256 = $Hash }
}

try {
  if (-not (Test-Path -LiteralPath $script:InstallScript)) {
    throw "install.ps1 not found at $script:InstallScript"
  }
  Write-Host "Testing $script:InstallScript with $script:HostExe"

  $script:WorkDir = Join-Path ([IO.Path]::GetTempPath()) ('ludex-install-tests-' + [guid]::NewGuid().ToString('N'))
  New-Item -ItemType Directory -Path $script:WorkDir -Force | Out-Null

  # -- static checks -------------------------------------------------------
  Write-Host 'static checks'

  $tokens = $null
  $errors = $null
  [System.Management.Automation.Language.Parser]::ParseFile(
    $script:InstallScript, [ref]$tokens, [ref]$errors) | Out-Null
  Assert-True ($errors.Count -eq 0) 'install.ps1 parses with the PowerShell AST parser'
  if ($errors.Count -gt 0) {
    $errors | ForEach-Object { Write-Host ("    {0}:{1} {2}" -f $_.Extent.StartLineNumber, $_.Extent.StartColumnNumber, $_.Message) }
  }

  $text = Get-Content -LiteralPath $script:InstallScript -Raw
  Assert-True (-not (Test-Contains $text 'Set-ExecutionPolicy')) 'install.ps1 never calls Set-ExecutionPolicy'
  Assert-True (-not (Test-Contains $text 'Unblock-File')) 'install.ps1 never calls Unblock-File'
  Assert-True (-not (Test-Contains $text '-Verb RunAs')) 'install.ps1 never elevates with -Verb RunAs'
  Assert-True (Test-Contains $text '#Requires -Version 5.1') 'install.ps1 declares Windows PowerShell 5.1 compatibility'
  Assert-True (Test-Contains $text 'SupportsShouldProcess') 'install.ps1 supports -WhatIf'
  Assert-True (Test-Contains $text 'Get-FileHash') 'install.ps1 verifies SHA-256 with Get-FileHash'
  Assert-True (Test-Contains $text 'minisign') 'install.ps1 can verify the minisign signature'
  Assert-True (Test-Contains $text "Start-Process") 'install.ps1 runs the installer with Start-Process'

  $tauriConfigPath = Join-Path $script:RepoRoot 'apps/desktop/src-tauri/tauri.conf.json'
  $tauriConfig = Get-Content -LiteralPath $tauriConfigPath -Raw | ConvertFrom-Json
  $pinnedKey = [string]$tauriConfig.plugins.updater.pubkey
  Assert-True (-not [string]::IsNullOrWhiteSpace($pinnedKey)) 'tauri.conf.json still publishes an updater pubkey'
  Assert-True (Test-Contains $text $pinnedKey) 'the minisign key pinned in install.ps1 matches tauri.conf.json'

  # -- -WhatIf on a local fixture ------------------------------------------
  Write-Host '-WhatIf on a local fixture'

  $fixture = New-Fixture -Name 'whatif'
  $result = Invoke-InstallScript -ScriptArguments @(
    '-WhatIf', '-BaseUri', $fixture.Dir, '-Dir', (Join-Path $fixture.Dir 'install dir')
  )
  Assert-True ($result.ExitCode -eq 0) '-WhatIf exits 0'
  Assert-True (Test-Contains $result.Output $fixture.SetupName) '-WhatIf names the installer it would use'
  Assert-True (Test-Contains $result.Output $fixture.Sha256) '-WhatIf prints the expected SHA-256'
  Assert-True (Test-Contains $result.Output 'Would install silently') '-WhatIf plans a silent install'
  Assert-True (Test-Contains $result.Output '/D=') '-WhatIf maps -Dir to the NSIS /D= switch'
  Assert-True (Test-Contains $result.Output 'WhatIf: nothing was downloaded') '-WhatIf says nothing was changed'
  Assert-True (-not (Test-Contains $result.Output 'Installing Ludex')) '-WhatIf does not run the installer'

  # -- tampered SHA256SUMS -------------------------------------------------
  Write-Host 'tampered checksum'

  $tampered = New-Fixture -Name 'tampered' -Hash ('0' * 64)
  $result = Invoke-InstallScript -ScriptArguments @('-BaseUri', $tampered.Dir)
  Assert-True ($result.ExitCode -ne 0) 'a mismatched SHA-256 exits non-zero'
  Assert-True (Test-Contains $result.Output 'SHA-256 mismatch') 'the failure names the SHA-256 mismatch'
  Assert-True (Test-Contains $result.Output 'Nothing was installed') 'the failure says nothing was installed'
  Assert-True (-not (Test-Contains $result.Output 'Installing Ludex')) 'a mismatched installer is never started'

  # -- -Uninstall -WhatIf with nothing installed ---------------------------
  Write-Host '-Uninstall -WhatIf'

  $result = Invoke-InstallScript -ScriptArguments @('-Uninstall', '-WhatIf')
  Assert-True ($result.ExitCode -eq 0) '-Uninstall -WhatIf exits 0 when Ludex is absent'
  Assert-True (Test-Contains $result.Output 'nothing to uninstall') '-Uninstall is idempotent when not installed'

  Write-Host ''
  Write-Host ("PASS: {0} checks" -f $script:Checks) -ForegroundColor Green
  exit 0
} catch {
  Write-Host ''
  Write-Host ("FAILED: {0}" -f $_.Exception.Message) -ForegroundColor Red
  exit 1
} finally {
  if ($script:WorkDir -and (Test-Path -LiteralPath $script:WorkDir)) {
    Remove-Item -LiteralPath $script:WorkDir -Recurse -Force -ErrorAction SilentlyContinue
  }
}
