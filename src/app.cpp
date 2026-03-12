#include "app.h"

#include <windows.h>
#include <shellapi.h>
#include <shlwapi.h>
#include <commctrl.h>
#include <strsafe.h>
#include <vector>

#include "webdav_client.h"

namespace {

constexpr wchar_t kWindowClassName[] = L"WebDavSyncMainWindow";
constexpr UINT kTrayMessage = WM_APP + 1;
constexpr UINT kTrayIconId = 1001;

constexpr int IDC_WATCH_FOLDER = 2001;
constexpr int IDC_WEBDAV_URL = 2002;
constexpr int IDC_USERNAME = 2003;
constexpr int IDC_PASSWORD = 2004;
constexpr int IDC_STARTUP = 2005;
constexpr int IDC_DELETE = 2006;
constexpr int IDC_STATUS = 2007;
constexpr int IDC_SAVE = 2008;
constexpr int IDC_SYNC_NOW = 2009;
constexpr int IDC_TEST = 2010;

constexpr UINT IDM_TRAY_OPEN = 3001;
constexpr UINT IDM_TRAY_SYNC = 3002;
constexpr UINT IDM_TRAY_LOG = 3003;
constexpr UINT IDM_TRAY_EXIT = 3004;

std::wstring JoinPath(const std::wstring& left, const std::wstring& right) {
    if (left.empty()) {
        return right;
    }
    if (left.back() == L'\\') {
        return left + right;
    }
    return left + L"\\" + right;
}

void EnsureDirectory(const std::wstring& path) {
    CreateDirectoryW(path.c_str(), nullptr);
}

std::wstring GetModulePath() {
    wchar_t buffer[MAX_PATH];
    DWORD length = GetModuleFileNameW(nullptr, buffer, MAX_PATH);
    return std::wstring(buffer, length);
}

std::string ToUtf8(const std::wstring& value) {
    if (value.empty()) {
        return {};
    }

    const int size = WideCharToMultiByte(CP_UTF8, 0, value.c_str(), static_cast<int>(value.size()), nullptr, 0, nullptr, nullptr);
    std::string utf8(size, '\0');
    WideCharToMultiByte(CP_UTF8, 0, value.c_str(), static_cast<int>(value.size()), utf8.data(), size, nullptr, nullptr);
    return utf8;
}

void AppendUtf8Line(const std::wstring& path, const std::wstring& line) {
    HANDLE file = CreateFileW(path.c_str(), FILE_APPEND_DATA, FILE_SHARE_READ, nullptr, OPEN_ALWAYS, FILE_ATTRIBUTE_NORMAL, nullptr);
    if (file == INVALID_HANDLE_VALUE) {
        return;
    }

    const std::string utf8 = ToUtf8(line + L"\r\n");
    DWORD written = 0;
    if (!utf8.empty()) {
        WriteFile(file, utf8.data(), static_cast<DWORD>(utf8.size()), &written, nullptr);
    }
    CloseHandle(file);
}

} // namespace

App::App(HINSTANCE instance, int show_command)
    : instance_(instance),
      show_command_(show_command) {
    LoadConfig(config_);
}

int App::Run() {
    INITCOMMONCONTROLSEX controls{};
    controls.dwSize = sizeof(controls);
    controls.dwICC = ICC_STANDARD_CLASSES;
    InitCommonControlsEx(&controls);

    if (!CreateMainWindow()) {
        return 1;
    }

    AddTrayIcon();
    LoadIntoControls();

    if (HasUsableConfig(config_)) {
        StartSync();
        ShowSettings(false);
    } else {
        ShowSettings(true);
    }

    MSG message{};
    while (GetMessageW(&message, nullptr, 0, 0)) {
        TranslateMessage(&message);
        DispatchMessageW(&message);
    }

    StopSync();
    RemoveTrayIcon();
    return static_cast<int>(message.wParam);
}

