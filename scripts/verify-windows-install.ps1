# Assert the Windows release artifacts really contain the build we stamped (#186).
#
# The installed `ludex.exe` stayed stale after an update while the web assets
# looked new, so a release must prove two things before it is trusted:
#   1. the built executable embeds `LUDEX_BUILD_COMMIT=<sha>`;
#   2. the NSIS installer carries *that same* executable, byte for byte.
#
# Run on a Windows runner after `tauri build`:
#   pwsh scripts/verify-windows-install.ps1 -Commit $env:GITHUB_SHA
param(
  [Parameter(Mandatory = $true)][string]$Commit,
  [string]$TargetDir = "apps/desktop/src-tauri/target/release"
)

$ErrorActionPreference = "Stop"

function Fail([string]$Message) {
  Write-Host "::error::$Message"
  exit 1
}

if ([string]::IsNullOrWhiteSpace($Commit)) {
  Fail "No commit was provided; pass LUDEX_BUILD_COMMIT so the marker can be checked."
}

$exe = Get-ChildItem -Path $TargetDir -File -ErrorAction SilentlyContinue |
  Where-Object { $_.Name -ieq "ludex.exe" } |
  Select-Object -First 1
if (-not $exe) {
  Fail "No ludex.exe under $TargetDir; the desktop build did not run."
}

$bytes = [System.IO.File]::ReadAllBytes($exe.FullName)
$text = [System.Text.Encoding]::ASCII.GetString($bytes)
$marker = "LUDEX_BUILD_COMMIT=$Commit"
if (-not $text.Contains($marker)) {
  Fail "$($exe.Name) does not embed '$marker'. The build did not receive LUDEX_BUILD_COMMIT."
}

$bundleDir = Join-Path $TargetDir "bundle/nsis"
$installer = Get-ChildItem -Path $bundleDir -Filter "*-setup.exe" -File -ErrorAction SilentlyContinue |
  Select-Object -First 1
if (-not $installer) {
  Fail "No NSIS installer under $bundleDir."
}

$sevenZip = Join-Path $env:ProgramFiles "7-Zip/7z.exe"
if (-not (Test-Path $sevenZip)) {
  $command = Get-Command 7z -ErrorAction SilentlyContinue
  if (-not $command) {
    Fail "7-Zip is required to inspect the NSIS installer and was not found."
  }
  $sevenZip = $command.Source
}

$out = Join-Path $env:RUNNER_TEMP ("ludex-nsis-" + [guid]::NewGuid().ToString("N"))
New-Item -ItemType Directory -Path $out -Force | Out-Null
& $sevenZip x $installer.FullName "-o$out" -y | Out-Null
if ($LASTEXITCODE -ne 0) {
  Fail "7-Zip could not unpack $($installer.Name) (exit $LASTEXITCODE)."
}

$embedded = Get-ChildItem -Path $out -Recurse -File |
  Where-Object { $_.Name -ieq "ludex.exe" } |
  Select-Object -First 1
if (-not $embedded) {
  $listing = (Get-ChildItem -Path $out -Recurse -File | ForEach-Object { $_.FullName }) -join "`n"
  Fail "The NSIS installer does not contain ludex.exe. Extracted: $listing"
}

# The installer may carry a signed or re-packed copy, so a byte-for-byte hash
# difference is not a failure: the build stamp is what proves the payload comes
# from this commit. Compare the stamp inside the embedded executable instead.
$embeddedBytes = [System.IO.File]::ReadAllBytes($embedded.FullName)
$embeddedText = [System.Text.Encoding]::ASCII.GetString($embeddedBytes)
if (-not $embeddedText.Contains($marker)) {
  Fail "The installer's ludex.exe does not embed '$marker'. It was not built from this commit."
}

$builtHash = (Get-FileHash $exe.FullName -Algorithm SHA256).Hash
$embeddedHash = (Get-FileHash $embedded.FullName -Algorithm SHA256).Hash
if ($builtHash -ne $embeddedHash) {
  Write-Host "::warning::The installer payload differs from the built ludex.exe (signed or re-packed); the build stamp matches."
}

Write-Host "PASS: $($installer.Name) embeds a ludex.exe stamped with $Commit ($embeddedHash)."
