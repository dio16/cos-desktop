# Independent N03 OS capability probe: window identity, UI Automation tree, screenshot.
# Emits one bounded JSON document on stdout; every failure is a closed code, never a
# traceback. Bounded by -MaxElements, -MaxDepth and -MaxScreenshotBytes.
param(
    [string]$Title,
    [int]$ProcessId = 0,
    [string]$Screenshot,
    [int]$MaxElements = 400,
    [int]$MaxDepth = 12,
    [int]$MaxScreenshotBytes = 33554432
)

$ErrorActionPreference = "Stop"
# The record is UTF-8 JSON: window titles are not ASCII on this machine.
[Console]::OutputEncoding = [System.Text.Encoding]::UTF8
$OutputEncoding = [System.Text.Encoding]::UTF8

function Emit([hashtable]$value) {
    $value | ConvertTo-Json -Depth 10 -Compress
}

function Fail([string]$code) {
    Emit @{ ok = $false; error = $code }
    exit 0
}

if ($MaxElements -lt 1 -or $MaxElements -gt 5000) { Fail "CAP_RANGE" }
if ($MaxDepth -lt 1 -or $MaxDepth -gt 64) { Fail "CAP_RANGE" }
if ([string]::IsNullOrEmpty($Title) -and $ProcessId -le 0) { Fail "TARGET_REQUIRED" }
if ([string]::IsNullOrEmpty($Screenshot)) { Fail "SCREENSHOT_PATH_REQUIRED" }

Add-Type -AssemblyName System.Drawing
$signature = @'
using System;
using System.Runtime.InteropServices;
using System.Text;
public static class NativeWindow {
    [DllImport("user32.dll")] public static extern bool EnumWindows(EnumProc callback, IntPtr extra);
    public delegate bool EnumProc(IntPtr handle, IntPtr extra);
    [DllImport("user32.dll")] public static extern int GetWindowTextLength(IntPtr handle);
    [DllImport("user32.dll", CharSet = CharSet.Unicode)] public static extern int GetWindowText(IntPtr handle, StringBuilder text, int max);
    [DllImport("user32.dll")] public static extern bool IsWindowVisible(IntPtr handle);
    [DllImport("user32.dll")] public static extern uint GetWindowThreadProcessId(IntPtr handle, out uint pid);
    [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr handle, out RECT rect);
    [DllImport("user32.dll")] public static extern bool SetForegroundWindow(IntPtr handle);
    [StructLayout(LayoutKind.Sequential)] public struct RECT { public int Left, Top, Right, Bottom; }
}
'@
try {
    Add-Type -TypeDefinition $signature -ErrorAction Stop
} catch {
    Fail "WIN32_UNAVAILABLE"
}

$candidates = New-Object System.Collections.ArrayList
$callback = [NativeWindow+EnumProc] {
    param($handle, $extra)
    if (-not [NativeWindow]::IsWindowVisible($handle)) { return $true }
    $length = [NativeWindow]::GetWindowTextLength($handle)
    if ($length -le 0) { return $true }
    $builder = New-Object System.Text.StringBuilder ($length + 1)
    [void][NativeWindow]::GetWindowText($handle, $builder, $builder.Capacity)
    $title = $builder.ToString()
    if ([string]::IsNullOrWhiteSpace($title)) { return $true }
    $owner = 0
    [void][NativeWindow]::GetWindowThreadProcessId($handle, [ref]$owner)
    [void]$candidates.Add([pscustomobject]@{ Handle = $handle; Title = $title; Pid = [int]$owner })
    return $true
}
[void][NativeWindow]::EnumWindows($callback, [IntPtr]::Zero)

$target = $null
if ($ProcessId -gt 0) {
    # A process can own popups and tool windows: take its largest visible window.
    $bestArea = -1
    foreach ($candidate in $candidates) {
        if ($candidate.Pid -ne $ProcessId) { continue }
        $areaRect = New-Object NativeWindow+RECT
        if (-not [NativeWindow]::GetWindowRect($candidate.Handle, [ref]$areaRect)) { continue }
        $area = ($areaRect.Right - $areaRect.Left) * ($areaRect.Bottom - $areaRect.Top)
        if ($area -gt $bestArea) { $bestArea = $area; $target = $candidate }
    }
} else {
    foreach ($candidate in $candidates) {
        if ($candidate.Title -eq $Title) { $target = $candidate; break }
    }
}
if ($null -eq $target -and $ProcessId -le 0) {
    foreach ($candidate in $candidates) {
        if ($candidate.Title -like "*$Title*") { $target = $candidate; break }
    }
}
if ($null -eq $target) { Fail "WINDOW_NOT_FOUND" }

$rect = New-Object NativeWindow+RECT
if (-not [NativeWindow]::GetWindowRect($target.Handle, [ref]$rect)) { Fail "WINDOW_RECT_UNAVAILABLE" }
$width = $rect.Right - $rect.Left
$height = $rect.Bottom - $rect.Top
if ($width -le 0 -or $height -le 0) { Fail "WINDOW_RECT_UNAVAILABLE" }

