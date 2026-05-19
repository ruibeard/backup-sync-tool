using System;
using System.Collections.Generic;
using System.Diagnostics.CodeAnalysis;
using System.Globalization;
using System.IO;
using System.Linq;
using System.Numerics;
using System.Security.Cryptography;
using System.Text;
using System.Text.Json;

var options = ParseArgs(args);
var licence = ReadLicence(options, allFields: !options.RemoteFolder);
var remoteFolder = BuildRemoteFolder(
    ValueOrEmpty(licence.Values, "Number"),
    ValueOrEmpty(licence.Values, "ComercialName"));

if (options.RemoteFolder)
{
    Console.WriteLine(remoteFolder);
    return 0;
}

if (options.Json)
{
    PrintJson(licence.Values, remoteFolder);
    return 0;
}

PrintSummary(licence.Values);
Console.WriteLine($"RemoteFolder: {remoteFolder}");
return 0;

static LicenceInfo ReadLicence(Options options, bool allFields)
{
    if (!File.Exists(options.LicensePath))
    {
        Fail($"License file not found at '{options.LicensePath}'.");
    }

    var pemPath = FindPemPath(options);
    if (pemPath is null)
    {
        Fail("XD public key file xd.pem was not found.");
    }

    using var document = JsonDocument.Parse(File.ReadAllText(options.LicensePath));
    var root = document.RootElement;

    using var rsa = RSA.Create();
    rsa.ImportFromPem(File.ReadAllText(pemPath));
    var publicKey = rsa.ExportParameters(includePrivateParameters: false);

    var values = allFields
        ? ReadAllFields(root, publicKey)
        : ReadRemoteFolderFields(root, publicKey);

    if (string.IsNullOrWhiteSpace(ValueOrEmpty(values, "Number")))
    {
        Fail("Decrypted licence number is empty.");
    }

    return new LicenceInfo(values);
}

static Dictionary<string, string> ReadRemoteFolderFields(JsonElement root, RSAParameters publicKey)
{
    var values = new Dictionary<string, string>(StringComparer.Ordinal);
    values["Number"] = DecryptRequiredJsonField(root, "Number", publicKey);
    values["ComercialName"] = DecryptRequiredJsonField(root, "ClientComercialName", publicKey);
    return values;
}

static Dictionary<string, string> ReadAllFields(JsonElement root, RSAParameters publicKey)
{
    var values = new Dictionary<string, string>(StringComparer.Ordinal);
    foreach (var property in root.EnumerateObject())
    {
        var key = OutputKey(property.Name);
        var value = property.Value.ValueKind == JsonValueKind.String
            ? property.Value.GetString() ?? ""
            : property.Value.ToString();
        values[key] = DecodeJsonField(value, publicKey);
    }

    return values;
}

static string DecryptRequiredJsonField(JsonElement root, string key, RSAParameters publicKey)
{
    if (!root.TryGetProperty(key, out var property))
    {
        Fail($"License JSON field '{key}' is missing.");
    }

    var value = property.GetString() ?? "";
    return DecodeJsonField(value, publicKey).Trim();
}

static string DecodeJsonField(string value, RSAParameters publicKey)
{
    if (IsEncryptedEmptyPlaceholder(value))
    {
        return "";
    }

    return TryDecryptXdField(value, publicKey, out var decrypted)
        ? decrypted.Trim()
        : value;
}

static string OutputKey(string input) =>
    input switch
    {
        "ClientComercialName" => "ComercialName",
        "ClientFiscalName" => "FiscalName",
        "ClientAddress" => "Address",
        "ClientPostalCode" => "PostalCode",
        "ClientCity" => "City",
        "ClientState" => "State",
        "ClientCountryCode" => "CountryCode",
        "ClientVat" => "Vat",
        "ClientPhoneNumber" => "PhoneNumber",
        "ClientEmail" => "Email",
        "AgentKeyId" => "PartnerKeyId",
        "AgentName" => "PartnerName",
        "SubAgentKeyId" => "SubPartnerKeyId",
        "SubAgentName" => "SubPartnerName",
        "XDDateWork" => "DateWork",
        "XDActiveProtectionDate" => "ActiveProtectionDate",
        "XDActiveProtectionType" => "ActiveProtectionType",
        "XDLicenseVersion" => "XDLicenseVersion",
        _ => input,
    };