LRESULT CALLBACK App::WndProc(HWND hwnd, UINT message, WPARAM wparam, LPARAM lparam) {
    App* app = reinterpret_cast<App*>(GetWindowLongPtrW(hwnd, GWLP_USERDATA));
    if (message == WM_NCCREATE) {
        const CREATESTRUCTW* create = reinterpret_cast<CREATESTRUCTW*>(lparam);
        app = reinterpret_cast<App*>(create->lpCreateParams);
        SetWindowLongPtrW(hwnd, GWLP_USERDATA, reinterpret_cast<LONG_PTR>(app));
    }

    if (app) {
        return app->HandleMessage(message, wparam, lparam);
    }

    return DefWindowProcW(hwnd, message, wparam, lparam);
}

bool App::CreateMainWindow() {
    WNDCLASSW window_class{};
    window_class.lpfnWndProc = &App::WndProc;
    window_class.hInstance = instance_;
    window_class.lpszClassName = kWindowClassName;
    window_class.hCursor = LoadCursorW(nullptr, IDC_ARROW);
    window_class.hIcon = LoadIconW(nullptr, IDI_APPLICATION);

    if (!RegisterClassW(&window_class) && GetLastError() != ERROR_CLASS_ALREADY_EXISTS) {
        return false;
    }

    hwnd_ = CreateWindowExW(
        WS_EX_APPWINDOW,
        kWindowClassName,
        L"WebDavSync",
        WS_OVERLAPPED | WS_CAPTION | WS_SYSMENU | WS_MINIMIZEBOX,
        CW_USEDEFAULT,
        CW_USEDEFAULT,
        520,
        320,
        nullptr,
        nullptr,
        instance_,
        this);

    if (!hwnd_) {
        return false;
    }

    CreateControls();
    return true;
}

void App::CreateControls() {
    const HFONT font = reinterpret_cast<HFONT>(GetStockObject(DEFAULT_GUI_FONT));

    auto make_label = [&](const wchar_t* text, int x, int y) {
        HWND label = CreateWindowW(L"STATIC", text, WS_CHILD | WS_VISIBLE, x, y, 120, 20, hwnd_, nullptr, instance_, nullptr);
        SendMessageW(label, WM_SETFONT, reinterpret_cast<WPARAM>(font), TRUE);
    };

    auto make_edit = [&](int id, int x, int y, int width, DWORD extra_style = 0) {
        HWND edit = CreateWindowExW(WS_EX_CLIENTEDGE, L"EDIT", L"", WS_CHILD | WS_VISIBLE | WS_TABSTOP | ES_AUTOHSCROLL | extra_style, x, y, width, 24, hwnd_, reinterpret_cast<HMENU>(static_cast<INT_PTR>(id)), instance_, nullptr);
        SendMessageW(edit, WM_SETFONT, reinterpret_cast<WPARAM>(font), TRUE);
    };

    make_label(L"Folder", 20, 22);
    make_edit(IDC_WATCH_FOLDER, 150, 20, 330);
    make_label(L"WebDAV URL", 20, 56);
    make_edit(IDC_WEBDAV_URL, 150, 54, 330);
    make_label(L"Username", 20, 90);
    make_edit(IDC_USERNAME, 150, 88, 330);
    make_label(L"Password", 20, 124);
    make_edit(IDC_PASSWORD, 150, 122, 330, ES_PASSWORD);

    HWND startup = CreateWindowW(L"BUTTON", L"Start with Windows", WS_CHILD | WS_VISIBLE | WS_TABSTOP | BS_AUTOCHECKBOX, 150, 158, 150, 24, hwnd_, reinterpret_cast<HMENU>(static_cast<INT_PTR>(IDC_STARTUP)), instance_, nullptr);
    SendMessageW(startup, WM_SETFONT, reinterpret_cast<WPARAM>(font), TRUE);

    HWND deletes = CreateWindowW(L"BUTTON", L"Delete remote files too", WS_CHILD | WS_VISIBLE | WS_TABSTOP | BS_AUTOCHECKBOX, 310, 158, 170, 24, hwnd_, reinterpret_cast<HMENU>(static_cast<INT_PTR>(IDC_DELETE)), instance_, nullptr);
    SendMessageW(deletes, WM_SETFONT, reinterpret_cast<WPARAM>(font), TRUE);

    HWND status = CreateWindowW(L"STATIC", L"Not configured", WS_CHILD | WS_VISIBLE, 20, 198, 460, 20, hwnd_, reinterpret_cast<HMENU>(static_cast<INT_PTR>(IDC_STATUS)), instance_, nullptr);
    SendMessageW(status, WM_SETFONT, reinterpret_cast<WPARAM>(font), TRUE);

    HWND save = CreateWindowW(L"BUTTON", L"Save", WS_CHILD | WS_VISIBLE | WS_TABSTOP, 150, 232, 90, 28, hwnd_, reinterpret_cast<HMENU>(static_cast<INT_PTR>(IDC_SAVE)), instance_, nullptr);
    SendMessageW(save, WM_SETFONT, reinterpret_cast<WPARAM>(font), TRUE);

    HWND test = CreateWindowW(L"BUTTON", L"Test Connection", WS_CHILD | WS_VISIBLE | WS_TABSTOP, 250, 232, 110, 28, hwnd_, reinterpret_cast<HMENU>(static_cast<INT_PTR>(IDC_TEST)), instance_, nullptr);
    SendMessageW(test, WM_SETFONT, reinterpret_cast<WPARAM>(font), TRUE);

    HWND sync = CreateWindowW(L"BUTTON", L"Sync Now", WS_CHILD | WS_VISIBLE | WS_TABSTOP, 370, 232, 110, 28, hwnd_, reinterpret_cast<HMENU>(static_cast<INT_PTR>(IDC_SYNC_NOW)), instance_, nullptr);
    SendMessageW(sync, WM_SETFONT, reinterpret_cast<WPARAM>(font), TRUE);
}

