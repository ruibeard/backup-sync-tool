#pragma once

#include <windows.h>
#include <string>
#include <vector>

#include "webdav_client.h"

struct AppConfig;

class WebDavFolderDialog {
public:
    static bool Show(HWND parent, AppConfig& config, std::wstring& selected_folder);

private:
    static LRESULT CALLBACK DialogProc(HWND hwnd, UINT message, WPARAM wparam, LPARAM lparam);
    LRESULT HandleMessage(HWND hwnd, UINT message, WPARAM wparam, LPARAM lparam);
    
    void InitializeDialog(HWND hwnd);
    void LoadFolders();
    void CreateNewFolder();
    void OnFolderSelected();
    void UpdateStatus(const std::wstring& message);
    
    HWND hwnd_ = nullptr;
    HWND folder_list_ = nullptr;
    HWND new_folder_edit_ = nullptr;
    HWND status_label_ = nullptr;
    AppConfig* config_ = nullptr;
    std::wstring* selected_folder_ = nullptr;
    std::vector<WebDavFolderInfo> folders_;
    HINSTANCE instance_ = nullptr;
    int result_ = IDCANCEL;

    static WebDavFolderDialog* current_dialog_;
};
