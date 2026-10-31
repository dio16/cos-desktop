# Independent N03 OS capability probe: window identity, UI Automation tree, screenshot.
# Emits one bounded JSON document on stdout; every failure is a closed code, never a
# traceback. Bounded by -MaxElements, -MaxDepth and -MaxScreenshotBytes.
param(
    [string]$Title,
    [int]$ProcessId = 0,
    [string]$Screenshot,
    [int]$MaxElements = 400,
    [int]$MaxDepth = 12,
    [int]$MaxScreenshotBytes = 33554432,
    # Explicit display scale for callers whose own DPI awareness makes GetWindowRect
    # report DIP coordinates while the display is physical. 1.0 keeps the legacy path.
    [double]$DpiScale = 1.0
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
if ([double]::IsNaN($DpiScale) -or [double]::IsInfinity($DpiScale) -or $DpiScale -lt 0.5 -or $DpiScale -gt 4.0) { Fail "CAP_RANGE" }
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
    [DllImport("user32.dll")] public static extern bool PrintWindow(IntPtr handle, IntPtr hdc, uint flags);
    [DllImport("user32.dll")] public static extern bool IsWindow(IntPtr handle);
    [DllImport("user32.dll")] public static extern IntPtr GetForegroundWindow();
    [DllImport("user32.dll")] public static extern int GetSystemMetrics(int index);
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
# Capture the target HWND itself: PrintWindow renders that window's surface, so a
# foreign window occupying the same screen rectangle cannot be recorded as this
# window. The bound image is then verified against the display over the window's
# on-screen region: an occluding foreign window disagrees and the capture is refused.
# Nothing about the window is changed: no raise, no focus, no z-order operation, so an
# occluded target is simply NOT_MEASURED. Every failure is a closed refusal, never a
# fallback image.
$screenshotRecord = @{ status = "NOT_MEASURED"; reason = "screenshot was not attempted" }
try {
    $bitmap = New-Object System.Drawing.Bitmap $width, $height
    $graphics = [System.Drawing.Graphics]::FromImage($bitmap)
    $hdc = $graphics.GetHdc()
    # PW_RENDERFULLCONTENT (2) asks the window to render its composed content.
    $printed = [NativeWindow]::PrintWindow($target.Handle, $hdc, 2)
    $graphics.ReleaseHdc($hdc)
    $graphics.Dispose()
    if (-not $printed) {
        $bitmap.Dispose()
        $screenshotRecord = @{ status = "NOT_MEASURED"; reason = "WINDOW_CAPTURE_FAILED" }
    } else {
        # Identity signal 1: the same window must still be current, at the same
        # rectangle, after the capture.
        $after = New-Object NativeWindow+RECT
        $sameWindow = [NativeWindow]::IsWindow($target.Handle) -and [NativeWindow]::GetWindowRect($target.Handle, [ref]$after)
        $sameRect = $sameWindow -and ($after.Left -eq $rect.Left) -and ($after.Top -eq $rect.Top) -and (($after.Right - $after.Left) -eq $width) -and (($after.Bottom - $after.Top) -eq $height)
        # Content signal: sampled agreement between the bound capture and the display
        # over the intersection of the window rectangle with the virtual screen.
        $vx = [NativeWindow]::GetSystemMetrics(76)
        $vy = [NativeWindow]::GetSystemMetrics(77)
        $vw = [NativeWindow]::GetSystemMetrics(78)
        $vh = [NativeWindow]::GetSystemMetrics(79)
        $ix = [Math]::Max($rect.Left, $vx)
        $iy = [Math]::Max($rect.Top, $vy)
        $ir = [Math]::Min($rect.Right, $vx + $vw)
        $ib = [Math]::Min($rect.Bottom, $vy + $vh)
        $iw = $ir - $ix
        $ih = $ib - $iy
        # Sampled content statistics of the bound capture: a blank/failed composite is
        # refused instead of being recorded as an image.
        $colors = @{}
        $nonBlack = 0
        for ($gx = 1; $gx -le 7; $gx++) {
            for ($gy = 1; $gy -le 7; $gy++) {
                $px = [int]([double]$width * $gx / 8.0)
                $py = [int]([double]$height * $gy / 8.0)
                if ($px -ge $width) { $px = $width - 1 }
                if ($py -ge $height) { $py = $height - 1 }
                $pixel = $bitmap.GetPixel($px, $py)
                $colors["$($pixel.R),$($pixel.G),$($pixel.B)"] = 1
                if ($pixel.R + $pixel.G + $pixel.B -gt 12) { $nonBlack++ }
            }
        }
        $samples = 0
        $matched = 0
        # Geometry is kept in the probe's DIP frame and only the screen sample is
        # mapped through DpiScale, so the DIP-sized PrintWindow bitmap and the physical
        # display are compared in one frame. At scale 1.0 this reduces to the legacy
        # one-pixel-per-DIP comparison.
        $physicalWidth = [int][Math]::Round($iw * $DpiScale)
        $physicalHeight = [int][Math]::Round($ih * $DpiScale)
        if ($sameRect -and $iw -gt 0 -and $ih -gt 0 -and $physicalWidth -gt 0 -and $physicalHeight -gt 0) {
            $screen = New-Object System.Drawing.Bitmap $physicalWidth, $physicalHeight
            $screenGraphics = [System.Drawing.Graphics]::FromImage($screen)
            $physicalX = [int][Math]::Round($ix * $DpiScale)
            $physicalY = [int][Math]::Round($iy * $DpiScale)
            $screenGraphics.CopyFromScreen($physicalX, $physicalY, 0, 0, $screen.Size)
            $screenGraphics.Dispose()
            for ($gx = 1; $gx -le 7; $gx++) {
                for ($gy = 1; $gy -le 7; $gy++) {
                    $px = [int]([double]$physicalWidth * $gx / 8.0)
                    $py = [int]([double]$physicalHeight * $gy / 8.0)
                    if ($px -ge $physicalWidth) { $px = $physicalWidth - 1 }
                    if ($py -ge $physicalHeight) { $py = $physicalHeight - 1 }
                    $bx = $ix - $rect.Left + [int][Math]::Round($px / $DpiScale)
                    $by = $iy - $rect.Top + [int][Math]::Round($py / $DpiScale)
                    if ($bx -ge $width) { $bx = $width - 1 }
                    if ($by -ge $height) { $by = $height - 1 }
                    $capturedPixel = $bitmap.GetPixel($bx, $by)
                    $screenPixel = $screen.GetPixel($px, $py)
                    $samples++
                    if ([Math]::Abs([int]$capturedPixel.R - [int]$screenPixel.R) -le 24 -and
                        [Math]::Abs([int]$capturedPixel.G - [int]$screenPixel.G) -le 24 -and
                        [Math]::Abs([int]$capturedPixel.B - [int]$screenPixel.B) -le 24) {
                        $matched++
                    }
                }
            }
            $screen.Dispose()
        }
        $content = @{
            status = "NOT_MEASURED"
            method = "bound capture versus display over the on-screen region"
            samples = $samples
            matched = $matched
            tolerance = 24
            distinct_sampled_colors = $colors.Count
            non_black_samples = $nonBlack
            geometry = @{
                dpi_scale = $DpiScale
                dip_rect = @{ left = $rect.Left; top = $rect.Top; width = $width; height = $height }
                physical_rect = @{
                    left = [int][Math]::Round($rect.Left * $DpiScale)
                    top = [int][Math]::Round($rect.Top * $DpiScale)
                    width = [int][Math]::Round($width * $DpiScale)
                    height = [int][Math]::Round($height * $DpiScale)
                }
                physical_visible = @{ left = $physicalX; top = $physicalY; width = $physicalWidth; height = $physicalHeight }
            }
        }
        if (-not $sameRect) {
            $bitmap.Dispose()
            $screenshotRecord = @{ status = "NOT_MEASURED"; reason = "WINDOW_CHANGED_DURING_CAPTURE" }
        } elseif ($iw -le 0 -or $ih -le 0) {
            $bitmap.Dispose()
            $screenshotRecord = @{ status = "NOT_MEASURED"; reason = "WINDOW_OFFSCREEN"; content = $content }
        } elseif ($colors.Count -lt 4 -or $nonBlack -lt 45) {
            $bitmap.Dispose()
            $screenshotRecord = @{ status = "NOT_MEASURED"; reason = "WINDOW_CAPTURE_EMPTY"; content = $content }
        } elseif ($matched -lt 45) {
            # The display at the target rectangle is not the target: it is occluded.
            # Nothing was changed to make it visible; this is a closed refusal.
            $bitmap.Dispose()
            $screenshotRecord = @{ status = "NOT_MEASURED"; reason = "WINDOW_OCCLUDED"; content = $content }
        } else {
            $content.status = "OBSERVED"
            $content.visible_region = @{ left = $ix; top = $iy; width = $iw; height = $ih }
            $bitmap.Save($Screenshot, [System.Drawing.Imaging.ImageFormat]::Png)
            $bitmap.Dispose()
            $info = Get-Item -LiteralPath $Screenshot
            if ($info.Length -gt $MaxScreenshotBytes) {
                Remove-Item -LiteralPath $Screenshot -Force -ErrorAction SilentlyContinue
                $screenshotRecord = @{ status = "NOT_MEASURED"; reason = "SCREENSHOT_TOO_LARGE" }
            } else {
                $screenshotRecord = @{
                    status = "OBSERVED"
                    width = $width
                    height = $height
                    bytes = $info.Length
                    method = "PrintWindow(PW_RENDERFULLCONTENT)"
                    handle = $target.Handle.ToInt64()
                    pid = $target.Pid
                    title = $target.Title
                    rect = @{ left = $rect.Left; top = $rect.Top; width = $width; height = $height }
                    still_current = $true
                    foreground_at_capture = ([NativeWindow]::GetForegroundWindow() -eq $target.Handle)
                    content_cross_check = $content
                }
            }
        }
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
