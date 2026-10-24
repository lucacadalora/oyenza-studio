# Weight check (CI, Windows): what Oyen Poni costs while it runs, measured
# from outside the process and compared with tools/weight-budget.json
# (AGENTS.md, Keep it light). tools/ci-smoke.ps1 runs it after its own checks.
#
# At rest: Poni idle in a home of its own (default settings), once start-up
# work is done. Music: a stand-in player (tools/ci-fake-music.ps1) says a
# song is playing, so the closed island shows the music pill and its
# equalizer.
#
# CPU is counted in processor cycles (QueryProcessCycleTime), which is exact:
# Task Manager and TotalProcessorTime count in 15.6 ms ticks and read 0 for
# work that takes a fraction of a millisecond. Wake-ups are the context
# switches of all Poni's threads, from the kernel's process list: each one is
# a thread that woke up. The table goes into the run's summary. A number over
# budget is a warning until "warn_only_until" in the budget file, then it
# fails the run.
param(
    [Parameter(Mandatory = $true)][string]$Exe,
    [string]$Out = "weight-out",
    [string]$Budget = (Join-Path $PSScriptRoot "weight-budget.json"),
    [int]$Seconds = 15
)

$ErrorActionPreference = "Stop"
New-Item -ItemType Directory -Force -Path $Out | Out-Null
$Out = (Resolve-Path $Out).Path

Add-Type @"
using System;
using System.Collections.Generic;
using System.Runtime.InteropServices;
public static class Weigh {
    [DllImport("kernel32.dll")] static extern bool QueryProcessCycleTime(IntPtr process, out ulong cycles);
    [DllImport("user32.dll")] static extern uint GetGuiResources(IntPtr process, uint flags);
    [DllImport("ntdll.dll")] static extern int NtQuerySystemInformation(int infoClass, IntPtr buffer, int length, out int returned);

    public static ulong Cycles(IntPtr process) {
        ulong c;
        return QueryProcessCycleTime(process, out c) ? c : 0;
    }

    public static uint GdiObjects(IntPtr process) { return GetGuiResources(process, 0); }
    public static uint UserObjects(IntPtr process) { return GetGuiResources(process, 1); }

    [DllImport("kernel32.dll")] static extern IntPtr OpenThread(uint access, bool inherit, uint threadId);
    [DllImport("kernel32.dll")] static extern bool CloseHandle(IntPtr handle);
    [DllImport("kernel32.dll")] static extern int GetThreadDescription(IntPtr thread, out IntPtr description);
    [DllImport("kernel32.dll")] static extern IntPtr LocalFree(IntPtr memory);

    // Context switches of each thread of process pid (thread id to count),
    // empty if it isn't running. SystemProcessInformation (5), x64: each
    // process entry is 0x100 bytes (its id at 0x50), then one 0x50-byte entry
    // per thread (its id at 0x30, context switches at 0x40); the same layout
    // poni_core::sysmon reads.
    public static Dictionary<long, long> Switches(int pid) {
        var found = new Dictionary<long, long>();
        int length = 1 << 20;
        for (int tries = 0; tries < 6; tries++) {
            IntPtr buf = Marshal.AllocHGlobal(length);
            try {
                int returned;
                int status = NtQuerySystemInformation(5, buf, length, out returned);
                if (status == unchecked((int)0xC0000004)) {
                    length = Math.Max(length * 2, returned + 65536);
                    continue;
                }
                if (status != 0) return found;
                long offset = 0;
                while (true) {
                    IntPtr entry = new IntPtr(buf.ToInt64() + offset);
                    int next = Marshal.ReadInt32(entry, 0);
                    int threads = Marshal.ReadInt32(entry, 4);
                    if (Marshal.ReadIntPtr(entry, 0x50).ToInt64() == pid) {
                        for (int i = 0; i < threads; i++) {
                            int t = 0x100 + i * 0x50;
                            found[Marshal.ReadIntPtr(entry, t + 0x30).ToInt64()] = (uint)Marshal.ReadInt32(entry, t + 0x40);
                        }
                        return found;
                    }
                    if (next == 0) return found;
                    offset += next;
                }
            } finally {
                Marshal.FreeHGlobal(buf);
            }
        }
        return found;
    }

    // A thread's name (Rust's std names its threads), or "".
    public static string ThreadName(long threadId) {
        IntPtr h = OpenThread(0x0800, false, (uint)threadId);  // THREAD_QUERY_LIMITED_INFORMATION
        if (h == IntPtr.Zero) return "";
        try {
            IntPtr p;
            if (GetThreadDescription(h, out p) < 0 || p == IntPtr.Zero) return "";
            try { return Marshal.PtrToStringUni(p) ?? ""; } finally { LocalFree(p); }
        } finally {
            CloseHandle(h);
        }
    }
}
"@

