using System.Net;
using System.Net.Http;
using System.Net.Http.Headers;
using System.Text;
using System.Xml.Linq;

namespace WebDavSync;

public class WebDavClient : IDisposable
{
    private readonly HttpClient _http;
    private readonly string _baseUrl;

    public WebDavClient(AppConfig cfg)
    {
        _baseUrl = cfg.WebDavUrl.TrimEnd('/');

        var handler = new HttpClientHandler
        {
            ServerCertificateCustomValidationCallback = HttpClientHandler.DangerousAcceptAnyServerCertificateValidator,
            Credentials = new NetworkCredential(cfg.Username, cfg.Password),
            PreAuthenticate = true,
        };

        _http = new HttpClient(handler)
        {
            Timeout = TimeSpan.FromSeconds(30),
        };

        var credentials = Convert.ToBase64String(Encoding.UTF8.GetBytes($"{cfg.Username}:{cfg.Password}"));
        _http.DefaultRequestHeaders.Authorization =
            new AuthenticationHeaderValue("Basic", credentials);
    }

    public bool TestConnection(out string errorMessage)
    {
        errorMessage = string.Empty;
        try
        {
            var request = new HttpRequestMessage(new HttpMethod("OPTIONS"), _baseUrl);
            var response = _http.Send(request);
            if (response.IsSuccessStatusCode || response.StatusCode == HttpStatusCode.MultiStatus)
                return true;

            errorMessage = $"Server returned {(int)response.StatusCode} {response.ReasonPhrase}";
            return false;
        }
        catch (Exception ex)
        {
            errorMessage = ex.Message;
            return false;
        }
    }

    public async Task<List<string>> ListFolderAsync(string remotePath)
    {
        var url = $"{_baseUrl}/{remotePath.TrimStart('/')}";
        var request = new HttpRequestMessage(new HttpMethod("PROPFIND"), url);
        request.Headers.Add("Depth", "1");
        request.Content = new StringContent(
            "<?xml version=\"1.0\"?><propfind xmlns=\"DAV:\"><prop><resourcetype/><displayname/></prop></propfind>",
            Encoding.UTF8, "application/xml");

        var response = await _http.SendAsync(request);
        if (!response.IsSuccessStatusCode && response.StatusCode != HttpStatusCode.MultiStatus)
            throw new Exception($"PROPFIND failed: {(int)response.StatusCode}");

        var xml  = await response.Content.ReadAsStringAsync();
        var doc  = XDocument.Parse(xml);
        XNamespace dav = "DAV:";

        var folders = new List<string>();
        foreach (var resp in doc.Descendants(dav + "response"))
        {
            var href = resp.Element(dav + "href")?.Value;
            if (href == null) continue;

            var isCollection = resp.Descendants(dav + "collection").Any();
            if (isCollection)
                folders.Add(Uri.UnescapeDataString(href));
        }
        return folders;
    }

    public async Task MakeDirectoryAsync(string remotePath)
    {
        var url     = $"{_baseUrl}/{remotePath.TrimStart('/')}";
        var request = new HttpRequestMessage(new HttpMethod("MKCOL"), url);
        var response = await _http.SendAsync(request);
        if (!response.IsSuccessStatusCode && response.StatusCode != HttpStatusCode.MethodNotAllowed)
            throw new Exception($"MKCOL failed: {(int)response.StatusCode}");
    }

    public async Task UploadFileAsync(string localPath, string remotePath,
        CancellationToken ct = default)
    {
        var url      = $"{_baseUrl}/{remotePath.TrimStart('/')}";
        using var fs = System.IO.File.OpenRead(localPath);
        var content  = new StreamContent(fs);
        var response = await _http.PutAsync(url, content, ct);
        if (!response.IsSuccessStatusCode)
            throw new Exception($"PUT failed: {(int)response.StatusCode} for {remotePath}");
    }

    public async Task DownloadFileAsync(string remotePath, string localPath,
        CancellationToken ct = default)
    {
        var url      = $"{_baseUrl}/{remotePath.TrimStart('/')}";
        var bytes    = await _http.GetByteArrayAsync(url, ct);
        var dir      = System.IO.Path.GetDirectoryName(localPath);
        if (dir != null) System.IO.Directory.CreateDirectory(dir);
        await System.IO.File.WriteAllBytesAsync(localPath, bytes, ct);
    }

    public void Dispose() => _http.Dispose();
}
