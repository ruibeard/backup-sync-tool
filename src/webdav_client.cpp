#include "webdav_client.h"

#include <windows.h>
#include <wincrypt.h>
#include <winhttp.h>

#include <cwctype>
#include <sstream>
#include <string>
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

std::wstring FromUtf8(const std::string& value) {
    if (value.empty()) {
        return {};
    }

    const int size = MultiByteToWideChar(CP_UTF8, 0, value.data(), static_cast<int>(value.size()), nullptr, 0);
    std::wstring wide(size, L'\0');
    MultiByteToWideChar(CP_UTF8, 0, value.data(), static_cast<int>(value.size()), wide.data(), size);
    return wide;
}

std::wstring XmlDecode(const std::wstring& value) {
    std::wstring decoded = value;
    struct Entity {
        const wchar_t* from;
        const wchar_t* to;
    };

    const Entity entities[] = {
        {L"&amp;", L"&"},
        {L"&lt;", L"<"},
        {L"&gt;", L">"},
        {L"&quot;", L"\""},
        {L"&apos;", L"'"},
    };

    for (const auto& entity : entities) {
        size_t position = 0;
        const std::wstring from = entity.from;
        const std::wstring to = entity.to;
        while ((position = decoded.find(from, position)) != std::wstring::npos) {
            decoded.replace(position, from.size(), to);
            position += to.size();
        }
    }

    return decoded;
}

bool EqualsIgnoreCase(const std::wstring& left, const std::wstring& right) {
    if (left.size() != right.size()) {
        return false;
    }

    for (size_t index = 0; index < left.size(); ++index) {
        if (towlower(left[index]) != towlower(right[index])) {
            return false;
        }
    }

    return true;
}

bool NameMatchesTag(const std::wstring& name, const std::wstring& tag) {
    if (EqualsIgnoreCase(name, tag)) {
        return true;
    }

    const size_t colon = name.find(L':');
    return colon != std::wstring::npos && EqualsIgnoreCase(name.substr(colon + 1), tag);
}

bool FindTagText(const std::wstring& text, const std::wstring& tag, std::wstring& value, size_t start_position = 0) {
    value.clear();

    size_t open_start = std::wstring::npos;
    size_t open_end = std::wstring::npos;
    for (size_t index = start_position; index < text.size(); ++index) {
        if (text[index] != L'<') {
            continue;
        }
        if (index + 1 < text.size() && text[index + 1] == L'/') {
            continue;
        }

        size_t name_start = index + 1;
        while (name_start < text.size() && iswspace(text[name_start])) {
            ++name_start;
        }

        size_t cursor = name_start;
        while (cursor < text.size() && text[cursor] != L'>' && !iswspace(text[cursor]) && text[cursor] != L'/') {
            ++cursor;
        }

        const std::wstring name = text.substr(name_start, cursor - name_start);
        if (!NameMatchesTag(name, tag)) {
            continue;
        }

        open_start = index;
        open_end = text.find(L'>', cursor);
        if (open_end == std::wstring::npos) {
            return false;
        }
        break;
    }

    if (open_start == std::wstring::npos || open_end == std::wstring::npos) {
        return false;
    }

    if (open_end > open_start && text[open_end - 1] == L'/') {
        value.clear();
        return true;
    }

    size_t close_start = open_end + 1;
    while (true) {
        close_start = text.find(L"</", close_start);
        if (close_start == std::wstring::npos) {
            return false;
        }

        size_t name_start = close_start + 2;
        while (name_start < text.size() && iswspace(text[name_start])) {
            ++name_start;
        }

        size_t cursor = name_start;
        while (cursor < text.size() && text[cursor] != L'>' && !iswspace(text[cursor])) {
            ++cursor;
        }

        const std::wstring name = text.substr(name_start, cursor - name_start);
        if (!NameMatchesTag(name, tag)) {
            close_start = cursor;
            continue;
        }

        const size_t close_end = text.find(L'>', cursor);
        if (close_end == std::wstring::npos) {
            return false;
        }

        value = text.substr(open_end + 1, close_start - open_end - 1);
        return true;
    }
}