static string ValueOrEmpty(IReadOnlyDictionary<string, string> values, string key) =>
    values.TryGetValue(key, out var value) ? value : "";

static void PrintSummary(IReadOnlyDictionary<string, string> values)
{
    var printed = new HashSet<string>(StringComparer.Ordinal);
    foreach (var key in PreferredOrder())
    {
        if (values.TryGetValue(key, out var value))
        {
            Console.WriteLine($"{key}: {value}");
            printed.Add(key);
        }
    }

    foreach (var key in values.Keys.Except(printed, StringComparer.Ordinal).OrderBy(key => key, StringComparer.Ordinal))
    {
        Console.WriteLine($"{key}: {values[key]}");
    }
}

static void PrintJson(IReadOnlyDictionary<string, string> values, string remoteFolder)
{
    var output = new Dictionary<string, string>(values, StringComparer.Ordinal)
    {
        ["RemoteFolder"] = remoteFolder,
    };
    var keys = PreferredOrder()
        .Where(output.ContainsKey)
        .Concat(output.Keys.Except(PreferredOrder(), StringComparer.Ordinal).OrderBy(key => key, StringComparer.Ordinal))
        .ToArray();

    Console.WriteLine("{");
    for (var i = 0; i < keys.Length; i++)
    {
        var key = keys[i];
        var comma = i == keys.Length - 1 ? "" : ",";
        Console.WriteLine($"  \"{JsonEscape(key)}\": \"{JsonEscape(output[key])}\"{comma}");
    }
    Console.WriteLine("}");
}

static string[] PreferredOrder() =>
[
    "ComercialName",
    "Number",
    "FiscalName",
    "Product",
    "Vat",
    "Email",
    "PhoneNumber",
    "Address",
    "PostalCode",
    "City",
    "PartnerName",
    "SoftwareCertificateNumber",
    "RemoteFolder",
];

static string? FindPemPath(Options options)
{
    var candidates = new[]
    {
        options.PemPath,
        Path.GetFullPath(Path.Combine(options.XdDir, "..", "..", "cfg", "xd.pem")),
        Path.Combine(Path.GetDirectoryName(options.LicensePath) ?? "", "xd.pem"),
    };

    return candidates
        .Where(path => !string.IsNullOrWhiteSpace(path))
        .FirstOrDefault(File.Exists);
}

static bool TryDecryptXdField(string value, RSAParameters publicKey, out string decrypted)
{
    decrypted = "";
    if (string.IsNullOrWhiteSpace(value))
    {
        return false;
    }

    try
    {
        var bytes = new List<byte>();
        foreach (var part in value.Split('=', StringSplitOptions.RemoveEmptyEntries))
        {
            var block = Convert.FromBase64String(part + "=");
            bytes.AddRange(RawRsaPublic(block, publicKey));
        }

        decrypted = new UTF8Encoding(encoderShouldEmitUTF8Identifier: false, throwOnInvalidBytes: true)
            .GetString(bytes.ToArray());
        return IsMostlyPrintable(decrypted);
    }
    catch
    {
        decrypted = "";
        return false;
    }
}

static bool IsEncryptedEmptyPlaceholder(string value)
{
    var trimmed = value.Trim();
    return trimmed.Length > 0 && trimmed.All(ch => ch is 'A' or '=');
}

static bool IsMostlyPrintable(string value)
{
    if (value.Length == 0)
    {
        return true;
    }

    var printable = value.Count(ch => !char.IsControl(ch) || ch is '\r' or '\n' or '\t');
    return printable == value.Length;
}

