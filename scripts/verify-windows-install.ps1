# Checks MediaForge-Setup.exe the way a person would use it: installs it silently for the current user, checks
# the Start Menu entry and "Open with" registration, runs the installed app's self-test (real conversions,
# downloads from a local server, video conversion with this PC's encoder, cancellation), takes screenshots of
# the window, then uninstalls and checks that nothing is left behind.
#
#   pwsh scripts/verify-windows-install.ps1 -Installer dist\windows\MediaForge-Setup.exe -Results dist\windows\verification
param(
    [Parameter(Mandatory = $true)][string]$Installer,
    [Parameter(Mandatory = $true)][string]$Results
)
$ErrorActionPreference = "Stop"
New-Item -ItemType Directory -Force $Results | Out-Null
$Results = (Resolve-Path $Results).Path
$log = Join-Path $Results "verification.txt"
function Note($text) { Write-Host $text; Add-Content -Path $log -Value $text }
function Fail($text) { Note "FAIL: $text"; throw $text }

Add-Type -AssemblyName System.Windows.Forms, System.Drawing
function Snapshot($name) {
    $bounds = [System.Windows.Forms.Screen]::PrimaryScreen.Bounds
    $bitmap = New-Object System.Drawing.Bitmap $bounds.Width, $bounds.Height
    $graphics = [System.Drawing.Graphics]::FromImage($bitmap)
    $graphics.CopyFromScreen($bounds.Location, [System.Drawing.Point]::Empty, $bounds.Size)
    $path = Join-Path $Results "$name.png"
    $bitmap.Save($path, [System.Drawing.Imaging.ImageFormat]::Png)
    $graphics.Dispose(); $bitmap.Dispose()
    Note "screenshot: $name.png"
}

function Find-Uninstall {
    foreach ($hive in @("HKCU:\Software\Microsoft\Windows\CurrentVersion\Uninstall", "HKLM:\Software\Microsoft\Windows\CurrentVersion\Uninstall")) {
        if (-not (Test-Path $hive)) { continue }
        foreach ($key in Get-ChildItem $hive) {
            $props = Get-ItemProperty $key.PSPath
            if ($props.DisplayName -eq "MediaForge") { return $props }
        }
    }
    return $null
}

Note "== Installer: $Installer ($([math]::Round((Get-Item $Installer).Length / 1MB)) MB)"
Note "Windows: $((Get-CimInstance Win32_OperatingSystem).Caption) $((Get-CimInstance Win32_OperatingSystem).Version)"

# 1. Install silently, for the current user (no administrator rights needed).
$install = Start-Process -FilePath $Installer -ArgumentList "/S" -Wait -PassThru
if ($install.ExitCode -ne 0) { Fail "the installer exited with $($install.ExitCode)" }
$entry = Find-Uninstall
if (-not $entry) { Fail "no 'MediaForge' entry in Apps & features" }
$dir = $entry.InstallLocation
if (-not $dir) { $dir = Split-Path -Parent ($entry.UninstallString -replace '"', '') }
$dir = $dir.Trim('"')
Note "installed to: $dir (version $($entry.DisplayVersion), publisher $($entry.Publisher))"

foreach ($file in @("MediaForge.exe", "deno.exe", "uninstall.exe", "tools\yt-dlp\yt-dlp.exe", "tools\ffmpeg\ffmpeg.exe", "tools\ffmpeg\ffprobe.exe", "ThirdPartyNotices\THIRD-PARTY-NOTICES.txt")) {
    if (-not (Test-Path (Join-Path $dir $file))) { Fail "missing after install: $file" }
}
Note "files: MediaForge.exe, deno.exe, tools\yt-dlp, tools\ffmpeg, licence notices: present"
$shortcut = Get-ChildItem "$env:APPDATA\Microsoft\Windows\Start Menu\Programs" -Recurse -Filter "MediaForge.lnk" | Select-Object -First 1
if (-not $shortcut) { Fail "no Start Menu shortcut" }
Note "Start Menu: $($shortcut.FullName)"
$desktopShortcut = Join-Path ([Environment]::GetFolderPath("Desktop")) "MediaForge.lnk"
Note ("Desktop shortcut: " + $(if (Test-Path $desktopShortcut) { $desktopShortcut } else { "none" }))
if (-not (Test-Path "HKCU:\Software\Classes\Applications\MediaForge.exe\SupportedTypes")) { Fail "'Open with' registration is missing" }
if ($null -eq (Get-ItemProperty "HKCU:\Software\Classes\Applications\MediaForge.exe\SupportedTypes" -Name ".webp" -ErrorAction SilentlyContinue)) { Fail "'Open with' isn't registered for WebP images" }
Note "Open with: registered for $(((Get-Item 'HKCU:\Software\Classes\Applications\MediaForge.exe\SupportedTypes').Property).Count) file types"

# 2. The installed app's own self-test, with screenshots while it works.
$exe = Join-Path $dir "MediaForge.exe"
$report = Join-Path $Results "selftest-windows.json"
$test = Start-Process -FilePath $exe -ArgumentList @("--self-test", "`"$report`"") -PassThru
Start-Sleep -Seconds 8; Snapshot "self-test-running-1"
Start-Sleep -Seconds 12; Snapshot "self-test-running-2"
if (-not $test.WaitForExit(600000)) { Stop-Process -Id $test.Id -Force; Fail "the self-test did not finish within 10 minutes" }
if (-not (Test-Path $report)) { Fail "the self-test wrote no report (exit $($test.ExitCode))" }
$result = Get-Content $report -Raw | ConvertFrom-Json
foreach ($check in $result.checks) {
    $status = if (-not $check.ran) { "skip" } elseif ($check.ok) { "PASS" } else { "FAIL" }
    Note ("self-test {0,-16} {1}  {2}" -f $check.name, $status, $check.detail)
}
if (-not $result.ok -or $test.ExitCode -ne 0) { Fail "the self-test reported failures" }

# 3. A normal first launch, as a person would see it.
$app = Start-Process -FilePath $exe -PassThru
Start-Sleep -Seconds 10
Snapshot "first-launch"
Stop-Process -Id $app.Id -Force
Start-Sleep -Seconds 2

# 4. Uninstall silently and check that it's gone.
$uninstaller = Join-Path $dir "uninstall.exe"
Start-Process -FilePath $uninstaller -ArgumentList "/S" -Wait | Out-Null
for ($i = 0; $i -lt 60 -and (Test-Path $exe); $i++) { Start-Sleep -Seconds 1 }
if (Test-Path $exe) { Fail "MediaForge.exe is still there after uninstalling" }
if (Find-Uninstall) { Fail "the Apps & features entry is still there" }
if (Get-ChildItem "$env:APPDATA\Microsoft\Windows\Start Menu\Programs" -Recurse -Filter "MediaForge.lnk") { Fail "the Start Menu shortcut is still there" }
if (Test-Path "HKCU:\Software\Classes\Applications\MediaForge.exe") { Fail "the 'Open with' registration is still there" }
$desktop = [Environment]::GetFolderPath("Desktop")
if (Test-Path (Join-Path $desktop "MediaForge.lnk")) { Fail "the desktop shortcut is still there" }
Note "uninstall: program files, Start Menu and desktop shortcuts, Apps & features entry and 'Open with' registration removed"
Note "== All installation checks passed"
