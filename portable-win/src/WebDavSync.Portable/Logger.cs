namespace WebDavSync;

public static class Logger
{
    private static readonly object _lock = new();

    private static string LogDir =>
        System.IO.Path.Combine(
            System.IO.Path.GetDirectoryName(Environment.ProcessPath) ?? ".", "logs");

    public static void Write(string message)
    {
        try
        {
            lock (_lock)
            {
                System.IO.Directory.CreateDirectory(LogDir);
                var file = System.IO.Path.Combine(LogDir, $"{DateTime.Now:yyyy-MM-dd}.log");
                var line = $"{DateTime.Now:HH:mm:ss}  {message}{Environment.NewLine}";
                System.IO.File.AppendAllText(file, line, System.Text.Encoding.UTF8);
            }
        }
        catch { /* best-effort */ }
    }
}
