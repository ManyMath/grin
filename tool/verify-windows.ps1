# Release build of grin-ffi for x86_64-pc-windows-msvc with the host
# toolchain (rustup with the MSVC linker), and the export gate: 35 grin_
# functions and no grin_test_ ones. This is what the hook's cargo mode does
# on a Windows host. Run in PowerShell:
#
#   tool\verify-windows.ps1 [-Fork C:\path\to\grin-wallet] [-Flutter]
#
# -Fork is only needed while the fork is not pushed (the same git URL
# rewrite as tool/local-fork-env.sh). -Flutter also builds the example app
# for Windows when flutter is on PATH.
param(
  [string]$Fork = $env:GRIN_WALLET_LOCAL,
  [switch]$Flutter
)
$ErrorActionPreference = 'Stop'
$root = Split-Path -Parent $PSScriptRoot
if ($Fork) {
  $forkUrl = 'file:///' + ($Fork -replace '\\', '/')
  $env:CARGO_NET_GIT_FETCH_WITH_CLI = 'true'
  $env:GIT_CONFIG_COUNT = '1'
  $env:GIT_CONFIG_KEY_0 = "url.$forkUrl.insteadOf"
  $env:GIT_CONFIG_VALUE_0 = 'ssh://git@git.cypherstack.com:222/sneurlax/grin-wallet.git'
}
# Hard guard: no real ssh from this script.
$env:GIT_SSH_COMMAND = 'cmd /c exit 1'

Write-Output '== host'
rustc --version; cargo --version
$fail = $false

Set-Location "$root\packages\grin\rust"
Write-Output '== cargo build --release --target x86_64-pc-windows-msvc'
cargo build -p grin-ffi --release --locked --target x86_64-pc-windows-msvc
if ($LASTEXITCODE -ne 0) { Write-Output 'BUILD_FAIL'; $fail = $true }
else {
  $dll = "target\x86_64-pc-windows-msvc\release\grin_ffi.dll"
  # llvm-readobj ships with rustup's llvm-tools component.
  rustup component add llvm-tools 2>$null | Out-Null
  $sysroot = (rustc --print sysroot).Trim()
  $readobj = Get-ChildItem -Path "$sysroot\lib\rustlib" -Recurse -Filter llvm-readobj.exe | Select-Object -First 1
  if (-not $readobj) { Write-Output 'NO_LLVM_READOBJ'; $fail = $true }
  else {
    $exports = & $readobj.FullName --coff-exports $dll | Select-String 'Name: (grin_\w+)' | ForEach-Object { $_.Matches[0].Groups[1].Value }
    $std = @($exports | Where-Object { $_ -notlike 'grin_test_*' }).Count
    $test = @($exports | Where-Object { $_ -like 'grin_test_*' }).Count
    Write-Output "exports grin_ffi.dll: $std standard, $test test symbols"
    if ($std -ne 35 -or $test -ne 0) { Write-Output 'EXPORT_FAIL'; $fail = $true }
  }
}

if ($Flutter -and (Get-Command flutter -ErrorAction SilentlyContinue)) {
  Write-Output '== flutter build windows (example)'
  Set-Location $root
  dart pub get | Out-Null
  Set-Location "$root\packages\grin_flutter\example"
  flutter build windows
  if ($LASTEXITCODE -ne 0) { Write-Output 'FLUTTER_FAIL'; $fail = $true }
}

if ($fail) { Write-Output 'VERIFY_WINDOWS_FAIL'; exit 1 } else { Write-Output 'VERIFY_WINDOWS_OK' }
