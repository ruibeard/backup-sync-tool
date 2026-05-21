using System;
using System.Drawing;
using System.Drawing.Imaging;
using System.Runtime.InteropServices;
using System.Text;
using System.Threading;

internal static class CaptureWindow
{
    private delegate bool EnumWindowsProc(IntPtr hWnd, IntPtr lParam);

    [DllImport("user32.dll")]
    private static extern bool EnumWindows(EnumWindowsProc lpEnumFunc, IntPtr lParam);

    [DllImport("user32.dll", CharSet = CharSet.Unicode)]
    private static extern int GetClassName(IntPtr hWnd, StringBuilder lpClassName, int nMaxCount);

    [DllImport("user32.dll")]
    private static extern bool ShowWindow(IntPtr hWnd, int nCmdShow);

    [DllImport("user32.dll")]
    private static extern bool SetForegroundWindow(IntPtr hWnd);

    [DllImport("user32.dll")]
    private static extern bool GetWindowRect(IntPtr hWnd, out RECT lpRect);

    [DllImport("user32.dll")]
    private static extern bool PrintWindow(IntPtr hWnd, IntPtr hdcBlt, uint nFlags);

    [StructLayout(LayoutKind.Sequential)]
    private struct RECT
    {
        public int Left, Top, Right, Bottom;
    }

    private static IntPtr target = IntPtr.Zero;

    private static bool FindClass(IntPtr hWnd, IntPtr lParam)
    {
        var sb = new StringBuilder(256);
        GetClassName(hWnd, sb, sb.Capacity);
        if (sb.ToString() == "BackupSyncToolWnd")
        {
            target = hWnd;
            return false;
        }
        return true;
    }

    private static int Main(string[] args)
    {
        var path = args.Length > 0 ? args[0] : "layout_h5_verify.png";
        EnumWindows(FindClass, IntPtr.Zero);
        if (target == IntPtr.Zero)
        {
            Console.Error.WriteLine("BackupSyncToolWnd not found");
            return 1;
        }

        ShowWindow(target, 5);
        SetForegroundWindow(target);
        Thread.Sleep(700);

        RECT r;
        GetWindowRect(target, out r);
        int w = r.Right - r.Left;
        int h = r.Bottom - r.Top;

        using (var bmp = new Bitmap(w, h, PixelFormat.Format32bppArgb))
        {
            using (var g = Graphics.FromImage(bmp))
            {
                IntPtr hdc = g.GetHdc();
                if (!PrintWindow(target, hdc, 2))
                {
                    PrintWindow(target, hdc, 0);
                }
                g.ReleaseHdc(hdc);
            }
            bmp.Save(path, ImageFormat.Png);
        }

        Console.WriteLine("PrintWindow saved " + path + " (" + w + "x" + h + ")");
        return 0;
    }
}
