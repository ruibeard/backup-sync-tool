#include "config.h"
#include "dpapi.h"

#include <windows.h>
#include <shlwapi.h>
#include <regex>
#include <sstream>
#include <vector>

namespace {

std::string ToUtf8(const std::wstring& value) {
    if (value.empty()) {
        return {};
    }

    const int size = WideCharToMultiByte(CP_UTF8, 0, value.c_str(), static_cast<int>(value.size()), nullptr, 0, nullptr, nullptr);
    std::string utf8(size, '\0');
    WideCharToMultiByte(CP_UTF8, 0, value.c_str(), static_cast<int>(value.size()), utf8.data(), size, nullptr, nullptr);
    return utf8;
}

std::wstring FromUtf8(const std::string& value) {
    if (value.empty()) {
        return {};
    }

    const int size = MultiByteToWideChar(CP_UTF8, 0, value.data(), static_cast<int>(value.size()), nullptr, 0);
    std::wstring wide(size, L'\0');
    MultiByteToWideChar(CP_UTF8, 0, value.data(), static_cast<int>(value.size()), wide.data(), size);
    return wide;
}

std::wstring ReadFileText(const std::wstring& path) {
    HANDLE file = CreateFileW(path.c_str(), GENERIC_READ, FILE_SHARE_READ, nullptr, OPEN_EXISTING, FILE_ATTRIBUTE_NORMAL, nullptr);
    if (file == INVALID_HANDLE_VALUE) {
        return L"";
    }

    LARGE_INTEGER size{};
    if (!GetFileSizeEx(file, &size) || size.QuadPart > MAXDWORD) {
        CloseHandle(file);
        return L"";
    }

    std::string bytes(static_cast<size_t>(size.QuadPart), '\0');
    DWORD read = 0;
    const BOOL ok = bytes.empty() ? TRUE : ReadFile(file, bytes.data(), static_cast<DWORD>(bytes.size()), &read, nullptr);
    CloseHandle(file);
    if (!ok || read != bytes.size()) {
        return L"";
    }

    if (bytes.size() >= 3 &&
        static_cast<unsigned char>(bytes[0]) == 0xEF &&
        static_cast<unsigned char>(bytes[1]) == 0xBB &&
        static_cast<unsigned char>(bytes[2]) == 0xBF) {
        bytes.erase(0, 3);
    }

    return FromUtf8(bytes);
}

bool WriteFileText(const std::wstring& path, const std::wstring& text) {
    // Write to a temp file first, then rename over the target so a crash or
    // disk-full during the write never leaves config.json in a partial state.
    const std::wstring tmp_path = path + L".tmp";

    HANDLE file = CreateFileW(tmp_path.c_str(), GENERIC_WRITE, 0, nullptr, CREATE_ALWAYS, FILE_ATTRIBUTE_NORMAL, nullptr);
    if (file == INVALID_HANDLE_VALUE) {
        return false;
    }

    std::string utf8 = ToUtf8(text);
    DWORD written = 0;
    const BOOL ok = utf8.empty() ? TRUE : WriteFile(file, utf8.data(), static_cast<DWORD>(utf8.size()), &written, nullptr);
    CloseHandle(file);

    if (!ok || written != utf8.size()) {
        DeleteFileW(tmp_path.c_str());
        return false;
    }

    // Atomic replace: MoveFileExW with MOVEFILE_REPLACE_EXISTING is atomic
    // on the same volume under NTFS.
    if (!MoveFileExW(tmp_path.c_str(), path.c_str(), MOVEFILE_REPLACE_EXISTING)) {
        DeleteFileW(tmp_path.c_str());
        return false;
    }

    return true;
}

std::wstring EscapeJson(const std::wstring& value) {
    std::wstring out;
    for (wchar_t ch : value) {
        switch (ch) {
        case L'\\':
            out += L"\\\\";
            break;
        case L'"':
            out += L"\\\"";
            break;
        case L'\r':
            out += L"\\r";
            break;
        case L'\n':
            out += L"\\n";
            break;
        case L'\t':
            out += L"\\t";
            break;
        default:
            out += ch;
            break;
        }
    }
    return out;
}

std::wstring UnescapeJson(const std::wstring& value) {
    std::wstring out;
    bool escape = false;
    for (wchar_t ch : value) {
        if (!escape) {
            if (ch == L'\\') {
                escape = true;
            } else {
                out += ch;
            }
            continue;
        }

        switch (ch) {
        case L'\\':
            out += L'\\';
            break;
        case L'"':
            out += L'"';
            break;
        case L'r':
            out += L'\r';
            break;
        case L'n':
            out += L'\n';
            break;
        case L't':
            out += L'\t';
            break;
        default:
            out += ch;
            break;
        }
        escape = false;
    }
    return out;
}

std::wstring ExtractString(const std::wstring& text, const std::wstring& key) {
    const std::wstring pattern = L"\"" + key + L"\"\\s*:\\s*\"((?:\\\\.|[^\"])*)\"";
    std::wregex regex(pattern, std::regex::icase);
    std::wsmatch match;
    if (!std::regex_search(text, match, regex) || match.size() < 2) {
        return L"";
    }
    return UnescapeJson(match[1].str());
}

bool ExtractBool(const std::wstring& text, const std::wstring& key) {
    const std::wstring pattern = L"\"" + key + L"\"\\s*:\\s*(true|false)";
    std::wregex regex(pattern, std::regex::icase);
    std::wsmatch match;
    if (!std::regex_search(text, match, regex) || match.size() < 2) {
        return false;
    }
    return _wcsicmp(match[1].str().c_str(), L"true") == 0;
}

} // namespace

