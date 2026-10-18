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
# LF line ending, so `shasum -c` on a Mac and `sha256sum -c` elsewhere can check it too.
[IO.File]::WriteAllText("$root\dist\windows\MediaForge-Setup.exe.sha256", "$hash  MediaForge-Setup.exe`n")

$exe = "src-tauri\target\release\MediaForge.exe"
# The Visual C++ runtime must be linked in (src-tauri/.cargo/config.toml): a PC without the Visual C++
# Redistributable would otherwise refuse to start MediaForge. (The Universal C Runtime, api-ms-win-crt-*, is
# part of Windows 10 and 11 and needs nothing installed.) Imported DLL names appear as plain text in the file.
$bytes = [IO.File]::ReadAllBytes((Resolve-Path $exe))
$text = [Text.Encoding]::ASCII.GetString($bytes)
foreach ($runtime in @("VCRUNTIME140.dll", "VCRUNTIME140_1.dll", "MSVCP140.dll")) {
    if ($text.IndexOf($runtime, [StringComparison]::OrdinalIgnoreCase) -ge 0) { throw "MediaForge.exe depends on $runtime; the C runtime is not linked statically." }
}
Write-Host "    runtime     Visual C++ runtime linked in (no redistributable needed)"
Write-Host "    installer   dist\windows\MediaForge-Setup.exe  ($([math]::Round($setup.Length / 1MB)) MB)"
Write-Host "    sha256      $hash"
Write-Host "    app         $((Get-Item $exe).VersionInfo.ProductVersion)"
