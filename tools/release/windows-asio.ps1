# Runs the Windows binaries on FlexASIO over a virtual audio cable: the daemon's ASIO device
# list, an in-process duplex self-test, and a daemon session whose capture is recorded for a
# while. CI installs VB-CABLE and FlexASIO first; FlexASIO is pointed at the cable's capture
# and render endpoints by writing %USERPROFILE%\FlexASIO.toml (FlexASIO reads it whenever the
# driver is loaded). Endpoint names vary by driver version and runner image ("CABLE Input" or
# "Speakers" for the render side), so both are found by -Match plus direction.
#
#   pwsh tools/release/windows-asio.ps1 -Bin target/debug -LogDir asio-logs
#
# Outputs stay silent: nothing here passes --emit or starts the generator. Every reply is
# kept as JSON in -LogDir, beside the daemon's stderr and the FlexASIO config and log.
param(
    [Parameter(Mandatory = $true)][string]$Bin,
    [Parameter(Mandatory = $true)][string]$LogDir,
    [string]$Match = 'VB-Audio Virtual Cable',
    [string]$Driver = 'FlexASIO',
    [int]$Seconds = 30
)
$ErrorActionPreference = 'Stop'

New-Item -ItemType Directory -Force -Path $LogDir | Out-Null
$LogDir = (Resolve-Path $LogDir).Path
$ac2 = (Resolve-Path (Join-Path $Bin 'ac2.exe')).Path
$ac2d = (Resolve-Path (Join-Path $Bin 'ac2d.exe')).Path
$work = Join-Path ([System.IO.Path]::GetTempPath()) "ac2-asio-$PID"
New-Item -ItemType Directory -Force -Path $work | Out-Null
$flexConfig = Join-Path $env:USERPROFILE 'FlexASIO.toml'
$flexLog = Join-Path $env:USERPROFILE 'FlexASIO.log'

# One ac2 call with --json: stdout to <Log>.json, stderr to <Log>.stderr.txt; the parsed
# reply, or a throw on a non-zero exit unless -Soft (then $null).
function Invoke-Ac2 {
    param([string]$Log, [string[]]$Arguments, [switch]$Soft)
    $json = Join-Path $LogDir "$Log.json"
    $err = Join-Path $LogDir "$Log.stderr.txt"
    $out = & $ac2 --json @Arguments 2> $err
    $code = $LASTEXITCODE
    $out | Set-Content -Path $json
    if ($code -ne 0) {
        if ($Soft) { return $null }
        Get-Content $err | Write-Host
        throw "ac2 $($Arguments -join ' ') exited $code"
    }
    return ($out | Out-String | ConvertFrom-Json)
}

function Wait-Until {
    param([string]$What, [int]$TimeoutS, [scriptblock]$Ready)
    $deadline = (Get-Date).AddSeconds($TimeoutS)
    while ((Get-Date) -lt $deadline) {
        $r = & $Ready
        if ($r) { return $r }
        Start-Sleep -Milliseconds 500
    }
    throw "timed out after $TimeoutS s waiting for $What"
}

function Show-Asio {
    param($Backend)
    $Backend.devices | ForEach-Object {
        $in = if ($_.input) { "in [$($_.input.channel_names -join ', ')]" } else { '' }
        $out = if ($_.output) { "out [$($_.output.channel_names -join ', ')]" } else { '' }
        Write-Host ("  {0}  {1} {2}" -f $_.id, $in, $out)
        if ($_.input) {
            Write-Host ("    rates {0}; buffer {1}; default {2} Hz, {3} frames" -f ($_.input.rates_hz | ConvertTo-Json -Compress),
                ($_.input.buffer_frames | ConvertTo-Json -Compress), $_.input.default_rate_hz, $_.input.default_buffer_frames)
        }
        $_.notes | ForEach-Object { Write-Host "    note: $_" }
    }
}

# The friendly name of the audio endpoint matching -Match in one direction, as Windows lists
# it: FlexASIO matches its `device` settings against these names. An endpoint's instance id
# carries its data flow: {0.0.0.…} render, {0.0.1.…} capture.
function Find-Endpoint {
    param([ValidateSet('render', 'capture')][string]$Flow)
    $flag = if ($Flow -eq 'render') { '0' } else { '1' }
    Get-PnpDevice -Class AudioEndpoint -PresentOnly -ErrorAction SilentlyContinue |
        Where-Object { $_.FriendlyName -like "*$Match*" -and $_.InstanceId -match "\{0\.0\.$flag\." } |
        Select-Object -First 1 -ExpandProperty FriendlyName
}

