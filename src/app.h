#pragma once

#include "config.h"
#include "sync_engine.h"
#include <string>

class App {
public:
    App(HINSTANCE instance, int show_command);
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
    void UpdateStatusLabel(const std::wstring& text);
    void AddTrayIcon();
    void RemoveTrayIcon();
    void ShowTrayMenu();
    void Log(const std::wstring& message);
    void OpenLogFolder();
    bool TestConnection();
    bool ValidateConfig(std::wstring& error_message);
    std::wstring GetControlText(int control_id) const;
    void SetControlText(int control_id, const std::wstring& value);
    void SetCheck(int control_id, bool checked);
    bool GetCheck(int control_id) const;
    std::wstring GetLogPath() const;
    void HandleCommand(int control_id);
    void HandleTrayAction(UINT action);
    LRESULT HandleMessage(UINT message, WPARAM wparam, LPARAM lparam);

    HINSTANCE instance_ = nullptr;
    int show_command_ = SW_SHOWDEFAULT;
    HWND hwnd_ = nullptr;
    AppConfig config_;
    SyncEngine engine_;
    bool tray_added_ = false;
};