void App::LoadIntoControls() {
    SetControlText(IDC_WATCH_FOLDER, config_.watch_folder);
    SetControlText(IDC_WEBDAV_URL, config_.webdav_url);
    SetControlText(IDC_USERNAME, config_.username);
    SetControlText(IDC_PASSWORD, config_.password);
    SetCheck(IDC_STARTUP, config_.start_with_windows);
    SetCheck(IDC_DELETE, config_.sync_deletes);
}

void App::SaveFromControls() {
    config_.watch_folder = GetControlText(IDC_WATCH_FOLDER);
    config_.webdav_url = GetControlText(IDC_WEBDAV_URL);
    config_.username = GetControlText(IDC_USERNAME);
    config_.password = GetControlText(IDC_PASSWORD);
    config_.start_with_windows = GetCheck(IDC_STARTUP);
    config_.sync_deletes = GetCheck(IDC_DELETE);
}

void App::ShowSettings(bool show) {
    ShowWindow(hwnd_, show ? show_command_ : SW_HIDE);
    if (show) {
        SetForegroundWindow(hwnd_);
    }
}

void App::StartSync() {
    if (!HasUsableConfig(config_)) {
        return;
    }

    engine_.Start(
        config_,
        [this](const std::wstring& line) { Log(line); },
        [this](SyncState, const std::wstring& text) { UpdateStatusLabel(text); });
}

void App::StopSync() {
    engine_.Stop();
}

void App::ApplyStartupSetting() {
    HKEY key = nullptr;
    if (RegOpenKeyExW(HKEY_CURRENT_USER, L"Software\\Microsoft\\Windows\\CurrentVersion\\Run", 0, KEY_SET_VALUE, &key) != ERROR_SUCCESS) {
        return;
    }

    const std::wstring exe_path = L"\"" + GetModulePath() + L"\"";
    if (config_.start_with_windows) {
        RegSetValueExW(key, L"WebDavSync", 0, REG_SZ, reinterpret_cast<const BYTE*>(exe_path.c_str()), static_cast<DWORD>((exe_path.size() + 1) * sizeof(wchar_t)));
    } else {
        RegDeleteValueW(key, L"WebDavSync");
    }

    RegCloseKey(key);
}