$daemon = $null
try {
    Write-Host "== audio endpoints matching '$Match'"
    $capture = Wait-Until 'the cable capture endpoint' 60 { Find-Endpoint capture }
    $render = Wait-Until 'the cable render endpoint' 60 { Find-Endpoint render }
    @(
        'backend = "Windows WASAPI"',
        '',
        '[input]',
        "device = `"$capture`"",
        '',
        '[output]',
        "device = `"$render`""
    ) | Set-Content -Path $flexConfig -Encoding utf8
    # FlexASIO writes a debug log when this file exists.
    New-Item -ItemType File -Force -Path $flexLog | Out-Null
    Copy-Item $flexConfig (Join-Path $LogDir 'FlexASIO.toml')
    Write-Host "  FlexASIO: input '$capture', output '$render'"

    Write-Host "== ac2d --backend asio"
    $daemon = Start-Process -FilePath $ac2d -PassThru -NoNewWindow `
        -ArgumentList @('--backend', 'asio', '--no-autosave', '--recordings', "`"$work`"") `
        -RedirectStandardOutput (Join-Path $LogDir 'ac2d.stdout.txt') `
        -RedirectStandardError (Join-Path $LogDir 'ac2d.stderr.txt')
    $null = $daemon.Handle
    Wait-Until 'the daemon to answer' 60 {
        if ($daemon.HasExited) { throw "ac2d exited $($daemon.ExitCode) at startup" }
        Invoke-Ac2 'status' @('status') -Soft
    } | Out-Null

    Write-Host "== ac2 devices (asio)"
    $asio = Wait-Until "$Driver in the ASIO device list" 30 {
        $d = Invoke-Ac2 'devices' @('devices') -Soft
        $b = $d | Where-Object { $_.kind -eq 'asio' }
        if ($b -and ($b.devices | Where-Object { $_.id -eq $Driver -and $_.input -and $_.output })) { $b }
    }
    Show-Asio $asio

    Write-Host "== ac2 daemon stop (the selftest loads the driver itself)"
    # ASIO runs one driver per process, and FlexASIO opens the cable endpoints itself: the
    # in-process selftest runs with no daemon holding the driver.
    Invoke-Ac2 'daemon-stop' @('daemon', 'stop') | Out-Null
    if (-not $daemon.WaitForExit(20000)) { throw 'ac2d did not stop' }
    $daemon = $null

    Write-Host "== ac2 selftest duplex --backend asio (silent)"
    $st = Invoke-Ac2 'selftest-duplex' @('selftest', 'duplex', '--backend', 'asio',
        '--device', $Driver, '--in', '1-2', '--outputs', '2', '--duration', '20s') -Soft
    if (-not $st) { $st = Get-Content (Join-Path $LogDir 'selftest-duplex.json') | Out-String | ConvertFrom-Json }
    $o = $st.opened
    Write-Host ("  {0} Hz, buffer {1}, index {2}, clock {3}, formats {4}/{5}" -f $o.sample_rate,
        $o.buffer_frames, $o.index, $o.clock, $o.input_format, $o.output_format)
    if (-not $st.pass) { throw "selftest duplex failed: $($st.failures | ConvertTo-Json -Compress)" }
    Write-Host "  pass"

    Write-Host "== ac2d again; session on $Driver, recording $Seconds s"
    $daemon = Start-Process -FilePath $ac2d -PassThru -NoNewWindow `
        -ArgumentList @('--backend', 'asio', '--no-autosave', '--recordings', "`"$work`"") `
        -RedirectStandardOutput (Join-Path $LogDir 'ac2d-session.stdout.txt') `
        -RedirectStandardError (Join-Path $LogDir 'ac2d-session.stderr.txt')
    $null = $daemon.Handle
    Wait-Until 'the daemon to answer' 60 {
        if ($daemon.HasExited) { throw "ac2d exited $($daemon.ExitCode) at startup" }
        Invoke-Ac2 'status-2' @('status') -Soft
    } | Out-Null
    $s = Invoke-Ac2 'session-open' @('session', 'open', '--backend', 'asio',
        '--device', $Driver, '--in', '1-2', '--outputs', '2')
    $rate = [int]$s.open.sample_rate_hz
    Write-Host "  $rate Hz, buffer $($s.open.buffer_frames) frames, clock $($s.open.clock)"
    Invoke-Ac2 'rec-start' @('rec', 'start', '--name', 'asio', '--max', "$($Seconds + 60)s") | Out-Null
    Start-Sleep -Seconds $Seconds
    $status = Invoke-Ac2 'session-status' @('session', 'status')
    $run = Invoke-Ac2 'rec-stop' @('rec', 'stop')
    Invoke-Ac2 'session-close' @('session', 'close') | Out-Null

    $problems = @()
    if ($status.stopped) { $problems += "session audio stopped: $($status.stopped | ConvertTo-Json -Compress)" }
    $expected = [int64]$rate * $Seconds
    if ($run.frames -lt 0.9 * $expected) { $problems += "recorded $($run.frames) frames, expected at least 90 % of $expected" }
    if ($run.discontinuities -ne 0) { $problems += "$($run.discontinuities) discontinuities in the capture" }
    Write-Host "  recorded $($run.frames) frames, $($run.discontinuities) discontinuities"
    if ($daemon.HasExited) { $problems += "ac2d exited $($daemon.ExitCode) during the run" }
    if ($problems) { throw ($problems -join "`n") }
    Write-Host "== ASIO ($Driver): pass"
}
finally {
    if ($daemon -and -not $daemon.HasExited) {
        Invoke-Ac2 'daemon-stop-final' @('daemon', 'stop') -Soft | Out-Null
        if (-not $daemon.WaitForExit(10000)) { Stop-Process -Id $daemon.Id -Force }
    }
    if (Test-Path $flexLog) { Copy-Item $flexLog (Join-Path $LogDir 'FlexASIO.log') -ErrorAction SilentlyContinue }
    Remove-Item -Recurse -Force -Path $work -ErrorAction SilentlyContinue
}
