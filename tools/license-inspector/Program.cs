using System;
using System.Collections.Generic;
using System.IO;
using System.Linq;
using System.Reflection;
using System.Reflection.Emit;
using System.Runtime.Loader;
using System.Security.Cryptography;
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

if (options.Probe)
{
    PrintProbe(options, assembly, xdLicenceType, loadToPreview);
    return 0;
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

Console.WriteLine($"RemoteFolder: {remoteFolder}");
var remaining = values.Keys.Except(preferredOrder, StringComparer.Ordinal).OrderBy(x => x, StringComparer.Ordinal);
foreach (var key in remaining)
{
    Console.WriteLine($"{key}: {values[key]}");
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
            case "--probe":
                options.Probe = true;
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
    Console.WriteLine("  dotnet run --project tools/license-inspector -- [--license <path>] [--xd-dir <path>] [--json] [--all] [--remote-folder] [--probe]");
}

static void PrintProbe(Options options, Assembly assembly, Type licenceType, MethodInfo loadToPreview)
{
    var licenceBytes = File.ReadAllBytes(options.LicensePath);
    var hash = Convert.ToHexString(SHA256.HashData(licenceBytes));

    Console.WriteLine("== Files ==");
    Console.WriteLine($"LicencePath: {options.LicensePath}");
    Console.WriteLine($"LicenceBytes: {licenceBytes.Length}");
    Console.WriteLine($"LicenceSha256: {hash}");
    Console.WriteLine($"LicenceFirst32Hex: {Convert.ToHexString(licenceBytes.Take(32).ToArray())}");
    Console.WriteLine($"LicencePrintableRatio: {PrintableRatio(licenceBytes):F3}");
    Console.WriteLine($"LicenceEntropyBitsPerByte: {EntropyBitsPerByte(licenceBytes):F3}");
    Console.WriteLine($"XdAssemblyPath: {Path.Combine(options.XdDir, "XDPeople.NET.dll")}");
    Console.WriteLine($"XdAssemblyName: {assembly.FullName}");
    Console.WriteLine();

    Console.WriteLine("== Entry Method ==");
    Console.WriteLine(MethodDisplay(loadToPreview));
    Console.WriteLine($"ReturnType: {TypeDisplay(loadToPreview.ReturnType)}");
    Console.WriteLine($"Parameters: {string.Join(", ", loadToPreview.GetParameters().Select(p => $"{TypeDisplay(p.ParameterType)} {p.Name}"))}");
    Console.WriteLine();

    var methods = new List<MethodBase> { loadToPreview };
    var seen = new HashSet<string>(StringComparer.Ordinal) { MethodKey(loadToPreview) };

    for (var index = 0; index < methods.Count && index < 40; index++)
    {
        var method = methods[index];
        foreach (var called in CalledAssemblyMethods(method, assembly))
        {
            if (seen.Add(MethodKey(called)))
            {
                methods.Add(called);
            }
        }
    }

    Console.WriteLine("== Disassembly ==");
    foreach (var method in methods)
    {
        Console.WriteLine();
        Console.WriteLine(MethodDisplay(method));
        foreach (var line in Disassemble(method))
        {
            Console.WriteLine(line);
        }
    }
}

static string TypeDisplay(Type type) =>
    type.FullName ?? type.Name;

static string MethodDisplay(MethodBase method)
{
    var declaringType = method.DeclaringType is null ? "<global>" : TypeDisplay(method.DeclaringType);
    return $"{declaringType}::{method.Name}";
}

static string MethodKey(MethodBase method) =>
    $"{method.Module.ModuleVersionId}:{method.MetadataToken}";

static IEnumerable<MethodBase> CalledAssemblyMethods(MethodBase method, Assembly assembly)
{
    foreach (var instruction in ReadInstructions(method))
    {
        if (instruction.Operand is MethodBase called && called.Module.Assembly == assembly)
        {
            yield return called;
        }
    }
}

static IEnumerable<string> Disassemble(MethodBase method)
{
    var instructions = ReadInstructions(method).ToArray();
    if (instructions.Length == 0)
    {
        yield return "  <no IL>";
        yield break;
    }

    foreach (var instruction in instructions)
    {
        yield return $"  IL_{instruction.Offset:X4}: {instruction.OpCode.Name,-12} {OperandDisplay(instruction.Operand)}".TrimEnd();
    }
}

static string OperandDisplay(object? operand)
{
    return operand switch
    {
        null => string.Empty,
        string value => $"\"{value}\"",
        MethodBase method => MethodDisplay(method),
        FieldInfo field => $"{TypeDisplay(field.DeclaringType!)}::{field.Name}",
        Type type => TypeDisplay(type),
        int value => value.ToString(CultureInfo.InvariantCulture),
        long value => value.ToString(CultureInfo.InvariantCulture),
        float value => value.ToString(CultureInfo.InvariantCulture),
        double value => value.ToString(CultureInfo.InvariantCulture),
        int[] labels => string.Join(", ", labels.Select(label => $"IL_{label:X4}")),
        _ => operand.ToString() ?? string.Empty,
    };
}

static IReadOnlyList<Instruction> ReadInstructions(MethodBase method)
{
    var body = method.GetMethodBody();
    var il = body?.GetILAsByteArray();
    if (il is null || il.Length == 0)
    {
        return [];
    }

    var instructions = new List<Instruction>();
    var module = method.Module;
    var position = 0;

    while (position < il.Length)
    {
        var offset = position;
        var code = il[position++];
        OpCode opCode;

        if (code == 0xFE)
        {
            opCode = IlOpCodeMap.MultiByte[il[position++]];
        }
        else
        {
            opCode = IlOpCodeMap.SingleByte[code];
        }

        object? operand = ReadOperand(il, ref position, offset, opCode, module);
        instructions.Add(new Instruction(offset, opCode, operand));
    }

    return instructions;
}

static object? ReadOperand(byte[] il, ref int position, int offset, OpCode opCode, Module module)
{
    try
    {
        return opCode.OperandType switch
        {
            OperandType.InlineNone => null,
            OperandType.ShortInlineBrTarget => position + 1 + unchecked((sbyte)il[position++]),
            OperandType.InlineBrTarget => ReadInt32(il, ref position) + position,
            OperandType.ShortInlineI => unchecked((sbyte)il[position++]),
            OperandType.InlineI => ReadInt32(il, ref position),
            OperandType.InlineI8 => ReadInt64(il, ref position),
            OperandType.ShortInlineR => ReadSingle(il, ref position),
            OperandType.InlineR => ReadDouble(il, ref position),
            OperandType.InlineString => module.ResolveString(ReadInt32(il, ref position)),
            OperandType.InlineSig => $"signature:0x{ReadInt32(il, ref position):X8}",
            OperandType.InlineSwitch => ReadSwitchTargets(il, ref position),
            OperandType.InlineTok or OperandType.InlineType or OperandType.InlineField or OperandType.InlineMethod => ResolveMember(module, ReadInt32(il, ref position)),
            OperandType.InlineVar => ReadUInt16(il, ref position),
            OperandType.ShortInlineVar => il[position++],
            _ => $"operand:{opCode.OperandType}@IL_{offset:X4}",
        };
    }
    catch (Exception ex)
    {
        return $"<unresolved: {ex.GetType().Name}: {ex.Message}>";
    }
}

static MemberInfo ResolveMember(Module module, int token)
{
    try
    {
        return module.ResolveMember(token) ?? throw new InvalidOperationException($"Token 0x{token:X8} did not resolve.");
    }
    catch
    {
        return module.ResolveType(token) ?? throw new InvalidOperationException($"Token 0x{token:X8} did not resolve as a type.");
    }
}

static int[] ReadSwitchTargets(byte[] il, ref int position)
{
    var count = ReadInt32(il, ref position);
    var basePosition = position + (count * 4);
    var labels = new int[count];

    for (var i = 0; i < count; i++)
    {
        labels[i] = basePosition + ReadInt32(il, ref position);
    }

    return labels;
}

static short ReadInt16(byte[] il, ref int position)
{
    var value = BitConverter.ToInt16(il, position);
    position += 2;
    return value;
}

static ushort ReadUInt16(byte[] il, ref int position) =>
    unchecked((ushort)ReadInt16(il, ref position));

static int ReadInt32(byte[] il, ref int position)
{
    var value = BitConverter.ToInt32(il, position);
    position += 4;
    return value;
}

static long ReadInt64(byte[] il, ref int position)
{
    var value = BitConverter.ToInt64(il, position);
    position += 8;
    return value;
}

static float ReadSingle(byte[] il, ref int position)
{
    var value = BitConverter.ToSingle(il, position);
    position += 4;
    return value;
}

static double ReadDouble(byte[] il, ref int position)
{
    var value = BitConverter.ToDouble(il, position);
    position += 8;
    return value;
}

static double PrintableRatio(byte[] bytes)
{
    if (bytes.Length == 0)
    {
        return 0;
    }

    var printable = bytes.Count(b => b is 9 or 10 or 13 || b is >= 32 and <= 126);
    return (double)printable / bytes.Length;
}

static double EntropyBitsPerByte(byte[] bytes)
{
    if (bytes.Length == 0)
    {
        return 0;
    }

    var counts = new int[256];
    foreach (var b in bytes)
    {
        counts[b]++;
    }

    var entropy = 0.0;
    foreach (var count in counts.Where(count => count > 0))
    {
        var probability = (double)count / bytes.Length;
        entropy -= probability * Math.Log2(probability);
    }

    return entropy;
}

sealed class Options
{
    public string XdDir { get; set; } = "";
    public string LicensePath { get; set; } = "";
    public bool Json { get; set; }
    public bool All { get; set; }
    public bool RemoteFolder { get; set; }
    public bool Probe { get; set; }
}

readonly record struct Instruction(int Offset, OpCode OpCode, object? Operand);

static class IlOpCodeMap
{
    public static readonly OpCode[] SingleByte = BuildOpCodeMap(singleByte: true);
    public static readonly OpCode[] MultiByte = BuildOpCodeMap(singleByte: false);

    private static OpCode[] BuildOpCodeMap(bool singleByte)
    {
        var map = new OpCode[256];
        foreach (var field in typeof(OpCodes).GetFields(BindingFlags.Public | BindingFlags.Static))
        {
            if (field.GetValue(null) is not OpCode opCode)
            {
                continue;
            }

            var value = unchecked((ushort)opCode.Value);
            if (singleByte && value <= 0xFF)
            {
                map[value] = opCode;
            }
            else if (!singleByte && (value & 0xFF00) == 0xFE00)
            {
                map[value & 0xFF] = opCode;
            }
        }

        return map;
    }
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
