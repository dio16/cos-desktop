# N06 independent attribution probe: sampled window-content differences, process CPU,
# DWM composition counters (desktop-wide) and GPU engine utilisation.
#
# Emits one bounded JSON document on stdout. Every endpoint degrades independently to
# NOT_MEASURED with a closed reason code; a refused endpoint never becomes a zero or an
# app-supplied number. The window content is sampled by asking the owning window to render
# itself into a device context with PrintWindow(PW_RENDERFULLCONTENT); that is not a
# compositor or presentation observation, and image differences have an unknown relationship
# to presented frames. CPU comes from GetProcessTimes and GPU from PDH counters.
#
# Bounded by -Seconds, -IntervalMs, -MaxSamples and -MaxWindowPixels. Every failure is a
# closed code in one JSON object, never a traceback.
param(
    [int]$ProcessId,
    [int]$Seconds = 10,
    [int]$IntervalMs = 20,
    [int]$MaxSamples = 2000,
    [int]$MaxWindowPixels = 4000000
)

$ErrorActionPreference = "Stop"
[Console]::OutputEncoding = [System.Text.Encoding]::UTF8
$OutputEncoding = [System.Text.Encoding]::UTF8

function Emit([hashtable]$value) {
    $value | ConvertTo-Json -Depth 6 -Compress
}

function Fail([string]$code) {
    Emit @{ ok = $false; error = $code }
    exit 0
}

if ($Seconds -lt 1 -or $Seconds -gt 120) { Fail "CAP_RANGE" }
if ($IntervalMs -lt 10 -or $IntervalMs -gt 1000) { Fail "CAP_RANGE" }
if ($MaxSamples -lt 10 -or $MaxSamples -gt 10000) { Fail "CAP_RANGE" }
if ($MaxWindowPixels -lt 10000 -or $MaxWindowPixels -gt 16000000) { Fail "CAP_RANGE" }
$expected = [math]::Ceiling(($Seconds * 1000) / $IntervalMs)
if ($expected -gt $MaxSamples) { Fail "CAP_RANGE" }
try {
    $null = Get-Process -Id $ProcessId -ErrorAction Stop
} catch {
    Fail "PROCESS_NOT_FOUND"
}

$signature = @'
using System;
using System.Runtime.InteropServices;
using System.Text;
using System.Collections.Generic;
using System.Drawing;
using System.Drawing.Imaging;
using System.Diagnostics;

public static class N06Native
{
    public delegate bool EnumProc(IntPtr handle, IntPtr extra);
    [DllImport("user32.dll")] public static extern bool EnumWindows(EnumProc callback, IntPtr extra);
    [DllImport("user32.dll", CharSet = CharSet.Unicode)] public static extern int GetWindowTextLength(IntPtr handle);
    [DllImport("user32.dll", CharSet = CharSet.Unicode)] public static extern int GetWindowText(IntPtr handle, StringBuilder text, int max);
    [DllImport("user32.dll")] public static extern bool IsWindowVisible(IntPtr handle);
    [DllImport("user32.dll")] public static extern bool IsIconic(IntPtr handle);
    [DllImport("user32.dll")] public static extern uint GetWindowThreadProcessId(IntPtr handle, out uint pid);
    [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr handle, out RECT rect);
    [DllImport("user32.dll")] public static extern bool PrintWindow(IntPtr handle, IntPtr hdc, uint flags);
    [StructLayout(LayoutKind.Sequential)] public struct RECT { public int Left, Top, Right, Bottom; }
}

public class WindowTarget
{
    public bool Found;
    public string Reason = "WINDOW_NOT_FOUND";
    public IntPtr Handle = IntPtr.Zero;
    public string Title = "";
    public int Left, Top, Width, Height;
    public bool Visible;
    public bool Minimized;

