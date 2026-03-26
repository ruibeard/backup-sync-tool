using System.IO;

namespace WebDavSync;

/// <summary>
/// Background sync engine using FileSystemWatcher with 500ms debounce.
/// </summary>
public class SyncEngine : IDisposable
{
    private readonly AppConfig _config;
    private readonly Action<string> _log;
    private FileSystemWatcher? _watcher;
    private CancellationTokenSource? _cts;
    private System.Timers.Timer? _debounce;
    private bool _running;
    private readonly object _lock = new();

    public SyncEngine(AppConfig config, Action<string> log)
    {
        _config = config;
        _log    = log;
    }

    public void Start()
    {
        lock (_lock)
        {
            if (_running) return;
            _running = true;
        }

        _cts = new CancellationTokenSource();
        _debounce = new System.Timers.Timer(500) { AutoReset = false };
        _debounce.Elapsed += (_, _) => _ = SyncNowAsync(_cts.Token);

        if (!Directory.Exists(_config.WatchFolder))
        {
            _log($"Watch folder not found: {_config.WatchFolder}");
            return;
        }

        _watcher = new FileSystemWatcher(_config.WatchFolder)
        {
            IncludeSubdirectories = true,
            NotifyFilter = NotifyFilters.FileName | NotifyFilters.DirectoryName
                         | NotifyFilters.LastWrite  | NotifyFilters.Size,
            EnableRaisingEvents = true,
        };

        _watcher.Created += OnChanged;
        _watcher.Changed += OnChanged;
        _watcher.Deleted += OnChanged;
        _watcher.Renamed += OnRenamed;

        _log($"Watching: {_config.WatchFolder}");

        // Run initial sync
        _ = SyncNowAsync(_cts.Token);
    }

    public void Stop()
    {
        lock (_lock) { _running = false; }
        _cts?.Cancel();
        _watcher?.Dispose();
        _watcher = null;
        _debounce?.Dispose();
        _debounce = null;
    }

    private void OnChanged(object sender, FileSystemEventArgs e)
    {
        _debounce?.Stop();
        _debounce?.Start();
    }

    private void OnRenamed(object sender, RenamedEventArgs e)
    {
        _debounce?.Stop();
        _debounce?.Start();
    }

    private async Task SyncNowAsync(CancellationToken ct)
    {
        if (!_running) return;

        _log("Sync started");
        try
        {
            using var client = new WebDavClient(_config);

            // Build local snapshot
            var localFiles = Directory.EnumerateFiles(_config.WatchFolder, "*", SearchOption.AllDirectories)
                .Select(f => Path.GetRelativePath(_config.WatchFolder, f))
                .ToList();

            int uploaded = 0;
            foreach (var rel in localFiles)
            {
                if (ct.IsCancellationRequested) break;

                var localPath  = Path.Combine(_config.WatchFolder, rel);
                var remotePath = _config.RemoteFolder.TrimEnd('/') + "/" + rel.Replace('\\', '/');

                try
                {
                    await client.UploadFileAsync(localPath, remotePath, ct);
                    uploaded++;
                    _log($"Uploaded: {rel}");
                }
                catch (Exception ex)
                {
                    _log($"Upload failed {rel}: {ex.Message}");
                }
            }

            _log($"Sync complete: {uploaded}/{localFiles.Count} files uploaded");
        }
        catch (Exception ex)
        {
            _log($"Sync error: {ex.Message}");
        }
    }

    public void Dispose() => Stop();
}
