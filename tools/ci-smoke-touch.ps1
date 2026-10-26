# Touch on a real Windows desktop (used by CI): finger taps go where they
# should around the island.
#
# The island's window is far bigger than the closed island (room for the
# widest, tallest page). Its transparent rest must let taps through to the
# windows under it, as it lets the mouse through. Up to 0.2.6 the GPU
# renderer drew into that same window, and Windows then gave every tap
# anywhere in its rectangle to it, where nothing answered: a tablet's top
# edge, browser tabs included, went dead around the island.
#
# Taps are injected with InjectTouchInput. A plain window (the probe) fills
# the top of the screen under the island and records the touches it gets.
# Checked: taps beside, below and in the corners of the island's window
# reach the probe; a tap on the island opens it (and the probe gets
# nothing); a tap outside the open island reaches the probe and closes it.
param(
    [Parameter(Mandatory = $true)][string]$Exe,
    [string]$Out = "smoke-touch",
    [ValidateSet("classic", "pill")][string]$Theme = "classic"
)

$ErrorActionPreference = "Stop"
New-Item -ItemType Directory -Force -Path $Out | Out-Null

Add-Type -AssemblyName System.Windows.Forms, System.Drawing
Add-Type -TypeDefinition @"
using System;
using System.Collections.Generic;
using System.Runtime.InteropServices;
using System.Text;

public static class TouchNative {
    [StructLayout(LayoutKind.Sequential)] public struct POINT { public int X; public int Y; }
    [StructLayout(LayoutKind.Sequential)] public struct RECT { public int Left; public int Top; public int Right; public int Bottom; }
    [DllImport("user32.dll")] public static extern bool SetProcessDPIAware();
    [DllImport("user32.dll")] public static extern bool SetCursorPos(int x, int y);
    [DllImport("user32.dll")] static extern IntPtr WindowFromPoint(POINT p);
    [DllImport("user32.dll")] static extern IntPtr GetAncestor(IntPtr h, uint flags);
    [DllImport("user32.dll", CharSet = CharSet.Unicode)] static extern int GetClassName(IntPtr h, StringBuilder s, int n);
    [DllImport("user32.dll", CharSet = CharSet.Unicode)] static extern IntPtr FindWindow(string cls, string title);
    [DllImport("user32.dll")] static extern bool GetWindowRect(IntPtr h, out RECT r);

    // Window class of the top-level window the mouse would hit at (x, y).
    public static string ClassAt(int x, int y) {
        POINT p; p.X = x; p.Y = y;
        IntPtr h = WindowFromPoint(p);
        if (h == IntPtr.Zero) return "";
        var sb = new StringBuilder(256);
        GetClassName(GetAncestor(h, 2), sb, 256);
        return sb.ToString();
    }

    // The island's window, in screen pixels (empty if it isn't there).
    public static RECT IslandRect() {
        RECT r = new RECT();
        IntPtr h = FindWindow("Poni.Island", null);
        if (h != IntPtr.Zero) GetWindowRect(h, out r);
        return r;
    }
}

[StructLayout(LayoutKind.Sequential)]
public struct POINTER_INFO {
    public uint pointerType;
    public uint pointerId;
    public uint frameId;
    public uint pointerFlags;
    public IntPtr sourceDevice;
    public IntPtr hwndTarget;
    public TouchNative.POINT ptPixelLocation;
    public TouchNative.POINT ptHimetricLocation;
    public TouchNative.POINT ptPixelLocationRaw;
    public TouchNative.POINT ptHimetricLocationRaw;
    public uint dwTime;
    public uint historyCount;
    public int InputData;
    public uint dwKeyStates;
    public ulong PerformanceCount;
    public int ButtonChangeType;
}

[StructLayout(LayoutKind.Sequential)]
public struct POINTER_TOUCH_INFO {
    public POINTER_INFO pointerInfo;
    public uint touchFlags;
    public uint touchMask;
    public TouchNative.RECT rcContact;
    public TouchNative.RECT rcContactRaw;
    public uint orientation;
    public uint pressure;
}

public static class Finger {
    [DllImport("user32.dll", SetLastError = true)] static extern bool InitializeTouchInjection(uint maxCount, uint mode);
    [DllImport("user32.dll", SetLastError = true)] static extern bool InjectTouchInput(uint count, [In] POINTER_TOUCH_INFO[] contacts);
    const uint INRANGE = 0x2, INCONTACT = 0x4, DOWN = 0x10000, UPDATE = 0x20000, UP = 0x40000;