    public static WindowTarget Find(int pid)
    {
        WindowTarget best = new WindowTarget();
        long bestArea = -1;
        N06Native.EnumProc callback = delegate(IntPtr handle, IntPtr extra)
        {
            uint owner = 0;
            N06Native.GetWindowThreadProcessId(handle, out owner);
            if ((int)owner != pid) return true;
            if (!N06Native.IsWindowVisible(handle)) return true;
            N06Native.RECT rect;
            if (!N06Native.GetWindowRect(handle, out rect)) return true;
            int width = rect.Right - rect.Left;
            int height = rect.Bottom - rect.Top;
            if (width <= 0 || height <= 0) return true;
            long area = (long)width * (long)height;
            if (area <= bestArea) return true;
            bestArea = area;
            best.Found = true;
            best.Reason = "";
            best.Handle = handle;
            best.Left = rect.Left;
            best.Top = rect.Top;
            best.Width = width;
            best.Height = height;
            best.Visible = true;
            best.Minimized = N06Native.IsIconic(handle);
            int length = N06Native.GetWindowTextLength(handle);
            if (length > 0)
            {
                StringBuilder text = new StringBuilder(length + 1);
                N06Native.GetWindowText(handle, text, text.Capacity);
                best.Title = text.ToString();
            }
            return true;
        };
        N06Native.EnumWindows(callback, IntPtr.Zero);
        return best;
    }
}

public class ProcessCpu
{
    public bool Ok;
    public string Reason = "";
    public double TotalMs;

    public static ProcessCpu Read(int pid)
    {
        ProcessCpu reading = new ProcessCpu();
        try
        {
            using (Process process = Process.GetProcessById(pid))
            {
                process.Refresh();
                reading.TotalMs = process.TotalProcessorTime.TotalMilliseconds;
                reading.Ok = true;
                return reading;
            }
        }
        catch (Exception)
        {
            reading.Ok = false;
            reading.Reason = "PROCESS_NOT_READABLE";
            return reading;
        }
    }
}

[StructLayout(LayoutKind.Sequential)]
public struct DWM_TIMING_INFO
{
    public uint cbSize;
    public ulong rateRefresh;
    public ulong rateCompose;
    public ulong qpcVBlank;
    public ulong cRefresh;
    public ulong cDXRefresh;
    public ulong qpcCompose;
    public ulong cFrame;
    public ulong cDXPresent;
    public ulong cRefreshFrame;
    public ulong cFrameSubmitted;
    public uint cDXPresentSubmitted;
    public ulong cFrameConfirmed;
    public uint cDXPresentConfirmed;
    public ulong cRefreshConfirmed;
    public uint cDXRefreshConfirmed;
    public ulong cFramesLate;
    public uint cFramesOutstanding;
    public ulong cFrameDisplayed;
    public ulong qpcFrameDisplayed;
    public ulong cRefreshFrameDisplayed;
    public ulong cFrameComplete;
    public ulong qpcFrameComplete;
    public ulong cFramePending;
    public ulong qpcFramePending;
    public ulong cFramesDisplayed;
    public ulong cFramesComplete;
    public ulong cFramesPending;
    public ulong cFramesAvailable;
    public ulong cFramesDropped;
    public ulong cFramesMissed;
    public ulong cRefreshNextDisplayed;
    public ulong cRefreshNextPresented;
    public ulong cRefreshesDisplayed;
    public ulong cRefreshesPresented;
    public ulong cRefreshStarted;
    public ulong cPixelsReceived;
    public ulong cPixelsDrawn;
    public ulong cBuffersEmpty;
}

public class DwmCounters
{
    [DllImport("dwmapi.dll")] public static extern int DwmGetCompositionTimingInfo(IntPtr hwnd, ref DWM_TIMING_INFO info);

    public bool Ok;
    public string Reason = "";
    public string Error = "";
    public ulong Composed, Displayed, Dropped, Missed, Late, Refreshes;

    public static DwmCounters Read()
    {
        DwmCounters counters = new DwmCounters();
        try
        {
            DWM_TIMING_INFO info = new DWM_TIMING_INFO();
            info.cbSize = (uint)Marshal.SizeOf(typeof(DWM_TIMING_INFO));
            int hr = DwmGetCompositionTimingInfo(IntPtr.Zero, ref info);
            if (hr != 0)
            {
                counters.Ok = false;
                counters.Reason = "DWM_TIMING_UNAVAILABLE";
                counters.Error = "hr=0x" + ((uint)hr).ToString("X8");
                return counters;
            }
            counters.Composed = info.cFrame;
            counters.Displayed = info.cFramesDisplayed;
            counters.Dropped = info.cFramesDropped;
            counters.Missed = info.cFramesMissed;
            counters.Late = info.cFramesLate;
            counters.Refreshes = info.cRefreshesDisplayed;
            counters.Ok = true;
            return counters;
        }
        catch (Exception error)
        {
            counters.Ok = false;
            counters.Reason = "DWM_TIMING_UNAVAILABLE";
            counters.Error = error.GetType().Name;
            return counters;
        }
    }
}