$mhz = (Get-CimInstance Win32_Processor | Select-Object -First 1).MaxClockSpeed
if (-not $mhz) { $mhz = 3000 }

function Sample($p) {
    [pscustomobject]@{ at = [DateTime]::UtcNow; cycles = [Weigh]::Cycles($p.Handle); switches = [Weigh]::Switches($p.Id) }
}

# The threads that woke most between two samples: "name 2.0/s, ..." (unnamed
# threads by id; the first one Windows starts is the UI thread).
function Busiest($a, $b, [double]$s, [int]$top = 4) {
    $rows = foreach ($tid in $b.switches.Keys) {
        $was = if ($a.switches.ContainsKey($tid)) { $a.switches[$tid] } else { 0 }
        $rate = ($b.switches[$tid] - $was) / $s
        if ($rate -gt 0) {
            $name = [Weigh]::ThreadName($tid)
            [pscustomobject]@{ name = if ($name) { $name } else { "thread $tid" }; rate = $rate }
        }
    }
    ($rows | Sort-Object rate -Descending | Select-Object -First $top | ForEach-Object { "$($_.name) $([Math]::Round($_.rate, 1))/s" }) -join ", "
}

# Cycles and wake-ups a second over $seconds, then memory and handles at the end.
function Read-Weight($p, [int]$seconds) {
    $a = Sample $p
    Start-Sleep -Seconds $seconds
    $b = Sample $p
    $s = ($b.at - $a.at).TotalSeconds
    $p.Refresh()
    $mcycles = ($b.cycles - $a.cycles) / $s / 1e6
    [ordered]@{
        mcycles_per_s = [Math]::Round($mcycles, 2)
        cpu_ms_per_s = [Math]::Round($mcycles / $mhz * 1000, 2)
        wakeups_per_s = if ($b.switches.Count) { [Math]::Round((($b.switches.Values | Measure-Object -Sum).Sum - ($a.switches.Values | Measure-Object -Sum).Sum) / $s, 1) } else { $null }
        busiest = Busiest $a $b $s
        private_mib = [Math]::Round($p.PrivateMemorySize64 / 1MB, 1)
        working_set_mib = [Math]::Round($p.WorkingSet64 / 1MB, 1)
        threads = $p.Threads.Count
        handles = $p.HandleCount
        gdi_objects = [Weigh]::GdiObjects($p.Handle)
        user_objects = [Weigh]::UserObjects($p.Handle)
    }
}

function WaitFor([string]$file, [string]$pattern, [int]$seconds) {
    for ($i = 0; $i -lt $seconds * 4; $i++) {
        if ((Test-Path $file) -and (Select-String -Path $file -Pattern $pattern -Quiet -ErrorAction SilentlyContinue)) { return $true }
        Start-Sleep -Milliseconds 250
    }
    return $false
}

$result = [ordered]@{ exe_mb = [Math]::Round((Get-Item $Exe).Length / 1e6, 2); cpu_mhz = $mhz }

# Only one island runs at a time: a Poni still on its way out (the smoke
# run's) would make this one quit at once.
Get-Process -Name poni -ErrorAction SilentlyContinue | Wait-Process -Timeout 20 -ErrorAction SilentlyContinue
if (Get-Process -Name poni -ErrorAction SilentlyContinue) { throw "another poni.exe is still running" }

# A home of its own: default settings, nothing cached.
$env:PONI_HOME = (New-Item -ItemType Directory -Force -Path (Join-Path $Out "home")).FullName
$log = Join-Path $env:PONI_HOME "cache\poni.log"
$p = Start-Process -FilePath $Exe -PassThru
try {
    if (-not (WaitFor $log "renderer:" 30)) { Write-Host "::warning title=Weight check::Poni logged no renderer line within 30 s" }
    # Start-up work (tray, services, the first readings) settles first.
    Start-Sleep -Seconds 8
    $result.rest = Read-Weight $p $Seconds

    $ready = Join-Path $Out "music-ready.txt"
    $player = Start-Process -FilePath powershell.exe -WindowStyle Hidden -PassThru -ArgumentList @(
        "-NoProfile", "-ExecutionPolicy", "Bypass", "-File", (Join-Path $PSScriptRoot "ci-fake-music.ps1"), "-Seconds", ($Seconds + 40), "-Ready", $ready)
    try {
        $seen = WaitFor $log "media: session appeared" 30
        $said = if (Test-Path $ready) { (Get-Content $ready -Raw).Trim() } else { "(no answer)" }
        if ($seen) {
            # The pill opens and the equalizer starts.
            Start-Sleep -Seconds 3
            $result.music = Read-Weight $p $Seconds
        } else {
            Write-Host "::warning title=Weight check::the stand-in player's session never reached Poni (player: $said)"
            $result.music = $null
        }
        $result.music_player = $said
    } finally {
        Stop-Process -Id $player.Id -Force -ErrorAction SilentlyContinue
    }
} finally {
    Stop-Process -Id $p.Id -Force -ErrorAction SilentlyContinue
    Start-Sleep -Milliseconds 500
}
Copy-Item -Path $log -Destination (Join-Path $Out "poni.log") -ErrorAction SilentlyContinue