std::wstring GetExecutableDirectory() {
    wchar_t buffer[MAX_PATH];
    GetModuleFileNameW(nullptr, buffer, MAX_PATH);
    PathRemoveFileSpecW(buffer);
    return std::wstring(buffer);
}

std::wstring GetConfigPath() {
    return GetExecutableDirectory() + L"\\config.json";
}

bool LoadConfig(AppConfig& config) {
    const std::wstring text = ReadFileText(GetConfigPath());
    if (text.empty()) {
        return false;
    }

    config.watch_folder = ExtractString(text, L"watch_folder");
    config.webdav_url = ExtractString(text, L"webdav_url");
    config.username = ExtractString(text, L"username");
    config.remote_folder = ExtractString(text, L"remote_folder");
    config.start_with_windows = ExtractBool(text, L"start_with_windows");
    config.download_remote_changes = ExtractBool(text, L"download_remote_changes");

    const std::wstring protected_password = ExtractString(text, L"password_protected");
    if (!protected_password.empty()) {
        UnprotectSecret(protected_password, config.password);
    }

    return true;
}

bool SaveConfig(const AppConfig& config) {
    std::wstring protected_password;
    if (!ProtectSecret(config.password, protected_password)) {
        return false;
    }

    std::wstringstream json;
    json << L"{\n";
    json << L"  \"watch_folder\": \"" << EscapeJson(config.watch_folder) << L"\",\n";
    json << L"  \"webdav_url\": \"" << EscapeJson(config.webdav_url) << L"\",\n";
    json << L"  \"remote_folder\": \"" << EscapeJson(config.remote_folder) << L"\",\n";
    json << L"  \"username\": \"" << EscapeJson(config.username) << L"\",\n";
    json << L"  \"password_protected\": \"" << EscapeJson(protected_password) << L"\",\n";
    json << L"  \"start_with_windows\": " << (config.start_with_windows ? L"true" : L"false") << L",\n";
    json << L"  \"download_remote_changes\": " << (config.download_remote_changes ? L"true" : L"false") << L"\n";
    json << L"}\n";

    return WriteFileText(GetConfigPath(), json.str());
}

bool HasUsableConfig(const AppConfig& config) {
    return !config.watch_folder.empty() &&
           !config.webdav_url.empty() &&
           !config.username.empty() &&
           !config.password.empty() &&
           !config.remote_folder.empty();
}