bool NextResponseBlock(const std::wstring& text, size_t& position, std::wstring& block) {
    block.clear();

    size_t open_start = std::wstring::npos;
    size_t open_end = std::wstring::npos;
    for (size_t index = position; index < text.size(); ++index) {
        if (text[index] != L'<') {
            continue;
        }
        if (index + 1 < text.size() && text[index + 1] == L'/') {
            continue;
        }

        size_t name_start = index + 1;
        while (name_start < text.size() && iswspace(text[name_start])) {
            ++name_start;
        }

        size_t cursor = name_start;
        while (cursor < text.size() && text[cursor] != L'>' && !iswspace(text[cursor]) && text[cursor] != L'/') {
            ++cursor;
        }

        const std::wstring name = text.substr(name_start, cursor - name_start);
        if (!NameMatchesTag(name, L"response")) {
            continue;
        }

        open_start = index;
        open_end = text.find(L'>', cursor);
        if (open_end == std::wstring::npos) {
            return false;
        }
        break;
    }

    if (open_start == std::wstring::npos || open_end == std::wstring::npos) {
        return false;
    }

    size_t close_start = open_end + 1;
    while (true) {
        close_start = text.find(L"</", close_start);
        if (close_start == std::wstring::npos) {
            return false;
        }

        size_t name_start = close_start + 2;
        while (name_start < text.size() && iswspace(text[name_start])) {
            ++name_start;
        }

        size_t cursor = name_start;
        while (cursor < text.size() && text[cursor] != L'>' && !iswspace(text[cursor])) {
            ++cursor;
        }

        const std::wstring name = text.substr(name_start, cursor - name_start);
        if (!NameMatchesTag(name, L"response")) {
            close_start = cursor;
            continue;
        }

        const size_t close_end = text.find(L'>', cursor);
        if (close_end == std::wstring::npos) {
            return false;
        }

        block = text.substr(open_start, close_end - open_start + 1);
        position = close_end + 1;
        return true;
    }
}

bool IsCollectionResponse(const std::wstring& text) {
    std::wstring resource_type;
    if (!FindTagText(text, L"resourcetype", resource_type)) {
        return false;
    }

    std::wstring ignored;
    return FindTagText(resource_type, L"collection", ignored);
}

ULONGLONG FileTimeToUInt64(const FILETIME& value) {
    ULARGE_INTEGER converted{};
    converted.LowPart = value.dwLowDateTime;
    converted.HighPart = value.dwHighDateTime;
    return converted.QuadPart;
}

