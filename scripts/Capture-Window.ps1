#Requires -Version 5.1
# Run with Windows PowerShell (powershell.exe), whose .NET Framework ships System.Drawing.
# Saves a PNG of a visible top-level window of the process whose title contains -TitleLike.
# PrintWindow with full-content rendering also captures WebView2 windows that are covered.
param(
    [Parameter(Mandatory)] [int] $ProcessId,
    [Parameter(Mandatory)] [string] $OutputPath,
    [string] $TitleLike = ''
)

$ErrorActionPreference = 'Stop'
Add-Type -ReferencedAssemblies System.Drawing -TypeDefinition @'
using System;
using System.Drawing;
using System.Drawing.Imaging;
using System.Runtime.InteropServices;
using System.Text;

public static class FirewolWindowShot {
    delegate bool EnumProc(IntPtr hwnd, IntPtr lparam);
    [DllImport("user32.dll")] static extern bool EnumWindows(EnumProc proc, IntPtr lparam);
    [DllImport("user32.dll")] static extern uint GetWindowThreadProcessId(IntPtr hwnd, out uint pid);
    [DllImport("user32.dll")] static extern bool IsWindowVisible(IntPtr hwnd);
    [DllImport("user32.dll", CharSet = CharSet.Unicode)] static extern int GetWindowTextW(IntPtr hwnd, StringBuilder text, int count);
    [StructLayout(LayoutKind.Sequential)] struct Rect { public int Left, Top, Right, Bottom; }
    [DllImport("user32.dll")] static extern bool GetWindowRect(IntPtr hwnd, out Rect rect);
    [DllImport("user32.dll")] static extern bool PrintWindow(IntPtr hwnd, IntPtr dc, uint flags);
    [DllImport("user32.dll")] static extern bool SetProcessDPIAware();

    public static string Capture(uint pid, string pattern, string path) {
        // Physical pixels; without this, scaled displays crop the capture.
        SetProcessDPIAware();
        IntPtr found = IntPtr.Zero;
        string title = "";
        EnumWindows((hwnd, _) => {
            uint owner;
            GetWindowThreadProcessId(hwnd, out owner);
            if (owner != pid || !IsWindowVisible(hwnd)) return true;
            var text = new StringBuilder(512);
            GetWindowTextW(hwnd, text, 512);
            if (text.Length == 0 || text.ToString().IndexOf(pattern, StringComparison.OrdinalIgnoreCase) < 0) return true;
            found = hwnd;
            title = text.ToString();
            return false;
        }, IntPtr.Zero);
        if (found == IntPtr.Zero) throw new InvalidOperationException("No matching window");
        Rect rect;
        GetWindowRect(found, out rect);
        using (var bitmap = new Bitmap(rect.Right - rect.Left, rect.Bottom - rect.Top, PixelFormat.Format32bppArgb))
        using (var graphics = Graphics.FromImage(bitmap)) {
            IntPtr dc = graphics.GetHdc();
            PrintWindow(found, dc, 2);
            graphics.ReleaseHdc(dc);
            bitmap.Save(path, ImageFormat.Png);
        }
        return title + " " + (rect.Right - rect.Left) + "x" + (rect.Bottom - rect.Top);
    }
}
'@
[FirewolWindowShot]::Capture([uint32]$ProcessId, $TitleLike, $OutputPath)
