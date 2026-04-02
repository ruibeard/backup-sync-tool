#include "webdav_folder_dialog.h"
#include "config.h"
#include "webdav_client.h"

#include <windows.h>
#include <commctrl.h>
#include <strsafe.h>
#include <algorithm>

WebDavFolderDialog* WebDavFolderDialog::current_dialog_ = nullptr;

namespace {

constexpr int IDC_FOLDER_LIST = 1001;
constexpr int IDC_NEW_FOLDER_EDIT = 1002;
constexpr int IDC_CREATE_FOLDER_BTN = 1003;
constexpr int IDC_STATUS_LABEL = 1004;
constexpr int IDC_REFRESH_BTN = 1005;
}

static const wchar_t* kFolderDialogClass = L"WebDavFolderDialogClass";

bool WebDavFolderDialog::Show(HWND parent, AppConfig& config, std::wstring& selected_folder) {
    WebDavFolderDialog dialog;
    dialog.config_ = &config;
    dialog.selected_folder_ = &selected_folder;
    dialog.instance_ = GetModuleHandleW(nullptr);

    // Register window class (idempotent)
    WNDCLASSEXW wc{};
    wc.cbSize = sizeof(wc);
    wc.lpfnWndProc = DialogProc;
    wc.hInstance = dialog.instance_;
    wc.hCursor = LoadCursorW(nullptr, IDC_ARROW);
    wc.hbrBackground = reinterpret_cast<HBRUSH>(COLOR_BTNFACE + 1);
    wc.lpszClassName = kFolderDialogClass;
    RegisterClassExW(&wc); // OK to fail if already registered

    current_dialog_ = &dialog;

    HWND hwnd = CreateWindowExW(
        WS_EX_DLGMODALFRAME | WS_EX_TOPMOST,
        kFolderDialogClass,
        L"Select Remote Folder",
        WS_OVERLAPPED | WS_CAPTION | WS_SYSMENU,
        CW_USEDEFAULT, CW_USEDEFAULT,
        410, 380,
        parent,
        nullptr,
        dialog.instance_,
        &dialog);

    if (!hwnd) {
        current_dialog_ = nullptr;
        return false;
    }

    // Disable parent while our pseudo-modal window is open
    if (parent) {
        EnableWindow(parent, FALSE);
    }

    ShowWindow(hwnd, SW_SHOW);
    UpdateWindow(hwnd);

    MSG msg{};
    while (GetMessageW(&msg, nullptr, 0, 0) > 0) {
        if (!IsDialogMessageW(hwnd, &msg)) {
            TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
        if (!IsWindow(hwnd)) {
            break;
        }
    }

    if (parent) {
        EnableWindow(parent, TRUE);
        SetForegroundWindow(parent);
    }

    current_dialog_ = nullptr;

    return dialog.result_ == IDOK;
}

LRESULT CALLBACK WebDavFolderDialog::DialogProc(HWND hwnd, UINT message, WPARAM wparam, LPARAM lparam) {
    if (message == WM_CREATE) {
        const CREATESTRUCTW* cs = reinterpret_cast<const CREATESTRUCTW*>(lparam);
        WebDavFolderDialog* dialog = reinterpret_cast<WebDavFolderDialog*>(cs->lpCreateParams);
        if (dialog) {
            dialog->hwnd_ = hwnd;
            SetWindowLongPtrW(hwnd, GWLP_USERDATA, reinterpret_cast<LONG_PTR>(dialog));
            dialog->InitializeDialog(hwnd);
        }
        return 0;
    }

    WebDavFolderDialog* dialog = reinterpret_cast<WebDavFolderDialog*>(
        GetWindowLongPtrW(hwnd, GWLP_USERDATA));

    if (dialog) {
        return dialog->HandleMessage(hwnd, message, wparam, lparam);
    }

    return DefWindowProcW(hwnd, message, wparam, lparam);
}

LRESULT WebDavFolderDialog::HandleMessage(HWND hwnd, UINT message, WPARAM wparam, LPARAM lparam) {
    switch (message) {
    case WM_COMMAND:
        switch (LOWORD(wparam)) {
        case IDOK:
            OnFolderSelected();
            result_ = IDOK;
            DestroyWindow(hwnd);
            return 0;
        case IDCANCEL:
            result_ = IDCANCEL;
            DestroyWindow(hwnd);
            return 0;
        case IDC_CREATE_FOLDER_BTN:
            CreateNewFolder();
            return 0;
        case IDC_REFRESH_BTN:
            LoadFolders();
            return 0;
        case IDC_FOLDER_LIST:
            if (HIWORD(wparam) == LBN_DBLCLK) {
                OnFolderSelected();
                result_ = IDOK;
                DestroyWindow(hwnd);
                return 0;
            }
            break;
        }
        break;

    case WM_CLOSE:
        result_ = IDCANCEL;
        DestroyWindow(hwnd);
        return 0;

    case WM_DESTROY:
        PostQuitMessage(0);
        return 0;
    }

    return DefWindowProcW(hwnd, message, wparam, lparam);
}

void WebDavFolderDialog::InitializeDialog(HWND hwnd) {
    SetWindowTextW(hwnd, L"Select Remote Folder");
    
    const HFONT font = reinterpret_cast<HFONT>(GetStockObject(DEFAULT_GUI_FONT));
    
    // Create folder list
    folder_list_ = CreateWindowExW(
        WS_EX_CLIENTEDGE,
        L"LISTBOX",
        nullptr,
        WS_CHILD | WS_VISIBLE | WS_VSCROLL | LBS_NOTIFY | LBS_NOINTEGRALHEIGHT,
        10, 10, 370, 200,
        hwnd,
        reinterpret_cast<HMENU>(static_cast<INT_PTR>(IDC_FOLDER_LIST)),
        instance_,
        nullptr);
    SendMessageW(folder_list_, WM_SETFONT, reinterpret_cast<WPARAM>(font), TRUE);
    
    // Create status label
    status_label_ = CreateWindowW(
        L"STATIC",
        L"Loading folders...",
        WS_CHILD | WS_VISIBLE,
        10, 220, 370, 20,
        hwnd,
        reinterpret_cast<HMENU>(static_cast<INT_PTR>(IDC_STATUS_LABEL)),
        instance_,
        nullptr);
    SendMessageW(status_label_, WM_SETFONT, reinterpret_cast<WPARAM>(font), TRUE);
    
    // Create new folder edit
    new_folder_edit_ = CreateWindowExW(
        WS_EX_CLIENTEDGE,
        L"EDIT",
        L"",
        WS_CHILD | WS_VISIBLE | WS_TABSTOP | ES_AUTOHSCROLL,
        10, 250, 200, 24,
        hwnd,
        reinterpret_cast<HMENU>(static_cast<INT_PTR>(IDC_NEW_FOLDER_EDIT)),
        instance_,
        nullptr);
    SendMessageW(new_folder_edit_, WM_SETFONT, reinterpret_cast<WPARAM>(font), TRUE);
    SendMessageW(new_folder_edit_, EM_SETCUEBANNER, FALSE, reinterpret_cast<LPARAM>(L"New folder name"));
    
    // Create buttons
    HWND create_btn = CreateWindowW(
        L"BUTTON",
        L"Create",
        WS_CHILD | WS_VISIBLE | WS_TABSTOP,
        220, 250, 80, 24,
        hwnd,
        reinterpret_cast<HMENU>(static_cast<INT_PTR>(IDC_CREATE_FOLDER_BTN)),
        instance_,
        nullptr);
    SendMessageW(create_btn, WM_SETFONT, reinterpret_cast<WPARAM>(font), TRUE);
    
    HWND refresh_btn = CreateWindowW(
        L"BUTTON",
        L"Refresh",
        WS_CHILD | WS_VISIBLE | WS_TABSTOP,
        310, 250, 70, 24,
        hwnd,
        reinterpret_cast<HMENU>(static_cast<INT_PTR>(IDC_REFRESH_BTN)),
        instance_,
        nullptr);
    SendMessageW(refresh_btn, WM_SETFONT, reinterpret_cast<WPARAM>(font), TRUE);
    
    HWND ok_btn = CreateWindowW(
        L"BUTTON",
        L"OK",
        WS_CHILD | WS_VISIBLE | WS_TABSTOP | BS_DEFPUSHBUTTON,
        200, 290, 80, 28,
        hwnd,
        reinterpret_cast<HMENU>(static_cast<INT_PTR>(IDOK)),
        instance_,
        nullptr);
    SendMessageW(ok_btn, WM_SETFONT, reinterpret_cast<WPARAM>(font), TRUE);
    
    HWND cancel_btn = CreateWindowW(
        L"BUTTON",
        L"Cancel",
        WS_CHILD | WS_VISIBLE | WS_TABSTOP,
        300, 290, 80, 28,
        hwnd,
        reinterpret_cast<HMENU>(static_cast<INT_PTR>(IDCANCEL)),
        instance_,
        nullptr);
    SendMessageW(cancel_btn, WM_SETFONT, reinterpret_cast<WPARAM>(font), TRUE);
    
    // Load folders
    LoadFolders();
}

void WebDavFolderDialog::LoadFolders() {
    if (!folder_list_ || !config_) {
        return;
    }
    
    SendMessageW(folder_list_, LB_RESETCONTENT, 0, 0);
    folders_.clear();
    
    UpdateStatus(L"Connecting to server...");
    
    WebDavClient client(*config_);
    std::wstring error_message;
    
    // List folders at root
    if (!client.ListRemoteFolder(folders_, error_message)) {
        UpdateStatus(L"Failed: " + error_message);
        return;
    }
    
    // Add folders to list
    for (const auto& folder : folders_) {
        if (folder.is_collection) {
            SendMessageW(folder_list_, LB_ADDSTRING, 0, reinterpret_cast<LPARAM>(folder.display_name.c_str()));
        }
    }
    
    UpdateStatus(L"Found " + std::to_wstring(folders_.size()) + L" folders");
}

void WebDavFolderDialog::CreateNewFolder() {
    if (!new_folder_edit_ || !config_) {
        return;
    }
    
    wchar_t buffer[256] = {};
    GetWindowTextW(new_folder_edit_, buffer, _countof(buffer));
    
    std::wstring folder_name(buffer);
    if (folder_name.empty()) {
        UpdateStatus(L"Please enter a folder name");
        return;
    }
    
    UpdateStatus(L"Creating folder...");
    
    WebDavClient client(*config_);
    std::wstring error_message;
    
    if (!client.CreateFolder(folder_name, error_message)) {
        UpdateStatus(L"Failed: " + error_message);
        return;
    }
    
    SetWindowTextW(new_folder_edit_, L"");
    LoadFolders();
    UpdateStatus(L"Folder created successfully");
}

void WebDavFolderDialog::OnFolderSelected() {
    if (!folder_list_ || !selected_folder_) {
        return;
    }
    
    int index = static_cast<int>(SendMessageW(folder_list_, LB_GETCURSEL, 0, 0));
    if (index >= 0 && index < static_cast<int>(folders_.size())) {
        // Find the folder by display name in the list
        wchar_t buffer[256] = {};
        SendMessageW(folder_list_, LB_GETTEXT, index, reinterpret_cast<LPARAM>(buffer));
        
        for (const auto& folder : folders_) {
            if (folder.display_name == buffer && folder.is_collection) {
                *selected_folder_ = folder.full_path;
                break;
            }
        }
    }
}

void WebDavFolderDialog::UpdateStatus(const std::wstring& message) {
    if (status_label_) {
        SetWindowTextW(status_label_, message.c_str());
    }
}