    // TOUCH_FEEDBACK_NONE: no touch circles in the screenshots.
    public static string Init() {
        return InitializeTouchInjection(10, 3) ? null : "InitializeTouchInjection failed (error " + Marshal.GetLastWin32Error() + ")";
    }

    static string Send(int x, int y, uint flags, string what) {
        var c = new POINTER_TOUCH_INFO();
        c.pointerInfo.pointerType = 2;  // PT_TOUCH
        c.pointerInfo.pointerFlags = flags;
        c.pointerInfo.ptPixelLocation.X = x;
        c.pointerInfo.ptPixelLocation.Y = y;
        c.touchMask = 0x7;  // contact area, orientation, pressure
        c.orientation = 90;
        c.pressure = 32000;
        // A fingertip, about 8 px across.
        c.rcContact.Left = x - 4; c.rcContact.Right = x + 4;
        c.rcContact.Top = y - 4; c.rcContact.Bottom = y + 4;
        if (InjectTouchInput(1, new[] { c })) return null;
        return what + " failed (error " + Marshal.GetLastWin32Error() + ")";
    }

    public static string Down(int x, int y) { return Send(x, y, DOWN | INRANGE | INCONTACT, "touch down"); }
    public static string Hold(int x, int y) { return Send(x, y, UPDATE | INRANGE | INCONTACT, "touch update"); }
    public static string Up(int x, int y) { return Send(x, y, UP, "touch up"); }
}

// The window under the island: records the touches it gets.
public static class Probe {
    delegate IntPtr WndProcFn(IntPtr h, uint msg, IntPtr wp, IntPtr lp);
    [StructLayout(LayoutKind.Sequential, CharSet = CharSet.Unicode)]
    struct WNDCLASSEX {
        public uint cbSize; public uint style; public IntPtr lpfnWndProc; public int cbClsExtra; public int cbWndExtra;
        public IntPtr hInstance; public IntPtr hIcon; public IntPtr hCursor; public IntPtr hbrBackground;
        public string lpszMenuName; public string lpszClassName; public IntPtr hIconSm;
    }
    [StructLayout(LayoutKind.Sequential)]
    struct MSG { public IntPtr hwnd; public uint message; public IntPtr wParam; public IntPtr lParam; public uint time; public TouchNative.POINT pt; public uint lPrivate; }
    [DllImport("user32.dll", CharSet = CharSet.Unicode, SetLastError = true)] static extern ushort RegisterClassEx(ref WNDCLASSEX wc);
    [DllImport("user32.dll", CharSet = CharSet.Unicode, SetLastError = true)]
    static extern IntPtr CreateWindowEx(uint ex, string cls, string title, uint style, int x, int y, int w, int h, IntPtr parent, IntPtr menu, IntPtr inst, IntPtr param);
    [DllImport("user32.dll", CharSet = CharSet.Unicode)] static extern IntPtr DefWindowProc(IntPtr h, uint msg, IntPtr wp, IntPtr lp);
    [DllImport("user32.dll", CharSet = CharSet.Unicode)] static extern bool PeekMessage(out MSG m, IntPtr h, uint min, uint max, uint remove);
    [DllImport("user32.dll")] static extern bool TranslateMessage(ref MSG m);
    [DllImport("user32.dll", CharSet = CharSet.Unicode)] static extern IntPtr DispatchMessage(ref MSG m);
    [DllImport("user32.dll")] static extern bool DestroyWindow(IntPtr h);
    [DllImport("user32.dll")] static extern bool SetForegroundWindow(IntPtr h);
    [DllImport("user32.dll")] static extern IntPtr LoadCursor(IntPtr inst, IntPtr name);
    [DllImport("gdi32.dll")] static extern IntPtr CreateSolidBrush(uint bgr);
    [DllImport("kernel32.dll", CharSet = CharSet.Unicode)] static extern IntPtr GetModuleHandle(string name);

    static readonly WndProcFn proc = Proc;  // kept alive with the window
    public static List<string> Got = new List<string>();
    static IntPtr hwnd;

    static IntPtr Proc(IntPtr h, uint msg, IntPtr wp, IntPtr lp) {
        // WM_POINTERDOWN, and the WM_LBUTTONDOWN Windows makes from it.
        if (msg == 0x0246 || msg == 0x0201) {
            long l = lp.ToInt64();
            Got.Add((msg == 0x0246 ? "pointerdown " : "lbuttondown ") + (short)(l & 0xFFFF) + "," + (short)((l >> 16) & 0xFFFF));
        }
        return DefWindowProc(h, msg, wp, lp);
    }

