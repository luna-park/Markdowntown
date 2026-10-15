# Скриншот окна Markdowntown для визуальной проверки (и, при необходимости, ввод).
#   .\tools\screenshot.ps1 -Exe target\debug\markdowntown.exe -FileArg examples\showcase.md -Out shot.png
# Ввод (-ClickX/-Clicks/-Wheel/-TypeText) отправляется только если окно на переднем плане.

param(
    [Parameter(Mandatory)] [string] $Exe,
    [string] $FileArg = "",
    [Parameter(Mandatory)] [string] $Out,
    [int] $WaitMs = 5000,
    # Клик в координатах относительно левого верхнего угла окна (-1 — без клика).
    [int] $ClickX = -1,
    [int] $ClickY = -1,
    # Несколько кликов: "x1,y1;x2,y2" (относительно окна).
    [string] $Clicks = "",
    # Прокрутка колесом: "x,y,notches" (отрицательные — вниз).
    [string] $Wheel = "",
    # Текст для ввода после клика (формат SendKeys) и второй скриншот через $Out2DelayMs.
    [string] $TypeText = "",
    [string] $Out2 = "",
    [int] $Out2DelayMs = 1500
)
Add-Type -AssemblyName System.Drawing
Add-Type @"
using System;
using System.Runtime.InteropServices;
public static class W {
    [StructLayout(LayoutKind.Sequential)] public struct RECT { public int L, T, R, B; }
    [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr h, out RECT r);
    [DllImport("user32.dll")] public static extern bool SetForegroundWindow(IntPtr h);
    [DllImport("user32.dll")] public static extern bool SetProcessDPIAware();
    [DllImport("user32.dll")] public static extern bool IsWindowVisible(IntPtr h);
    [DllImport("user32.dll")] public static extern bool SetCursorPos(int x, int y);
    [DllImport("user32.dll")] public static extern IntPtr GetForegroundWindow();
    [DllImport("user32.dll")] public static extern void mouse_event(uint f, uint x, uint y, uint d, IntPtr e);
    [DllImport("user32.dll", EntryPoint = "mouse_event")] public static extern void mouse_wheel(uint f, uint x, uint y, int d, IntPtr e);
    [DllImport("user32.dll")] public static extern uint GetWindowThreadProcessId(IntPtr h, out uint pid);
    public delegate bool EnumProc(IntPtr h, IntPtr l);
    [DllImport("user32.dll")] public static extern bool EnumWindows(EnumProc f, IntPtr l);
    public static IntPtr Largest(uint pid) {
        IntPtr best = IntPtr.Zero; long area = 0;
        EnumWindows((h, l) => {
            uint p; GetWindowThreadProcessId(h, out p);
            RECT r;
            if (p == pid && IsWindowVisible(h) && GetWindowRect(h, out r)) {
                long a = (long)(r.R - r.L) * (r.B - r.T);
                if (a > area) { area = a; best = h; }
            }
            return true;
        }, IntPtr.Zero);
        return best;
    }
}
"@
[W]::SetProcessDPIAware() | Out-Null
$p = if ($FileArg) { Start-Process $Exe -ArgumentList "`"$FileArg`"" -PassThru } else { Start-Process $Exe -PassThru }
# Ввод (мышь/клавиатура) отправляется только если наше окно на переднем плане,
# иначе он попадёт в чужое окно пользователя.
function Assert-Foreground([IntPtr] $h) {
    [W]::SetForegroundWindow($h) | Out-Null
    Start-Sleep -Milliseconds 100
    if ([W]::GetForegroundWindow() -ne $h) { throw "app window is not in the foreground; input aborted" }
}
try {
    $deadline = (Get-Date).AddSeconds(20)
    $hwnd = [IntPtr]::Zero
    while ((Get-Date) -lt $deadline) {
        $hwnd = [W]::Largest([uint32]$p.Id)
        $r0 = New-Object W+RECT
        if ($hwnd -ne [IntPtr]::Zero -and [W]::GetWindowRect($hwnd, [ref]$r0) -and ($r0.R - $r0.L) -gt 200) { break }
        Start-Sleep -Milliseconds 200
    }
    if ($hwnd -eq [IntPtr]::Zero) { throw "window not found" }
    [W]::SetForegroundWindow($hwnd) | Out-Null
    Start-Sleep -Milliseconds $WaitMs
    $r = New-Object W+RECT
    [W]::GetWindowRect($hwnd, [ref]$r) | Out-Null
    if ($ClickX -ge 0) {
        Assert-Foreground $hwnd
        [W]::SetCursorPos($r.L + $ClickX, $r.T + $ClickY) | Out-Null
        Start-Sleep -Milliseconds 300
        [W]::mouse_event(0x2, 0, 0, 0, [IntPtr]::Zero)
        Start-Sleep -Milliseconds 50
        [W]::mouse_event(0x4, 0, 0, 0, [IntPtr]::Zero)
        Start-Sleep -Milliseconds 800
    }
    foreach ($c in ($Clicks -split ';' | Where-Object { $_ })) {
        $xy = $c -split ','
        Assert-Foreground $hwnd
        [W]::SetCursorPos($r.L + [int]$xy[0], $r.T + [int]$xy[1]) | Out-Null
        Start-Sleep -Milliseconds 250
        [W]::mouse_event(0x2, 0, 0, 0, [IntPtr]::Zero)
        Start-Sleep -Milliseconds 50
        [W]::mouse_event(0x4, 0, 0, 0, [IntPtr]::Zero)
        Start-Sleep -Milliseconds 700
    }
    if ($Wheel) {
        $w = $Wheel -split ','
        Assert-Foreground $hwnd
        [W]::SetCursorPos($r.L + [int]$w[0], $r.T + [int]$w[1]) | Out-Null
        Start-Sleep -Milliseconds 200
        for ($k = 0; $k -lt [Math]::Abs([int]$w[2]); $k++) {
            $d = if ([int]$w[2] -lt 0) { -120 } else { 120 }
            [W]::mouse_wheel(0x0800, 0, 0, $d, [IntPtr]::Zero)
            Start-Sleep -Milliseconds 60
        }
        Start-Sleep -Milliseconds 1200
    }
    if ($TypeText) {
        Add-Type -AssemblyName System.Windows.Forms
        Assert-Foreground $hwnd
        [System.Windows.Forms.SendKeys]::SendWait($TypeText)
        Start-Sleep -Milliseconds 150
    }
    $w = $r.R - $r.L; $h = $r.B - $r.T
    if ([W]::GetForegroundWindow() -ne $hwnd) { Write-Warning "app window lost foreground; screenshot may show another window" }
    $bmp = New-Object System.Drawing.Bitmap $w, $h
    $g = [System.Drawing.Graphics]::FromImage($bmp)
    $g.CopyFromScreen($r.L, $r.T, 0, 0, $bmp.Size)
    $bmp.Save($Out, [System.Drawing.Imaging.ImageFormat]::Png)
    "saved $Out ($w x $h)"
    if ($Out2) {
        Start-Sleep -Milliseconds $Out2DelayMs
        $g.CopyFromScreen($r.L, $r.T, 0, 0, $bmp.Size)
        $bmp.Save($Out2, [System.Drawing.Imaging.ImageFormat]::Png)
        "saved $Out2"
    }
} finally {
    if (!$p.HasExited) { Stop-Process $p -Force }
}
