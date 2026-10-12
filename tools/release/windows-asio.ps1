# Runs the Windows binaries on FlexASIO over a virtual audio cable: the daemon's ASIO device
# list, an in-process duplex self-test, and a daemon session whose capture is recorded for a
# while. CI installs VB-CABLE and FlexASIO first; FlexASIO is pointed at the cable's capture
# and render endpoints by writing %USERPROFILE%\FlexASIO.toml (FlexASIO reads it whenever the
# driver is loaded). Endpoint names vary by driver version and runner image ("CABLE Input" or
# "Speakers" for the render side), so both are found by -Match plus direction.
#
#   pwsh tools/release/windows-asio.ps1 -Bin target/debug -LogDir asio-logs
#   pwsh tools/release/windows-asio.ps1 -Bin target/debug -LogDir asio-logs -SoakMinutes 60
#
# -SoakMinutes N runs the session for N minutes instead of -Seconds and samples it once a
# minute into soak-samples.csv: recorded frames against rate x time, discontinuities, the
# stopped flag, and the daemon's CPU, working set and private bytes (a leak shows as steady
# growth). It passes with no discontinuity, the frame count within 0.1 % of rate x time and
# the daemon still running. This is the closest a hosted runner comes to the 1 h hardware
# gate (docs/design/asio.md): the timing comes from a virtual cable through WASAPI, not from
# an interface's clock and driver.
#
# Outputs stay silent: nothing here passes --emit or starts the generator. Every reply is
# kept as JSON in -LogDir, beside the daemon's stderr and the FlexASIO config and log.
param(
    [Parameter(Mandatory = $true)][string]$Bin,
    [Parameter(Mandatory = $true)][string]$LogDir,
    [string]$Match = 'VB-Audio Virtual Cable',
    [string]$Driver = 'FlexASIO',
    [int]$Seconds = 30,
    [int]$SoakMinutes = 0
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
if ($SoakMinutes -gt 0) { $Seconds = $SoakMinutes * 60 }

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

# Discontinuities the recording's sidecar puts down to the host's xruns; the sidecar is kept
# in -LogDir. Run after `rec stop`, when the sidecar has its final form.
function Get-Xruns {
    param($Run)
    $sidecar = Join-Path (Split-Path -Parent $Run.path) "$($Run.name).ac2rec.json"
    if (-not (Test-Path $sidecar)) { return 'unknown' }
    Copy-Item $sidecar (Join-Path $LogDir 'recording.ac2rec.json')
    $s = Get-Content $sidecar -Raw | ConvertFrom-Json
    return @($s.discontinuities | Where-Object { $_.causes -contains 'xrun' }).Count
}

# The soak: once a minute until -SoakMinutes have passed since $Clock started, one row of
# soak-samples.csv. Returns the problems seen while it ran; a daemon that exits ends it.
function Watch-Soak {
    param([System.Diagnostics.Stopwatch]$Clock, [int]$Rate)
    $csv = Join-Path $LogDir 'soak-samples.csv'
    'minute,elapsed_s,frames,expected_frames,discontinuities,stopped,cpu_pct_of_one_core,working_set_mb,private_mb,handles' |
        Set-Content -Path $csv
    $found = @()
    $first = $null
    $last = $null
    $prev = @{ wall = 0.0; cpu = $daemon.TotalProcessorTime.TotalSeconds }
    for ($m = 1; $m -le $SoakMinutes; $m++) {
        $due = $m * 60.0
        $wait = $due - $Clock.Elapsed.TotalSeconds
        if ($wait -gt 0) { Start-Sleep -Milliseconds ([int]($wait * 1000)) }
        if ($daemon.HasExited) {
            $found += "ac2d exited $($daemon.ExitCode) in minute $m of the soak"
            break
        }
        $wall = $Clock.Elapsed.TotalSeconds
        $rec = Invoke-Ac2 'soak-rec-status' @('rec', 'status') -Soft
        $ses = Invoke-Ac2 'soak-session-status' @('session', 'status') -Soft
        $daemon.Refresh()
        $cpu = $daemon.TotalProcessorTime.TotalSeconds
        $row = [pscustomobject]@{
            minute          = $m
            elapsed_s       = [math]::Round($wall, 1)
            frames          = if ($rec) { $rec.frames } else { '' }
            expected_frames = [int64]($Rate * $wall)
            discontinuities = if ($rec) { $rec.discontinuities } else { '' }
            stopped         = if ($ses) { [bool]$ses.stopped } else { '' }
            cpu_pct         = [math]::Round(100.0 * ($cpu - $prev.cpu) / ($wall - $prev.wall), 2)
            working_set_mb  = [math]::Round($daemon.WorkingSet64 / 1MB, 1)
            private_mb      = [math]::Round($daemon.PrivateMemorySize64 / 1MB, 1)
            handles         = $daemon.HandleCount
        }
        $prev = @{ wall = $wall; cpu = $cpu }
        ($row.PSObject.Properties.Value -join ',') | Add-Content -Path $csv
        Write-Host ("  minute {0}: {1} frames ({2} expected), {3} discontinuities, stopped {4}; cpu {5} %, ws {6} MB, private {7} MB, {8} handles" -f
            $row.minute, $row.frames, $row.expected_frames, $row.discontinuities, $row.stopped,
            $row.cpu_pct, $row.working_set_mb, $row.private_mb, $row.handles)
        if (-not $rec -or -not $ses) { $found += "minute ${m}: the daemon did not answer" }
        elseif ($rec.discontinuities -ne 0 -or $ses.stopped) {
            $found += "minute ${m}: $($rec.discontinuities) discontinuities, stopped $([bool]$ses.stopped)"
        }
        if (-not $first) { $first = $row }
        $last = $row
    }
    if ($first -and $last -and $last.minute -gt $first.minute) {
        $grow = $last.private_mb - $first.private_mb
        $msg = "private bytes {0} MB at minute {1}, {2} MB at minute {3} ({4:+0.0;-0.0} MB)" -f
            $first.private_mb, $first.minute, $last.private_mb, $last.minute, $grow
        Write-Host "  $msg"
        # Growth is reported, not failed: what a long run legitimately keeps (Leq history,
        # the SPL log) grows too. A leak is steady growth in the CSV.
        if ($grow -gt 64) { Write-Host "::warning title=ac2d soak memory::$msg" }
    }
    return $found
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
    $clock = [System.Diagnostics.Stopwatch]::StartNew()
    $problems = @()
    if ($SoakMinutes -gt 0) {
        $problems += Watch-Soak $clock $rate
    }
    else {
        Start-Sleep -Seconds $Seconds
    }
    $status = Invoke-Ac2 'session-status' @('session', 'status')
    $run = Invoke-Ac2 'rec-stop' @('rec', 'stop')
    $elapsed = $clock.Elapsed.TotalSeconds
    Invoke-Ac2 'session-close' @('session', 'close') | Out-Null

    if ($status.stopped) { $problems += "session audio stopped: $($status.stopped | ConvertTo-Json -Compress)" }
    if ($SoakMinutes -gt 0) {
        # Command latency is well under a second, 0.1 % of a minute-long run or more.
        $expected = [double]$rate * $elapsed
        $off = [math]::Abs($run.frames - $expected) / $expected
        Write-Host ("  recorded {0} frames in {1:N1} s, {2:P3} from {3:N0}" -f $run.frames, $elapsed, $off, $expected)
        if ($off -gt 0.001) { $problems += ("recorded {0} frames in {1:N1} s: {2:P3} from rate x time" -f $run.frames, $elapsed, $off) }
    }
    else {
        $expected = [int64]$rate * $Seconds
        if ($run.frames -lt 0.9 * $expected) { $problems += "recorded $($run.frames) frames, expected at least 90 % of $expected" }
    }
    if ($run.discontinuities -ne 0) { $problems += "$($run.discontinuities) discontinuities in the capture" }
    $xruns = Get-Xruns $run
    Write-Host "  recorded $($run.frames) frames, $($run.discontinuities) discontinuities ($xruns xruns)"
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
