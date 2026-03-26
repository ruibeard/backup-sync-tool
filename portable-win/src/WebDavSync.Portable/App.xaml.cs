using System.Windows;
using WpfApp = System.Windows.Application;

namespace WebDavSync;

public partial class App : WpfApp
{
    private System.Windows.Forms.NotifyIcon? _trayIcon;
    private MainWindow? _mainWindow;

    protected override void OnStartup(StartupEventArgs e)
    {
        base.OnStartup(e);

        // Single-instance enforcement via mutex
        var mutex = new System.Threading.Mutex(true, "WebDavSyncSingleInstance", out bool createdNew);
        if (!createdNew)
        {
            // Bring existing window to front via named pipe or just exit
            Shutdown();
            return;
        }

        _mainWindow = new MainWindow();

        SetupTrayIcon();

        var config = ConfigStore.Load();
        if (ConfigStore.IsUsable(config))
        {
            _mainWindow.Hide();
        }
        else
        {
            _mainWindow.Show();
        }
    }

    private void SetupTrayIcon()
    {
        _trayIcon = new System.Windows.Forms.NotifyIcon
        {
            Text = "WebDavSync",
            Visible = true,
        };

        // Try to load icon from resources
        try
        {
            var iconStream = GetResourceStream(new Uri("Assets/app.ico", UriKind.Relative))?.Stream;
            if (iconStream != null)
                _trayIcon.Icon = new System.Drawing.Icon(iconStream);
            else
                _trayIcon.Icon = System.Drawing.SystemIcons.Application;
        }
        catch
        {
            _trayIcon.Icon = System.Drawing.SystemIcons.Application;
        }

        var contextMenu = new System.Windows.Forms.ContextMenuStrip();
        contextMenu.Items.Add("Open", null, (_, _) => ShowMainWindow());
        contextMenu.Items.Add("Open Logs", null, (_, _) => OpenLogs());
        contextMenu.Items.Add(new System.Windows.Forms.ToolStripSeparator());
        contextMenu.Items.Add("Exit", null, (_, _) => ExitApp());
        _trayIcon.ContextMenuStrip = contextMenu;

        _trayIcon.DoubleClick += (_, _) => ShowMainWindow();
    }

    private void ShowMainWindow()
    {
        _mainWindow ??= new MainWindow();
        _mainWindow.Show();
        _mainWindow.WindowState = WindowState.Normal;
        _mainWindow.Activate();
    }

    private void OpenLogs()
    {
        var logsDir = System.IO.Path.Combine(
            System.IO.Path.GetDirectoryName(Environment.ProcessPath) ?? ".", "logs");
        System.IO.Directory.CreateDirectory(logsDir);
        System.Diagnostics.Process.Start("explorer.exe", logsDir);
    }

    private void ExitApp()
    {
        _trayIcon?.Dispose();
        Shutdown();
    }

    protected override void OnExit(ExitEventArgs e)
    {
        _trayIcon?.Dispose();
        base.OnExit(e);
    }
}
