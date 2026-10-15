# Локальные проверки перед коммитом: форматирование, линтер, тесты, сборка.
#
#   .\check.ps1            # fmt, clippy, тесты, debug-сборка
#   .\check.ps1 -Release   # то же + release-сборка и размер exe
#   .\check.ps1 -Perf      # то же + замеры кадра (release)
#
# Код возврата 0 — всё в порядке, иначе — номер первого упавшего шага.

param(
    [switch] $Release,
    [switch] $Perf
)

# Не "Stop": cargo пишет прогресс в stderr, и PowerShell 5.1 счёл бы это ошибкой.
# Успех шагов проверяется по коду возврата.
$ErrorActionPreference = "Continue"
Set-Location $PSScriptRoot

$steps = @(
    @{ Name = "fmt";    Cmd = { cargo fmt --check } },
    @{ Name = "clippy"; Cmd = { cargo clippy --all-targets -- -D warnings } },
    @{ Name = "test";   Cmd = { cargo test } },
    @{ Name = "build";  Cmd = { cargo build } }
)
if ($Release) {
    $steps += @{ Name = "release"; Cmd = { cargo build --release } }
}
if ($Perf) {
    $steps += @{ Name = "perf"; Cmd = { cargo test --release perf -- --ignored --nocapture } }
}

$i = 0
foreach ($step in $steps) {
    $i++
    Write-Host "==> $($step.Name)" -ForegroundColor Cyan
    $started = Get-Date
    & $step.Cmd
    if ($LASTEXITCODE -ne 0) {
        Write-Host "FAILED: $($step.Name)" -ForegroundColor Red
        exit $i
    }
    Write-Host ("    ok ({0:N0} s)" -f ((Get-Date) - $started).TotalSeconds) -ForegroundColor DarkGray
}

if ($Release) {
    $exe = Get-Item target\release\markdowntown.exe
    Write-Host ("release exe: {0:N1} MB" -f ($exe.Length / 1MB))
}
Write-Host "All checks passed" -ForegroundColor Green
