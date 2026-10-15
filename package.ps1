# Release-сборка и portable-архив: dist\Markdowntown-<версия>-windows-x64.zip
#
#   .\package.ps1            # проверки (check.ps1), сборка, архив
#   .\package.ps1 -SkipCheck # без check.ps1
#
# Архив: markdowntown.exe, README.md, лицензии проекта, THIRD_PARTY_LICENSES.html
# (cargo-about), лицензия шрифтов KaTeX. Перед упаковкой exe проверяется на зависимость
# только от системных DLL Windows (portable: ничего не нужно устанавливать).

param([switch] $SkipCheck)

# Не "Stop": cargo пишет прогресс в stderr, и PowerShell 5.1 счёл бы это ошибкой.
$ErrorActionPreference = "Continue"
Set-Location $PSScriptRoot

function Fail([string] $message) {
    Write-Host "FAILED: $message" -ForegroundColor Red
    exit 1
}

if (-not $SkipCheck) {
    & .\check.ps1
    if ($LASTEXITCODE -ne 0) { Fail "check.ps1" }
}

$version = (Select-String -Path Cargo.toml -Pattern '^version = "(.+)"' | Select-Object -First 1).Matches[0].Groups[1].Value
Write-Host "==> release build $version" -ForegroundColor Cyan
cargo build --release
if ($LASTEXITCODE -ne 0) { Fail "cargo build --release" }
$exe = "target\release\markdowntown.exe"

# --- Зависимости exe: только системные DLL --------------------------------------
Write-Host "==> DLL dependencies" -ForegroundColor Cyan
$vswhere = "${env:ProgramFiles(x86)}\Microsoft Visual Studio\Installer\vswhere.exe"
$dumpbin = $null
if (Test-Path $vswhere) {
    $vs = & $vswhere -latest -products * -property installationPath
    if ($vs) {
        $dumpbin = Get-ChildItem "$vs\VC\Tools\MSVC" -Recurse -Filter dumpbin.exe -ErrorAction SilentlyContinue |
            Where-Object FullName -Match 'Hostx64\\x64' | Select-Object -First 1
    }
}
if ($dumpbin) {
    $dlls = & $dumpbin.FullName /dependents $exe | Where-Object { $_ -match '^\s+\S+\.dll\s*$' } | ForEach-Object { $_.Trim() }
    # Не системные: CRT/VC++ Redistributable (должны быть вшиты: +crt-static) и прочее.
    $bad = $dlls | Where-Object { $_ -match '^(vcruntime|msvcp|ucrtbase|api-ms-win-crt)' }
    $dlls | ForEach-Object { Write-Host "    $_" -ForegroundColor DarkGray }
    if ($bad) { Fail "exe depends on non-system DLLs: $($bad -join ', ')" }
    Write-Host "    ok: only system DLLs" -ForegroundColor DarkGray
} else {
    Write-Host "    dumpbin not found (Visual Studio Build Tools) - skipped" -ForegroundColor Yellow
}

# --- Лицензии зависимостей ------------------------------------------------------
Write-Host "==> third-party licenses" -ForegroundColor Cyan
$licenses = "target\THIRD_PARTY_LICENSES.html"
cargo about generate about.hbs -o $licenses
if ($LASTEXITCODE -ne 0) { Fail "cargo about (install: cargo install cargo-about --locked --features cli)" }

# --- Архив ----------------------------------------------------------------------
Write-Host "==> archive" -ForegroundColor Cyan
$name = "Markdowntown-$version-windows-x64"
$stage = "target\package\$name"
Remove-Item -Recurse -Force "target\package" -ErrorAction SilentlyContinue
New-Item -ItemType Directory -Force "$stage\licenses" | Out-Null
Copy-Item $exe, README.md, LICENSE-MIT, LICENSE-APACHE $stage
Copy-Item $licenses $stage
Copy-Item -Recurse licenses\katex-fonts "$stage\licenses\katex-fonts"
# Примеры: все элементы Markdown (showcase.md ссылается на images\ и mermaid.md).
Copy-Item -Recurse examples "$stage\examples"

New-Item -ItemType Directory -Force dist | Out-Null
$zip = "dist\$name.zip"
Remove-Item -Force $zip -ErrorAction SilentlyContinue
Compress-Archive -Path $stage -DestinationPath $zip

$exeSize = (Get-Item $exe).Length / 1MB
$zipSize = (Get-Item $zip).Length / 1MB
Write-Host ("exe: {0:N1} MB, zip: {1:N1} MB -> {2}" -f $exeSize, $zipSize, $zip) -ForegroundColor Green
