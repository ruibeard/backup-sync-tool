#pragma once

#include "config.h"
#include "file_scanner.h"
#include <windows.h>
#include <winhttp.h>
#include <string>
#include <vector>

class WebDavClient {
public:
    explicit WebDavClient(const AppConfig& config);

    bool TestConnection(std::wstring& error_message);
    bool IsRemoteFileCurrent(const std::wstring& relative_path, const FileEntry& local_entry, bool& is_current, std::wstring& error_message);
    bool ListRemoteFiles(FileSnapshot& snapshot, std::wstring& error_message);
    bool DownloadFile(const std::wstring& relative_path, std::vector<BYTE>& data, std::wstring& error_message);
    bool UploadFile(const std::wstring& local_path, const std::wstring& relative_path, std::wstring& error_message);
    bool DeleteFile(const std::wstring& relative_path, std::wstring& error_message);

private:
    bool SendRequest(
        const std::wstring& method,
        const std::wstring& relative_path,
        const void* body,
        DWORD body_size,
        DWORD& status_code,
        std::wstring& error_message,
        const wchar_t* extra_headers = nullptr,
        std::vector<BYTE>* response_body = nullptr);
    bool EnsureCollectionExists(
        const std::wstring& full_path,
        std::wstring& error_message);
    bool EnsureBaseCollectionExists(std::wstring& error_message);
    bool EnsureRemoteFolders(const std::wstring& relative_path, std::wstring& error_message);
    std::wstring BuildRequestPath(const std::wstring& relative_path) const;
    std::wstring UrlDecode(const std::wstring& value) const;
    std::wstring UrlEncode(const std::wstring& value) const;
    std::wstring StatusToMessage(const std::wstring& operation, DWORD status_code) const;
    bool TryGetRelativePathFromHref(const std::wstring& href, std::wstring& relative_path) const;

    bool secure_ = true;
    INTERNET_PORT port_ = 0;
    std::wstring host_;
    std::wstring base_path_;
    std::wstring auth_header_;
};
