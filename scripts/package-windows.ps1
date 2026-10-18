# Builds MediaForge for Windows (x64) and its installer, dist\windows\MediaForge-Setup.exe.
#
#   node scripts/fetch-tools.mjs windows      (first: the pinned, checksum-verified tools)
#   pwsh scripts/package-windows.ps1
#
# Needs Node.js, Rust (rustup) and the Visual Studio C++ build tools, as any Tauri app on Windows does.
# Tauri downloads NSIS itself the first time. The installer is not code-signed: see docs/DISTRIBUTION.md.
$ErrorActionPreference = "Stop"
$root = Split-Path -Parent $PSScriptRoot
Set-Location $root

foreach ($required in @("tools\windows\yt-dlp\yt-dlp.exe", "tools\windows\ffmpeg\ffmpeg.exe", "tools\windows\ffmpeg\ffprobe.exe", "tools\windows\bin\deno-x86_64-pc-windows-msvc.exe")) {
    if (-not (Test-Path $required)) { throw "Missing $required. Run: node scripts/fetch-tools.mjs windows" }
}

Write-Host "==> Building MediaForge (x64)"
npx tauri build --bundles nsis --config packaging/bundle.windows.json
if ($LASTEXITCODE -ne 0) { throw "tauri build failed" }

$setup = Get-ChildItem "src-tauri\target\release\bundle\nsis\*-setup.exe" | Sort-Object LastWriteTime -Descending | Select-Object -First 1
if (-not $setup) { throw "The NSIS installer was not produced." }
New-Item -ItemType Directory -Force "dist\windows" | Out-Null
Copy-Item $setup.FullName "dist\windows\MediaForge-Setup.exe" -Force
$hash = (Get-FileHash "dist\windows\MediaForge-Setup.exe" -Algorithm SHA256).Hash.ToLower()
"$hash  MediaForge-Setup.exe" | Out-File "dist\windows\MediaForge-Setup.exe.sha256" -Encoding ascii

$exe = "src-tauri\target\release\MediaForge.exe"
Write-Host "    installer   dist\windows\MediaForge-Setup.exe  ($([math]::Round($setup.Length / 1MB)) MB)"
Write-Host "    sha256      $hash"
Write-Host "    app         $((Get-Item $exe).VersionInfo.ProductVersion)"
