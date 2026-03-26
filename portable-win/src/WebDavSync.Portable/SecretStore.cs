using System.Security.Cryptography;
using System.Text;

namespace WebDavSync;

/// <summary>
/// DPAPI-backed secret storage — protects secrets to the current Windows user account.
/// </summary>
public static class SecretStore
{
    private static readonly byte[] _entropy = Encoding.UTF8.GetBytes("WebDavSync_v1");

    public static string Protect(string plainText)
    {
        var data      = Encoding.UTF8.GetBytes(plainText);
        var protected_ = ProtectedData.Protect(data, _entropy, DataProtectionScope.CurrentUser);
        return Convert.ToBase64String(protected_);
    }

    public static string Unprotect(string base64)
    {
        var data        = Convert.FromBase64String(base64);
        var unprotected = ProtectedData.Unprotect(data, _entropy, DataProtectionScope.CurrentUser);
        return Encoding.UTF8.GetString(unprotected);
    }
}
