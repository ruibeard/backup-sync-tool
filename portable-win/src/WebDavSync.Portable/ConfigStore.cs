using System.Text.Json;
using System.Text.Json.Serialization;

namespace WebDavSync;

public class AppConfig
{
    [JsonPropertyName("watchFolder")]
    public string WatchFolder { get; set; } = string.Empty;

    [JsonPropertyName("webDavUrl")]
    public string WebDavUrl { get; set; } = string.Empty;

    [JsonPropertyName("username")]
    public string Username { get; set; } = string.Empty;

    // Password stored as DPAPI-protected base64 in JSON; plain-text in memory
    [JsonPropertyName("passwordProtected")]
    public string PasswordProtected { get; set; } = string.Empty;

    [JsonIgnore]
    public string Password { get; set; } = string.Empty;

    [JsonPropertyName("remoteFolder")]
    public string RemoteFolder { get; set; } = "/";

    [JsonPropertyName("startWithWindows")]
    public bool StartWithWindows { get; set; }

    [JsonPropertyName("downloadRemoteChanges")]
    public bool DownloadRemoteChanges { get; set; } = true;
}

public static class ConfigStore
{
    private static readonly JsonSerializerOptions _opts = new()
    {
        WriteIndented = true,
        DefaultIgnoreCondition = JsonIgnoreCondition.WhenWritingNull,
    };

    public static string ConfigPath
    {
        get
        {
            var dir = System.IO.Path.GetDirectoryName(Environment.ProcessPath) ?? ".";
            return System.IO.Path.Combine(dir, "config.json");
        }
    }

    public static AppConfig Load()
    {
        try
        {
            if (!System.IO.File.Exists(ConfigPath))
                return new AppConfig();

            var json = System.IO.File.ReadAllText(ConfigPath);
            var cfg  = JsonSerializer.Deserialize<AppConfig>(json, _opts) ?? new AppConfig();

            // Decrypt password
            if (!string.IsNullOrEmpty(cfg.PasswordProtected))
            {
                try { cfg.Password = SecretStore.Unprotect(cfg.PasswordProtected); }
                catch { cfg.Password = string.Empty; }
            }
            return cfg;
        }
        catch { return new AppConfig(); }
    }

    public static void Save(AppConfig cfg)
    {
        // Encrypt password before saving
        cfg.PasswordProtected = string.IsNullOrEmpty(cfg.Password)
            ? string.Empty
            : SecretStore.Protect(cfg.Password);

        var json = JsonSerializer.Serialize(cfg, _opts);
        var tmp  = ConfigPath + ".tmp";
        System.IO.File.WriteAllText(tmp, json, System.Text.Encoding.UTF8);
        System.IO.File.Move(tmp, ConfigPath, overwrite: true);
    }

    public static bool IsUsable(AppConfig cfg) =>
        !string.IsNullOrWhiteSpace(cfg.WatchFolder) &&
        !string.IsNullOrWhiteSpace(cfg.WebDavUrl)   &&
        !string.IsNullOrWhiteSpace(cfg.Username)    &&
        !string.IsNullOrWhiteSpace(cfg.Password)    &&
        !string.IsNullOrWhiteSpace(cfg.RemoteFolder);
}