public class FrameSampler : IDisposable
{
    private IntPtr handle;
    private int width;
    private int height;
    private byte[] previous;
    private byte[] current;
    private int sampleStride;
    private Bitmap bitmap;
    private Graphics graphics;

    public int Samples;
    public int Changes;
    public int UniformCaptures;
    public int FailedCaptures;
    public string Reason = "";
    public string Method = "PrintWindow(PW_RENDERFULLCONTENT)";

    public FrameSampler(IntPtr windowHandle, int windowWidth, int windowHeight)
    {
        handle = windowHandle;
        width = windowWidth;
        height = windowHeight;
        int pixels = width * height;
        int skip = 64;
        if (pixels > 250000) skip = 128;
        if (pixels > 1000000) skip = 256;
        sampleStride = skip;
    }

    public void Sample()
    {
        if (handle == IntPtr.Zero || width <= 0 || height <= 0)
        {
            FailedCaptures++;
            Reason = "WINDOW_NOT_FOUND";
            return;
        }
        IntPtr hdc = IntPtr.Zero;
        try
        {
            if (bitmap == null)
            {
                bitmap = new Bitmap(width, height, PixelFormat.Format32bppArgb);
                graphics = Graphics.FromImage(bitmap);
            }
            hdc = graphics.GetHdc();
            bool painted = N06Native.PrintWindow(handle, hdc, 2);
            graphics.ReleaseHdc(hdc);
            hdc = IntPtr.Zero;
            if (!painted)
            {
                FailedCaptures++;
                Reason = "WINDOW_CAPTURE_FAILED";
                return;
            }
            Rectangle area = new Rectangle(0, 0, width, height);
            BitmapData data = bitmap.LockBits(area, ImageLockMode.ReadOnly, PixelFormat.Format32bppArgb);
            int first = -1;
            bool uniform = true;
            try
            {
                int absStride = Math.Abs(data.Stride);
                int perRow = absStride / sampleStride + 1;
                int length = perRow * height;
                if (current == null || current.Length != length)
                {
                    current = new byte[length];
                    if (previous != null && previous.Length != length) { previous = null; }
                }
                int offset = 0;
                for (int y = 0; y < height; y++)
                {
                    int row = (data.Stride >= 0) ? y * data.Stride : (height - 1 - y) * absStride;
                    for (int x = 0; x < absStride; x += sampleStride)
                    {
                        int value = Marshal.ReadByte(data.Scan0, row + x);
                        if (first < 0) first = value;
                        else if (value != first) uniform = false;
                        current[offset++] = (byte)value;
                    }
                }
            }
            finally
            {
                bitmap.UnlockBits(data);
            }
            Samples++;
            if (uniform) UniformCaptures++;
            bool changed = false;
            if (previous != null)
            {
                for (int index = 0; index < current.Length; index++)
                {
                    if (previous[index] != current[index]) { changed = true; break; }
                }
            }
            if (changed) Changes++;
            byte[] swap = previous;
            previous = current;
            current = swap;
        }
        catch (Exception)
        {
            FailedCaptures++;
            Reason = "WINDOW_CAPTURE_FAILED";
            if (hdc != IntPtr.Zero)
            {
                try { graphics.ReleaseHdc(hdc); } catch (Exception) { }
            }
        }
    }

    public void Dispose()
    {
        if (graphics != null) { graphics.Dispose(); graphics = null; }
        if (bitmap != null) { bitmap.Dispose(); bitmap = null; }
        previous = null;
        current = null;
    }
}

public class GpuSampler : IDisposable
{
    [StructLayout(LayoutKind.Sequential)]
    private struct PDH_FMT_COUNTERVALUE_ITEM
    {
        public IntPtr szName;
        public uint CStatus;
        public double doubleValue;
    }

    [DllImport("pdh.dll", CharSet = CharSet.Unicode)]
    private static extern uint PdhOpenQueryW(string source, IntPtr userData, out IntPtr query);
    [DllImport("pdh.dll", CharSet = CharSet.Unicode)]
    private static extern uint PdhAddEnglishCounterW(IntPtr query, string path, IntPtr userData, out IntPtr counter);
    [DllImport("pdh.dll")]
    private static extern uint PdhCollectQueryData(IntPtr query);
    [DllImport("pdh.dll", CharSet = CharSet.Unicode)]
    private static extern uint PdhGetFormattedCounterArrayW(IntPtr counter, uint format, ref uint bufferSize, out uint itemCount, IntPtr buffer);
    [DllImport("pdh.dll")]
    private static extern uint PdhCloseQuery(IntPtr query);
    [DllImport("pdh.dll")]
    private static extern uint PdhRemoveCounter(IntPtr counter);

