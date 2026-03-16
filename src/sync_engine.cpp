#include "sync_engine.h"

#include "file_scanner.h"
#include "webdav_client.h"

#include <chrono>
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
    if (worker_.joinable()) {
        worker_.join();
    }
}

void SyncEngine::SyncNow() {
    force_sync_ = true;
}

void SyncEngine::WorkerLoop() {
    FileSnapshot previous;

    while (running_) {
        if (!force_sync_) {
            for (int i = 0; i < 10 && running_ && !force_sync_; ++i) {
                std::this_thread::sleep_for(std::chrono::seconds(1));
            }
        }
        if (!running_) {
            break;
        }
        force_sync_ = false;

        if (status_fn_) {
            status_fn_(SyncState::Syncing, L"Scanning folder...");
        }

        FileSnapshot current;
        if (!BuildSnapshot(config_.watch_folder, current)) {
            if (log_fn_) {
                log_fn_(L"Watch folder is not available.");
            }
            if (status_fn_) {
                status_fn_(SyncState::Error, L"Folder not found");
            }
            continue;
        }

        WebDavClient client(config_);
        bool had_error = false;

        for (const auto& pair : current) {
            const auto found = previous.find(pair.first);
            if (found != previous.end() && SameFileEntry(found->second, pair.second)) {
                continue;
            }

            std::wstring error_message;
            const std::wstring local_path = JoinPath(config_.watch_folder, pair.first);
            if (client.UploadFile(local_path, pair.first, error_message)) {
                if (log_fn_) {
                    log_fn_(L"Uploaded: " + pair.first);
                }
            } else {
                had_error = true;
                if (log_fn_) {
                    log_fn_(L"Upload failed: " + pair.first + L" - " + error_message);
                }
            }
        }

        if (config_.sync_deletes) {
            for (const auto& pair : previous) {
                if (current.find(pair.first) != current.end()) {
                    continue;
                }

                std::wstring error_message;
                if (client.DeleteFile(pair.first, error_message)) {
                    if (log_fn_) {
                        log_fn_(L"Deleted remote file: " + pair.first);
                    }
                } else {
                    had_error = true;
                    if (log_fn_) {
                        log_fn_(L"Delete failed: " + pair.first + L" - " + error_message);
                    }
                }
            }
        }

        previous = current;
        if (status_fn_) {
            status_fn_(had_error ? SyncState::Error : SyncState::Idle, had_error ? L"Sync finished with errors" : L"Up to date");
        }
    }
}
