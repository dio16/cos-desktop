param(
    [string]$Exe = "C:\dev\cos-desktop\target\debug\examples\peak_probe.exe",
    [ValidateSet('cap', 'cancel', 'all')]
    [string]$Mode = 'all',
    [int64]$LowCapBytes = 1MB,
    [int64]$HighCapBytes = 512MB,
    [int64]$ResolutionBytes = 1MB,
    [int]$CancelRuns = 5,
    [int]$CancelDelayMs = 50,
    [string]$EvidenceDir = "C:\dev\cos-desktop-verification\rel1-2026-10-30",
    [string]$WorkRoot = ""
)

# MP-IMG2 hard memory bound and cancellation latency, without unsafe Rust:
#   cap    - run the 2048x2048 8-bit RGB PNG probe inside a Job Object
#            JOB_OBJECT_LIMIT_PROCESS_MEMORY cap and binary-search the smallest cap that
#            still produces the complete 16,777,216 B RGBA output.
#   cancel - run the probe with --cancel-file, request cancellation during the decode and
#            measure request -> process exit on the parent side.
$ErrorActionPreference = 'Stop'
if ($ResolutionBytes -lt 64KB) { [Console]::Error.WriteLine("-ResolutionBytes must be >= 64 KiB"); exit 2 }
if ($CancelRuns -lt 1) { [Console]::Error.WriteLine("-CancelRuns must be >= 1"); exit 2 }

$stamp = (Get-Date).ToUniversalTime().ToString('yyyyMMddTHHmmssZ')
New-Item -ItemType Directory -Force -Path $EvidenceDir | Out-Null
if ([string]::IsNullOrWhiteSpace($WorkRoot)) { $WorkRoot = Join-Path $env:TEMP "cos-img2-job-$stamp" }
New-Item -ItemType Directory -Force -Path $WorkRoot | Out-Null

Add-Type -TypeDefinition @'
using System;
using System.ComponentModel;
using System.Runtime.InteropServices;

public static class Win32Job
{
    [DllImport("kernel32.dll", CharSet = CharSet.Unicode)]
    private static extern IntPtr CreateJobObject(IntPtr attributes, string name);

    [DllImport("kernel32.dll")]
    private static extern bool SetInformationJobObject(IntPtr job, int infoClass, IntPtr info, uint length);

    [DllImport("kernel32.dll")]
    private static extern bool AssignProcessToJobObject(IntPtr job, IntPtr process);

    [DllImport("kernel32.dll")]
    private static extern bool CloseHandle(IntPtr handle);

    [StructLayout(LayoutKind.Sequential)]
    private struct JOBOBJECT_BASIC_LIMIT_INFORMATION
    {
        public long PerProcessUserTimeLimit;
        public long PerJobUserTimeLimit;
        public uint LimitFlags;
        public UIntPtr MinimumWorkingSetSize;
        public UIntPtr MaximumWorkingSetSize;
        public uint ActiveProcessLimit;
        public UIntPtr Affinity;
        public uint PriorityClass;
        public uint SchedulingClass;
    }

    [StructLayout(LayoutKind.Sequential)]
    private struct IO_COUNTERS
    {
        public ulong ReadOperationCount;
        public ulong WriteOperationCount;
        public ulong OtherOperationCount;
        public ulong ReadTransferCount;
        public ulong WriteTransferCount;
        public ulong OtherTransferCount;
    }

    [StructLayout(LayoutKind.Sequential)]
    private struct JOBOBJECT_EXTENDED_LIMIT_INFORMATION
    {
        public JOBOBJECT_BASIC_LIMIT_INFORMATION BasicLimitInformation;
        public IO_COUNTERS IoInfo;
        public UIntPtr ProcessMemoryLimit;
        public UIntPtr JobMemoryLimit;
        public UIntPtr PeakProcessMemoryUsed;
        public UIntPtr PeakJobMemoryUsed;
    }

    public static IntPtr Create(long processMemoryLimit)
    {
        IntPtr job = CreateJobObject(IntPtr.Zero, null);
        if (job == IntPtr.Zero) throw new Win32Exception();
        var info = new JOBOBJECT_EXTENDED_LIMIT_INFORMATION();
        info.BasicLimitInformation.LimitFlags = 0x100; // JOB_OBJECT_LIMIT_PROCESS_MEMORY
        info.ProcessMemoryLimit = (UIntPtr)processMemoryLimit;
        int length = Marshal.SizeOf(info);
        IntPtr buffer = Marshal.AllocHGlobal(length);
        try
        {
            Marshal.StructureToPtr(info, buffer, false);
            if (!SetInformationJobObject(job, 9, buffer, (uint)length)) // ExtendedLimitInformation
            {
                CloseHandle(job);
                throw new Win32Exception();
            }
        }
        finally
        {
            Marshal.FreeHGlobal(buffer);
        }
        return job;
    }