    private IntPtr query = IntPtr.Zero;
    private IntPtr counter = IntPtr.Zero;
    private string prefix;
    private double sum;
    private int collections;
    private int failedCollections;
    private int instances;
    private double maximum;
    private HashSet<string> engineTypes = new HashSet<string>();

    public bool Ok;
    public string Reason = "";
    public string Error = "";
    public double MeanPercent { get { return collections == 0 ? 0.0 : sum / collections; } }
    public double MaxPercent { get { return maximum; } }
    public int Samples { get { return collections; } }
    public int Failed { get { return failedCollections; } }
    public int Instances { get { return instances; } }
    public string EngineTypes
    {
        get
        {
            List<string> names = new List<string>(engineTypes);
            names.Sort();
            return string.Join(",", names.ToArray());
        }
    }

    public GpuSampler(int pid)
    {
        prefix = "pid_" + pid.ToString() + "_";
        try
        {
            uint status = PdhOpenQueryW(null, IntPtr.Zero, out query);
            if (status != 0) { Fail("GPU_COUNTER_UNAVAILABLE", status); return; }
            status = PdhAddEnglishCounterW(query, "\\GPU Engine(*)\\Utilization Percentage", IntPtr.Zero, out counter);
            if (status != 0) { Fail("GPU_COUNTER_UNAVAILABLE", status); return; }
            status = PdhCollectQueryData(query);
            if (status != 0) { Fail("GPU_COUNTER_UNAVAILABLE", status); return; }
            Ok = true;
        }
        catch (Exception error)
        {
            Fail("GPU_COUNTER_UNAVAILABLE", 0);
            Error = error.GetType().Name;
        }
    }

    private void Fail(string reason, uint status)
    {
        Ok = false;
        Reason = reason;
        if (status != 0) Error = "pdh=0x" + status.ToString("X8");
    }

    public void Collect()
    {
        if (!Ok) return;
        try
        {
            uint status = PdhCollectQueryData(query);
            if (status != 0) { failedCollections++; return; }
            uint size = 0;
            uint items = 0;
            status = PdhGetFormattedCounterArrayW(counter, 0x00000200, ref size, out items, IntPtr.Zero);
            if (status != 0x800007D2 || size == 0)
            {
                failedCollections++;
                return;
            }
            IntPtr buffer = Marshal.AllocHGlobal((int)size);
            try
            {
                status = PdhGetFormattedCounterArrayW(counter, 0x00000200, ref size, out items, buffer);
                if (status != 0)
                {
                    failedCollections++;
                    return;
                }
                int itemSize = Marshal.SizeOf(typeof(PDH_FMT_COUNTERVALUE_ITEM));
                double total = 0.0;
                int count = 0;
                for (uint index = 0; index < items; index++)
                {
                    IntPtr itemPointer = new IntPtr(buffer.ToInt64() + index * itemSize);
                    PDH_FMT_COUNTERVALUE_ITEM item = (PDH_FMT_COUNTERVALUE_ITEM)Marshal.PtrToStructure(itemPointer, typeof(PDH_FMT_COUNTERVALUE_ITEM));
                    if (item.szName == IntPtr.Zero) continue;
                    string name = Marshal.PtrToStringUni(item.szName);
                    if (name == null || !name.StartsWith(prefix, StringComparison.OrdinalIgnoreCase)) continue;
                    if (item.CStatus != 0) continue;
                    total += item.doubleValue;
                    count++;
                    int marker = name.LastIndexOf("engtype_", StringComparison.OrdinalIgnoreCase);
                    if (marker >= 0)
                    {
                        string kind = name.Substring(marker + 8);
                        int underscore = kind.IndexOf('_');
                        if (underscore > 0) kind = kind.Substring(0, underscore);
                        if (kind.Length > 0) engineTypes.Add(kind.ToLowerInvariant());
                    }
                }
                collections++;
                sum += total;
                if (total > maximum) maximum = total;
                if (count > instances) instances = count;
            }
            finally
            {
                Marshal.FreeHGlobal(buffer);
            }
        }
        catch (Exception)
        {
            failedCollections++;
        }
    }

