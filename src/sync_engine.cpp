#include "sync_engine.h"

#include "file_scanner.h"
#include "webdav_client.h"

#include <chrono>
#include <sstream>
#include <vector>
#include <utility>

namespace {

bool SameFileEntry(const FileEntry& left, const FileEntry& right) {
    return left.size == right.size &&
           left.last_write.dwLowDateTime == right.last_write.dwLowDateTime &&
           left.last_write.dwHighDateTime == right.last_write.dwHighDateTime;
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

void SyncEngine::PerformSync(FileSnapshot& previous) {
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
    int uploaded_count = 0;
    std::vector<PendingUpload> pending_uploads;

    for (const auto& pair : current) {
        const auto found = previous.find(pair.first);
        if (found != previous.end() && SameFileEntry(found->second, pair.second)) {
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

    const int total_uploads = static_cast<int>(pending_uploads.size());
    if (status_fn_) {
        if (total_uploads > 0) {
            status_fn_(SyncState::Syncing, L"Uploading files...", 0, total_uploads);
        } else {
            status_fn_(SyncState::Idle, L"Watching for changes", 0, 0);
        }
    }

    for (size_t index = 0; index < pending_uploads.size(); ++index) {
        const auto& pending = pending_uploads[index];
        std::wstringstream progress_text;
        progress_text << L"Uploading " << (index + 1) << L" of " << total_uploads;
        if (status_fn_) {
            status_fn_(SyncState::Syncing, progress_text.str(), static_cast<int>(index), total_uploads);
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

        if (status_fn_) {
            status_fn_(SyncState::Syncing, progress_text.str(), static_cast<int>(index + 1), total_uploads);
        }
    }
    previous = current;
    if (status_fn_) {
        std::wstringstream status;
        if (had_error) {
            status << L"Sync finished with errors";
            if (uploaded_count > 0) {
                status << L" (" << uploaded_count << L" uploaded)";
            }
            status_fn_(SyncState::Error, status.str(), total_uploads, total_uploads);
        } else if (uploaded_count > 0) {
            status << L"Watching for changes";
            status << L" (" << uploaded_count << L" uploaded)";
            status_fn_(SyncState::Idle, status.str(), total_uploads, total_uploads);
        } else {
            status_fn_(SyncState::Idle, L"Watching for changes", 0, 0);
        }
    }
}

void SyncEngine::WorkerLoop() {
    FileSnapshot previous;

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

    PerformSync(previous);

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
                PerformSync(previous);
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
            PerformSync(previous);
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
