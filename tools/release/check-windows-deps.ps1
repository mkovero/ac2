# Fails if a Windows executable imports the Visual C++ runtime DLLs.
#
#   pwsh tools/release/check-windows-deps.ps1 target/release/ac2.exe target/release/ac2d.exe ...
#
# The workspace builds MSVC targets with +crt-static (.cargo/config.toml): Rust, libzmq (C++)
# and libsodium all link the static CRT, so a clean machine needs no VC++ redistributable.
# A DLL from that redistributable in the import table means some object was compiled for
# the DLL CRT, and the installer would fail on such a machine at first start.
param([Parameter(Mandatory = $true, ValueFromRemainingArguments = $true)][string[]]$Exe)
$ErrorActionPreference = 'Stop'

$vswhere = Join-Path ${env:ProgramFiles(x86)} 'Microsoft Visual Studio\Installer\vswhere.exe'
$vs = & $vswhere -latest -products * -property installationPath
$dumpbin = Get-ChildItem -Path (Join-Path $vs 'VC\Tools\MSVC') -Recurse -Filter dumpbin.exe |
    Where-Object { $_.FullName -match 'Hostx64\\x64' } | Select-Object -First 1
if (-not $dumpbin) { throw "dumpbin.exe not found under $vs" }

$forbidden = '^(msvcp\d+|vcruntime\d+(_\d+)?|concrt\d+|ucrtbase|api-ms-win-crt-.*)\.dll$'
$bad = @()
foreach ($e in $Exe) {
    $out = & $dumpbin.FullName /nologo /dependents $e
    if ($LASTEXITCODE -ne 0) { throw "dumpbin failed on $e" }
    $dlls = $out | ForEach-Object { $_.Trim() } | Where-Object { $_ -match '\.dll$' }
    Write-Host "$e imports: $($dlls -join ', ')"
    $hits = $dlls | Where-Object { $_ -imatch $forbidden }
    if ($hits) { $bad += "${e}: $($hits -join ', ')" }
}
if ($bad) {
    Write-Error ("VC++ runtime imports found:`n" + ($bad -join "`n"))
    exit 1
}
Write-Host 'no VC++ runtime imports'
