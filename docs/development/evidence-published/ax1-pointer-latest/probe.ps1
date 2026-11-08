# AX1 palette focus probe v6 (ASCII-only script: Windows PowerShell 5.1 reads .ps1 as ANSI).
# Click the invoking rail search, open the palette by Ctrl+K, type, Escape, type into the
# restored focus, read the rail UIA value. Lock: directory C:\tmp\cos-desktop-native.lock
# with OWNER.txt inside. Isolated LOCALAPPDATA under the workspace target (C:/tmp and %TEMP%
# make the instance guard refuse with open-lock-file on this sandboxed host).
$ErrorActionPreference = 'Continue'
$lockDir = 'C:\tmp\cos-desktop-native.lock'
function Lock-Acquire {
    for ($i = 0; $i -lt 12; $i++) {
        if (Test-Path $lockDir) {
            $ownerFile = Join-Path $lockDir 'OWNER.txt'
            $ownerText = if (Test-Path $ownerFile) { Get-Content $ownerFile -Raw } else { '' }
            $alive = $false; $age = 999
            if ($ownerText -match 'pid=(\d+)') { try { $null = Get-Process -Id ([int]$Matches[1]) -ErrorAction Stop; $alive = $true } catch {} }
            if ($ownerText -match 'at=([0-9T:\-\.\+Z]+)') { try { $age = ((Get-Date).ToUniversalTime() - [datetime]::Parse($Matches[1]).ToUniversalTime()).TotalMinutes } catch {} }
            if (-not $alive -and $age -gt 5) { Remove-Item -Recurse -Force $lockDir -ErrorAction SilentlyContinue }
            else { Write-Output "LOCK_WAIT alive=$alive age_min=$([math]::Round($age,1))"; Start-Sleep -Seconds 15; continue }
        }
        New-Item -ItemType Directory -Path $lockDir -Force | Out-Null
        Set-Content -Path (Join-Path $lockDir 'OWNER.txt') -Value "owner=impl-flow2 pid=$PID at=$((Get-Date).ToUniversalTime().ToString('o'))"
        return $true
    }
    return $false
}
if (-not (Lock-Acquire)) { Write-Output 'LOCK_TIMEOUT'; exit 2 }
$base = 'C:\dev\cos-desktop\target\ax1-native-' + [guid]::NewGuid().ToString('N')
try {
    Add-Type -AssemblyName UIAutomationClient
    Add-Type -AssemblyName UIAutomationTypes
    Add-Type @"
using System;
using System.Runtime.InteropServices;
public static class W6 {
  [DllImport("user32.dll")] public static extern bool PostMessage(IntPtr hWnd, uint Msg, IntPtr wParam, IntPtr lParam);
  [DllImport("user32.dll")] public static extern void keybd_event(byte bVk, byte bScan, uint dwFlags, UIntPtr dwExtraInfo);
  [DllImport("user32.dll")] public static extern bool ScreenToClient(IntPtr hWnd, ref POINT p);
  [StructLayout(LayoutKind.Sequential)] public struct POINT { public int X; public int Y; }
}
"@
    $exe = 'C:\dev\cos-desktop\target\release\cos-desktop.exe'
    New-Item -ItemType Directory -Path (Join-Path $base 'cos-desktop') -Force | Out-Null
    $env:LOCALAPPDATA = $base
    $err = Join-Path $base 'err.txt'
    $p = Start-Process -FilePath $exe -ArgumentList @('--fixture','--fixture-file','C:\dev\cos-desktop\crates\cos-client\fixtures\task-184.json') -PassThru -NoNewWindow -RedirectStandardError $err
    Start-Sleep -Seconds 8
    $root = [System.Windows.Automation.AutomationElement]::RootElement
    $cond = New-Object System.Windows.Automation.PropertyCondition([System.Windows.Automation.AutomationElement]::ProcessIdProperty, $p.Id)
    $window = $root.FindFirst([System.Windows.Automation.TreeScope]::Children, $cond)
    if (-not $window) { Write-Output 'NO_WINDOW'; if (-not $p.HasExited) { $p.Kill() }; exit 3 }
    $hwnd = [IntPtr]$window.Current.NativeWindowHandle
    function Snapshot {
        $all = $window.FindAll([System.Windows.Automation.TreeScope]::Descendants, [System.Windows.Automation.Condition]::TrueCondition)
        $set = @{}
        foreach ($el in $all) { try { $set[$el.Current.AutomationId + '|' + $el.Current.Name] = $el } catch {} }
        return $set
    }
    function Value-Of($el) {
        if (-not $el) { return '<missing>' }
        try { return $el.GetCurrentPattern([System.Windows.Automation.ValuePattern]::Pattern).Current.Value } catch { return '<no-value-pattern>' }
    }
    function Find-Id([string]$id) {
        $c = New-Object System.Windows.Automation.PropertyCondition([System.Windows.Automation.AutomationElement]::AutomationIdProperty, $id)
        return $window.FindFirst([System.Windows.Automation.TreeScope]::Descendants, $c)
    }
    function Send-Char([int]$code) { [void][W6]::PostMessage($hwnd, 0x0102, [IntPtr]$code, [IntPtr]1); Start-Sleep -Milliseconds 150 }
    function Send-CtrlK {
        [W6]::keybd_event(0x11, 0, 0, [UIntPtr]::Zero); Start-Sleep -Milliseconds 120
        [void][W6]::PostMessage($hwnd, 0x0100, [IntPtr]0x4B, [IntPtr]0x001E0001)
        [void][W6]::PostMessage($hwnd, 0x0101, [IntPtr]0x4B, [IntPtr]0xC01E0001); Start-Sleep -Milliseconds 120
        [W6]::keybd_event(0x11, 0, 2, [UIntPtr]::Zero); Start-Sleep -Milliseconds 800
    }
    function Click-Element($el) {
        $r = $el.Current.BoundingRectangle
        $x = [int]($r.Left + $r.Width / 2); $y = [int]($r.Top + $r.Height / 2)
        $pt = New-Object W6+POINT; $pt.X = $x; $pt.Y = $y
        [void][W6]::ScreenToClient($hwnd, [ref]$pt)
        $lp = [IntPtr](($pt.Y -shl 16) -bor ($pt.X -band 0xFFFF))
        [void][W6]::PostMessage($hwnd, 0x0200, [IntPtr]0, $lp)
        Start-Sleep -Milliseconds 120
        [void][W6]::PostMessage($hwnd, 0x0201, [IntPtr]1, $lp)
        Start-Sleep -Milliseconds 120
        [void][W6]::PostMessage($hwnd, 0x0202, [IntPtr]0, $lp)
        Start-Sleep -Milliseconds 500
        return "screen=($x,$y) client=($($pt.X),$($pt.Y))"
    }

    $before = Snapshot
    $search = Find-Id 'task-search'
    Write-Output "TASK_SEARCH found=$([bool]$search) value_before='$(Value-Of $search)'"
    if ($search) { Write-Output "TASK_SEARCH click $(Click-Element $search)" }
    Send-Char ([int][char]'x')
    Write-Output "TASK_SEARCH after_click_x='$(Value-Of (Find-Id 'task-search'))'"
    Send-CtrlK
    $after = Snapshot
    Write-Output "PALETTE_OPEN delta=$($after.Count - $before.Count)"
    foreach ($k in $after.Keys) {
        if (-not $before.ContainsKey($k)) {
            $el = $after[$k]
            $patterns = @()
            foreach ($pair in @(@('Invoke', [System.Windows.Automation.InvokePattern]::Pattern), @('Value', [System.Windows.Automation.ValuePattern]::Pattern), @('Text', [System.Windows.Automation.TextPattern]::Pattern), @('SelectionItem', [System.Windows.Automation.SelectionItemPattern]::Pattern))) {
                try { [void]$el.GetCurrentPattern($pair[1]); $patterns += $pair[0] } catch {}
            }
            Write-Output ("NEW id='" + $el.Current.AutomationId + "' name='" + $el.Current.Name + "' type=" + $el.Current.ControlType.ProgrammaticName + " patterns=[" + ($patterns -join ',') + "] value='" + (Value-Of $el) + "'")
        }
    }
    $edit = $null
    foreach ($k in $after.Keys) {
        if (-not $before.ContainsKey($k)) {
            $el = $after[$k]
            $ct = $el.Current.ControlType.ProgrammaticName
            if ($ct -match 'Edit' -or $ct -match 'Document') { $edit = $el; break }
        }
    }
    Write-Output "PALETTE_INPUT found=$([bool]$edit) value_initial='$(Value-Of $edit)'"
    if ($edit) {
        foreach ($ch in 'abc'.ToCharArray()) { Send-Char ([int][char]$ch) }
        Write-Output "PALETTE_INPUT after_abc='$(Value-Of $edit)'"
        Write-Output "RAIL during_palette='$(Value-Of (Find-Id 'task-search'))'"
        [void][W6]::PostMessage($hwnd, 0x0100, [IntPtr]0x1B, [IntPtr]0x00010001)
        [void][W6]::PostMessage($hwnd, 0x0101, [IntPtr]0x1B, [IntPtr]0xC0010001)
        Start-Sleep -Milliseconds 900
        $afterEscape = Snapshot
        $paletteKeys = @($after.Keys | Where-Object { -not $before.ContainsKey($_) })
        $stillPresent = @($paletteKeys | Where-Object { $afterEscape.ContainsKey($_) })
        Write-Output "AFTER_ESCAPE palette_delta=$($afterEscape.Count - $before.Count) palette_keys_left=$($stillPresent.Count) rail_present=$([bool](Find-Id 'task-search'))"
        Send-Char ([int][char]'z')
        Start-Sleep -Milliseconds 600
        Write-Output "AFTER_ESCAPE rail_value='$(Value-Of (Find-Id 'task-search'))'"
        Send-CtrlK
        Write-Output "REOPEN palette_delta=$((Snapshot).Count - $afterEscape.Count)"
        [void][W6]::PostMessage($hwnd, 0x0100, [IntPtr]0x0D, [IntPtr]0x001C0001)
        [void][W6]::PostMessage($hwnd, 0x0101, [IntPtr]0x0D, [IntPtr]0xC01C0001)
        Start-Sleep -Milliseconds 1200
        $afterEnter = Snapshot
        Write-Output "AFTER_ENTER palette_delta=$($afterEnter.Count - $afterEscape.Count)"
        Write-Output "AFTER_ENTER descendant_count=$($afterEnter.Count)"
    }
    if (Test-Path $err) { Write-Output 'stderr:'; Get-Content $err | Select-Object -Last 8 }
    if (-not $p.HasExited) { $p.Kill() | Out-Null; try { $null = $p.WaitForExit(3000) } catch {} }
    Write-Output 'DONE'
} finally {
    Remove-Item -Recurse -Force $lockDir -ErrorAction SilentlyContinue
    Remove-Item -Recurse -Force $base -ErrorAction SilentlyContinue
}
