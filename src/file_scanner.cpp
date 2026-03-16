#include "file_scanner.h"

#include <windows.h>

namespace {

bool ScanDirectory(const std::wstring& root, const std::wstring& relative, FileSnapshot& snapshot) {
    const std::wstring search_path = root + L"\\" + relative + L"*";

    WIN32_FIND_DATAW find_data{};
    HANDLE handle = FindFirstFileW(search_path.c_str(), &find_data);
    if (handle == INVALID_HANDLE_VALUE) {
        return false;
    }

    do {
        const std::wstring name = find_data.cFileName;
        if (name == L"." || name == L"..") {
            continue;
        }

        const std::wstring child_relative = relative + name;
        if (find_data.dwFileAttributes & FILE_ATTRIBUTE_DIRECTORY) {
            if (!ScanDirectory(root, child_relative + L"\\", snapshot)) {
                FindClose(handle);
                return false;
            }
            continue;
        }

        FileEntry entry;
        entry.size = (static_cast<ULONGLONG>(find_data.nFileSizeHigh) << 32) | find_data.nFileSizeLow;
        entry.last_write = find_data.ftLastWriteTime;
        snapshot[child_relative] = entry;
    } while (FindNextFileW(handle, &find_data));

    FindClose(handle);
    return true;
}

} // namespace

bool BuildSnapshot(const std::wstring& root, FileSnapshot& snapshot) {
    snapshot.clear();

    DWORD attributes = GetFileAttributesW(root.c_str());
    if (attributes == INVALID_FILE_ATTRIBUTES || !(attributes & FILE_ATTRIBUTE_DIRECTORY)) {
        return false;
    }

    return ScanDirectory(root, L"", snapshot);
}
