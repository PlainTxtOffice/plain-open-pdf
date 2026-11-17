#requires -Version 7.3
# Build the PDF engine's Python module and install it into an interpreter's
# site-packages, where the app, its tests, and Nuitka import it. Paths
# resolve from platform/windows, not the caller.
[CmdletBinding()]
param(
    [string]$PythonExecutable = '.venv/Scripts/python.exe',
    # Build without network access, from crates already downloaded.
    [switch]$Offline
)

$ErrorActionPreference = 'Stop'
$projectRoot = [IO.Path]::GetFullPath((Join-Path $PSScriptRoot '../..'))
$python = if ([IO.Path]::IsPathRooted($PythonExecutable)) {
    [IO.Path]::GetFullPath($PythonExecutable)
} else {
    [IO.Path]::GetFullPath((Join-Path $projectRoot $PythonExecutable))
}
if (-not (Test-Path -LiteralPath $python -PathType Leaf)) {
    throw "Interpreter does not exist: $python. Pass -PythonExecutable to use another interpreter."
}
if (-not (Get-Command cargo -ErrorAction SilentlyContinue)) {
    throw 'cargo was not found. Install the Rust toolchain from https://rustup.rs.'
}

$manifest = Join-Path $projectRoot 'plain_open_pdf_engine/python/Cargo.toml'
# PyO3 builds against the interpreter that will import the module.
$env:PYO3_PYTHON = $python
$arguments = @('build', '--release', '--manifest-path', $manifest)
if ($Offline) { $arguments += '--offline' }
Write-Host "Building the PDF engine for $python"
& cargo @arguments
if ($LASTEXITCODE -ne 0) { throw 'Building the PDF engine failed.' }

$built = Join-Path $projectRoot 'plain_open_pdf_engine/python/target/release/plain_open_pdf_engine_python.dll'
$sitePackages = & $python -c 'import sysconfig; print(sysconfig.get_paths()["platlib"])'
if ($LASTEXITCODE -ne 0 -or -not $sitePackages) { throw "Cannot find site-packages for $python." }
$installed = Join-Path $sitePackages 'plain_open_pdf_engine.pyd'
Copy-Item -LiteralPath $built -Destination $installed -Force
& $python -c 'import plain_open_pdf_engine; print("Installed", plain_open_pdf_engine.__file__)'
if ($LASTEXITCODE -ne 0) { throw "The installed engine module does not import: $installed" }