bool FileTimesMatchWithinSeconds(const FILETIME& left, const FILETIME& right, ULONGLONG seconds_tolerance) {
    const ULONGLONG left_value = FileTimeToUInt64(left);
    const ULONGLONG right_value = FileTimeToUInt64(right);
    const ULONGLONG difference = left_value > right_value ? (left_value - right_value) : (right_value - left_value);
    return difference <= seconds_tolerance * 10000000ULL;
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

bool WebDavClient::IsRemoteFileCurrent(const std::wstring& relative_path, const FileEntry& local_entry, bool& is_current, std::wstring& error_message) {
    is_current = false;

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
    const DWORD flags = secure_ ? WINHTTP_FLAG_SECURE : 0;
    HINTERNET request = WinHttpOpenRequest(connection, L"HEAD", request_path.c_str(), nullptr, WINHTTP_NO_REFERER, WINHTTP_DEFAULT_ACCEPT_TYPES, flags);
    if (!request) {
        WinHttpCloseHandle(connection);
        WinHttpCloseHandle(session);
        error_message = L"Could not open HTTP request.";
        return false;
    }

    const BOOL sent = WinHttpSendRequest(request, auth_header_.c_str(), static_cast<DWORD>(-1L), WINHTTP_NO_REQUEST_DATA, 0, 0, 0);
    if (!sent || !WinHttpReceiveResponse(request, nullptr)) {
        error_message = L"HTTP request failed.";
        WinHttpCloseHandle(request);
        WinHttpCloseHandle(connection);
        WinHttpCloseHandle(session);
        return false;
    }

    DWORD status_code = 0;
    DWORD status_size = sizeof(status_code);
    if (!WinHttpQueryHeaders(request, WINHTTP_QUERY_STATUS_CODE | WINHTTP_QUERY_FLAG_NUMBER, WINHTTP_HEADER_NAME_BY_INDEX, &status_code, &status_size, WINHTTP_NO_HEADER_INDEX)) {
        error_message = L"Could not read response status.";
        WinHttpCloseHandle(request);
        WinHttpCloseHandle(connection);
        WinHttpCloseHandle(session);
        return false;
    }

    if (status_code == 404 || status_code == 405 || status_code == 501) {
        WinHttpCloseHandle(request);
        WinHttpCloseHandle(connection);
        WinHttpCloseHandle(session);
        return true;
    }

    if (status_code < 200 || status_code >= 300) {
        error_message = StatusToMessage(L"Remote file check failed", status_code);
        WinHttpCloseHandle(request);
        WinHttpCloseHandle(connection);
        WinHttpCloseHandle(session);
        return false;
    }

    wchar_t content_length[64] = {};
    DWORD content_length_size = sizeof(content_length);
    if (!WinHttpQueryHeaders(request, WINHTTP_QUERY_CONTENT_LENGTH, WINHTTP_HEADER_NAME_BY_INDEX, content_length, &content_length_size, WINHTTP_NO_HEADER_INDEX)) {
        WinHttpCloseHandle(request);
        WinHttpCloseHandle(connection);
        WinHttpCloseHandle(session);
        return true;
    }

    const ULONGLONG remote_size = _wcstoui64(content_length, nullptr, 10);
    if (remote_size != local_entry.size) {
        WinHttpCloseHandle(request);
        WinHttpCloseHandle(connection);
        WinHttpCloseHandle(session);
        return true;
    }

    wchar_t last_modified[128] = {};
    DWORD last_modified_size = sizeof(last_modified);
    if (!WinHttpQueryHeaders(request, WINHTTP_QUERY_LAST_MODIFIED, WINHTTP_HEADER_NAME_BY_INDEX, last_modified, &last_modified_size, WINHTTP_NO_HEADER_INDEX)) {
        WinHttpCloseHandle(request);
        WinHttpCloseHandle(connection);
        WinHttpCloseHandle(session);
        return true;
    }

    SYSTEMTIME remote_system_time{};
    FILETIME remote_file_time{};
    if (!WinHttpTimeToSystemTime(last_modified, &remote_system_time) || !SystemTimeToFileTime(&remote_system_time, &remote_file_time)) {
        WinHttpCloseHandle(request);
        WinHttpCloseHandle(connection);
        WinHttpCloseHandle(session);
        return true;
    }

    is_current = FileTimesMatchWithinSeconds(local_entry.last_write, remote_file_time, 2);

    WinHttpCloseHandle(request);
    WinHttpCloseHandle(connection);
    WinHttpCloseHandle(session);
    return true;
}

bool WebDavClient::ListRemoteFiles(FileSnapshot& snapshot, std::wstring& error_message) {
    snapshot.clear();

    static constexpr char kPropfindBody[] =
        "<?xml version=\"1.0\" encoding=\"utf-8\"?>"
        "<propfind xmlns=\"DAV:\">"
        "<prop><getcontentlength/><getlastmodified/><resourcetype/></prop>"
        "</propfind>";

    DWORD status_code = 0;
    std::vector<BYTE> response_body;
    if (!SendRequest(
            L"PROPFIND",
            L"",
            kPropfindBody,
            static_cast<DWORD>(sizeof(kPropfindBody) - 1),
            status_code,
            error_message,
            L"Depth: infinity\r\nContent-Type: text/xml; charset=utf-8\r\n",
            &response_body)) {
        return false;
    }

    if (!(status_code == 200 || status_code == 207)) {
        error_message = StatusToMessage(L"Remote listing failed", status_code);
        return false;
    }

    const std::wstring response_text = FromUtf8(std::string(response_body.begin(), response_body.end()));
    size_t response_position = 0;
    std::wstring response_block;
    while (NextResponseBlock(response_text, response_position, response_block)) {
        if (IsCollectionResponse(response_block)) {
            continue;
        }

        std::wstring href;
        if (!FindTagText(response_block, L"href", href)) {
            continue;
        }

        std::wstring relative_path;
        if (!TryGetRelativePathFromHref(XmlDecode(href), relative_path) || relative_path.empty()) {
            continue;
        }

        std::wstring content_length_text;
        ULONGLONG size = 0;
        if (FindTagText(response_block, L"getcontentlength", content_length_text) && !content_length_text.empty()) {
            size = _wcstoui64(content_length_text.c_str(), nullptr, 10);
        }

        FILETIME last_write{};
        std::wstring last_modified_text;
        if (FindTagText(response_block, L"getlastmodified", last_modified_text) && !last_modified_text.empty()) {
            SYSTEMTIME remote_system_time{};
            if (WinHttpTimeToSystemTime(last_modified_text.c_str(), &remote_system_time)) {
                SystemTimeToFileTime(&remote_system_time, &last_write);
            }
        }

        snapshot[relative_path] = FileEntry{size, last_write};
    }

    return true;
}

bool WebDavClient::DownloadFile(const std::wstring& relative_path, std::vector<BYTE>& data, std::wstring& error_message) {
    data.clear();

    DWORD status_code = 0;
    if (!SendRequest(L"GET", relative_path, nullptr, 0, status_code, error_message, nullptr, &data)) {
        return false;
    }

    if (status_code == 200) {
        return true;
    }

    error_message = StatusToMessage(L"Download failed", status_code);
    data.clear();
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
    std::wstring& error_message,
    const wchar_t* extra_headers,
    std::vector<BYTE>* response_body) {
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
    const DWORD flags = secure_ ? WINHTTP_FLAG_SECURE : 0;
    HINTERNET request = WinHttpOpenRequest(connection, method.c_str(), request_path.c_str(), nullptr, WINHTTP_NO_REFERER, WINHTTP_DEFAULT_ACCEPT_TYPES, flags);
    if (!request) {
        WinHttpCloseHandle(connection);
        WinHttpCloseHandle(session);
        error_message = L"Could not open HTTP request.";
        return false;
    }

    std::wstring headers = auth_header_;
    if (extra_headers) {
        headers += extra_headers;
    }

    const BOOL sent = WinHttpSendRequest(
        request,
        headers.empty() ? WINHTTP_NO_ADDITIONAL_HEADERS : headers.c_str(),
        headers.empty() ? 0 : static_cast<DWORD>(-1L),
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

    if (response_body) {
        response_body->clear();
        while (true) {
            DWORD available = 0;
            if (!WinHttpQueryDataAvailable(request, &available)) {
                error_message = L"Could not read response body.";
                WinHttpCloseHandle(request);
                WinHttpCloseHandle(connection);
                WinHttpCloseHandle(session);
                return false;
            }

            if (available == 0) {
                break;
            }

            const size_t offset = response_body->size();
            response_body->resize(offset + available);
            DWORD read = 0;
            if (!WinHttpReadData(request, response_body->data() + offset, available, &read)) {
                error_message = L"Could not read response body.";
                WinHttpCloseHandle(request);
                WinHttpCloseHandle(connection);
                WinHttpCloseHandle(session);
                return false;
            }

            response_body->resize(offset + read);
            if (read == 0) {
                break;
            }
        }
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

bool WebDavClient::EnsureCollectionExists(const std::wstring& full_path, std::wstring& error_message) {
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

std::wstring WebDavClient::UrlDecode(const std::wstring& value) const {
    std::string bytes;
    bytes.reserve(value.size());

    auto hex_value = [](wchar_t digit) -> int {
        if (digit >= L'0' && digit <= L'9') {
            return digit - L'0';
        }
        if (digit >= L'a' && digit <= L'f') {
            return 10 + digit - L'a';
        }
        if (digit >= L'A' && digit <= L'F') {
            return 10 + digit - L'A';
        }
        return -1;
    };

    for (size_t index = 0; index < value.size(); ++index) {
        const wchar_t ch = value[index];
        if (ch == L'%' && index + 2 < value.size()) {
            const int high = hex_value(value[index + 1]);
            const int low = hex_value(value[index + 2]);
            if (high >= 0 && low >= 0) {
                bytes.push_back(static_cast<char>((high << 4) | low));
                index += 2;
                continue;
            }
        }

        if (ch == L'+') {
            bytes.push_back(' ');
        } else if (ch <= 0x7F) {
            bytes.push_back(static_cast<char>(ch));
        } else {
            bytes.append(ToUtf8(std::wstring(1, ch)));
        }
    }

    return FromUtf8(bytes);
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

bool WebDavClient::TryGetRelativePathFromHref(const std::wstring& href, std::wstring& relative_path) const {
    relative_path.clear();

    std::wstring request_path = href;
    URL_COMPONENTSW components{};
    components.dwStructSize = sizeof(components);
    wchar_t path[2048] = {};
    components.lpszUrlPath = path;
    components.dwUrlPathLength = _countof(path);
    if (WinHttpCrackUrl(request_path.c_str(), 0, 0, &components) && components.dwUrlPathLength > 0) {
        request_path.assign(components.lpszUrlPath, components.dwUrlPathLength);
    } else {
        const size_t query_index = request_path.find_first_of(L"?#");
        if (query_index != std::wstring::npos) {
            request_path.erase(query_index);
        }
    }

    std::wstring base_request_path = BuildRequestPath(L"");
    if (base_request_path.size() > 1 && base_request_path.back() == L'/') {
        base_request_path.pop_back();
    }

    if (request_path == base_request_path || request_path == base_request_path + L"/") {
        return true;
    }

    const std::wstring base_prefix = base_request_path == L"/" ? base_request_path : base_request_path + L"/";
    if (request_path.rfind(base_prefix, 0) != 0) {
        return false;
    }

    relative_path = UrlDecode(request_path.substr(base_prefix.size()));
    while (!relative_path.empty() && relative_path.front() == L'/') {
        relative_path.erase(relative_path.begin());
    }
    while (!relative_path.empty() && relative_path.back() == L'/') {
        relative_path.pop_back();
    }
    for (wchar_t& ch : relative_path) {
        if (ch == L'/') {
            ch = L'\\';
        }
    }

    return true;
}

bool WebDavClient::ListRemoteFolder(std::vector<WebDavFolderInfo>& folders, std::wstring& error_message) {
    folders.clear();

    static constexpr char kPropfindBody[] =
        "<?xml version=\"1.0\" encoding=\"utf-8\"?>"
        "<propfind xmlns=\"DAV:\">"
        "<prop><resourcetype/><displayname/></prop>"
        "</propfind>";

    DWORD status_code = 0;
    std::vector<BYTE> response_body;
    if (!SendRequest(
            L"PROPFIND",
            L"",
            kPropfindBody,
            static_cast<DWORD>(sizeof(kPropfindBody) - 1),
            status_code,
            error_message,
            L"Depth: 1\r\nContent-Type: text/xml; charset=utf-8\r\n",
            &response_body)) {
        return false;
    }

    if (!(status_code == 200 || status_code == 207)) {
        error_message = StatusToMessage(L"Remote listing failed", status_code);
        return false;
    }

    const std::wstring response_text = FromUtf8(std::string(response_body.begin(), response_body.end()));
    size_t response_position = 0;
    std::wstring response_block;
    
    while (NextResponseBlock(response_text, response_position, response_block)) {
        std::wstring href;
        if (!FindTagText(response_block, L"href", href)) {
            continue;
        }

        std::wstring relative_path;
        if (!TryGetRelativePathFromHref(href, relative_path)) {
            continue;
        }

        // Skip the root folder itself
        if (relative_path.empty()) {
            continue;
        }

        std::wstring resource_type;
        FindTagText(response_block, L"resourcetype", resource_type);
        
        std::wstring display_name;
        FindTagText(response_block, L"displayname", display_name);
        if (display_name.empty()) {
            // Extract from relative path
            size_t last_slash = relative_path.find_last_of(L"/\\");
            if (last_slash != std::wstring::npos) {
                display_name = relative_path.substr(last_slash + 1);
            } else {
                display_name = relative_path;
            }
        }

        bool is_collection = false;
        std::wstring ignored;
        if (FindTagText(resource_type, L"collection", ignored)) {
            is_collection = true;
        }

        WebDavFolderInfo info;
        info.display_name = XmlDecode(display_name);
        info.full_path = relative_path;
        info.is_collection = is_collection;
        folders.push_back(info);
    }

    return true;
}

bool WebDavClient::CreateFolder(const std::wstring& folder_name, std::wstring& error_message) {
    if (folder_name.empty()) {
        error_message = L"Folder name is required.";
        return false;
    }

    // Build full path
    std::wstring full_path = folder_name;
    // Ensure it ends with /
    if (!full_path.empty() && full_path.back() != L'/' && full_path.back() != L'\\') {
        full_path += L'/';
    }

    DWORD status_code = 0;
    if (!SendRequest(L"MKCOL", full_path, nullptr, 0, status_code, error_message)) {
        return false;
    }

    if (status_code == 201 || status_code == 200 || status_code == 204) {
        return true;
    }

    if (status_code == 405 || status_code == 409) {
        error_message = L"Folder may already exist or path is invalid.";
    } else {
        error_message = StatusToMessage(L"Create folder failed", status_code);
    }
    
    return false;
}
