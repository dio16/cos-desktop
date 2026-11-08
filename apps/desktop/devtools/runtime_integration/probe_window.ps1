# Live integration window probe (ASCII-only: Windows PowerShell 5.1 reads .ps1 as ANSI).
# Actions: dump (window + bounded UIA name/id list), shot (PrintWindow PNG; never
# CopyFromScreen and never foregrounds), close (post WM_CLOSE), invoke (invoke one
# descendant whose accessible name contains -NameMatch, through its UIA pattern; no
# mouse input and no foregrounding).
# The caller supplies -ProcessId and -Out; the JSON result is written to -Out.
param(
    [Parameter(Mandatory = $true)][int]$ProcessId,
    [Parameter(Mandatory = $true)][ValidateSet('dump', 'shot', 'close', 'invoke')][string]$Action,
    [Parameter(Mandatory = $true)][string]$Out,
    [string]$ImageOut = '',
    [string]$NameMatch = ''
)
$ErrorActionPreference = 'Stop'
Add-Type -AssemblyName UIAutomationClient
Add-Type -AssemblyName UIAutomationTypes
Add-Type -AssemblyName System.Drawing
Add-Type @"
using System;
using System.Text;
using System.Runtime.InteropServices;
public static class WProbe {
    public delegate bool EnumWindowsProc(IntPtr hWnd, IntPtr lParam);
    [DllImport("user32.dll")] public static extern bool EnumWindows(EnumWindowsProc callback, IntPtr lParam);
    [DllImport("user32.dll")] public static extern uint GetWindowThreadProcessId(IntPtr hWnd, out uint processId);
    [DllImport("user32.dll")] public static extern bool IsWindowVisible(IntPtr hWnd);
    [DllImport("user32.dll", CharSet = CharSet.Unicode)] public static extern int GetWindowText(IntPtr hWnd, StringBuilder text, int count);
    [DllImport("user32.dll", CharSet = CharSet.Unicode)] public static extern int GetClassName(IntPtr hWnd, StringBuilder text, int count);
    [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr hWnd, out RECT rect);
    [DllImport("user32.dll")] public static extern bool PrintWindow(IntPtr hWnd, IntPtr hdc, uint flags);
    [DllImport("user32.dll")] public static extern bool PostMessage(IntPtr hWnd, uint msg, IntPtr wParam, IntPtr lParam);
    [DllImport("user32.dll")] public static extern bool RedrawWindow(IntPtr hWnd, IntPtr rect, IntPtr region, uint flags);
    [StructLayout(LayoutKind.Sequential)] public struct RECT { public int Left; public int Top; public int Right; public int Bottom; }
    public static IntPtr Found = IntPtr.Zero;
    public static uint TargetPid = 0;
    private static bool Callback(IntPtr hWnd, IntPtr lParam) {
        uint pid;
        GetWindowThreadProcessId(hWnd, out pid);
        if (pid == TargetPid && IsWindowVisible(hWnd)) { Found = hWnd; return false; }
        return true;
    }
    public static IntPtr FindByPid(uint pid) {
        Found = IntPtr.Zero; TargetPid = pid;
        EnumWindows(new EnumWindowsProc(Callback), IntPtr.Zero);
        return Found;
    }
    public static string TextOf(IntPtr hWnd) {
        StringBuilder sb = new StringBuilder(512);
        GetWindowText(hWnd, sb, sb.Capacity);
        return sb.ToString();
    }
    public static string ClassOf(IntPtr hWnd) {
        StringBuilder sb = new StringBuilder(256);
        GetClassName(hWnd, sb, sb.Capacity);
        return sb.ToString();
    }
}
"@
$result = [ordered]@{ action = $Action; found = $false }
$hwnd = [WProbe]::FindByPid([uint32]$ProcessId)
if ($hwnd -eq [IntPtr]::Zero) {
    $result['error'] = 'window_not_found'
} else {
    $result['found'] = $true
    $result['hwnd'] = [int64]$hwnd
    $result['title'] = [WProbe]::TextOf($hwnd)
    $result['class'] = [WProbe]::ClassOf($hwnd)
    $rect = New-Object WProbe+RECT
    if ([WProbe]::GetWindowRect($hwnd, [ref]$rect)) {
        $result['width'] = $rect.Right - $rect.Left
        $result['height'] = $rect.Bottom - $rect.Top
    }
    if ($Action -eq 'dump') {
        $names = New-Object System.Collections.Generic.List[string]
        $ids = New-Object System.Collections.Generic.List[string]
        try {
            $element = [System.Windows.Automation.AutomationElement]::FromHandle($hwnd)
            $all = $element.FindAll([System.Windows.Automation.TreeScope]::Descendants, [System.Windows.Automation.Condition]::TrueCondition)
            $result['elements'] = $all.Count
            for ($i = 0; $i -lt $all.Count; $i++) {
                $item = $all.Item($i)
                if ($names.Count -ge 400) { break }
                try {
                    $name = $item.Current.Name
                    if ($name) { $names.Add($name) }
                    $id = $item.Current.AutomationId
                    if ($id) { $ids.Add($id) }
                } catch {}
            }
        } catch {
            $result['error'] = 'uia_failed: ' + $_.Exception.Message
        }
        $result['names'] = $names.ToArray()
        $result['ids'] = $ids.ToArray()
        $result['name_count'] = $names.Count
    } elseif ($Action -eq 'shot') {
        $width = [int]$result['width']; $height = [int]$result['height']
        if ($width -le 0 -or $height -le 0) {
            $result['ok'] = $false
            $result['error'] = 'empty_window_rect'
        } elseif (-not $ImageOut) {
            $result['ok'] = $false
            $result['error'] = 'missing_image_out'
        } else {
            [void][WProbe]::RedrawWindow($hwnd, [IntPtr]::Zero, [IntPtr]::Zero, 0x0001 -bor 0x0080)
            Start-Sleep -Milliseconds 150
            $bitmap = New-Object System.Drawing.Bitmap($width, $height)
            $graphics = [System.Drawing.Graphics]::FromImage($bitmap)
            $hdc = $graphics.GetHdc()
            $printed = [WProbe]::PrintWindow($hwnd, $hdc, 2)
            $graphics.ReleaseHdc($hdc)
            $graphics.Dispose()
            $bitmap.Save($ImageOut, [System.Drawing.Imaging.ImageFormat]::Png)
            $bitmap.Dispose()
            $result['ok'] = [bool]$printed
            $result['printwindow'] = [bool]$printed
            $result['image'] = $ImageOut
        }
    } elseif ($Action -eq 'invoke') {
        $result['name_match'] = $NameMatch
        $result['invoked'] = $false
        if (-not $NameMatch) {
            $result['error'] = 'missing_name_match'
        } else {
            try {
                $element = [System.Windows.Automation.AutomationElement]::FromHandle($hwnd)
                $condition = New-Object System.Windows.Automation.PropertyCondition(
                    [System.Windows.Automation.AutomationElement]::ControlTypeProperty,
                    [System.Windows.Automation.ControlType]::Button)
                $buttons = $element.FindAll([System.Windows.Automation.TreeScope]::Descendants, $condition)
                $matched = $null
                $matchedName = ''
                for ($i = 0; $i -lt $buttons.Count; $i++) {
                    $item = $buttons.Item($i)
                    try {
                        $name = $item.Current.Name
                    } catch { continue }
                    if ($name -and $name.Contains($NameMatch)) { $matched = $item; $matchedName = $name; break }
                }
                if ($matched -eq $null) {
                    $result['error'] = 'match_not_found'
                    $result['button_count'] = $buttons.Count
                } else {
                    $result['matched_name'] = $matchedName
                    $result['matched_id'] = $matched.Current.AutomationId
                    $result['matched_control_type'] = $matched.Current.ControlType.ProgrammaticName
                    $tried = New-Object System.Collections.Generic.List[string]
                    $invoked = $false
                    $pattern = $null
                    $tried.Add('invoke')
                    if ($matched.TryGetCurrentPattern([System.Windows.Automation.InvokePattern]::Pattern, [ref]$pattern)) {
                        ([System.Windows.Automation.InvokePattern]$pattern).Invoke()
                        $invoked = $true
                        $result['pattern'] = 'invoke'
                    } else {
                        $tried.Add('selection_item')
                        $selection = $null
                        if ($matched.TryGetCurrentPattern([System.Windows.Automation.SelectionItemPattern]::Pattern, [ref]$selection)) {
                            ([System.Windows.Automation.SelectionItemPattern]$selection).Select()
                            $invoked = $true
                            $result['pattern'] = 'selection_item'
                        } else {
                            $tried.Add('legacy_default_action')
                            $legacy = $null
                            if ($matched.TryGetCurrentPattern([System.Windows.Automation.LegacyIAccessiblePattern]::Pattern, [ref]$legacy)) {
                                ([System.Windows.Automation.LegacyIAccessiblePattern]$legacy).DoDefaultAction()
                                $invoked = $true
                                $result['pattern'] = 'legacy_default_action'
                            }
                        }
                    }
                    $result['patterns_tried'] = $tried.ToArray()
                    $result['invoked'] = $invoked
                    if (-not $invoked) { $result['error'] = 'no_action_pattern' }
                }
            } catch {
                $result['error'] = 'uia_failed: ' + $_.Exception.Message
            }
        }
    } elseif ($Action -eq 'close') {
        $sent = [WProbe]::PostMessage($hwnd, 0x0010, [IntPtr]::Zero, [IntPtr]::Zero)
        $result['sent'] = [bool]$sent
    }
}
$json = $result | ConvertTo-Json -Depth 4 -Compress
$encoding = New-Object System.Text.UTF8Encoding($false)
[System.IO.File]::WriteAllText($Out, $json, $encoding)
