using System;
using System.Collections.Generic;
using System.IO;
using System.Linq;
using System.Reflection;
using System.Runtime.Loader;
using System.Text.Json;
using System.Text;
using System.Globalization;

var preferredOrder = new[]
{
    "ComercialName",
    "Number",
    "FiscalName",
    "ProductString",
    "ProgramLineString",
    "Vat",
    "Email",
    "PhoneNumber",
    "Address",
    "PostalCode",
    "City",
    "PartnerName",
    "SoftwareCertificateNumber",
    "ValidLicense",
};

var options = ParseArgs(args);
var dllPath = Path.Combine(options.XdDir, "XDPeople.NET.dll");

if (!File.Exists(dllPath))
{
    Console.Error.WriteLine($"XDPeople.NET.dll not found at '{dllPath}'.");
    return 1;
}

if (!File.Exists(options.LicensePath))
{
    Console.Error.WriteLine($"License file not found at '{options.LicensePath}'.");
    return 1;
}

var loadContext = new XdLoadContext(dllPath, options.XdDir);
Assembly assembly;

try
{
    assembly = loadContext.LoadFromAssemblyPath(dllPath);
}
catch (Exception ex)
{
    Console.Error.WriteLine($"Failed to load XDPeople.NET.dll: {ex.Message}");
    return 1;
}

var xdLicenceType = assembly.GetType("XDPeople.Utils.XDLicence", throwOnError: false);
if (xdLicenceType is null)
{
    Console.Error.WriteLine("Type 'XDPeople.Utils.XDLicence' was not found.");
    return 1;
}

var loadToPreview = xdLicenceType.GetMethod(
    "LoadToPreview",
    BindingFlags.Public | BindingFlags.NonPublic | BindingFlags.Static,
    binder: null,
    types: [typeof(string)],
    modifiers: null);

if (loadToPreview is null)
{
    Console.Error.WriteLine("Method 'XDPeople.Utils.XDLicence.LoadToPreview(string)' was not found.");
    return 1;
}

object? licenceData;
try
{
    licenceData = loadToPreview.Invoke(null, [options.LicensePath]);
}
catch (TargetInvocationException ex)
{
    var inner = ex.InnerException ?? ex;
    Console.Error.WriteLine($"Invocation failed: {inner.GetType().FullName}: {inner.Message}");
    return 1;
}
catch (Exception ex)
{
    Console.Error.WriteLine($"Invocation failed: {ex.GetType().FullName}: {ex.Message}");
    return 1;
}

if (licenceData is null)
{
    Console.Error.WriteLine("LoadToPreview returned null.");
    return 1;
}

var values = ReadProperties(licenceData);
var remoteFolder = BuildRemoteFolder(values);

if (options.RemoteFolder)
{
    if (string.IsNullOrWhiteSpace(remoteFolder))
    {
        Console.Error.WriteLine("Could not derive remote folder name from the licence.");
        return 1;
    }

    Console.WriteLine(remoteFolder);
    return 0;
}

if (options.Json)
{
    var payload = new Dictionary<string, string>(values, StringComparer.Ordinal)
    {
        ["RemoteFolder"] = remoteFolder,
    };
    var json = JsonSerializer.Serialize(payload, new JsonSerializerOptions { WriteIndented = true });
    Console.WriteLine(json);
    return 0;
}

foreach (var key in preferredOrder)
{
    if (values.TryGetValue(key, out var value))
    {
        Console.WriteLine($"{key}: {value}");
    }
}

if (options.All)
{
    Console.WriteLine($"RemoteFolder: {remoteFolder}");
    var remaining = values.Keys.Except(preferredOrder, StringComparer.Ordinal).OrderBy(x => x, StringComparer.Ordinal);
    foreach (var key in remaining)
    {
        Console.WriteLine($"{key}: {values[key]}");
    }
}

return 0;

static Dictionary<string, string> ReadProperties(object instance)
{
    var result = new Dictionary<string, string>(StringComparer.Ordinal);
    foreach (var prop in instance.GetType().GetProperties(BindingFlags.Public | BindingFlags.Instance).OrderBy(p => p.Name, StringComparer.Ordinal))
    {
        object? value;
        try
        {
            value = prop.GetValue(instance);
        }
        catch (Exception ex)
        {
            value = $"<error: {ex.GetType().Name}: {ex.Message}>";
        }

        result[prop.Name] = FormatValue(value);
    }

    return result;
}

static string FormatValue(object? value)
{
    if (value is null)
    {
        return "<null>";
    }

    if (value is string s)
    {
        return s;
    }

    if (value is System.Collections.IEnumerable enumerable and not string)
    {
        var items = new List<string>();
        foreach (var item in enumerable)
        {
            items.Add(item?.ToString() ?? "<null>");
            if (items.Count >= 10)
            {
                break;
            }
        }

        return $"[{string.Join(", ", items)}]";
    }

    return value.ToString() ?? "<null>";
}