static byte[] RawRsaPublic(byte[] block, RSAParameters publicKey)
{
    if (publicKey.Modulus is null || publicKey.Exponent is null)
    {
        Fail("RSA public key is missing modulus or exponent.");
    }

    var modulus = new BigInteger(publicKey.Modulus, isUnsigned: true, isBigEndian: true);
    var exponent = new BigInteger(publicKey.Exponent, isUnsigned: true, isBigEndian: true);
    var cipher = new BigInteger(block, isUnsigned: true, isBigEndian: true);
    var plain = BigInteger.ModPow(cipher, exponent, modulus);
    return plain.ToByteArray(isUnsigned: true, isBigEndian: true);
}

static string BuildRemoteFolder(string number, string name)
{
    number = number.Trim();
    name = name.Trim();

    if (string.IsNullOrEmpty(number))
    {
        return "";
    }

    var slug = Slugify(name);
    return string.IsNullOrEmpty(slug) ? number : $"{number}-{slug}";
}

static string Slugify(string value)
{
    if (string.IsNullOrWhiteSpace(value))
    {
        return "";
    }

    var normalized = value.Normalize(NormalizationForm.FormD);
    var sb = new StringBuilder(normalized.Length);
    var previousDash = false;

    foreach (var ch in normalized)
    {
        var category = CharUnicodeInfo.GetUnicodeCategory(ch);
        if (category == UnicodeCategory.NonSpacingMark)
        {
            continue;
        }

        if (char.IsLetterOrDigit(ch))
        {
            sb.Append(ch);
            previousDash = false;
        }
        else if (!previousDash)
        {
            sb.Append('-');
            previousDash = true;
        }
    }

    return sb.ToString().Trim('-');
}

static string JsonEscape(string value)
{
    var sb = new StringBuilder(value.Length + 8);
    foreach (var ch in value)
    {
        switch (ch)
        {
            case '\\':
                sb.Append(@"\\");
                break;
            case '"':
                sb.Append("\\\"");
                break;
            case '\b':
                sb.Append(@"\b");
                break;
            case '\f':
                sb.Append(@"\f");
                break;
            case '\n':
                sb.Append(@"\n");
                break;
            case '\r':
                sb.Append(@"\r");
                break;
            case '\t':
                sb.Append(@"\t");
                break;
            default:
                if (char.IsControl(ch))
                {
                    sb.Append("\\u");
                    sb.Append(((int)ch).ToString("X4", CultureInfo.InvariantCulture));
                }
                else
                {
                    sb.Append(ch);
                }
                break;
        }
    }

    return sb.ToString();
}

static Options ParseArgs(string[] args)
{
    var options = new Options
    {
        XdDir = @"C:\XDSoftware\bin\xd",
        LicensePath = @"C:\XDSoftware\cfg\xd.lic",
    };

    for (var i = 0; i < args.Length; i++)
    {
        switch (args[i])
        {
            case "--xd-dir":
                options.XdDir = ReadValue(args, ref i, "--xd-dir");
                break;
            case "--license":
                options.LicensePath = ReadValue(args, ref i, "--license");
                break;
            case "--pem":
                options.PemPath = ReadValue(args, ref i, "--pem");
                break;
            case "--json":
                options.Json = true;
                break;
            case "--remote-folder":
                options.RemoteFolder = true;
                break;
            case "--help":
            case "-h":
                PrintUsage();
                Environment.Exit(0);
                break;
            default:
                Fail($"Unknown argument: {args[i]}");
                break;
        }
    }

    return options;
}

static string ReadValue(string[] args, ref int index, string optionName)
{
    if (index + 1 >= args.Length)
    {
        Fail($"Missing value for {optionName}.");
    }

    index++;
    return args[index];
}

static void PrintUsage()
{
    Console.WriteLine("Usage:");
    Console.WriteLine("  license-inspector [--remote-folder] [--json] [--license <path>] [--xd-dir <path>] [--pem <path>]");
}

[DoesNotReturn]
static void Fail(string message)
{
    Console.Error.WriteLine(message);
    Environment.Exit(1);
}

sealed class Options
{
    public string XdDir { get; set; } = "";
    public string LicensePath { get; set; } = "";
    public string? PemPath { get; set; }
    public bool Json { get; set; }
    public bool RemoteFolder { get; set; }
}

readonly record struct LicenceInfo(IReadOnlyDictionary<string, string> Values);