    public static string Open(int w, int h) {
        IntPtr inst = GetModuleHandle(null);
        var wc = new WNDCLASSEX();
        wc.cbSize = (uint)Marshal.SizeOf(typeof(WNDCLASSEX));
        wc.lpfnWndProc = Marshal.GetFunctionPointerForDelegate(proc);
        wc.hInstance = inst;
        wc.hCursor = LoadCursor(IntPtr.Zero, (IntPtr)32512);
        wc.hbrBackground = CreateSolidBrush(0x008C5430);
        wc.lpszClassName = "PoniTouchProbe";
        if (RegisterClassEx(ref wc) == 0) return "RegisterClassEx failed (error " + Marshal.GetLastWin32Error() + ")";
        // WS_POPUP | WS_VISIBLE: an ordinary window at the top left, under the island.
        hwnd = CreateWindowEx(0, "PoniTouchProbe", "Touch probe", 0x90000000, 0, 0, w, h, IntPtr.Zero, IntPtr.Zero, inst, IntPtr.Zero);
        if (hwnd == IntPtr.Zero) return "CreateWindowEx failed (error " + Marshal.GetLastWin32Error() + ")";
        SetForegroundWindow(hwnd);
        return null;
    }

    // Handle the probe's messages for `ms` milliseconds.
    public static void Pump(int ms) {
        DateTime end = DateTime.UtcNow.AddMilliseconds(ms);
        MSG m;
        while (DateTime.UtcNow < end) {
            while (PeekMessage(out m, IntPtr.Zero, 0, 0, 1)) { TranslateMessage(ref m); DispatchMessage(ref m); }
            System.Threading.Thread.Sleep(5);
        }
    }

    public static void Close() { if (hwnd != IntPtr.Zero) DestroyWindow(hwnd); }
}
"@
[TouchNative]::SetProcessDPIAware() | Out-Null

$metrics = Join-Path $Out "metrics.txt"
$failures = @()

function Expect([string]$name, [bool]$ok, [string]$detail) {
    $line = "check ${name}: $(if ($ok) { 'ok' } else { 'FAILED' }) ($detail)"
    $line | Tee-Object -FilePath $metrics -Append
    if (-not $ok) { $script:failures += $name }
}

function Shot([string]$name, [int]$height = 480) {
    $b = [System.Windows.Forms.Screen]::PrimaryScreen.Bounds
    $h = [Math]::Min($height, $b.Height)
    $bmp = New-Object System.Drawing.Bitmap $b.Width, $h
    $g = [System.Drawing.Graphics]::FromImage($bmp)
    $g.CopyFromScreen($b.Left, $b.Top, 0, 0, $bmp.Size)
    $bmp.Save((Join-Path $Out "$name.png"), [System.Drawing.Imaging.ImageFormat]::Png)
    $g.Dispose(); $bmp.Dispose()
}

$bounds = [System.Windows.Forms.Screen]::PrimaryScreen.Bounds
$g = [System.Drawing.Graphics]::FromHwnd([IntPtr]::Zero)
$scale = $g.DpiY / 96.0
$g.Dispose()
function Px([double]$dip) { [int][Math]::Round($dip * $scale) }
$cx = [int]($bounds.Width / 2)
$pill = $Theme -eq "pill"
"screen: $($bounds.Width)x$($bounds.Height), scale $scale, theme $Theme$(if ($env:PONI_RENDERER) { ", renderer $env:PONI_RENDERER" })" | Tee-Object -FilePath $metrics

# The mouse waits far below the island, so only the finger moves it.
$park = @($cx, [int]($bounds.Height * 0.92))
[TouchNative]::SetCursorPos($park[0], $park[1]) | Out-Null

$env:PONI_HOME = Join-Path (Resolve-Path ".").Path "$(Split-Path $Out -Leaf)-home"
New-Item -ItemType Directory -Force -Path $env:PONI_HOME | Out-Null
if ($pill) { '{"theme":"pill"}' | Set-Content -Path (Join-Path $env:PONI_HOME "settings.json") -Encoding ascii }
$p = Start-Process -FilePath $Exe -PassThru
Start-Sleep -Seconds 4