static string BuildRemoteFolder(IReadOnlyDictionary<string, string> values)
{
    values.TryGetValue("Number", out var number);
    values.TryGetValue("ComercialName", out var name);

    number = (number ?? string.Empty).Trim();
    name = (name ?? string.Empty).Trim();

    if (string.IsNullOrEmpty(number))
    {
        return string.Empty;
    }

    var slug = Slugify(name);
    return string.IsNullOrEmpty(slug) ? number : $"{number}-{slug}";
}

static string Slugify(string value)
{
    if (string.IsNullOrWhiteSpace(value))
    {
        return string.Empty;
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
            case "--json":
                options.Json = true;
                break;
            case "--remote-folder":
                options.RemoteFolder = true;
                break;
            case "--all":
                options.All = true;
                break;
            case "--help":
            case "-h":
                PrintUsage();
                Environment.Exit(0);
                break;
            default:
                Console.Error.WriteLine($"Unknown argument: {args[i]}");
                PrintUsage();
                Environment.Exit(1);
                break;
        }
    }

    return options;
}

static string ReadValue(string[] args, ref int index, string optionName)
{
    if (index + 1 >= args.Length)
    {
        Console.Error.WriteLine($"Missing value for {optionName}.");
        Environment.Exit(1);
    }

    index++;
    return args[index];
}

static void PrintUsage()
{
    Console.WriteLine("Usage:");
    Console.WriteLine("  dotnet run --project tools/license-inspector -- [--license <path>] [--xd-dir <path>] [--json] [--all] [--remote-folder]");
}

sealed class Options
{
    public string XdDir { get; set; } = "";
    public string LicensePath { get; set; } = "";
    public bool Json { get; set; }
    public bool All { get; set; }
    public bool RemoteFolder { get; set; }
}

sealed class XdLoadContext : AssemblyLoadContext
{
    private readonly string _xdDir;
    private readonly string[] _runtimeProbeDirs;

    public XdLoadContext(string mainAssemblyPath, string xdDir)
    {
        _xdDir = xdDir;
        _runtimeProbeDirs = GetRuntimeProbeDirs().ToArray();
    }

    protected override Assembly? Load(AssemblyName assemblyName)
    {
        if (assemblyName.Name is null)
        {
            return null;
        }

        if (IsFrameworkAssembly(assemblyName.Name))
        {
            var runtimeAssembly = TryLoadFromProbeDirs(assemblyName.Name, _runtimeProbeDirs);
            if (runtimeAssembly is not null)
            {
                return runtimeAssembly;
            }
        }

        if (!IsFrameworkAssembly(assemblyName.Name))
        {
            var localAssembly = TryLoadFromProbeDirs(assemblyName.Name, [_xdDir]);
            if (localAssembly is not null)
            {
                return localAssembly;
            }
        }

        var fallbackRuntimeAssembly = TryLoadFromProbeDirs(assemblyName.Name, _runtimeProbeDirs);
        if (fallbackRuntimeAssembly is not null)
        {
            return fallbackRuntimeAssembly;
        }

        return null;
    }

    private static IEnumerable<string> GetRuntimeProbeDirs()
    {
        var sharedRoot = Path.Combine(Environment.GetFolderPath(Environment.SpecialFolder.ProgramFiles), "dotnet", "shared");
        foreach (var runtimeName in new[] { "Microsoft.NETCore.App", "Microsoft.WindowsDesktop.App" })
        {
            var runtimeRoot = Path.Combine(sharedRoot, runtimeName);
            if (!Directory.Exists(runtimeRoot))
            {
                continue;
            }

            foreach (var dir in new DirectoryInfo(runtimeRoot).GetDirectories().OrderByDescending(d => d.Name, StringComparer.OrdinalIgnoreCase))
            {
                yield return dir.FullName;
            }
        }
    }

    private Assembly? TryLoadFromProbeDirs(string assemblySimpleName, IEnumerable<string> probeDirs)
    {
        foreach (var probeDir in probeDirs)
        {
            var candidate = Path.Combine(probeDir, $"{assemblySimpleName}.dll");
            if (File.Exists(candidate))
            {
                return LoadFromAssemblyPath(candidate);
            }
        }

        return null;
    }

    private static bool IsFrameworkAssembly(string name) =>
        name.StartsWith("System.", StringComparison.Ordinal) ||
        name.Equals("System", StringComparison.Ordinal) ||
        name.StartsWith("Microsoft.", StringComparison.Ordinal) ||
        name.Equals("mscorlib", StringComparison.Ordinal) ||
        name.Equals("netstandard", StringComparison.Ordinal) ||
        name.Equals("WindowsBase", StringComparison.Ordinal) ||
        name.Equals("PresentationCore", StringComparison.Ordinal) ||
        name.Equals("PresentationFramework", StringComparison.Ordinal);
}