    public void Dispose()
    {
        try
        {
            if (counter != IntPtr.Zero) { PdhRemoveCounter(counter); counter = IntPtr.Zero; }
            if (query != IntPtr.Zero) { PdhCloseQuery(query); query = IntPtr.Zero; }
        }
        catch (Exception) { }
    }
}
'@

try {
    Add-Type -TypeDefinition $signature -ErrorAction Stop -ReferencedAssemblies System.Drawing
} catch {
    Fail "PROBE_UNAVAILABLE"
}

try {
    $target = [WindowTarget]::Find($ProcessId)
    $frames = $null
    $framesReason = ""
    if (-not $target.Found) {
        $framesReason = "WINDOW_NOT_FOUND"
    } elseif ($target.Minimized) {
        $framesReason = "WINDOW_MINIMIZED"
    } elseif (($target.Width * $target.Height) -gt $MaxWindowPixels) {
        $framesReason = "WINDOW_TOO_LARGE"
    } else {
        $frames = New-Object FrameSampler $target.Handle, $target.Width, $target.Height
    }

    # DWM warm-up: cFramesDisplayed/cFramesDropped/cFramesMissed are documented as valid
    # only after a second DwmGetCompositionTimingInfo call, so the first read is discarded
    # and the baseline below is a validated second call. On a host that refuses the API the
    # warm-up and baseline both fail closed to NOT_MEASURED; this host refuses it
    # (DWM_TIMING_UNAVAILABLE with hr=0x88980090), so the success branch is unexercised here.
    $null = [DwmCounters]::Read()
    $cpuStart = [ProcessCpu]::Read($ProcessId)
    $dwmStart = [DwmCounters]::Read()
    $gpu = New-Object GpuSampler $ProcessId

    $watch = [System.Diagnostics.Stopwatch]::StartNew()
    $nextGpu = 0.0
    $iterations = 0
    $iterationCap = [int]$expected
    while (($watch.Elapsed.TotalSeconds -lt $Seconds) -and ($iterations -lt $iterationCap)) {
        $iterations++
        if ($frames -ne $null) {
            $frames.Sample()
        }
        if ($watch.Elapsed.TotalMilliseconds -ge $nextGpu) {
            $gpu.Collect()
            $nextGpu += 1000.0
        }
        $spent = [int]($watch.Elapsed.TotalMilliseconds - (($iterations - 1) * $IntervalMs))
        $pause = $IntervalMs - $spent
        if ($pause -lt 1) { $pause = 1 }
        Start-Sleep -Milliseconds $pause
    }
    $watch.Stop()
    $windowMs = [math]::Round($watch.Elapsed.TotalMilliseconds, 3)

    $cpuEnd = [ProcessCpu]::Read($ProcessId)
    $dwmEnd = [DwmCounters]::Read()

    $cpuStatus = "NOT_MEASURED"
    $cpuReason = "PROCESS_NOT_READABLE"
    $cpuMs = 0.0
    if ($cpuStart.Ok -and $cpuEnd.Ok) {
        $cpuStatus = "OBSERVED"
        $cpuReason = ""
        $cpuMs = [math]::Round([math]::Max(0.0, $cpuEnd.TotalMs - $cpuStart.TotalMs), 3)
    } elseif ($cpuStart.Ok -and -not $cpuEnd.Ok) {
        $cpuReason = "PROCESS_ENDED_DURING_WINDOW"
    }

    $dwmStatus = "NOT_MEASURED"
    $dwmReason = "DWM_TIMING_UNAVAILABLE"
    $dwmError = $dwmStart.Error
    if ($dwmStart.Ok -and $dwmEnd.Ok) {
        $dwmStatus = "OBSERVED"
        $dwmReason = ""
        if (($dwmEnd.Composed -lt $dwmStart.Composed) -or ($dwmEnd.Displayed -lt $dwmStart.Displayed)) {
            $dwmStatus = "NOT_MEASURED"
            $dwmReason = "DWM_COUNTER_REGRESSED"
        }
    } elseif ($dwmEnd.Ok -and -not $dwmStart.Ok) {
        $dwmError = $dwmStart.Error
    }
    $dwmComposed = 0
    $dwmDisplayed = 0
    $dwmDropped = 0
    $dwmMissed = 0
    $dwmLate = 0
    if ($dwmStatus -eq "OBSERVED") {
        $dwmComposed = [long]($dwmEnd.Composed - $dwmStart.Composed)
        $dwmDisplayed = [long]($dwmEnd.Displayed - $dwmStart.Displayed)
        $dwmDropped = [long]($dwmEnd.Dropped - $dwmStart.Dropped)
        $dwmMissed = [long]($dwmEnd.Missed - $dwmStart.Missed)
        $dwmLate = [long]($dwmEnd.Late - $dwmStart.Late)
    }

    $framesStatus = "NOT_MEASURED"
    $framesFinalReason = $framesReason
    $frameSamples = 0
    $frameChanges = 0
    $frameUniform = 0
    $frameFailed = 0
    if ($frames -ne $null) {
        $frameSamples = $frames.Samples
        $frameChanges = $frames.Changes
        $frameUniform = $frames.UniformCaptures
        $frameFailed = $frames.FailedCaptures
        if ($frameSamples -le 0) {
            $framesFinalReason = "NO_SAMPLES"
        } elseif ($frames.Reason -ne "" -and $frameFailed -ge $frameSamples) {
            $framesFinalReason = $frames.Reason
        } elseif ($frameUniform -ge $frameSamples) {
            $framesFinalReason = "WINDOW_CAPTURE_UNIFORM"
        } else {
            $framesStatus = "OBSERVED"
            $framesFinalReason = ""
        }
    }

    $gpuStatus = "NOT_MEASURED"
    $gpuReason = $gpu.Reason
    if ($gpu.Ok -and $gpu.Samples -gt 0 -and $gpu.Instances -gt 0) {
        $gpuStatus = "OBSERVED"
        $gpuReason = ""
    } elseif ($gpu.Ok -and $gpu.Samples -gt 0) {
        $gpuReason = "GPU_NO_PID_INSTANCES"
    } elseif ($gpu.Ok) {
        $gpuReason = "GPU_NO_SAMPLES"
    }
    $gpuMean = 0.0
    $gpuMax = 0.0
    $gpuInstances = 0
    $gpuTypes = ""
    $gpuSamples = 0
    if ($gpuStatus -eq "OBSERVED") {
        $gpuMean = [math]::Round($gpu.MeanPercent, 6)
        $gpuMax = [math]::Round($gpu.MaxPercent, 6)
        $gpuInstances = $gpu.Instances
        $gpuTypes = $gpu.EngineTypes
        $gpuSamples = $gpu.Samples
    }

    $document = @{
        ok = $true
        probe_version = 1
        pid = $ProcessId
        window_ms = $windowMs
        caps = @{
            seconds = $Seconds
            interval_ms = $IntervalMs
            max_samples = $MaxSamples
            max_window_pixels = $MaxWindowPixels
        }
        window = @{
            status = $(if ($target.Found) { "OBSERVED" } else { "NOT_MEASURED" })
            reason = $(if ($target.Found) { "" } else { "WINDOW_NOT_FOUND" })
            title = $target.Title
            handle = [long]$target.Handle.ToInt64()
            left = $target.Left
            top = $target.Top
            width = $target.Width
            height = $target.Height
            visible = [bool]$target.Visible
            minimized = [bool]$target.Minimized
        }
        cpu = @{
            status = $cpuStatus
            reason = $cpuReason
            cpu_ms = $cpuMs
            source = "GetProcessTimes"
            attribution = "process_over_window"
        }
        dwm = @{
            status = $dwmStatus
            reason = $dwmReason
            error = $dwmError
            composed_delta = $dwmComposed
            displayed_delta = $dwmDisplayed
            dropped_delta = $dwmDropped
            missed_delta = $dwmMissed
            late_delta = $dwmLate
        }
        frames = @{
            status = $framesStatus
            reason = $framesFinalReason
            method = "PrintWindow(PW_RENDERFULLCONTENT)"
            samples = $frameSamples
            changes = $frameChanges
            uniform_captures = $frameUniform
            failed_captures = $frameFailed
            interval_ms = $IntervalMs
        }
        gpu = @{
            status = $gpuStatus
            reason = $gpuReason
            error = $gpu.Error
            samples = $gpuSamples
            engine_instances = $gpuInstances
            mean_percent_summed_over_engines = $gpuMean
            max_percent_summed_over_engines = $gpuMax
            engine_types = $gpuTypes
        }
    }
    $gpu.Dispose()
    if ($frames -ne $null) { $frames.Dispose() }
    Emit $document
    exit 0
} catch {
    Fail "PROBE_FAILED"
}
