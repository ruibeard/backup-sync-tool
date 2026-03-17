#include "webdav_client.h"

#include <windows.h>
#include <winhttp.h>
#include <wincrypt.h>
#include <fstream>
#include <sstream>
#include <vector>

namespace {

std::wstring Base64EncodeUtf8(const std::wstring& value) {
    const int utf8_len = WideCharToMultiByte(CP_UTF8, 0, value.c_str(), -1, nullptr, 0, nullptr, nullptr);
    std::vector<char> utf8(utf8_len);
    WideCharToMultiByte(CP_UTF8, 0, value.c_str(), -1, utf8.data(), utf8_len, nullptr, nullptr);

    DWORD chars_needed = 0;
    CryptBinaryToStringA(reinterpret_cast<const BYTE*>(utf8.data()), utf8_len - 1, CRYPT_STRING_BASE64 | CRYPT_STRING_NOCRLF, nullptr, &chars_needed);
    std::vector<char> buffer(chars_needed);
    CryptBinaryToStringA(reinterpret_cast<const BYTE*>(utf8.data()), utf8_len - 1, CRYPT_STRING_BASE64 | CRYPT_STRING_NOCRLF, buffer.data(), &chars_needed);

    const int wide_len = MultiByteToWideChar(CP_ACP, 0, buffer.data(), -1, nullptr, 0);
    std::vector<wchar_t> wide(wide_len);
    MultiByteToWideChar(CP_ACP, 0, buffer.data(), -1, wide.data(), wide_len);
    return std::wstring(wide.data());
}

std::string ToUtf8(const std::wstring& value) {
    if (value.empty()) {
        return {};
    }

    const int size = WideCharToMultiByte(CP_UTF8, 0, value.c_str(), static_cast<int>(value.size()), nullptr, 0, nullptr, nullptr);
    std::string utf8(size, '\0');
    WideCharToMultiByte(CP_UTF8, 0, value.c_str(), static_cast<int>(value.size()), utf8.data(), size, nullptr, nullptr);
    return utf8;
}

bool ReadFileBytes(const std::wstring& path, std::vector<BYTE>& data, std::wstring& error_message) {
    HANDLE file = CreateFileW(path.c_str(), GENERIC_READ, FILE_SHARE_READ, nullptr, OPEN_EXISTING, FILE_ATTRIBUTE_NORMAL, nullptr);
    if (file == INVALID_HANDLE_VALUE) {
        error_message = L"Could not open file for upload.";
        return false;
    }

    LARGE_INTEGER file_size{};
    if (!GetFileSizeEx(file, &file_size) || file_size.QuadPart > MAXDWORD) {
        CloseHandle(file);
        error_message = L"File is too large for the current uploader.";
        return false;
    }

    data.resize(static_cast<size_t>(file_size.QuadPart));
    DWORD read_bytes = 0;
    const BOOL ok = data.empty() ? TRUE : ReadFile(file, data.data(), static_cast<DWORD>(data.size()), &read_bytes, nullptr);
    CloseHandle(file);
    if (!ok || read_bytes != data.size()) {
        error_message = L"Could not read file contents.";
        return false;
    }

    return true;
}

} // namespace

WebDavClient::WebDavClient(const AppConfig& config) {
    URL_COMPONENTSW components{};
    components.dwStructSize = sizeof(components);

    wchar_t host[256] = {};
    wchar_t path[2048] = {};
    components.lpszHostName = host;
    components.dwHostNameLength = _countof(host);
    components.lpszUrlPath = path;
    components.dwUrlPathLength = _countof(path);

    if (WinHttpCrackUrl(config.webdav_url.c_str(), 0, 0, &components)) {
        secure_ = components.nScheme == INTERNET_SCHEME_HTTPS;
        port_ = components.nPort;
        host_.assign(components.lpszHostName, components.dwHostNameLength);
        base_path_.assign(components.lpszUrlPath, components.dwUrlPathLength);
    }

    if (base_path_.empty()) {
        base_path_ = L"/";
    }
    if (base_path_.back() != L'/') {
        base_path_ += L"/";
    }

    auth_header_ = L"Authorization: Basic " + Base64EncodeUtf8(config.username + L":" + config.password) + L"\r\n";
}

bool WebDavClient::TestConnection(std::wstring& error_message) {
    DWORD status_code = 0;
    if (!SendRequest(L"OPTIONS", L"", nullptr, 0, status_code, error_message)) {
        return false;
    }

    if (status_code >= 200 && status_code < 500) {
        return true;
    }

    error_message = StatusToMessage(L"Connection test failed", status_code);
    return false;
}

bool WebDavClient::UploadFile(const std::wstring& local_path, const std::wstring& relative_path, std::wstring& error_message) {
    if (!EnsureBaseCollectionExists(error_message)) {
        return false;
    }

    if (!EnsureRemoteFolders(relative_path, error_message)) {
        return false;
    }

    std::vector<BYTE> data;
    if (!ReadFileBytes(local_path, data, error_message)) {
        return false;
    }

    DWORD status_code = 0;
    if (!SendRequest(L"PUT", relative_path, data.data(), static_cast<DWORD>(data.size()), status_code, error_message)) {
        return false;
    }

    if (status_code == 200 || status_code == 201 || status_code == 204) {
        return true;
    }

    error_message = StatusToMessage(L"Upload failed", status_code);
    return false;
}

bool WebDavClient::DeleteFile(const std::wstring& relative_path, std::wstring& error_message) {
    DWORD status_code = 0;
    if (!SendRequest(L"DELETE", relative_path, nullptr, 0, status_code, error_message)) {
        return false;
    }

    if (status_code == 200 || status_code == 202 || status_code == 204 || status_code == 404) {
        return true;
    }

    error_message = StatusToMessage(L"Delete failed", status_code);
    return false;
}