$err = [Probe]::Open($bounds.Width, [int]($bounds.Height * 0.8))
if (-not $err) { $err = [Finger]::Init() }
if ($err) {
    Stop-Process -Id $p.Id -Force -ErrorAction SilentlyContinue
    Write-Error "touch smoke can't run: $err"
    exit 1
}
[Probe]::Pump(1500)

$wr = [TouchNative]::IslandRect()
"island window: ($($wr.Left), $($wr.Top)) to ($($wr.Right), $($wr.Bottom))" | Tee-Object -FilePath $metrics -Append
Expect "island-window" (($wr.Right - $wr.Left) -gt (Px 100)) "$($wr.Right - $wr.Left)x$($wr.Bottom - $wr.Top) px"

# Open = the open card takes the mouse 200 DIPs right of centre, 100 down.
function IsOpen { [TouchNative]::ClassAt($cx + (Px 200), (Px 100)) -eq "Poni.Island" }

# One tap; returns what the probe got (empty: nothing reached it).
function Tap([int]$x, [int]$y) {
    [Probe]::Got.Clear()
    $e = [Finger]::Down($x, $y)
    if (-not $e) { [Probe]::Pump(30); $e = [Finger]::Hold($x, $y) }
    if (-not $e) { [Probe]::Pump(30); $e = [Finger]::Up($x, $y) }
    [Probe]::Pump(700)
    # (Not Tee-Object: its output would join the function's return value.)
    if ($e) { Add-Content -Path $metrics -Value "tap at ($x, $y): $e"; Write-Host "tap at ($x, $y): $e" }
    [Probe]::Got -join "; "
}

# A control first: a tap well outside the island's window reaches the probe.
$far = @([Math]::Max((Px 20), $wr.Left - (Px 60)), (Px 200))
$got = Tap $far[0] $far[1]
Expect "touch-reaches-probe" ($got -like "*pointerdown*") "tap at ($($far[0]), $($far[1])), outside the island's window: the probe got '$got'"

# Inside the island's window, off the island: every tap goes through.
$islandAt = if ($pill) { @($cx, (Px 24)) } else { @($cx, (Px 4)) }
$through = @(
    @("beside", ($cx + (Px 220)), (Px 4), "beside the island"),
    @("below", $cx, (Px 120), "below the island"),
    @("corner", ($wr.Left + (Px 12)), ($wr.Bottom - (Px 12)), "the window's bottom left corner"),
    @("edge", ($wr.Right - (Px 12)), ($wr.Top + (Px 30)), "the window's right edge")
)
foreach ($t in $through) {
    $got = Tap $t[1] $t[2]
    Expect "touch-through-$($t[0])" ($got -like "*pointerdown*" -and $got -like "*lbuttondown*") "tap at ($($t[1]), $($t[2])), $($t[3]): the probe got '$(if ($got) { $got } else { 'nothing' })'"
}
Expect "touch-through-leaves-it-closed" (-not (IsOpen)) "island open after those taps: $(IsOpen)"
Shot "1-after-taps-through"

# A tap on the island opens it, and the window under it gets nothing.
$got = Tap $islandAt[0] $islandAt[1]
[Probe]::Pump(800)
$open = IsOpen
Expect "touch-opens-island" ($open -and -not $got) "tap at ($($islandAt[0]), $($islandAt[1])): island open: $open; the probe got '$(if ($got) { $got } else { 'nothing' })'"
Shot "2-opened-by-touch"

# A tap outside the open island reaches the window there and closes it.
$got = Tap $far[0] $far[1]
[Probe]::Pump(1500)
$open = IsOpen
Expect "touch-outside-closes-island" (($got -like "*pointerdown*") -and -not $open) "tap at ($($far[0]), $($far[1])): the probe got '$got'; island open after 1.5 s: $open"
Shot "3-closed-by-touch"

$alive = -not $p.HasExited
"still running: $alive" | Tee-Object -FilePath $metrics -Append
if ($alive) { Stop-Process -Id $p.Id -Force; $p.WaitForExit(10000) | Out-Null }
[Probe]::Close()
Copy-Item -Path (Join-Path $env:PONI_HOME "cache\poni.log") -Destination (Join-Path $Out "poni.log") -ErrorAction SilentlyContinue
if (-not $alive) { Write-Error "Poni exited during the touch smoke test"; exit 1 }
if ($failures.Count -gt 0) { Write-Error "touch checks failed: $($failures -join ', ')"; exit 1 }