    public static bool Assign(IntPtr job, IntPtr process) => AssignProcessToJobObject(job, process);

    [StructLayout(LayoutKind.Sequential, CharSet = CharSet.Unicode)]
    private struct STARTUPINFO
    {
        public int cb;
        public string lpReserved;
        public string lpDesktop;
        public string lpTitle;
        public int dwX;
        public int dwY;
        public int dwXSize;
        public int dwYSize;
        public int dwXCountChars;
        public int dwYCountChars;
        public int dwFillAttribute;
        public int dwFlags;
        public short wShowWindow;
        public short cbReserved2;
        public IntPtr lpReserved2;
        public IntPtr hStdInput;
        public IntPtr hStdOutput;
        public IntPtr hStdError;
    }

    [StructLayout(LayoutKind.Sequential)]
    private struct PROCESS_INFORMATION
    {
        public IntPtr hProcess;
        public IntPtr hThread;
        public int dwProcessId;
        public int dwThreadId;
    }

    [DllImport("kernel32.dll", CharSet = CharSet.Unicode, SetLastError = true)]
    private static extern bool CreateProcess(string application, string commandLine, IntPtr processAttributes, IntPtr threadAttributes, bool inheritHandles, uint creationFlags, IntPtr environment, string currentDirectory, ref STARTUPINFO startupInfo, out PROCESS_INFORMATION processInformation);

    [DllImport("kernel32.dll")]
    private static extern uint ResumeThread(IntPtr thread);

    [DllImport("kernel32.dll")]
    private static extern uint WaitForSingleObject(IntPtr handle, uint milliseconds);

    [DllImport("kernel32.dll")]
    private static extern bool GetExitCodeProcess(IntPtr process, out uint exitCode);

    /// Create the process suspended, assign it to the job, then resume it: the cap applies
    /// from the first instruction, so the boundary is not a start/assign race.
    public static IntPtr StartSuspendedInJob(string commandLine, IntPtr job, out int processId)
    {
        var startup = new STARTUPINFO();
        startup.cb = Marshal.SizeOf(startup);
        PROCESS_INFORMATION information;
        if (!CreateProcess(null, commandLine, IntPtr.Zero, IntPtr.Zero, false, 0x4, IntPtr.Zero, null, ref startup, out information))
        {
            throw new Win32Exception();
        }
        try
        {
            if (!AssignProcessToJobObject(job, information.hProcess)) throw new Win32Exception();
            ResumeThread(information.hThread);
        }
        finally
        {
            CloseHandle(information.hThread);
        }
        processId = information.dwProcessId;
        return information.hProcess;
    }

    /// Wait for the process and return its exit code (-1 when the wait itself fails).
    public static int WaitExitCode(IntPtr process, int timeoutMs)
    {
        if (WaitForSingleObject(process, (uint)timeoutMs) != 0) return -1;
        uint code;
        if (!GetExitCodeProcess(process, out code)) return -1;
        return unchecked((int)code);
    }

    public static void Close(IntPtr job)
    {
        if (job != IntPtr.Zero) CloseHandle(job);
    }

    public static void CloseHandlePublic(IntPtr handle)
    {
        if (handle != IntPtr.Zero) CloseHandle(handle);
    }
}
'@

function Get-ProbeOutput([string]$stdout) {
    if (Test-Path -LiteralPath $stdout) { return @(Get-Content -LiteralPath $stdout) }
    return @()
}

function Invoke-CappedProbe([int64]$capBytes) {
    $work = Join-Path $WorkRoot ("cap-" + [guid]::NewGuid().ToString('N'))
    New-Item -ItemType Directory -Path $work | Out-Null
    $ready = Join-Path $work 'ready'
    $go = Join-Path $work 'go'
    $stdout = Join-Path $work 'stdout.txt'
    $stderr = Join-Path $work 'stderr.txt'
    $result = Join-Path $work 'result.txt'
    $process = $null
    $job = [Win32Job]::Create($capBytes)
    try {
        $commandLine = '"' + $Exe + '" ' + (@(2048, 2048, $ready, $go, '--result-file', $result) -join ' ')
        $processId = 0
        $processHandle = [Win32Job]::StartSuspendedInJob($commandLine, $job, [ref]$processId)
        $process = [System.Diagnostics.Process]::GetProcessById($processId)
        $deadline = (Get-Date).AddSeconds(60)
        while (-not (Test-Path -LiteralPath $ready)) {
            if ($process.HasExited) { break }
            if ((Get-Date) -gt $deadline) { break }
            Start-Sleep -Milliseconds 10
        }
        if (Test-Path -LiteralPath $ready) {
            Set-Content -LiteralPath $go -Value 'go'
        }
        $exitCode = [Win32Job]::WaitExitCode($processHandle, 120000)
        $finished = $exitCode -ge 0
        if (-not $finished) { $process.Kill(); $null = $process.WaitForExit(5000) }
        $lines = if (Test-Path -LiteralPath $result) { @(Get-Content -LiteralPath $result) } else { Get-ProbeOutput $stdout }
        $ok = ($finished) -and ($exitCode -eq 0) -and ($lines -contains 'decode-result ok rgba-bytes=16777216')
        return [ordered]@{
            cap_bytes = $capBytes
            success = $ok
            ready = (Test-Path -LiteralPath $ready)
            exit_code = $exitCode
            finished = $finished
            decode_ok = ($lines -contains 'decode-result ok rgba-bytes=16777216')
        }
    } finally {
        if ($null -ne $process -and -not $process.HasExited) { $process.Kill(); $null = $process.WaitForExit(5000) }
        if ($null -ne $processHandle -and $processHandle -ne [IntPtr]::Zero) { [Win32Job]::CloseHandlePublic($processHandle) }
        [Win32Job]::Close($job)
        if (Test-Path -LiteralPath $work) { Remove-Item -LiteralPath $work -Recurse -Force -ErrorAction SilentlyContinue }
    }
}