bool WebDavClient::SendRequest(
    const std::wstring& method,
    const std::wstring& relative_path,
    const void* body,
    DWORD body_size,
    DWORD& status_code,
    std::wstring& error_message) {
    HINTERNET session = WinHttpOpen(L"WebDavSync/1.0", WINHTTP_ACCESS_TYPE_DEFAULT_PROXY, WINHTTP_NO_PROXY_NAME, WINHTTP_NO_PROXY_BYPASS, 0);
    if (!session) {
        error_message = L"WinHTTP session failed.";
        return false;
    }

    if (host_.empty()) {
        WinHttpCloseHandle(session);
        error_message = L"Invalid WebDAV URL.";
        return false;
    }

    HINTERNET connection = WinHttpConnect(session, host_.c_str(), port_, 0);
    if (!connection) {
        WinHttpCloseHandle(session);
        error_message = L"Could not connect to server host.";
        return false;
    }

    const std::wstring request_path = BuildRequestPath(relative_path);
    DWORD flags = secure_ ? WINHTTP_FLAG_SECURE : 0;
    HINTERNET request = WinHttpOpenRequest(connection, method.c_str(), request_path.c_str(), nullptr, WINHTTP_NO_REFERER, WINHTTP_DEFAULT_ACCEPT_TYPES, flags);
    if (!request) {
        WinHttpCloseHandle(connection);
        WinHttpCloseHandle(session);
        error_message = L"Could not open HTTP request.";
        return false;
    }

    const wchar_t* headers = auth_header_.c_str();
    const BOOL sent = WinHttpSendRequest(
        request,
        headers,
        static_cast<DWORD>(-1L),
        body ? const_cast<void*>(body) : WINHTTP_NO_REQUEST_DATA,
        body_size,
        body_size,
        0);

    if (!sent || !WinHttpReceiveResponse(request, nullptr)) {
        error_message = L"HTTP request failed.";
        WinHttpCloseHandle(request);
        WinHttpCloseHandle(connection);
        WinHttpCloseHandle(session);
        return false;
    }

    DWORD size = sizeof(status_code);
    if (!WinHttpQueryHeaders(request, WINHTTP_QUERY_STATUS_CODE | WINHTTP_QUERY_FLAG_NUMBER, WINHTTP_HEADER_NAME_BY_INDEX, &status_code, &size, WINHTTP_NO_HEADER_INDEX)) {
        error_message = L"Could not read response status.";
        WinHttpCloseHandle(request);
        WinHttpCloseHandle(connection);
        WinHttpCloseHandle(session);
        return false;
    }

    WinHttpCloseHandle(request);
    WinHttpCloseHandle(connection);
    WinHttpCloseHandle(session);
    return true;
}

bool WebDavClient::EnsureRemoteFolders(const std::wstring& relative_path, std::wstring& error_message) {
    size_t position = 0;
    while (true) {
        const size_t next = relative_path.find(L'\\', position);
        if (next == std::wstring::npos) {
            break;
        }

        const std::wstring folder = relative_path.substr(0, next + 1);
        DWORD status_code = 0;
        if (!SendRequest(L"MKCOL", folder, nullptr, 0, status_code, error_message)) {
            return false;
        }

        if (!(status_code == 201 || status_code == 301 || status_code == 405)) {
            error_message = StatusToMessage(L"Folder creation failed", status_code);
            return false;
        }
        position = next + 1;
    }
    return true;
}

bool WebDavClient::EnsureCollectionExists(
    const std::wstring& full_path,
    std::wstring& error_message) {
    if (full_path.empty() || full_path == L"/") {
        return true;
    }

    DWORD status_code = 0;
    if (!SendRequest(L"MKCOL", full_path, nullptr, 0, status_code, error_message)) {
        return false;
    }

    if (status_code == 201 || status_code == 301 || status_code == 405) {
        return true;
    }

    error_message = StatusToMessage(L"Folder creation failed", status_code);
    return false;
}

bool WebDavClient::EnsureBaseCollectionExists(std::wstring& error_message) {
    if (base_path_.empty() || base_path_ == L"/") {
        return true;
    }
    return EnsureCollectionExists(base_path_, error_message);
}

std::wstring WebDavClient::BuildRequestPath(const std::wstring& relative_path) const {
    std::wstring normalized = relative_path;
    for (wchar_t& ch : normalized) {
        if (ch == L'\\') {
            ch = L'/';
        }
    }

    if (!normalized.empty() && normalized.front() == L'/') {
        return UrlEncode(normalized);
    }

    std::wstring path = base_path_;
    if (normalized.empty()) {
        return path;
    }

    if (!path.empty() && path.back() != L'/') {
        path += L'/';
    }

    path += UrlEncode(normalized);
    return path;
}

std::wstring WebDavClient::UrlEncode(const std::wstring& value) const {
    std::wstringstream output;
    const char* hex = "0123456789ABCDEF";
    const std::string utf8 = ToUtf8(value);
    for (unsigned char ch : utf8) {
        if ((ch >= 'a' && ch <= 'z') ||
            (ch >= 'A' && ch <= 'Z') ||
            (ch >= '0' && ch <= '9') ||
            ch == '-' || ch == '_' || ch == '.' || ch == '/') {
            output << static_cast<wchar_t>(ch);
            continue;
        }

        output << L'%';
        output << static_cast<wchar_t>(hex[(ch >> 4) & 0xF]);
        output << static_cast<wchar_t>(hex[ch & 0xF]);
    }
    return output.str();
}

std::wstring WebDavClient::StatusToMessage(const std::wstring& operation, DWORD status_code) const {
    std::wstringstream text;
    text << operation << L" (HTTP " << status_code << L")";
    return text.str();
}