function Bound([double]$value) {
    # UIA reports infinity or NaN for offscreen elements; the record stays integral.
    if ([double]::IsNaN($value) -or [double]::IsInfinity($value)) { return 0 }
    if ($value -gt 2147483647) { return 2147483647 }
    if ($value -lt -2147483648) { return -2147483648 }
    return [int]$value
}

$window = @{
    status = "OBSERVED"
    title = $target.Title
    pid = $target.Pid
    handle = $target.Handle.ToInt64()
    left = $rect.Left
    top = $rect.Top
    width = $width
    height = $height
}

# ----------------------------------------------------------------- screenshot
$screenshotRecord = @{ status = "NOT_MEASURED"; reason = "screenshot was not attempted" }
try {
    $bitmap = New-Object System.Drawing.Bitmap $width, $height
    $graphics = [System.Drawing.Graphics]::FromImage($bitmap)
    $graphics.CopyFromScreen($rect.Left, $rect.Top, 0, 0, $bitmap.Size)
    $bitmap.Save($Screenshot, [System.Drawing.Imaging.ImageFormat]::Png)
    $graphics.Dispose()
    $bitmap.Dispose()
    $info = Get-Item -LiteralPath $Screenshot
    if ($info.Length -gt $MaxScreenshotBytes) {
        Remove-Item -LiteralPath $Screenshot -Force -ErrorAction SilentlyContinue
        $screenshotRecord = @{ status = "NOT_MEASURED"; reason = "SCREENSHOT_TOO_LARGE" }
    } else {
        $screenshotRecord = @{ status = "OBSERVED"; width = $width; height = $height; bytes = $info.Length }
    }
} catch {
    $screenshotRecord = @{ status = "NOT_MEASURED"; reason = "SCREENSHOT_FAILED" }
}

# ----------------------------------------------------------------- UI Automation
$uia = @{
    status = "NOT_MEASURED"
    reason = "UI Automation was not attempted"
    elements = @()
    focused = $null
    truncated = $false
    depth_limited = $false
    elements_visited = 0
}
try {
    Add-Type -AssemblyName UIAutomationClient
    Add-Type -AssemblyName UIAutomationTypes
    $root = [System.Windows.Automation.AutomationElement]::FromHandle($target.Handle)
    if ($null -eq $root) { throw "no root" }
    $elements = New-Object System.Collections.ArrayList
    $visited = 0
    $truncated = $false
    $depthLimited = $false
    $walker = [System.Windows.Automation.TreeWalker]::ControlViewWalker
    $queue = New-Object System.Collections.Queue
    $queue.Enqueue(@{ Element = $root; Depth = 0 })
    while ($queue.Count -gt 0) {
        $item = $queue.Dequeue()
        if ($elements.Count -ge $MaxElements) { $truncated = $true; break }
        $current = $item.Element.Current
        $bounds = $current.BoundingRectangle
        $name = $current.Name
        if ($null -eq $name) { $name = "" }
        if ($name.Length -gt 200) { $name = $name.Substring(0, 200) }
        [void]$elements.Add(@{
            depth = $item.Depth
            name = $name
            control_type = $current.ControlType.ProgrammaticName
            automation_id = $current.AutomationId
            focused = $current.HasKeyboardFocus
            left = Bound $bounds.Left
            top = Bound $bounds.Top
            width = Bound $bounds.Width
            height = Bound $bounds.Height
        })
        $visited = $visited + 1
        if ($item.Depth -ge $MaxDepth) { $depthLimited = $true; continue }
        $child = $walker.GetFirstChild($item.Element)
        while ($null -ne $child) {
            $queue.Enqueue(@{ Element = $child; Depth = $item.Depth + 1 })
            $child = $walker.GetNextSibling($child)
        }
    }
    $focused = $null
    try {
        $focusedElement = [System.Windows.Automation.AutomationElement]::FocusedElement
        if ($null -ne $focusedElement) {
            $focused = @{
                name = $focusedElement.Current.Name
                control_type = $focusedElement.Current.ControlType.ProgrammaticName
                automation_id = $focusedElement.Current.AutomationId
            }
        }
    } catch {
        $focused = $null
    }
    $uia = @{
        status = "OBSERVED"
        reason = ""
        elements = $elements.ToArray()
        focused = $focused
        truncated = $truncated
        depth_limited = $depthLimited
        elements_visited = $visited
    }
} catch {
    # The reason stays a closed code; the detail goes to stderr for the operator.
    [Console]::Error.WriteLine("UIA: " + $_.Exception.GetType().Name + ": " + $_.Exception.Message)
    $uia = @{
        status = "NOT_MEASURED"
        reason = "UIA_UNAVAILABLE"
        elements = @()
        focused = $null
        truncated = $false
        depth_limited = $false
        elements_visited = 0
    }
}

Emit @{
    ok = $true
    window = $window
    screenshot = $screenshotRecord
    uia = $uia
    caps = @{ max_elements = $MaxElements; max_depth = $MaxDepth; max_screenshot_bytes = $MaxScreenshotBytes }
}
