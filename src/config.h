#pragma once

#include <string>

struct AppConfig {
    std::wstring watch_folder;
    std::wstring webdav_url;
    std::wstring username;
    std::wstring password;
    bool start_with_windows = false;
    bool sync_deletes = false;
};

std::wstring GetExecutableDirectory();
std::wstring GetConfigPath();
bool LoadConfig(AppConfig& config);
bool SaveConfig(const AppConfig& config);
bool HasUsableConfig(const AppConfig& config);
