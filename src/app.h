#pragma once

#include "config.h"
#include "sync_engine.h"
#include <windows.h>
#include <string>

enum class ConnectionState {
    NotConnected,
    Connecting,
    Connected,
    Failed,
};

class App {
public:
    App(HINSTANCE instance, int show_command);
    ~App();
    int Run();

private:
    static LRESULT CALLBACK WndProc(HWND hwnd, UINT message, WPARAM wparam, LPARAM lparam);

    bool CreateMainWindow();
    void CreateControls();
    void LoadIntoControls();
    void SaveFromControls();
    void ShowSettings(bool show);
    void StartSync();
    void StopSync();
    void ApplyStartupSetting();
    void UpdateStatus(SyncState state, const std::wstring& text);
    void UpdateStatusLabel(const std::wstring& text);
    void UpdateTrayIcon(SyncState state);
    void AddTrayIcon();
    void RemoveTrayIcon();
    void ShowTrayMenu();
    void Log(const std::wstring& message);
    void OpenLogFolder();
    void OpenWebDavUrl();
    void BrowseForWatchFolder();
    bool Connect();
    bool ValidateConfig(std::wstring& error_message);
    void SetConnectionState(ConnectionState state, const std::wstring& text = L"");
    std::wstring GetControlText(int control_id) const;
    void SetControlText(int control_id, const std::wstring& value);
    void SetCheck(int control_id, bool checked);
    bool GetCheck(int control_id) const;
    std::wstring GetLogPath() const;
    void HandleCommand(int control_id);
    void HandleTrayAction(UINT action);
    LRESULT HandleMessage(HWND hwnd, UINT message, WPARAM wparam, LPARAM lparam);

    HINSTANCE instance_ = nullptr;
    int show_command_ = SW_SHOWDEFAULT;
    HWND hwnd_ = nullptr;
    HICON large_icon_ = nullptr;
    HICON idle_icon_ = nullptr;
    HICON syncing_icon_ = nullptr;
    HICON error_icon_ = nullptr;
    HICON open_url_icon_ = nullptr;
    SyncState sync_state_ = SyncState::Idle;
    ConnectionState connection_state_ = ConnectionState::NotConnected;
    std::wstring connection_text_ = L"Not connected";
    AppConfig config_;
    SyncEngine engine_;
    bool tray_added_ = false;
};