# Compare with the budget.
$b = Get-Content $Budget -Raw | ConvertFrom-Json
$rows = @(
    @{ label = "poni.exe"; value = $result.exe_mb; budget = $b.exe_mb; unit = "MB" },
    @{ label = "At rest: memory (private)"; value = $result.rest.private_mib; budget = $b.rest.private_mib; unit = "MiB" },
    @{ label = "At rest: in RAM (working set)"; value = $result.rest.working_set_mib; budget = $b.rest.working_set_mib; unit = "MiB" },
    @{ label = "At rest: CPU"; value = $result.rest.mcycles_per_s; budget = $b.rest.mcycles_per_s; unit = "M cycles/s"; note = "≈ $($result.rest.cpu_ms_per_s) ms/s" },
    @{ label = "At rest: wake-ups"; value = $result.rest.wakeups_per_s; budget = $b.rest.wakeups_per_s; unit = "/s"; note = $result.rest.busiest },
    @{ label = "At rest: threads, handles"; value = $result.rest.threads; budget = $b.rest.threads; unit = ""; note = "$($result.rest.handles) handles, $($result.rest.gdi_objects) GDI, $($result.rest.user_objects) USER" }
)
if ($result.music) {
    $rows += @(
        @{ label = "Music playing: CPU"; value = $result.music.mcycles_per_s; budget = $b.music.mcycles_per_s; unit = "M cycles/s"; note = "≈ $($result.music.cpu_ms_per_s) ms/s" },
        @{ label = "Music playing: wake-ups"; value = $result.music.wakeups_per_s; budget = $b.music.wakeups_per_s; unit = "/s"; note = $result.music.busiest },
        @{ label = "Music playing: memory (private)"; value = $result.music.private_mib; budget = $b.music.private_mib; unit = "MiB" }
    )
}

$over = @()
$lines = @("### Weight check (Windows, $(if ($env:PONI_RENDERER) { $env:PONI_RENDERER } else { 'GPU' }) renderer)", "", "| | Measured | Budget | |", "|---|---|---|---|")
foreach ($r in $rows) {
    $v = if ($null -eq $r.value) { "n/a" } else { "$($r.value) $($r.unit)".Trim() }
    if ($r.note) { $v = "$v ($($r.note))" }
    $bud = if ($null -eq $r.budget) { "" } else { "$($r.budget) $($r.unit)".Trim() }
    $ok = ($null -eq $r.budget) -or ($null -eq $r.value) -or ([double]$r.value -le [double]$r.budget)
    if (-not $ok) { $over += "$($r.label) $($r.value) $($r.unit) (budget $($r.budget))" }
    $lines += "| $($r.label) | $v | $bud | $(if ($ok) { 'ok' } else { '**over**' }) |"
}
if (-not $result.music) { $lines += "| Music playing | not measured: $($result.music_player) | | |" }
$lines += ""
$lines += "Budget: ``tools/weight-budget.json``. CPU in processor cycles at about $mhz MHz. A PR that needs more says why and raises the budget in the same PR."
$lines | Set-Content -Path (Join-Path $Out "weight.md")
$result | ConvertTo-Json -Depth 4 | Set-Content -Path (Join-Path $Out "weight.json")
$lines | Write-Host
if ($env:GITHUB_STEP_SUMMARY) { $lines | Add-Content -Path $env:GITHUB_STEP_SUMMARY }

if ($over.Count -gt 0) {
    $until = [DateTime]::Parse($b.warn_only_until, [Globalization.CultureInfo]::InvariantCulture)
    $msg = "over budget: $($over -join '; ')"
    if ([DateTime]::UtcNow -lt $until) {
        Write-Host "::warning title=Weight check::$msg (warnings only until $($b.warn_only_until))"
    } else {
        Write-Host "::error title=Weight check::$msg"
        exit 1
    }
}
