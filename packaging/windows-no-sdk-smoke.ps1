# Smoke test for the self-contained Windows AOT toolchain, run INSIDE a clean
# Windows base-image container with NO Visual Studio / Windows SDK installed.
# Proves `cb --setup-toolchain` + `cb --backend llvm` produce a runnable exe
# whose stdout + exit code match the interpreter oracle, using only the per-user
# Microsoft import-lib fetch — i.e. the released compiler is self-contained.
#
# Expects the extracted release tree mounted at C:\dist (cb.exe, bin\, lib\, and
# this script copied in as smoke.ps1). Adds only the VC++ Redistributable, which
# is needed to *run* cb.exe + produced exes; the base image does not ship it.
$ErrorActionPreference = "Stop"
[Net.ServicePointManager]::SecurityProtocol = [Net.SecurityProtocolType]::Tls12

Write-Host "== Installing the VC++ Redistributable (to run cb.exe + output) =="
Invoke-WebRequest "https://aka.ms/vs/17/release/vc_redist.x64.exe" -OutFile "$env:TEMP\vc_redist.x64.exe"
Start-Process "$env:TEMP\vc_redist.x64.exe" -ArgumentList "/install", "/quiet", "/norestart" -Wait

Set-Location C:\dist
'Print "hello from cb"' | Set-Content -Encoding ASCII hello.cb

Write-Host "== cb --setup-toolchain (fetch MS CRT + Windows SDK import libs) =="
.\cb.exe --setup-toolchain
if ($LASTEXITCODE -ne 0) { throw "setup-toolchain failed ($LASTEXITCODE)" }

Write-Host "== AOT compile with no system SDK present =="
.\cb.exe --backend llvm hello.cb -o hello.exe
if ($LASTEXITCODE -ne 0) { throw "AOT compile failed ($LASTEXITCODE)" }

$native = (& .\hello.exe | Out-String).Trim(); $nrc = $LASTEXITCODE
$interp = (& .\cb.exe --backend interp hello.cb | Out-String).Trim(); $irc = $LASTEXITCODE
Write-Host "native: [$native] rc=$nrc"
Write-Host "interp: [$interp] rc=$irc"
if ($native -ne $interp) { throw "stdout mismatch (native vs interp)" }
if ($nrc -ne $irc) { throw "exit-code mismatch (native=$nrc interp=$irc)" }
Write-Host "OK: native AOT output matches the interpreter oracle with no VS/SDK installed."
