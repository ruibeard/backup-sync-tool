#include "sync_engine.h"

#include "file_scanner.h"
#include "webdav_client.h"

#include <chrono>
#include <set>
#include <shlobj.h>
#include <sstream>
#include <utility>
#include <vector>

namespace {

bool SameFileEntry(const FileEntry& left, const FileEntry& right) {
    return left.size == right.size &&
           left.last_write.dwLowDateTime == right.last_write.dwLowDateTime &&
           left.last_write.dwHighDateTime == right.last_write.dwHighDateTime;
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

bool EquivalentEntries(const FileEntry& left, const FileEntry& right) {
    return left.size == right.size && FileTimesMatchWithinSeconds(left.last_write, right.last_write, 2);
}

std::wstring JoinPath(const std::wstring& left, const std::wstring& right) {
    if (left.empty()) {
        return right;
    }
    if (left.back() == L'\\') {
        return left + right;
    }
    return left + L"\\" + right;
}

struct PendingUpload {
    std::wstring relative_path;
};

struct PendingDownload {
    std::wstring relative_path;
    FileEntry remote_entry;
};

bool SnapshotEntryChanged(const FileSnapshot& previous, const std::wstring& relative_path, const FileEntry& current) {
    const auto found = previous.find(relative_path);
    return found == previous.end() || !SameFileEntry(found->second, current);
}

std::wstring GetParentPath(const std::wstring& path) {
    const size_t separator = path.find_last_of(L"\\/");
    if (separator == std::wstring::npos) {
        return L"";
    }
    return path.substr(0, separator);
}

bool WriteLocalFile(const std::wstring& local_path, const std::vector<BYTE>& data, const FILETIME& last_write, std::wstring& error_message) {
    const std::wstring parent_path = GetParentPath(local_path);
    if (!parent_path.empty()) {
        const int directory_result = SHCreateDirectoryExW(nullptr, parent_path.c_str(), nullptr);
        if (!(directory_result == ERROR_SUCCESS || directory_result == ERROR_ALREADY_EXISTS || directory_result == ERROR_FILE_EXISTS)) {
            error_message = L"Could not create local folder.";
            return false;
        }
    }

    const std::wstring temp_path = local_path + L".tmp";
    HANDLE file = CreateFileW(temp_path.c_str(), GENERIC_WRITE, 0, nullptr, CREATE_ALWAYS, FILE_ATTRIBUTE_NORMAL, nullptr);
    if (file == INVALID_HANDLE_VALUE) {
        error_message = L"Could not create local file.";
        return false;
    }

    DWORD written = 0;
    const BOOL write_ok = data.empty() ? TRUE : WriteFile(file, data.data(), static_cast<DWORD>(data.size()), &written, nullptr);
    const bool has_last_write = last_write.dwLowDateTime != 0 || last_write.dwHighDateTime != 0;
    const BOOL time_ok = has_last_write ? SetFileTime(file, nullptr, nullptr, &last_write) : TRUE;
    CloseHandle(file);

    if (!write_ok || written != data.size() || !time_ok) {
        DeleteFileW(temp_path.c_str());
        error_message = L"Could not write downloaded file.";
        return false;
    }

    if (!MoveFileExW(temp_path.c_str(), local_path.c_str(), MOVEFILE_REPLACE_EXISTING)) {
        DeleteFileW(temp_path.c_str());
        error_message = L"Could not replace local file.";
        return false;
    }

    return true;
}

} // namespace

SyncEngine::SyncEngine() = default;

SyncEngine::~SyncEngine() {
    Stop();
}

void SyncEngine::Start(const AppConfig& config, LogFn log_fn, StatusFn status_fn) {
    Stop();

    config_ = config;
    log_fn_ = std::move(log_fn);
    status_fn_ = std::move(status_fn);
    running_ = true;
    force_sync_ = true;
    worker_ = std::thread(&SyncEngine::WorkerLoop, this);
}

void SyncEngine::Stop() {
    running_ = false;
    force_sync_ = true;
    {
        std::lock_guard<std::mutex> lock(mutex_);
        if (wake_event_) {
            SetEvent(wake_event_);
        }
    }
    if (worker_.joinable()) {
        worker_.join();
    }
}

void SyncEngine::SyncNow() {
    force_sync_ = true;
    std::lock_guard<std::mutex> lock(mutex_);
    if (wake_event_) {
        SetEvent(wake_event_);
    }
}

void SyncEngine::PerformSync(FileSnapshot& previous_local, FileSnapshot& previous_remote) {
    force_sync_ = false;

    if (status_fn_) {
        status_fn_(SyncState::Syncing, L"Checking remote files...", 0, 0);
    }

    FileSnapshot current;
    if (!BuildSnapshot(config_.watch_folder, current)) {
        if (log_fn_) {
            log_fn_(L"Watch folder is not available.");
        }
        if (status_fn_) {
            status_fn_(SyncState::Error, L"Folder not found", 0, 0);
        }
        return;
    }

    WebDavClient client(config_);
    bool had_error = false;
    int downloaded_count = 0;
    int uploaded_count = 0;
    bool remote_snapshot_available = false;
    FileSnapshot remote_current;
    std::vector<PendingDownload> pending_downloads;
    std::vector<PendingUpload> pending_uploads;
    std::set<std::wstring> conflict_paths;

    if (config_.download_remote_changes) {
        std::wstring error_message;
        if (client.ListRemoteFiles(remote_current, error_message)) {
            remote_snapshot_available = true;
        } else {
            had_error = true;
            if (log_fn_) {
                log_fn_(L"Remote listing failed: " + error_message);
            }
        }
    }

    if (remote_snapshot_available) {
        for (const auto& pair : remote_current) {
            const auto local_found = current.find(pair.first);
            if (local_found == current.end()) {
                pending_downloads.push_back({pair.first, pair.second});
                continue;
            }

            if (EquivalentEntries(local_found->second, pair.second)) {
                continue;
            }

            const bool local_changed = SnapshotEntryChanged(previous_local, pair.first, local_found->second);
            const bool remote_changed = SnapshotEntryChanged(previous_remote, pair.first, pair.second);
            if (remote_changed && !local_changed) {
                pending_downloads.push_back({pair.first, pair.second});
                continue;
            }

            if (remote_changed && local_changed) {
                conflict_paths.insert(pair.first);
                if (log_fn_) {
                    log_fn_(L"Conflict skipped: " + pair.first + L" changed locally and remotely.");
                }
            }
        }
    }

    for (const auto& pair : current) {
        if (remote_snapshot_available) {
            const auto remote_found = remote_current.find(pair.first);
            if (remote_found != remote_current.end() && EquivalentEntries(remote_found->second, pair.second)) {
                continue;
            }

            const bool local_changed = SnapshotEntryChanged(previous_local, pair.first, pair.second);
            const bool remote_changed = remote_found != remote_current.end() && SnapshotEntryChanged(previous_remote, pair.first, remote_found->second);
            if (remote_found == remote_current.end() || (local_changed && !remote_changed)) {
                pending_uploads.push_back({pair.first});
                continue;
            }

            if (local_changed && remote_changed) {
                conflict_paths.insert(pair.first);
            }
            continue;
        }

        const auto found = previous_local.find(pair.first);
        if (found != previous_local.end() && SameFileEntry(found->second, pair.second)) {
            continue;
        }

        std::wstring error_message;
        bool is_current = false;
        if (!client.IsRemoteFileCurrent(pair.first, pair.second, is_current, error_message)) {
            had_error = true;
            if (log_fn_) {
                log_fn_(L"Remote check failed: " + pair.first + L" - " + error_message);
            }
            continue;
        }

        if (is_current) {
            if (log_fn_) {
                log_fn_(L"Skipped existing: " + pair.first);
            }
            continue;
        }

        pending_uploads.push_back({pair.first});
    }

    const int total_downloads = static_cast<int>(pending_downloads.size());
    const int total_uploads = static_cast<int>(pending_uploads.size());
    const int total_operations = total_downloads + total_uploads;
    if (status_fn_) {
        if (total_operations > 0) {
            status_fn_(SyncState::Syncing, total_downloads > 0 ? L"Downloading files..." : L"Uploading files...", 0, total_operations);
        } else {
            status_fn_(SyncState::Idle, L"Watching for changes", 0, 0);
        }
    }

    int completed_operations = 0;

    for (size_t index = 0; index < pending_downloads.size(); ++index) {
        const auto& pending = pending_downloads[index];
        std::wstringstream progress_text;
        progress_text << L"Downloading " << (index + 1) << L" of " << total_downloads;
        if (status_fn_) {
            status_fn_(SyncState::Syncing, progress_text.str(), completed_operations, total_operations);
        }

        std::vector<BYTE> data;
        std::wstring error_message;
        if (client.DownloadFile(pending.relative_path, data, error_message) &&
            WriteLocalFile(JoinPath(config_.watch_folder, pending.relative_path), data, pending.remote_entry.last_write, error_message)) {
            ++downloaded_count;
            if (log_fn_) {
                log_fn_(L"Downloaded: " + pending.relative_path);
            }
        } else {
            had_error = true;
            if (log_fn_) {
                log_fn_(L"Download failed: " + pending.relative_path + L" - " + error_message);
            }
        }

        ++completed_operations;
        if (status_fn_) {
            status_fn_(SyncState::Syncing, progress_text.str(), completed_operations, total_operations);
        }
    }

    for (size_t index = 0; index < pending_uploads.size(); ++index) {
        const auto& pending = pending_uploads[index];
        std::wstringstream progress_text;
        progress_text << L"Uploading " << (index + 1) << L" of " << total_uploads;
        if (status_fn_) {
            status_fn_(SyncState::Syncing, progress_text.str(), completed_operations, total_operations);
        }

        std::wstring error_message;
        const std::wstring local_path = JoinPath(config_.watch_folder, pending.relative_path);
        if (client.UploadFile(local_path, pending.relative_path, error_message)) {
            ++uploaded_count;
            if (log_fn_) {
                log_fn_(L"Uploaded: " + pending.relative_path);
            }
        } else {
            had_error = true;
            if (log_fn_) {
                log_fn_(L"Upload failed: " + pending.relative_path + L" - " + error_message);
            }
        }

        ++completed_operations;
        if (status_fn_) {
            status_fn_(SyncState::Syncing, progress_text.str(), completed_operations, total_operations);
        }
    }

    FileSnapshot refreshed_local;
    if (BuildSnapshot(config_.watch_folder, refreshed_local)) {
        previous_local = std::move(refreshed_local);
    } else {
        previous_local = current;
    }

    if (config_.download_remote_changes) {
        FileSnapshot refreshed_remote;
        std::wstring error_message;
        if (client.ListRemoteFiles(refreshed_remote, error_message)) {
            previous_remote = std::move(refreshed_remote);
        } else if (remote_snapshot_available) {
            previous_remote = remote_current;
        } else {
            previous_remote.clear();
        }
    } else {
        previous_remote.clear();
    }

    if (status_fn_) {
        std::wstringstream status;
        if (had_error) {
            status << L"Sync finished with errors";
            if (downloaded_count > 0 || uploaded_count > 0) {
                status << L" (";
                if (downloaded_count > 0) {
                    status << downloaded_count << L" downloaded";
                }
                if (downloaded_count > 0 && uploaded_count > 0) {
                    status << L", ";
                }
                if (uploaded_count > 0) {
                    status << uploaded_count << L" uploaded";
                }
                status << L")";
            }
            if (!conflict_paths.empty()) {
                status << L" - conflicts skipped";
            }
            status_fn_(SyncState::Error, status.str(), total_operations, total_operations);
        } else if (downloaded_count > 0 || uploaded_count > 0 || !conflict_paths.empty()) {
            status << L"Watching for changes";
            if (downloaded_count > 0 || uploaded_count > 0) {
                status << L" (";
                if (downloaded_count > 0) {
                    status << downloaded_count << L" downloaded";
                }
                if (downloaded_count > 0 && uploaded_count > 0) {
                    status << L", ";
                }
                if (uploaded_count > 0) {
                    status << uploaded_count << L" uploaded";
                }
                status << L")";
            }
            if (!conflict_paths.empty()) {
                status << L" - conflicts skipped";
            }
            status_fn_(SyncState::Idle, status.str(), total_operations, total_operations);
        } else {
            status_fn_(SyncState::Idle, L"Watching for changes", 0, 0);
        }
    }
}

void SyncEngine::WorkerLoop() {
    FileSnapshot previous_local;
    FileSnapshot previous_remote;

    HANDLE wake_event = CreateEventW(nullptr, TRUE, FALSE, nullptr);
    if (!wake_event) {
        if (status_fn_) {
            status_fn_(SyncState::Error, L"Watcher startup failed", 0, 0);
        }
        return;
    }

    HANDLE watch = FindFirstChangeNotificationW(
        config_.watch_folder.c_str(),
        TRUE,
        FILE_NOTIFY_CHANGE_FILE_NAME |
            FILE_NOTIFY_CHANGE_DIR_NAME |
            FILE_NOTIFY_CHANGE_ATTRIBUTES |
            FILE_NOTIFY_CHANGE_SIZE |
            FILE_NOTIFY_CHANGE_LAST_WRITE |
            FILE_NOTIFY_CHANGE_CREATION);

    {
        std::lock_guard<std::mutex> lock(mutex_);
        wake_event_ = wake_event;
    }

    PerformSync(previous_local, previous_remote);

    if (watch == INVALID_HANDLE_VALUE) {
        if (log_fn_) {
            log_fn_(L"Could not start folder watcher.");
        }
        if (status_fn_) {
            status_fn_(SyncState::Error, L"Folder watcher unavailable", 0, 0);
        }
    } else {
        HANDLE wait_handles[] = {watch, wake_event};

        while (running_) {
            if (force_sync_) {
                PerformSync(previous_local, previous_remote);
                continue;
            }

            const DWORD wait_result = WaitForMultipleObjects(2, wait_handles, FALSE, INFINITE);
            if (!running_) {
                break;
            }

            if (wait_result == WAIT_OBJECT_0 + 1) {
                ResetEvent(wake_event);
                continue;
            }

            if (wait_result != WAIT_OBJECT_0) {
                if (status_fn_) {
                    status_fn_(SyncState::Error, L"Folder watcher failed", 0, 0);
                }
                break;
            }

            if (!FindNextChangeNotification(watch)) {
                if (status_fn_) {
                    status_fn_(SyncState::Error, L"Folder watcher failed", 0, 0);
                }
                break;
            }

            std::this_thread::sleep_for(std::chrono::milliseconds(150));
            PerformSync(previous_local, previous_remote);
        }
    }

    {
        std::lock_guard<std::mutex> lock(mutex_);
        wake_event_ = nullptr;
    }

    if (watch != INVALID_HANDLE_VALUE) {
        FindCloseChangeNotification(watch);
    }
    CloseHandle(wake_event);
}
