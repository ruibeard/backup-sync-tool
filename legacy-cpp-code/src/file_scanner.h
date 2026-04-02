#pragma once

#include <windows.h>
#include <map>
#include <string>

struct FileEntry {
    ULONGLONG size = 0;
    FILETIME last_write = {};
};

using FileSnapshot = std::map<std::wstring, FileEntry>;

bool BuildSnapshot(const std::wstring& root, FileSnapshot& snapshot);
