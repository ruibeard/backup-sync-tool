#pragma once

#include "config.h"
#include "file_scanner.h"
#include <windows.h>
#include <atomic>
#include <functional>
#include <mutex>
#include <string>
#include <thread>

enum class SyncState {
    Idle,
    Syncing,
    Error
};

class SyncEngine {
public:
    using LogFn = std::function<void(const std::wstring&)>;
    using StatusFn = std::function<void(SyncState, const std::wstring&, int, int)>;

    SyncEngine();
    ~SyncEngine();

    void Start(const AppConfig& config, LogFn log_fn, StatusFn status_fn);
    void Stop();
    void SyncNow();

private:
    void WorkerLoop();
    void PerformSync(FileSnapshot& previous_local, FileSnapshot& previous_remote);

    AppConfig config_;
    LogFn log_fn_;
    StatusFn status_fn_;
    std::thread worker_;
    std::mutex mutex_;
    std::atomic<bool> running_{false};
    std::atomic<bool> force_sync_{false};
    HANDLE wake_event_ = nullptr;
};
