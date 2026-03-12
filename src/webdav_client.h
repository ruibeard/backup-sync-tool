#pragma once

#include "config.h"
#include <string>

class WebDavClient {
public:
    explicit WebDavClient(const AppConfig& config);

    bool TestConnection(std::wstring& error_message);
    bool UploadFile(const std::wstring& local_path, const std::wstring& relative_path, std::wstring& error_message);
    bool DeleteFile(const std::wstring& relative_path, std::wstring& error_message);

private:
    bool SendRequest(
        const std::wstring& method,
        const std::wstring& relative_path,
        const void* body,
        DWORD body_size,
        DWORD& status_code,
        std::wstring& error_message);
    bool EnsureRemoteFolders(const std::wstring& relative_path, std::wstring& error_message);
    std::wstring BuildRequestPath(const std::wstring& relative_path) const;
    std::wstring UrlEncode(const std::wstring& value) const;
    std::wstring StatusToMessage(const std::wstring& operation, DWORD status_code) const;

    bool secure_ = true;
    INTERNET_PORT port_ = 0;
    std::wstring host_;
    std::wstring base_path_;
    std::wstring auth_header_;
};