void App::UpdateStatusLabel(const std::wstring& text) {
    if (!hwnd_) {
        return;
    }
    SetControlText(IDC_STATUS, text);
}

void App::AddTrayIcon() {
    if (tray_added_) {
        return;
    }

    NOTIFYICONDATAW data{};
    data.cbSize = sizeof(data);
    data.hWnd = hwnd_;
    data.uID = kTrayIconId;
    data.uFlags = NIF_MESSAGE | NIF_ICON | NIF_TIP;
    data.uCallbackMessage = kTrayMessage;
    data.hIcon = LoadIconW(nullptr, IDI_APPLICATION);
    StringCchCopyW(data.szTip, _countof(data.szTip), L"WebDavSync");
    tray_added_ = Shell_NotifyIconW(NIM_ADD, &data) == TRUE;
}

void App::RemoveTrayIcon() {
    if (!tray_added_) {
        return;
    }

    NOTIFYICONDATAW data{};
    data.cbSize = sizeof(data);
    data.hWnd = hwnd_;
    data.uID = kTrayIconId;
    Shell_NotifyIconW(NIM_DELETE, &data);
    tray_added_ = false;
}

void App::ShowTrayMenu() {
    HMENU menu = CreatePopupMenu();
    if (!menu) {
        return;
    }

    AppendMenuW(menu, MF_STRING, IDM_TRAY_OPEN, L"Open Settings");
    AppendMenuW(menu, MF_STRING, IDM_TRAY_SYNC, L"Sync Now");
    AppendMenuW(menu, MF_STRING, IDM_TRAY_LOG, L"Open Logs");
    AppendMenuW(menu, MF_SEPARATOR, 0, nullptr);
    AppendMenuW(menu, MF_STRING, IDM_TRAY_EXIT, L"Exit");

    POINT point{};
    GetCursorPos(&point);
    SetForegroundWindow(hwnd_);
    TrackPopupMenu(menu, TPM_BOTTOMALIGN | TPM_LEFTALIGN, point.x, point.y, 0, hwnd_, nullptr);
    DestroyMenu(menu);
}

void App::Log(const std::wstring& message) {
    const std::wstring log_folder = JoinPath(GetExecutableDirectory(), L"logs");
    EnsureDirectory(log_folder);

    SYSTEMTIME now{};
    GetLocalTime(&now);

    wchar_t file_name[64];
    StringCchPrintfW(file_name, _countof(file_name), L"%04u-%02u-%02u.log", now.wYear, now.wMonth, now.wDay);

    wchar_t line[2048];
    StringCchPrintfW(line, _countof(line), L"%02u:%02u:%02u %ls", now.wHour, now.wMinute, now.wSecond, message.c_str());
    AppendUtf8Line(JoinPath(log_folder, file_name), line);
}

void App::OpenLogFolder() {
    const std::wstring log_folder = JoinPath(GetExecutableDirectory(), L"logs");
    EnsureDirectory(log_folder);
    ShellExecuteW(hwnd_, L"open", log_folder.c_str(), nullptr, nullptr, SW_SHOWNORMAL);
}

bool App::TestConnection() {
    SaveFromControls();

    std::wstring error_message;
    if (!ValidateConfig(error_message)) {
        MessageBoxW(hwnd_, error_message.c_str(), L"WebDavSync", MB_ICONWARNING);
        return false;
    }

    WebDavClient client(config_);
    if (!client.TestConnection(error_message)) {
        MessageBoxW(hwnd_, error_message.c_str(), L"WebDavSync", MB_ICONERROR);
        return false;
    }

    UpdateStatusLabel(L"Connection successful");
    return true;
}

