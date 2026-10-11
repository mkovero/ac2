# Runs the Windows binaries on a virtual audio cable through WASAPI: the daemon's device
# list, an in-process duplex self-test, and a daemon session whose capture is recorded for a
# while. CI installs VB-CABLE first (its render endpoint "CABLE Input" loops into its capture
# endpoint "CABLE Output"); any pair of endpoints works with -Capture/-Playback.
#
#   pwsh tools/release/windows-virtual-audio.ps1 -Bin target/debug -LogDir audio-logs
#
# Outputs stay silent: nothing here passes --emit or starts the generator. Every reply is
# kept as JSON in -LogDir, beside the daemon's stderr.
param(
    [Parameter(Mandatory = $true)][string]$Bin,
    [Parameter(Mandatory = $true)][string]$LogDir,
    [string]$Capture = 'CABLE Output (VB-Audio Virtual Cable)',
    [string]$Playback = 'CABLE Input (VB-Audio Virtual Cable)',
    [int]$Seconds = 30
)
$ErrorActionPreference = 'Stop'

New-Item -ItemType Directory -Force -Path $LogDir | Out-Null
$LogDir = (Resolve-Path $LogDir).Path
$ac2 = (Resolve-Path (Join-Path $Bin 'ac2.exe')).Path
$ac2d = (Resolve-Path (Join-Path $Bin 'ac2d.exe')).Path
# Recordings stay out of the uploaded logs: a stereo minute of f32 is ~23 MB.
$work = Join-Path ([System.IO.Path]::GetTempPath()) "ac2-virtual-audio-$PID"
New-Item -ItemType Directory -Force -Path $work | Out-Null

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

$daemon = $null
try {
    Write-Host "== ac2d --backend cpal"
    $daemon = Start-Process -FilePath $ac2d -PassThru -NoNewWindow `
        -ArgumentList @('--backend', 'cpal', '--no-autosave', '--recordings', "`"$work`"") `
        -RedirectStandardOutput (Join-Path $LogDir 'ac2d.stdout.txt') `
        -RedirectStandardError (Join-Path $LogDir 'ac2d.stderr.txt')
    # Holding the handle keeps ExitCode readable after the process ends.
    $null = $daemon.Handle
    Wait-Until 'the daemon to answer' 60 {
        if ($daemon.HasExited) { throw "ac2d exited $($daemon.ExitCode) at startup" }
        Invoke-Ac2 'status' @('status') -Soft
    } | Out-Null

    Write-Host "== ac2 devices: waiting for '$Capture' and '$Playback'"
    # The endpoints appear once the audio service has picked up the freshly installed driver.
    $cpal = Wait-Until 'both endpoints in the cpal device list' 60 {
        $d = Invoke-Ac2 'devices' @('devices') -Soft
        $b = $d | Where-Object { $_.kind -eq 'cpal' }
        if ($b -and ($b.devices | Where-Object { $_.name -eq $Capture -and $_.input }) -and
            ($b.devices | Where-Object { $_.name -eq $Playback -and $_.output })) { $b }
    }
    $cpal.devices | ForEach-Object {
        $in = if ($_.input) { "in $($_.input.max_channels)" } else { '' }
        $out = if ($_.output) { "out $($_.output.max_channels)" } else { '' }
        Write-Host ("  {0}  {1} {2}  [{3}]" -f $_.name, $in, $out, $_.id)
    }

    Write-Host "== ac2 selftest duplex (silent)"
    # In process, beside the idle daemon: shared-mode WASAPI lets both open the endpoints.
    $st = Invoke-Ac2 'selftest-duplex' @('selftest', 'duplex', '--backend', 'cpal',
        '--device', $Capture, '--out-device', $Playback, '--in', '1-2', '--outputs', '2',
        '--duration', '20s')
    if (-not $st.pass) { throw "selftest duplex failed: $($st.failures | ConvertTo-Json -Compress)" }
    Write-Host "  pass"

    Write-Host "== ac2 session open, recording $Seconds s"
    $s = Invoke-Ac2 'session-open' @('session', 'open', '--backend', 'cpal',
        '--device', $Capture, '--out-device', $Playback, '--in', '1-2', '--outputs', '2')
    $rate = [int]$s.open.sample_rate_hz
    Write-Host "  $rate Hz, buffer $($s.open.buffer_frames) frames"
    Invoke-Ac2 'rec-start' @('rec', 'start', '--name', 'virtual-audio', '--max', "$($Seconds + 60)s") | Out-Null
    Start-Sleep -Seconds $Seconds
    $status = Invoke-Ac2 'session-status' @('session', 'status')
    $run = Invoke-Ac2 'rec-stop' @('rec', 'stop')
    Invoke-Ac2 'session-close' @('session', 'close') | Out-Null

    $problems = @()
    if ($status.stopped) { $problems += "session audio stopped: $($status.stopped | ConvertTo-Json -Compress)" }
    # The recording spans from rec start to rec stop, a little over $Seconds.
    $expected = [int64]$rate * $Seconds
    if ($run.frames -lt 0.9 * $expected) { $problems += "recorded $($run.frames) frames, expected at least 90 % of $expected" }
    if ($run.discontinuities -ne 0) { $problems += "$($run.discontinuities) discontinuities in the capture" }
    Write-Host "  recorded $($run.frames) frames, $($run.discontinuities) discontinuities"
    if ($daemon.HasExited) { $problems += "ac2d exited $($daemon.ExitCode) during the run" }
    if ($problems) { throw ($problems -join "`n") }
    Write-Host "== virtual audio: pass"
}
finally {
    if ($daemon -and -not $daemon.HasExited) {
        Invoke-Ac2 'daemon-stop' @('daemon', 'stop') -Soft | Out-Null
        if (-not $daemon.WaitForExit(10000)) { Stop-Process -Id $daemon.Id -Force }
    }
    Remove-Item -Recurse -Force -Path $work -ErrorAction SilentlyContinue
}
