#pragma once

#include <string>

struct AppConfig {
    std::wstring watch_folder;
    std::wstring webdav_url;
    std::wstring username;
    std::wstring password;
    std::wstring remote_folder;
    bool start_with_windows = true;
    bool download_remote_changes = false;
};

std::wstring GetExecutableDirectory();
std::wstring GetConfigPath();
bool LoadConfig(AppConfig& config);
bool SaveConfig(const AppConfig& config);
bool HasUsableConfig(const AppConfig& config);