function Invoke-CancelProbe([int]$index) {
    $work = Join-Path $WorkRoot ("cancel-$index-" + [guid]::NewGuid().ToString('N'))
    New-Item -ItemType Directory -Path $work | Out-Null
    $ready = Join-Path $work 'ready'
    $go = Join-Path $work 'go'
    $cancel = Join-Path $work 'cancel'
    $stdout = Join-Path $work 'stdout.txt'
    $stderr = Join-Path $work 'stderr.txt'
    $process = Start-Process -FilePath $Exe -ArgumentList @(2048, 2048, $ready, $go, '--cancel-file', $cancel) -PassThru -NoNewWindow `
        -RedirectStandardOutput $stdout -RedirectStandardError $stderr
    $deadline = (Get-Date).AddSeconds(60)
    while (-not (Test-Path -LiteralPath $ready)) {
        if ($process.HasExited) { throw "cancel run ${index}: the probe exited before it was ready" }
        if ((Get-Date) -gt $deadline) { throw "cancel run ${index}: no ready marker within 60 s" }
        Start-Sleep -Milliseconds 10
    }
    Set-Content -LiteralPath $go -Value 'go'
    Start-Sleep -Milliseconds $CancelDelayMs
    $requested_at = Get-Date
    Set-Content -LiteralPath $cancel -Value 'cancel'
    $finished = $process.WaitForExit(120000)
    if (-not $finished) { $process.Kill(); $null = $process.WaitForExit(5000) }
    $latency_ms = [int]((Get-Date) - $requested_at).TotalMilliseconds
    $lines = Get-ProbeOutput $stdout
    $cancelled = $lines -contains 'decode-result cancelled typed=cancelled'
    $joined = ($lines -join "`n")
    $internal_delay = $null
    if ($joined -match 'decode-returned-after-cancel-ms=([0-9]+)') {
        $internal_delay = [int]$Matches[1]
    }
    $decode_ms = $null
    if ($joined -match '(?m)^decode-ms=([0-9]+)$') { $decode_ms = [int]$Matches[1] }
    $residual = @(Get-Process -Name 'peak_probe' -ErrorAction SilentlyContinue).Count
    $result = [ordered]@{
        run = $index
        cancelled = $cancelled
        typed_cancelled = $cancelled
        latency_ms = $latency_ms
        probe_delay_ms = $internal_delay
        decode_ms = $decode_ms
        exit_code = if ($process.HasExited) { $process.ExitCode } else { $null }
        finished = $finished
        residual_processes = $residual
        success = ($cancelled -and $finished -and ($process.ExitCode -eq 0) -and ($residual -eq 0))
    }
    if (Test-Path -LiteralPath $stderr) { Copy-Item -LiteralPath $stderr -Destination (Join-Path $EvidenceDir ("img2-cancel-$stamp-run$index.stderr.txt")) -Force }
    Remove-Item -LiteralPath $work -Recurse -Force -ErrorAction SilentlyContinue
    return $result
}

$record = [ordered]@{
    class = 'static 8-bit RGB PNG 2048x2048 -> RGBA8 16,777,216 B'
    exe = $Exe
    exe_sha256 = (Get-FileHash -LiteralPath $Exe -Algorithm SHA256).Hash.ToLowerInvariant()
    mode = $Mode
    sampled_at_utc = $stamp
}