bool App::ValidateConfig(std::wstring& error_message) {
    if (config_.watch_folder.empty()) {
        error_message = L"Folder is required.";
        return false;
    }
    if (config_.webdav_url.empty()) {
        error_message = L"WebDAV URL is required.";
        return false;
    }
    if (config_.username.empty()) {
        error_message = L"Username is required.";
        return false;
    }
    if (config_.password.empty()) {
        error_message = L"Password is required.";
        return false;
    }

    DWORD attributes = GetFileAttributesW(config_.watch_folder.c_str());
    if (attributes == INVALID_FILE_ATTRIBUTES || !(attributes & FILE_ATTRIBUTE_DIRECTORY)) {
        error_message = L"Folder does not exist.";
        return false;
    }

    return true;
}

std::wstring App::GetControlText(int control_id) const {
    HWND control = GetDlgItem(hwnd_, control_id);
    int length = GetWindowTextLengthW(control);
    std::vector<wchar_t> buffer(length + 1, L'\0');
    GetWindowTextW(control, buffer.data(), static_cast<int>(buffer.size()));
    return std::wstring(buffer.data());
}

void App::SetControlText(int control_id, const std::wstring& value) {
    SetWindowTextW(GetDlgItem(hwnd_, control_id), value.c_str());
}

void App::SetCheck(int control_id, bool checked) {
    SendMessageW(GetDlgItem(hwnd_, control_id), BM_SETCHECK, checked ? BST_CHECKED : BST_UNCHECKED, 0);
}

bool App::GetCheck(int control_id) const {
    return SendMessageW(GetDlgItem(hwnd_, control_id), BM_GETCHECK, 0, 0) == BST_CHECKED;
}

std::wstring App::GetLogPath() const {
    return JoinPath(GetExecutableDirectory(), L"logs");
}

void App::HandleCommand(int control_id) {
    switch (control_id) {
    case IDC_SAVE: {
        SaveFromControls();
        std::wstring error_message;
        if (!ValidateConfig(error_message)) {
            MessageBoxW(hwnd_, error_message.c_str(), L"WebDavSync", MB_ICONWARNING);
            return;
        }

        if (!SaveConfig(config_)) {
            MessageBoxW(hwnd_, L"Could not write config.json.", L"WebDavSync", MB_ICONERROR);
            return;
        }

        ApplyStartupSetting();
        StopSync();
        StartSync();
        UpdateStatusLabel(L"Configuration saved");
        ShowSettings(false);
        break;
    }
    case IDC_TEST:
        TestConnection();
        break;
    case IDC_SYNC_NOW:
        engine_.SyncNow();
        UpdateStatusLabel(L"Manual sync requested");
        break;
    default:
        break;
    }
}

void App::HandleTrayAction(UINT action) {
    switch (action) {
    case IDM_TRAY_OPEN:
        ShowSettings(true);
        break;
    case IDM_TRAY_SYNC:
        engine_.SyncNow();
        break;
    case IDM_TRAY_LOG:
        OpenLogFolder();
        break;
    case IDM_TRAY_EXIT:
        DestroyWindow(hwnd_);
        break;
    default:
        break;
    }
}

LRESULT App::HandleMessage(UINT message, WPARAM wparam, LPARAM lparam) {
    switch (message) {
    case WM_COMMAND:
        HandleCommand(LOWORD(wparam));
        HandleTrayAction(LOWORD(wparam));
        return 0;
    case WM_CLOSE:
        ShowSettings(false);
        return 0;
    case WM_DESTROY:
        PostQuitMessage(0);
        return 0;
    default:
        break;
    }

    if (message == kTrayMessage) {
        if (lparam == WM_RBUTTONUP) {
            ShowTrayMenu();
        } else if (lparam == WM_LBUTTONDBLCLK) {
            ShowSettings(true);
        }
        return 0;
    }

    return DefWindowProcW(hwnd_, message, wparam, lparam);
}
