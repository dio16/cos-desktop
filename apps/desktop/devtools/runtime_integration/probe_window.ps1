# Live integration window probe (ASCII-only: Windows PowerShell 5.1 reads .ps1 as ANSI).
# Actions: dump (window + bounded UIA name/id list), shot (PrintWindow PNG; never
# CopyFromScreen and never foregrounds), close (post WM_CLOSE), invoke (invoke one
# descendant BUTTON whose accessible name contains -NameMatch, through its UIA pattern),
# activate (same pattern search over any control type: Invoke, SelectionItem, Legacy),
# focus (move accessibility focus to one descendant by name), type (focus one descendant
# and post WM_CHAR code units to the top-level window; no mouse input and no foreground)
# and valueread (read the element's UIA Value). The caller supplies -ProcessId and -Out;
# the JSON result is written to -Out.
param(
    [Parameter(Mandatory = $true)][int]$ProcessId,
    [Parameter(Mandatory = $true)][ValidateSet('dump', 'shot', 'close', 'invoke', 'activate', 'focus', 'type', 'valueread', 'click', 'key', 'hwheel')][string]$Action,
    [Parameter(Mandatory = $true)][string]$Out,
    [string]$ImageOut = '',
    [string]$NameMatch = '',
    [string]$Text = ''
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
    [StructLayout(LayoutKind.Sequential)] public struct POINT { public int X; public int Y; }
    [DllImport("user32.dll")] public static extern bool ScreenToClient(IntPtr hWnd, ref POINT point);
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

# Exact accessible-name match first, then the first contains match. A stable
# helper keeps every action on the same resolution rule.
function Find-Descendant([System.Windows.Automation.AutomationElement]$root, [string]$match) {
    $all = $root.FindAll([System.Windows.Automation.TreeScope]::Descendants, [System.Windows.Automation.Condition]::TrueCondition)
    $partial = $null
    for ($i = 0; $i -lt $all.Count; $i++) {
        $item = $all.Item($i)
        try { $name = $item.Current.Name } catch { continue }
        if (-not $name) { continue }
        if ($name -eq $match) { return $item }
        if ($null -eq $partial -and $name.Contains($match)) { $partial = $item }
    }
    return $partial
}

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
                $partial = $null
                for ($i = 0; $i -lt $buttons.Count; $i++) {
                    $item = $buttons.Item($i)
                    try { $name = $item.Current.Name } catch { continue }
                    if (-not $name) { continue }
                    if ($name -eq $NameMatch) { $matched = $item; break }
                    if ($null -eq $partial -and $name.Contains($NameMatch)) { $partial = $item }
                }
                if ($null -eq $matched) { $matched = $partial }
                $matchedName = ''
                if ($null -ne $matched) {
                    try { $matchedName = $matched.Current.Name } catch { $matchedName = '' }
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
                            $result['legacy_note'] = 'LegacyIAccessiblePattern is not used by this probe build'
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
    } elseif ($Action -eq 'activate' -or $Action -eq 'focus' -or $Action -eq 'type' -or $Action -eq 'valueread') {
        $result['name_match'] = $NameMatch
        $result['matched'] = $false
        if (-not $NameMatch) {
            $result['error'] = 'missing_name_match'
        } else {
            try {
                $element = [System.Windows.Automation.AutomationElement]::FromHandle($hwnd)
                $matched = Find-Descendant $element $NameMatch
                $matchedName = ''
                if ($null -ne $matched) {
                    try { $matchedName = $matched.Current.Name } catch { $matchedName = '' }
                }
                if ($null -eq $matched) {
                    $result['error'] = 'match_not_found'
                    $result['element_count'] = $element.FindAll([System.Windows.Automation.TreeScope]::Descendants, [System.Windows.Automation.Condition]::TrueCondition).Count
                } else {
                    $result['matched'] = $true
                    $result['matched_name'] = $matchedName
                    try { $result['matched_id'] = $matched.Current.AutomationId } catch {}
                    try { $result['matched_control_type'] = $matched.Current.ControlType.ProgrammaticName } catch {}
                    if ($Action -eq 'valueread') {
                        $valuePattern = $null
                        if ($matched.TryGetCurrentPattern([System.Windows.Automation.ValuePattern]::Pattern, [ref]$valuePattern)) {
                            try { $result['value'] = ([System.Windows.Automation.ValuePattern]$valuePattern).Current.Value } catch {
                                $result['error'] = 'value_read_failed: ' + $_.Exception.Message
                            }
                        } else {
                            $result['error'] = 'value_pattern_unavailable'
                        }
                    } elseif ($Action -eq 'focus' -or $Action -eq 'type') {
                        try {
                            $matched.SetFocus()
                            $result['set_focus_called'] = $true
                        } catch {
                            $result['set_focus_called'] = $false
                            $result['focus_error'] = $_.Exception.Message
                        }
                        Start-Sleep -Milliseconds 250
                        try {
                            $focused = [System.Windows.Automation.AutomationElement]::FocusedElement
                            if ($null -ne $focused) {
                                $result['focused_name'] = $focused.Current.Name
                                try { $result['focused_control_type'] = $focused.Current.ControlType.ProgrammaticName } catch {}
                            }
                        } catch {}
                        $result['focused'] = ($result['focused_name'] -eq $matchedName)
                        if ($Action -eq 'type') {
                            $result['text_length'] = $Text.Length
                            $result['code_units_sent'] = 0
                            if (-not $Text) {
                                $result['error'] = 'missing_text'
                            } else {
                                $sent = 0
                                foreach ($character in $Text.ToCharArray()) {
                                    $code = [int][char]$character
                                    if ([WProbe]::PostMessage($hwnd, 0x0102, [IntPtr]$code, [IntPtr]1)) {
                                        $sent++
                                    }
                                    Start-Sleep -Milliseconds 15
                                }
                                $result['code_units_sent'] = $sent
                                $result['posted_all'] = ($sent -eq $Text.Length)
                                Start-Sleep -Milliseconds 250
                            }
                        }
                    } else {
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
                                $result['legacy_note'] = 'LegacyIAccessiblePattern is not used by this probe build'
                            }
                        }
                        $result['patterns_tried'] = $tried.ToArray()
                        $result['invoked'] = $invoked
                        if (-not $invoked) { $result['error'] = 'no_action_pattern' }
                    }
                    if ($Action -eq 'focus' -or $Action -eq 'type') {
                        $valuePattern = $null
                        try {
                            if ($matched.TryGetCurrentPattern([System.Windows.Automation.ValuePattern]::Pattern, [ref]$valuePattern)) {
                                $result['value_after'] = ([System.Windows.Automation.ValuePattern]$valuePattern).Current.Value
                            }
                        } catch {}
                    }
                }
            } catch {
                $result['error'] = 'uia_failed: ' + $_.Exception.Message
            }
        }
    } elseif ($Action -eq 'click') {
        $result['name_match'] = $NameMatch
        $result['clicked'] = $false
        if (-not $NameMatch) {
            $result['error'] = 'missing_name_match'
        } else {
            try {
                $element = [System.Windows.Automation.AutomationElement]::FromHandle($hwnd)
                $matched = Find-Descendant $element $NameMatch
                if ($null -eq $matched) {
                    $result['error'] = 'match_not_found'
                } else {
                    $result['matched_name'] = $matched.Current.Name
                    try { $result['matched_id'] = $matched.Current.AutomationId } catch {}
                    try { $result['matched_control_type'] = $matched.Current.ControlType.ProgrammaticName } catch {}
                    $rect = $matched.Current.BoundingRectangle
                    $result['screen_bounds'] = @([int]$rect.Left, [int]$rect.Top, [int]$rect.Width, [int]$rect.Height)
                    $point = New-Object WProbe+POINT
                    $point.X = [int]($rect.Left + ($rect.Width / 2))
                    $point.Y = [int]($rect.Top + ($rect.Height / 2))
                    if ([WProbe]::ScreenToClient($hwnd, [ref]$point)) {
                        $result['client_x'] = $point.X
                        $result['client_y'] = $point.Y
                        $lparam = ((($point.Y -band 0xFFFF) -shl 16) -bor ($point.X -band 0xFFFF))
                        [void][WProbe]::PostMessage($hwnd, 0x0200, [IntPtr]::Zero, [IntPtr]$lparam)
                        Start-Sleep -Milliseconds 40
                        $down = [WProbe]::PostMessage($hwnd, 0x0201, [IntPtr]1, [IntPtr]$lparam)
                        Start-Sleep -Milliseconds 60
                        $up = [WProbe]::PostMessage($hwnd, 0x0202, [IntPtr]::Zero, [IntPtr]$lparam)
                        $result['mouse_down_posted'] = [bool]$down
                        $result['mouse_up_posted'] = [bool]$up
                        $result['clicked'] = ([bool]$down -and [bool]$up)
                    } else {
                        $result['error'] = 'screen_to_client_failed'
                    }
                }
            } catch {
                $result['error'] = 'uia_failed: ' + $_.Exception.Message
            }
        }
    } elseif ($Action -eq 'key') {
        $result['key'] = $Text
        $map = @{ 'end' = 0x23; 'home' = 0x24; 'left' = 0x25; 'up' = 0x26;
                  'right' = 0x27; 'down' = 0x28; 'enter' = 0x0D; 'space' = 0x20;
                  'escape' = 0x1B; 'tab' = 0x09; 'pagedown' = 0x22; 'pageup' = 0x21 }
        $vk = $map[$Text.ToLower()]
        if ($null -eq $vk) {
            $result['error'] = 'unsupported_key'
        } else {
            $down = [WProbe]::PostMessage($hwnd, 0x0100, [IntPtr]$vk, [IntPtr]1)
            Start-Sleep -Milliseconds 80
            $up = [WProbe]::PostMessage($hwnd, 0x0101, [IntPtr]$vk, [IntPtr]0xC0000001)
            $result['key_down_posted'] = [bool]$down
            $result['key_up_posted'] = [bool]$up
            $result['posted'] = ([bool]$down -and [bool]$up)
            Start-Sleep -Milliseconds 200
        }
    } elseif ($Action -eq 'hwheel') {
        # Horizontal wheel at the anchor element's centre (screen coords in
        # WM_MOUSEHWHEEL). Text is the signed wheel delta; -120 scrolls right
        # in GPUI's handler (it negates the high word).
        $result['name_match'] = $NameMatch
        $result['wheel_delta'] = $Text
        $result['posted'] = $false
        if (-not $NameMatch -or -not $Text) {
            $result['error'] = 'missing_name_match_or_delta'
        } else {
            try {
                $element = [System.Windows.Automation.AutomationElement]::FromHandle($hwnd)
                $matched = Find-Descendant $element $NameMatch
                if ($null -eq $matched) {
                    $result['error'] = 'match_not_found'
                } else {
                    $rect = $matched.Current.BoundingRectangle
                    $x = [int]($rect.Left + ($rect.Width / 2))
                    $y = [int]($rect.Top + ($rect.Height / 2))
                    $delta = [int]$Text
                    $hi = $delta -band 0xFFFF
                    $wp = [int64]$hi * 65536
                    $lparam = (((($y -band 0xFFFF) -shl 16) -bor ($x -band 0xFFFF)))
                    $posted = [WProbe]::PostMessage($hwnd, 0x020E, [IntPtr]$wp, [IntPtr]$lparam)
                    $result['posted'] = [bool]$posted
                    $result['screen_x'] = $x
                    $result['screen_y'] = $y
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