if ($Mode -eq 'cap' -or $Mode -eq 'all') {
    $low = Invoke-CappedProbe $LowCapBytes
    $high = Invoke-CappedProbe $HighCapBytes
    if (-not $high.success) {
        Write-Host ("DEBUG high: " + ($high | ConvertTo-Json -Compress))
        throw "the upper bound $HighCapBytes does not succeed; increase -HighCapBytes"
    }
    if ($low.success) {
        throw "the lower bound $LowCapBytes unexpectedly succeeds; lower -LowCapBytes"
    }
    $cap_low = $LowCapBytes
    $cap_high = $HighCapBytes
    $runs = @($low, $high)
    while (($cap_high - $cap_low) -gt $ResolutionBytes) {
        $mid = [int64]([math]::Floor((($cap_low + $cap_high) / 2) / $ResolutionBytes) * $ResolutionBytes)
        if ($mid -le $cap_low) { $mid = $cap_low + $ResolutionBytes }
        $result = Invoke-CappedProbe $mid
        $runs += $result
        if ($result.success) { $cap_high = $mid } else { $cap_low = $mid }
    }
    # Stability: the minimal success cap must succeed twice and the next failing cap must
    # fail; both are re-measured rather than inferred.
    $minimal = $cap_high
    $failing = $cap_high - $ResolutionBytes
    $confirm = @((Invoke-CappedProbe $minimal), (Invoke-CappedProbe $minimal), (Invoke-CappedProbe $failing))
    $record.cap = [ordered]@{
        resolution_bytes = $ResolutionBytes
        minimal_success_cap_bytes = $minimal
        next_failing_cap_bytes = $failing
        minimal_success_confirmed_twice = ($confirm[0].success -and $confirm[1].success)
        next_failing_confirmed = (-not $confirm[2].success)
        runs = $runs
        confirmations = $confirm
        measurement_limit = 'The Job Object cap limits whole-process commit, which includes the in-process generated fixture, the Rust runtime and the decoder working set; it is not a decoder-only allocation figure. Resolution is the binary-search step.'
    }
}

if ($Mode -eq 'cancel' -or $Mode -eq 'all') {
    $cancels = @()
    for ($index = 1; $index -le $CancelRuns; $index++) {
        $cancels += Invoke-CancelProbe $index
    }
    $latencies = @($cancels | Where-Object { $_.cancelled } | ForEach-Object { $_.latency_ms } | Sort-Object)
    $record.cancel = [ordered]@{
        runs = $cancels
        success_count = @($cancels | Where-Object { $_.success }).Count
        latency_median_ms = if ($latencies.Count -gt 0) { $latencies[[int][math]::Floor(($latencies.Count - 1) / 2)] } else { $null }
        latency_max_ms = if ($latencies.Count -gt 0) { $latencies[-1] } else { $null }
        measurement_limit = 'The image decoder is not interruptible during the call; the observed delay is bounded by the remaining decode time. Parent-side latency includes process exit and observer scheduling.'
    }
}

$json_path = Join-Path $EvidenceDir ("img2-job-$stamp.json")
$txt_path = Join-Path $EvidenceDir ("img2-job-$stamp.txt")
$record | ConvertTo-Json -Depth 10 | Set-Content -LiteralPath $json_path -Encoding UTF8
$lines = @("class: $($record.class)", "exe_sha256: $($record.exe_sha256)", "mode: $($record.mode)")
if ($null -ne $record.cap) {
    $lines += "cap minimal_success=$($record.cap.minimal_success_cap_bytes) next_failing=$($record.cap.next_failing_cap_bytes) resolution=$($record.cap.resolution_bytes)"
    $lines += "cap confirmed_success=$($record.cap.minimal_success_confirmed_twice) confirmed_failure=$($record.cap.next_failing_confirmed) search_runs=$($record.cap.runs.Count)"
}
if ($null -ne $record.cancel) {
    $lines += "cancel success=$($record.cancel.success_count)/$($record.cancel.runs.Count) latency_median_ms=$($record.cancel.latency_median_ms) latency_max_ms=$($record.cancel.latency_max_ms)"
    foreach ($run in $record.cancel.runs) {
        $lines += "cancel run $($run.run): cancelled=$($run.cancelled) latency_ms=$($run.latency_ms) probe_delay_ms=$($run.probe_delay_ms) decode_ms=$($run.decode_ms) exit=$($run.exit_code) residual=$($run.residual_processes)"
    }
}
$lines | Set-Content -LiteralPath $txt_path -Encoding UTF8
Write-Host "record: $json_path"
$lines | ForEach-Object { Write-Host $_ }

$cap_ok = ($Mode -eq 'cancel') -or ($null -ne $record.cap -and $record.cap.minimal_success_confirmed_twice -and $record.cap.next_failing_confirmed)
$cancel_ok = ($Mode -eq 'cap') -or ($null -ne $record.cancel -and $record.cancel.success_count -eq $CancelRuns)
if (-not ($cap_ok -and $cancel_ok)) { exit 1 }
exit 0
