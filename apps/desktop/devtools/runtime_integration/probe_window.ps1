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
    [Parameter(Mandatory = $true)][ValidateSet('dump', 'tree', 'shot', 'close', 'invoke', 'activate', 'focus', 'type', 'valueread', 'click', 'key', 'hwheel', 'ime', 'dpi', 'foreground', 'raise', 'sendinput', 'sequence')][string]$Action,
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
    [DllImport("user32.dll")] public static extern int GetSystemMetrics(int index);
    [DllImport("user32.dll")] public static extern IntPtr MonitorFromWindow(IntPtr hWnd, uint flags);
    [DllImport("user32.dll")] public static extern uint GetDpiForWindow(IntPtr hWnd);
    [DllImport("user32.dll")] public static extern IntPtr GetWindowDpiAwarenessContext(IntPtr hWnd);
    [DllImport("user32.dll")] public static extern int GetAwarenessFromDpiAwarenessContext(IntPtr context);
    [DllImport("user32.dll")] public static extern IntPtr SetThreadDpiAwarenessContext(IntPtr context);
    [DllImport("shcore.dll")] public static extern int GetDpiForMonitor(IntPtr monitor, int type, out uint x, out uint y);
    [DllImport("imm32.dll")] public static extern IntPtr ImmGetContext(IntPtr hWnd);
    [DllImport("imm32.dll")] public static extern bool ImmReleaseContext(IntPtr hWnd, IntPtr himc);
    [DllImport("imm32.dll", CharSet = CharSet.Unicode)] public static extern int ImmGetCompositionStringW(IntPtr himc, uint index, System.Text.StringBuilder buffer, int length);
    [DllImport("imm32.dll", CharSet = CharSet.Unicode)] public static extern bool ImmSetCompositionStringW(IntPtr himc, uint index, string composition, int compositionLength, IntPtr reading, int readingLength);
    [DllImport("imm32.dll")] public static extern bool ImmAssociateContext(IntPtr hWnd, IntPtr himc);
    [DllImport("kernel32.dll")] public static extern uint GetLastError();
    [DllImport("user32.dll")] public static extern IntPtr GetForegroundWindow();
    [DllImport("user32.dll")] public static extern bool SetForegroundWindow(IntPtr hWnd);
    [DllImport("user32.dll")] public static extern IntPtr SetFocus(IntPtr hWnd);
    [DllImport("user32.dll")] public static extern bool BringWindowToTop(IntPtr hWnd);
    [DllImport("user32.dll")] public static extern bool AttachThreadInput(uint attach, uint attachTo, bool doAttach);
    [DllImport("kernel32.dll")] public static extern uint GetCurrentThreadId();
    [StructLayout(LayoutKind.Sequential)] public struct MOUSEINPUT {
        public int dx; public int dy; public uint mouseData; public uint dwFlags;
        public uint time; public IntPtr dwExtraInfo;
    }
    [StructLayout(LayoutKind.Sequential)] public struct HARDWAREINPUT {
        public uint uMsg; public ushort wParamL; public ushort wParamH;
    }
    [StructLayout(LayoutKind.Sequential)] public struct KEYBDINPUT {
        public ushort wVk; public ushort wScan; public uint dwFlags; public uint time; public IntPtr dwExtraInfo;
    }
    [StructLayout(LayoutKind.Explicit)] public struct INPUTUNION {
        [FieldOffset(0)] public MOUSEINPUT mi;
        [FieldOffset(0)] public KEYBDINPUT ki;
        [FieldOffset(0)] public HARDWAREINPUT hi;
    }
    [StructLayout(LayoutKind.Sequential)] public struct INPUT {
        public uint type; public INPUTUNION u;
    }
    public static int LastSendInputError = 0;
    [DllImport("user32.dll", SetLastError = true)] public static extern uint SendInput(uint count, INPUT[] inputs, int size);
    public static uint SendKey(ushort vk, bool up) {
        INPUT input = new INPUT();
        input.type = 1;
        input.u.ki.wVk = vk;
        input.u.ki.wScan = 0;
        input.u.ki.dwFlags = up ? (uint)2 : 0u;
        input.u.ki.time = 0;
        input.u.ki.dwExtraInfo = IntPtr.Zero;
        INPUT[] batch = new INPUT[] { input };
        uint sent = SendInput(1, batch, Marshal.SizeOf(typeof(INPUT)));
        LastSendInputError = Marshal.GetLastWin32Error();
        return sent;
    }
    public static int InputSize() { return Marshal.SizeOf(typeof(INPUT)); }
    // Raise/foreground without SetFocus: the activation path a real user takes
    // (the OS sends WM_SETFOCUS to the app), so an AX audit does not override
    // the application's own focused control with the top-level HWND.
    public static bool RaiseWindow(IntPtr hWnd) {
        uint pid;
        uint foregroundThread = GetWindowThreadProcessId(GetForegroundWindow(), out pid);
        uint currentThread = GetCurrentThreadId();
        bool attached = false;
        if (foregroundThread != 0 && foregroundThread != currentThread) {
            attached = AttachThreadInput(currentThread, foregroundThread, true);
        }
        bool raised = BringWindowToTop(hWnd);
        bool foreground = SetForegroundWindow(hWnd);
        if (attached) { AttachThreadInput(currentThread, foregroundThread, false); }
        return foreground && raised;
    }
    public static bool FocusWindow(IntPtr hWnd) {
        uint pid;
        uint foregroundThread = GetWindowThreadProcessId(GetForegroundWindow(), out pid);
        uint currentThread = GetCurrentThreadId();
        bool attached = false;
        if (foregroundThread != 0 && foregroundThread != currentThread) {
            attached = AttachThreadInput(currentThread, foregroundThread, true);
        }
        bool raised = BringWindowToTop(hWnd);
        bool foreground = SetForegroundWindow(hWnd);
        SetFocus(hWnd);
        if (attached) { AttachThreadInput(currentThread, foregroundThread, false); }
        return foreground && raised;
    }
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
                  'escape' = 0x1B; 'tab' = 0x09; 'pagedown' = 0x22; 'pageup' = 0x21;
                  'backspace' = 0x08; 'delete' = 0x2E;
                  'k' = 0x4B; 'a' = 0x41; '1' = 0x31; '2' = 0x32; '3' = 0x33 }
        $parts = $Text.ToLower().Split('+')
        $key_name = $parts[$parts.Length - 1]
        $vk = $map[$key_name]
        $modifiers = New-Object System.Collections.Generic.List[int]
        if ($parts.Length -gt 1) {
            for ($i = 0; $i -lt ($parts.Length - 1); $i++) {
                if ($parts[$i] -eq 'shift') { $modifiers.Add(0x10) }
                elseif ($parts[$i] -eq 'ctrl' -or $parts[$i] -eq 'control') { $modifiers.Add(0x11) }
                elseif ($parts[$i] -eq 'alt') { $modifiers.Add(0x12) }
            }
        }
        $result['modifiers'] = $modifiers.ToArray()
        if ($null -eq $vk) {
            $result['error'] = 'unsupported_key'
        } else {
            $posted = $true
            foreach ($modifier in $modifiers) {
                if (-not [WProbe]::PostMessage($hwnd, 0x0100, [IntPtr]$modifier, [IntPtr]1)) { $posted = $false }
                Start-Sleep -Milliseconds 30
            }
            $down = [WProbe]::PostMessage($hwnd, 0x0100, [IntPtr]$vk, [IntPtr]1)
            Start-Sleep -Milliseconds 80
            $up = [WProbe]::PostMessage($hwnd, 0x0101, [IntPtr]$vk, [IntPtr]0xC0000001)
            Start-Sleep -Milliseconds 40
            for ($i = $modifiers.Count - 1; $i -ge 0; $i--) {
                if (-not [WProbe]::PostMessage($hwnd, 0x0101, [IntPtr]$modifiers[$i], [IntPtr]0xC0000001)) { $posted = $false }
                Start-Sleep -Milliseconds 30
            }
            $result['key_down_posted'] = [bool]$down
            $result['key_up_posted'] = [bool]$up
            $result['modifiers_posted'] = $posted
            $result['posted'] = ([bool]$down -and [bool]$up)
            Start-Sleep -Milliseconds 200
        }
    } elseif ($Action -eq 'tree') {
        # Detailed accessibility rows: role, name, value, state and DIP bounds
        # for every descendant, bounded so one dump stays readable.
        $rows = New-Object System.Collections.Generic.List[object]
        try {
            $element = [System.Windows.Automation.AutomationElement]::FromHandle($hwnd)
            $all = $element.FindAll([System.Windows.Automation.TreeScope]::Descendants, [System.Windows.Automation.Condition]::TrueCondition)
            $result['elements'] = $all.Count
            for ($i = 0; $i -lt $all.Count; $i++) {
                if ($rows.Count -ge 400) { break }
                $item = $all.Item($i)
                $row = @{ index = $i }
                try { $row.name = $item.Current.Name } catch { $row.name = '' }
                try { $row.id = $item.Current.AutomationId } catch { $row.id = '' }
                try { $row.control_type = $item.Current.ControlType.ProgrammaticName } catch { $row.control_type = '' }
                try { $row.help = $item.Current.HelpText } catch { $row.help = '' }
                try { $row.enabled = $item.Current.IsEnabled } catch { $row.enabled = $null }
                try { $row.focusable = $item.Current.IsKeyboardFocusable } catch { $row.focusable = $null }
                try { $row.focused = $item.Current.HasKeyboardFocus } catch { $row.focused = $null }
                try { $row.offscreen = $item.Current.IsOffscreen } catch { $row.offscreen = $null }
                $value = ''
                $value_pattern = $null
                try {
                    if ($item.TryGetCurrentPattern([System.Windows.Automation.ValuePattern]::Pattern, [ref]$value_pattern)) {
                        $value = ([System.Windows.Automation.ValuePattern]$value_pattern).Current.Value
                    }
                } catch { $value = '' }
                $row.value = $value
                try {
                    $rect = $item.Current.BoundingRectangle
                    $row.bounds = @([int]$rect.Left, [int]$rect.Top, [int]$rect.Width, [int]$rect.Height)
                } catch { $row.bounds = $null }
                $row.patterns = @()
                try {
                    $found = @()
                    if ($value_pattern) { $found += 'value' }
                    $pattern = $null
                    if ($item.TryGetCurrentPattern([System.Windows.Automation.InvokePattern]::Pattern, [ref]$pattern)) { $found += 'invoke' }
                    $pattern = $null
                    if ($item.TryGetCurrentPattern([System.Windows.Automation.SelectionItemPattern]::Pattern, [ref]$pattern)) { $found += 'selection_item' }
                    $pattern = $null
                    if ($item.TryGetCurrentPattern([System.Windows.Automation.TextPattern]::Pattern, [ref]$pattern)) { $found += 'text' }
                    $pattern = $null
                    if ($item.TryGetCurrentPattern([System.Windows.Automation.ScrollPattern]::Pattern, [ref]$pattern)) { $found += 'scroll' }
                    $row.patterns = $found
                } catch {}
                $rows.Add([pscustomobject]$row)
            }
        } catch {
            $result['error'] = 'uia_failed: ' + $_.Exception.Message
        }
        $result['rows'] = $rows.ToArray()
        $result['row_count'] = $rows.Count
    } elseif ($Action -eq 'ime') {
        # Attempt one synthetic IMM32 composition: take the window's input
        # context, set a composition string, read it back and post the messages
        # the app's IME handler consumes. A real IME session is what the app
        # actually reads, so every API answer and the value readback are recorded
        # instead of assuming a composition happened.
        $result['text'] = $Text
        $himc = [WProbe]::ImmGetContext($hwnd)
        $result['himc'] = [int64]$himc
        $result['imm_get_context_error'] = [int][WProbe]::GetLastError()
        if ($himc -eq [IntPtr]::Zero) {
            $result['error'] = 'imm_get_context_null'
        } else {
            $set = [WProbe]::ImmSetCompositionStringW($himc, 0x0001, $Text, ($Text.Length * 2), [IntPtr]::Zero, 0)
            $result['set_composition_string'] = [bool]$set
            $result['set_composition_error'] = [int][WProbe]::GetLastError()
            $buffer = New-Object System.Text.StringBuilder 512
            $read = [WProbe]::ImmGetCompositionStringW($himc, 0x0008, $buffer, 1024)
            $result['composition_string_bytes'] = $read
            if ($read -gt 0) {
                $result['composition_string'] = $buffer.ToString(0, [int]($read / 2))
            } else {
                $result['composition_string'] = ''
            }
            $result['ime_start_posted'] = [bool][WProbe]::PostMessage($hwnd, 0x010D, [IntPtr]::Zero, [IntPtr]::Zero)
            Start-Sleep -Milliseconds 100
            $result['ime_compstr_posted'] = [bool][WProbe]::PostMessage($hwnd, 0x010F, [IntPtr]::Zero, [IntPtr]0x0008)
            Start-Sleep -Milliseconds 100
            $result['ime_result_posted'] = [bool][WProbe]::PostMessage($hwnd, 0x010F, [IntPtr]::Zero, [IntPtr]0x0800)
            Start-Sleep -Milliseconds 100
            $result['ime_end_posted'] = [bool][WProbe]::PostMessage($hwnd, 0x010E, [IntPtr]::Zero, [IntPtr]::Zero)
            Start-Sleep -Milliseconds 250
            [void][WProbe]::ImmReleaseContext($hwnd, $himc)
        }
        try {
            $focused = [System.Windows.Automation.AutomationElement]::FocusedElement
            if ($null -ne $focused) {
                $result['focused_name'] = $focused.Current.Name
                $value_pattern = $null
                if ($focused.TryGetCurrentPattern([System.Windows.Automation.ValuePattern]::Pattern, [ref]$value_pattern)) {
                    $result['focused_value_after'] = ([System.Windows.Automation.ValuePattern]$value_pattern).Current.Value
                }
            }
        } catch {}
    } elseif ($Action -eq 'dpi') {
        # Report the real DPI facts the window exposes and the probe's own
        # awareness switch. A synthetic WM_DPICHANGED is never posted: its
        # suggested rectangle is an lParam pointer that cannot be marshalled
        # into another process, so posting it would fault the target app.
        $result['monitor_count'] = [WProbe]::GetSystemMetrics(80)
        $monitor = [WProbe]::MonitorFromWindow($hwnd, 2)
        $result['monitor'] = [int64]$monitor
        $dpi_x = 0
        $dpi_y = 0
        $result['get_dpi_for_monitor_hr'] = [WProbe]::GetDpiForMonitor($monitor, 0, [ref]$dpi_x, [ref]$dpi_y)
        $result['monitor_dpi_x'] = $dpi_x
        $result['monitor_dpi_y'] = $dpi_y
        $result['window_dpi'] = [WProbe]::GetDpiForWindow($hwnd)
        $awareness_context = [WProbe]::GetWindowDpiAwarenessContext($hwnd)
        $result['awareness_context'] = [int64]$awareness_context
        $result['awareness'] = if ($awareness_context -ne [IntPtr]::Zero) {
            [WProbe]::GetAwarenessFromDpiAwarenessContext($awareness_context)
        } else {
            $null
        }
        $previous = [WProbe]::SetThreadDpiAwarenessContext([IntPtr](-4))
        $result['probe_thread_awareness_previous'] = [int64]$previous
        $rect = New-Object WProbe+RECT
        [void][WProbe]::GetWindowRect($hwnd, [ref]$rect)
        $result['window_rect_physical'] = @($rect.Left, $rect.Top, ($rect.Right - $rect.Left), ($rect.Bottom - $rect.Top))
    } elseif ($Action -eq 'raise') {
        # Activation without SetFocus: the path a person's click produces. The
        # application keeps its own focused control instead of having the
        # top-level HWND forced on it by the measuring process.
        $result['foreground_before'] = [int64][WProbe]::GetForegroundWindow()
        $result['raise'] = [bool][WProbe]::RaiseWindow($hwnd)
        Start-Sleep -Milliseconds 400
        $result['foreground_after'] = [int64][WProbe]::GetForegroundWindow()
        $result['target'] = [int64]$hwnd
        $result['is_foreground'] = ([int64][WProbe]::GetForegroundWindow() -eq [int64]$hwnd)
    } elseif ($Action -eq 'foreground') {
        # Raise and focus the window so SendInput can deliver real keystrokes.
        # This is a measurement step (posted messages cannot carry a trustworthy
        # modifier transition); callers record that the window was foregrounded.
        $result['foreground_before'] = [int64][WProbe]::GetForegroundWindow()
        $result['raise_and_focus'] = [bool][WProbe]::FocusWindow($hwnd)
        Start-Sleep -Milliseconds 400
        $result['foreground_after'] = [int64][WProbe]::GetForegroundWindow()
        $result['target'] = [int64]$hwnd
        $result['is_foreground'] = ([int64][WProbe]::GetForegroundWindow() -eq [int64]$hwnd)
    } elseif ($Action -eq 'sendinput') {
        # SendInput keystrokes update the async key state, so modifier+key pairs
        # arrive the way a person types them. Requires the caller to have
        # foregrounded the window in a previous action.
        $map = @{ 'tab' = 0x09; 'escape' = 0x1B; 'enter' = 0x0D; 'space' = 0x20;
                  'left' = 0x25; 'up' = 0x26; 'right' = 0x27; 'down' = 0x28;
                  'pagedown' = 0x22; 'pageup' = 0x21; 'k' = 0x4B }
        $parts = $Text.ToLower().Split('+')
        $key_name = $parts[$parts.Length - 1]
        $vk = $map[$key_name]
        $modifiers = New-Object System.Collections.Generic.List[int]
        if ($parts.Length -gt 1) {
            for ($i = 0; $i -lt ($parts.Length - 1); $i++) {
                if ($parts[$i] -eq 'shift') { $modifiers.Add(0x10) }
                elseif ($parts[$i] -eq 'ctrl' -or $parts[$i] -eq 'control') { $modifiers.Add(0x11) }
                elseif ($parts[$i] -eq 'alt') { $modifiers.Add(0x12) }
            }
        }
        $result['key'] = $Text
        $result['modifiers'] = $modifiers.ToArray()
        if ($null -eq $vk) {
            $result['error'] = 'unsupported_key'
        } else {
            $result['is_foreground'] = ([int64][WProbe]::GetForegroundWindow() -eq [int64]$hwnd)
            $result['input_size'] = [int][WProbe]::InputSize()
            $sent = 0
            foreach ($modifier in $modifiers) {
                $sent += [int][WProbe]::SendKey([uint16]$modifier, $false)
                Start-Sleep -Milliseconds 40
            }
            $sent += [int][WProbe]::SendKey([uint16]$vk, $false)
            Start-Sleep -Milliseconds 60
            $sent += [int][WProbe]::SendKey([uint16]$vk, $true)
            Start-Sleep -Milliseconds 40
            for ($i = $modifiers.Count - 1; $i -ge 0; $i--) {
                $sent += [int][WProbe]::SendKey([uint16]$modifiers[$i], $true)
                Start-Sleep -Milliseconds 40
            }
            $result['inputs_sent'] = $sent
            $result['sendinput_error'] = [int][WProbe]::LastSendInputError
        }
        Start-Sleep -Milliseconds 250
    } elseif ($Action -eq 'sequence') {
        # One process performs a whole foregrounded keyboard sequence, so the
        # window stays active between steps (a fresh probe process per key would
        # drop the activation and hide the real focus transitions). Steps are
        # separated by '|'; ``focus:<name>`` moves UIA focus, ``wait:<ms>``
        # pauses, ``names`` records the visible names and every other step is a
        # SendInput chord such as ``tab``, ``shift+tab``, ``enter``, ``ctrl+k``.
        $key_map = @{ 'tab' = 0x09; 'escape' = 0x1B; 'enter' = 0x0D; 'space' = 0x20;
                      'left' = 0x25; 'up' = 0x26; 'right' = 0x27; 'down' = 0x28;
                      'pagedown' = 0x22; 'pageup' = 0x21; 'home' = 0x24; 'end' = 0x23;
                      'k' = 0x4B; 'h' = 0x48; 'n' = 0x4E; 'd' = 0x44 }
        $log = New-Object System.Collections.Generic.List[object]
        foreach ($step in $Text.Split('|')) {
            $trimmed = $step.Trim()
            if (-not $trimmed) { continue }
            $entry = @{ step = $trimmed }
            # Wall-clock boundaries so an external UIA focus-event listener can
            # attribute each recorded event to the input step that caused it.
            $entry.start_utc = [DateTime]::UtcNow.ToString('yyyy-MM-ddTHH:mm:ss.fffZ')
            $focused = [System.Windows.Automation.AutomationElement]::FocusedElement
            if ($null -ne $focused) {
                try { $entry.focus_before = $focused.Current.Name } catch { $entry.focus_before = '' }
            }
            if ($trimmed -eq 'fg') {
                $entry.raise_and_focus = [bool][WProbe]::FocusWindow($hwnd)
                Start-Sleep -Milliseconds 400
                $entry.is_foreground = ([int64][WProbe]::GetForegroundWindow() -eq [int64]$hwnd)
            } elseif ($trimmed -eq 'fg2') {
                # Activation without SetFocus: keeps the application's own
                # focused control intact, which is what a person's click on the
                # window produces.
                $entry.raise = [bool][WProbe]::RaiseWindow($hwnd)
                Start-Sleep -Milliseconds 400
                $entry.is_foreground = ([int64][WProbe]::GetForegroundWindow() -eq [int64]$hwnd)
            } elseif ($trimmed.StartsWith('focus:')) {
                $needle = $trimmed.Substring(6)
                $root = [System.Windows.Automation.AutomationElement]::FromHandle($hwnd)
                $target = Find-Descendant $root $needle
                if ($null -eq $target) {
                    $entry.error = 'match_not_found'
                } else {
                    try { $target.SetFocus(); $entry.focused_by_name = $target.Current.Name } catch {
                        $entry.error = 'focus_failed: ' + $_.Exception.Message
                    }
                }
            } elseif ($trimmed.StartsWith('wait:')) {
                $ms = 0
                [void][int]::TryParse($trimmed.Substring(5), [ref]$ms)
                if ($ms -gt 0) { Start-Sleep -Milliseconds $ms }
            } elseif ($trimmed.StartsWith('ime:')) {
                # Synthetic IMM32 composition attempted while the window is
                # foreground and an editable control owns focus.
                $composition = $trimmed.Substring(4)
                $himc = [WProbe]::ImmGetContext($hwnd)
                $entry.himc = [int64]$himc
                $entry.imm_get_context_error = [int][WProbe]::GetLastError()
                if ($himc -eq [IntPtr]::Zero) {
                    $entry.error = 'imm_get_context_null'
                } else {
                    $entry.set_composition_string = [bool][WProbe]::ImmSetCompositionStringW(
                        $himc, 0x0001, $composition, ($composition.Length * 2), [IntPtr]::Zero, 0)
                    $entry.set_composition_error = [int][WProbe]::GetLastError()
                    $buffer = New-Object System.Text.StringBuilder 512
                    $read = [WProbe]::ImmGetCompositionStringW($himc, 0x0008, $buffer, 1024)
                    $entry.composition_string_bytes = $read
                    $entry.composition_string = if ($read -gt 0) { $buffer.ToString(0, [int]($read / 2)) } else { '' }
                    $entry.ime_start_posted = [bool][WProbe]::PostMessage($hwnd, 0x010D, [IntPtr]::Zero, [IntPtr]::Zero)
                    Start-Sleep -Milliseconds 100
                    $entry.ime_compstr_posted = [bool][WProbe]::PostMessage($hwnd, 0x010F, [IntPtr]::Zero, [IntPtr]0x0008)
                    Start-Sleep -Milliseconds 100
                    $entry.ime_result_posted = [bool][WProbe]::PostMessage($hwnd, 0x010F, [IntPtr]::Zero, [IntPtr]0x0800)
                    Start-Sleep -Milliseconds 100
                    $entry.ime_end_posted = [bool][WProbe]::PostMessage($hwnd, 0x010E, [IntPtr]::Zero, [IntPtr]::Zero)
                    Start-Sleep -Milliseconds 200
                    [void][WProbe]::ImmReleaseContext($hwnd, $himc)
                }
                $value_pattern = $null
                $focused_element = [System.Windows.Automation.AutomationElement]::FocusedElement
                if ($null -ne $focused_element -and $focused_element.TryGetCurrentPattern(
                        [System.Windows.Automation.ValuePattern]::Pattern, [ref]$value_pattern)) {
                    $entry.value_after = ([System.Windows.Automation.ValuePattern]$value_pattern).Current.Value
                }
            } elseif ($trimmed -eq 'names') {
                $root = [System.Windows.Automation.AutomationElement]::FromHandle($hwnd)
                $all = $root.FindAll([System.Windows.Automation.TreeScope]::Descendants, [System.Windows.Automation.Condition]::TrueCondition)
                $entry.element_count = $all.Count
                $collected = New-Object System.Collections.Generic.List[string]
                $focused_names = New-Object System.Collections.Generic.List[string]
                for ($i = 0; $i -lt $all.Count; $i++) {
                    if ($collected.Count -ge 400) { break }
                    $item = $all.Item($i)
                    try {
                        $name = $item.Current.Name
                        if ($name) { $collected.Add($name) }
                    } catch {}
                    try {
                        if ($item.Current.HasKeyboardFocus) {
                            $focused_names.Add($name)
                        }
                    } catch {}
                }
                $entry.names = $collected.ToArray()
                $entry.has_keyboard_focus = $focused_names.ToArray()
            } else {
                $parts = $trimmed.ToLower().Split('+')
                $vk = $key_map[$parts[$parts.Length - 1]]
                if ($null -eq $vk) {
                    $entry.error = 'unsupported_key'
                } else {
                    $modifiers = New-Object System.Collections.Generic.List[int]
                    if ($parts.Length -gt 1) {
                        for ($i = 0; $i -lt ($parts.Length - 1); $i++) {
                            if ($parts[$i] -eq 'shift') { $modifiers.Add(0x10) }
                            elseif ($parts[$i] -eq 'ctrl' -or $parts[$i] -eq 'control') { $modifiers.Add(0x11) }
                            elseif ($parts[$i] -eq 'alt') { $modifiers.Add(0x12) }
                        }
                    }
                    $sent = 0
                    foreach ($modifier in $modifiers) {
                        $sent += [int][WProbe]::SendKey([uint16]$modifier, $false)
                        Start-Sleep -Milliseconds 40
                    }
                    $sent += [int][WProbe]::SendKey([uint16]$vk, $false)
                    Start-Sleep -Milliseconds 60
                    $sent += [int][WProbe]::SendKey([uint16]$vk, $true)
                    Start-Sleep -Milliseconds 40
                    for ($i = $modifiers.Count - 1; $i -ge 0; $i--) {
                        $sent += [int][WProbe]::SendKey([uint16]$modifiers[$i], $true)
                        Start-Sleep -Milliseconds 40
                    }
                    $entry.inputs_sent = $sent
                    $entry.sendinput_error = [int][WProbe]::LastSendInputError
                }
            }
            Start-Sleep -Milliseconds 300
            $after = [System.Windows.Automation.AutomationElement]::FocusedElement
            if ($null -ne $after) {
                try { $entry.focus_after = $after.Current.Name } catch { $entry.focus_after = '' }
                try { $entry.focus_after_type = $after.Current.ControlType.ProgrammaticName } catch {}
                try { $rid = $after.GetRuntimeId(); if ($null -ne $rid) { $entry.focus_after_rid = ($rid -join '.') } } catch {}
            }
            $entry.end_utc = [DateTime]::UtcNow.ToString('yyyy-MM-ddTHH:mm:ss.fffZ')
            $log.Add([pscustomobject]$entry)
        }
        $result['steps'] = $log.ToArray()
        $result['step_count'] = $log.Count
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
