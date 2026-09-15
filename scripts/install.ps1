# Build redfishctl and install the binary on the PATH.
#
#   powershell -ExecutionPolicy Bypass -File scripts\install.ps1
#
# Uses `cargo install`, which drops the executable in %USERPROFILE%\.cargo\bin.
# That directory is already on the PATH if Rust was installed through rustup, so
# afterwards `redfishctl` works from anywhere.

$ErrorActionPreference = "Stop"

$root = Split-Path -Parent $PSScriptRoot

Write-Host "`nBuilding and installing redfishctl...`n" -ForegroundColor Cyan
cargo install --path $root --force

# $ErrorActionPreference does not govern native executables, only cmdlets.
# Without checking the exit code the script would carry on and report success
# even when cargo had failed, which is the worst way to fail.
if ($LASTEXITCODE -ne 0) {
    Write-Host "`nInstall failed (cargo install returned $LASTEXITCODE)." -ForegroundColor Red

    # On Windows this is almost always the reason, and cargo's own error
    # ("Access is denied") does not say so: the .exe is locked while it runs.
    $running = Get-Process redfishctl -ErrorAction SilentlyContinue
    if ($running) {
        Write-Host "$($running.Count) redfishctl process(es) are running: Windows will not" -ForegroundColor Yellow
        Write-Host "replace a running executable. Quit them (q) and try again.`n" -ForegroundColor Yellow
    } else {
        Write-Host ""
    }
    exit 1
}

$dest = Join-Path $env:USERPROFILE ".cargo\bin"
$onPath = ($env:Path -split ';') -contains $dest

Write-Host "`nInstalled in $dest" -ForegroundColor Green

if ($onPath) {
    Write-Host "Run it with: redfishctl`n" -ForegroundColor Green
} else {
    Write-Host "`nThat directory is not on your PATH. To add it permanently:" -ForegroundColor Yellow
    Write-Host "  [Environment]::SetEnvironmentVariable('Path', `"`$env:Path;$dest`", 'User')"
    Write-Host "Then open a new terminal.`n"
}
