#include "app.h"
#include "resource.h"
#include "webdav_folder_dialog.h"

#include <windows.h>
#include <shellapi.h>
#include <shlobj.h>
#include <shlwapi.h>
#include <commctrl.h>
#include <strsafe.h>
#include <algorithm>
#include <memory>
#include <vector>

#include "webdav_client.h"

namespace {

constexpr wchar_t kWindowClassName[] = L"WebDavSyncMainWindow";
constexpr UINT kTrayMessage = WM_APP + 1;
constexpr UINT kActivityMessage = WM_APP + 2;
constexpr UINT kTrayIconId = 1001;

constexpr int IDC_WATCH_FOLDER = 2001;
constexpr int IDC_WEBDAV_URL = 2002;
constexpr int IDC_USERNAME = 2003;
constexpr int IDC_PASSWORD = 2004;
constexpr int IDC_STARTUP = 2005;
constexpr int IDC_DOWNLOAD_REMOTE = 2006;
constexpr int IDC_STATUS = 2007;
constexpr int IDC_SAVE = 2008;
constexpr int IDC_CLOSE = 2009;
constexpr int IDC_PROGRESS = 2010;
constexpr int IDC_BROWSE_FOLDER = 2011;
constexpr int IDC_OPEN_WEBDAV_URL = 2012;
constexpr int IDC_ACTIVITY = 2013;
constexpr int IDC_REMOTE_FOLDER = 2014;
constexpr int IDC_BROWSE_REMOTE = 2015;
constexpr int IDC_CONNECT = 2016;
constexpr int IDC_CONNECTION_STATUS = 2017;

constexpr UINT IDM_TRAY_LOG = 3003;
constexpr UINT IDM_TRAY_EXIT = 3004;
constexpr wchar_t kDefaultWatchFolder[] = L"C:\\XDSoftware\\backups";

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

HICON LoadStockIcon(SHSTOCKICONID icon_id, UINT flags) {
    SHSTOCKICONINFO info{};
    info.cbSize = sizeof(info);
    if (SUCCEEDED(SHGetStockIconInfo(icon_id, flags, &info)) && info.hIcon) {
        return info.hIcon;
    }
    return LoadIconW(nullptr, IDI_APPLICATION);
}

HICON LoadOpenUrlIcon() {
    return LoadStockIcon(SIID_INTERNET, SHGSI_ICON | SHGSI_SMALLICON);
}

HICON LoadResourceIcon(int resource_id, int size, SHSTOCKICONID fallback_id, UINT fallback_flags) {
    const HICON icon = static_cast<HICON>(LoadImageW(
        GetModuleHandleW(nullptr),
        MAKEINTRESOURCEW(resource_id),
        IMAGE_ICON,
        size,
        size,
        LR_DEFAULTCOLOR));
    if (icon) {
        return icon;
    }
    return LoadStockIcon(fallback_id, fallback_flags);
}

} // namespace

App::App(HINSTANCE instance, int show_command)
    : instance_(instance),
      show_command_(show_command) {
    large_icon_ = LoadResourceIcon(IDI_APP_IDLE, GetSystemMetrics(SM_CXICON), SIID_FOLDER, SHGSI_ICON | SHGSI_LARGEICON);
    idle_icon_ = LoadResourceIcon(IDI_APP_IDLE, GetSystemMetrics(SM_CXSMICON), SIID_FOLDER, SHGSI_ICON | SHGSI_SMALLICON);
    syncing_icon_ = LoadResourceIcon(IDI_APP_SYNCING, GetSystemMetrics(SM_CXSMICON), SIID_FOLDEROPEN, SHGSI_ICON | SHGSI_SMALLICON);
    error_icon_ = LoadStockIcon(SIID_ERROR, SHGSI_ICON | SHGSI_SMALLICON);
    open_url_icon_ = LoadOpenUrlIcon();
    LoadConfig(config_);
    if (config_.watch_folder.empty()) {
        const DWORD attributes = GetFileAttributesW(kDefaultWatchFolder);
        if (attributes != INVALID_FILE_ATTRIBUTES && (attributes & FILE_ATTRIBUTE_DIRECTORY)) {
            config_.watch_folder = kDefaultWatchFolder;
        }
    }
}

App::~App() {
    if (large_icon_) {
        DestroyIcon(large_icon_);
    }
    if (idle_icon_) {
        DestroyIcon(idle_icon_);
    }
    if (syncing_icon_) {
        DestroyIcon(syncing_icon_);
    }
    if (error_icon_) {
        DestroyIcon(error_icon_);
    }
    if (open_url_icon_) {
        DestroyIcon(open_url_icon_);
    }
    if (section_label_brush_) {
        DeleteObject(section_label_brush_);
    }
    if (section_font_ && section_font_ != GetStockObject(DEFAULT_GUI_FONT)) {
        DeleteObject(section_font_);
    }
}

int App::Run() {
    INITCOMMONCONTROLSEX controls{};
    controls.dwSize = sizeof(controls);
    controls.dwICC = ICC_STANDARD_CLASSES | ICC_PROGRESS_CLASS;
    InitCommonControlsEx(&controls);

    if (!CreateMainWindow()) {
        return 1;
    }

    AddTrayIcon();
    LoadIntoControls();

    if (HasUsableConfig(config_)) {
        // Auto-connect on startup
        ConnectToServer();
        StartSync();
        ShowSettings(false);
    } else {
        ShowSettings(true);
    }

    MSG message{};
    while (GetMessageW(&message, nullptr, 0, 0)) {
        if (IsDialogMessageW(hwnd_, &message)) {
            continue;
        }
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
        return app->HandleMessage(hwnd, message, wparam, lparam);
    }

    return DefWindowProcW(hwnd, message, wparam, lparam);
}

bool App::CreateMainWindow() {
    WNDCLASSW window_class{};
    window_class.lpfnWndProc = &App::WndProc;
    window_class.hInstance = instance_;
    window_class.lpszClassName = kWindowClassName;
    window_class.hCursor = LoadCursorW(nullptr, IDC_ARROW);
    window_class.hIcon = large_icon_;
    window_class.hbrBackground = reinterpret_cast<HBRUSH>(COLOR_WINDOW + 1);

    if (!RegisterClassW(&window_class) && GetLastError() != ERROR_CLASS_ALREADY_EXISTS) {
        return false;
    }

    hwnd_ = CreateWindowExW(
        WS_EX_APPWINDOW | WS_EX_CONTROLPARENT,
        kWindowClassName,
        L"WebDavSync",
        WS_OVERLAPPED | WS_CAPTION | WS_SYSMENU | WS_MINIMIZEBOX,
        CW_USEDEFAULT,
        CW_USEDEFAULT,
        520,
        800,
        nullptr,
        nullptr,
        instance_,
        this);

    if (!hwnd_) {
        return false;
    }

    SendMessageW(hwnd_, WM_SETICON, ICON_BIG, reinterpret_cast<LPARAM>(large_icon_));
    SendMessageW(hwnd_, WM_SETICON, ICON_SMALL, reinterpret_cast<LPARAM>(idle_icon_));

    CreateControls();
    return true;
}

void App::CreateControls() {
    // ── Fonts ────────────────────────────────────────────────────────────────
    const HFONT ui_font = reinterpret_cast<HFONT>(GetStockObject(DEFAULT_GUI_FONT));

    if (!section_font_) {
        LOGFONTW lf{};
        GetObjectW(ui_font, sizeof(lf), &lf);
        // Create smaller font for section headers
        lf.lfHeight = static_cast<LONG>(lf.lfHeight * 0.90);
        lf.lfWeight = FW_BOLD;
        section_font_ = CreateFontIndirectW(&lf);
        if (!section_font_) section_font_ = ui_font;
    }

    // ── Grid ─────────────────────────────────────────────────────────────────
    //
    //  Window outer 520 × 800  →  client ≈ 500 × 761
    //  Left margin  M  = 30
    //  Label width  LW = 100
    //  Field start  FX = 140
    //  Right margin     (30 px from right edge) → RX = 470

    constexpr int M   = 30;
    constexpr int LW  = 100;
    constexpr int FX  = 140;
    constexpr int RX  = 470;
    constexpr int RH  = 38;    // Row stride
    constexpr int CH  = 26;    // Control height
    constexpr int BW  = 85;    // Button width
    constexpr int BH  = 28;    // Button height

    // Lazy-create transparent brush for standard controls
    if (!section_label_brush_) {
        section_label_brush_ = CreateSolidBrush(GetSysColor(COLOR_WINDOW));
    }

    // ── Helper lambdas ───────────────────────────────────────────────────────

    // Left-aligned field label (no colons)
    auto label = [&](const wchar_t* text, int row_y) {
        HWND h = CreateWindowW(L"STATIC", text,
            WS_CHILD | WS_VISIBLE | SS_LEFT | SS_NOPREFIX,
            M, row_y + 5, LW, 16,
            hwnd_, nullptr, instance_, nullptr);
        SendMessageW(h, WM_SETFONT, reinterpret_cast<WPARAM>(ui_font), TRUE);
    };

    // Single-line edit box
    auto edit = [&](int id, int x, int row_y, int w,
                    const wchar_t* cue = nullptr, DWORD extra_style = 0) -> HWND {
        HWND h = CreateWindowExW(WS_EX_CLIENTEDGE, L"EDIT", L"",
            WS_CHILD | WS_VISIBLE | WS_TABSTOP | ES_AUTOHSCROLL | extra_style,
            x, row_y, w, CH,
            hwnd_, reinterpret_cast<HMENU>(static_cast<INT_PTR>(id)), instance_, nullptr);
        SendMessageW(h, WM_SETFONT, reinterpret_cast<WPARAM>(ui_font), TRUE);
        if (cue) SendMessageW(h, EM_SETCUEBANNER, FALSE, reinterpret_cast<LPARAM>(cue));
        return h;
    };

    // Standard push / check button
    auto btn = [&](int id, const wchar_t* text, int x, int row_y, int w, int h,
                   DWORD extra_style = 0) -> HWND {
        HWND hw = CreateWindowW(L"BUTTON", text,
            WS_CHILD | WS_VISIBLE | WS_TABSTOP | extra_style,
            x, row_y, w, h,
            hwnd_, reinterpret_cast<HMENU>(static_cast<INT_PTR>(id)), instance_, nullptr);
        SendMessageW(hw, WM_SETFONT, reinterpret_cast<WPARAM>(ui_font), TRUE);
        return hw;
    };

    // Bold section header (no line)
    auto section_header = [&](const wchar_t* text, int y) {
        HWND lbl = CreateWindowW(L"STATIC", text,
            WS_CHILD | WS_VISIBLE | SS_NOPREFIX,
            M, y, 200, 16,
            hwnd_, nullptr, instance_, nullptr);
        SendMessageW(lbl, WM_SETFONT, reinterpret_cast<WPARAM>(section_font_), TRUE);
        section_labels_.push_back(lbl);
    };

    auto separator = [&](int y) {
        CreateWindowW(L"STATIC", L"",
            WS_CHILD | WS_VISIBLE | SS_ETCHEDHORZ,
            M, y, RX - M, 2,
            hwnd_, nullptr, instance_, nullptr);
    };

    // ── Layout ───────────────────────────────────────────────────────────────
    int y = 20;

    // ════════════════════════════════════════════════════════════════════════
    // LOCAL FOLDER
    // ════════════════════════════════════════════════════════════════════════
    section_header(L"LOCAL FOLDER", y);
    y += 30;

    label(L"Folder", y);
    edit(IDC_WATCH_FOLDER, FX, y, RX - FX - BW - 10, L"C:\\path\\to\\sync");
    btn(IDC_BROWSE_FOLDER, L"Browse...", RX - BW, y, BW, BH);
    y += RH + 10;

    // ════════════════════════════════════════════════════════════════════════
    // SERVER
    // ════════════════════════════════════════════════════════════════════════
    section_header(L"SERVER", y);
    y += 30;

    label(L"URL", y);
    edit(IDC_WEBDAV_URL, FX, y, RX - FX - 40 - 10, L"https://example.com/webdav/");
    // Small info/open button
    {
        HWND ou = CreateWindowW(L"BUTTON", L"i",
            WS_CHILD | WS_VISIBLE | WS_TABSTOP,
            RX - 40, y, 40, BH,
            hwnd_, reinterpret_cast<HMENU>(static_cast<INT_PTR>(IDC_OPEN_WEBDAV_URL)), instance_, nullptr);
        SendMessageW(ou, WM_SETFONT, reinterpret_cast<WPARAM>(ui_font), TRUE);
    }
    y += RH;

    label(L"Username", y);
    edit(IDC_USERNAME, FX, y, RX - FX, L"username");
    y += RH;

    label(L"Password", y);
    edit(IDC_PASSWORD, FX, y, RX - FX, L"password", ES_PASSWORD);
    y += RH;

    label(L"Remote folder", y);
    edit(IDC_REMOTE_FOLDER, FX, y, RX - FX - BW - 10, L"/backups");
    btn(IDC_BROWSE_REMOTE, L"Browse...", RX - BW, y, BW, BH);
    y += RH + 8;

    btn(IDC_CONNECT, L"Connect", M, y, BW + 20, BH);
    {
        HWND cs = CreateWindowW(L"STATIC", L"\u25CF NOT CONNECTED", // ● 
            WS_CHILD | WS_VISIBLE | SS_NOPREFIX,
            M + BW + 35, y + 6, RX - (M + BW + 35), 16,
            hwnd_, reinterpret_cast<HMENU>(static_cast<INT_PTR>(IDC_CONNECTION_STATUS)),
            instance_, nullptr);
        SendMessageW(cs, WM_SETFONT, reinterpret_cast<WPARAM>(section_font_), TRUE);
    }
    y += RH + 15;
    
    separator(y);
    y += 20;

    // ════════════════════════════════════════════════════════════════════════
    // OPTIONS
    // ════════════════════════════════════════════════════════════════════════
    section_header(L"OPTIONS", y);
    y += 30;

    btn(IDC_STARTUP, L"Start with Windows", M, y, 200, CH, BS_AUTOCHECKBOX);
    y += RH;
    btn(IDC_DOWNLOAD_REMOTE, L"Sync remote changes", M, y, 200, CH, BS_AUTOCHECKBOX);
    y += RH + 5;

    // ════════════════════════════════════════════════════════════════════════
    // SYNC STATUS
    // ════════════════════════════════════════════════════════════════════════
    {
        HWND st = CreateWindowW(L"STATIC", L"\u26A0 NOT CONFIGURED", // ⚠
            WS_CHILD | WS_VISIBLE | SS_NOPREFIX,
            M, y, RX - M, 16,
            hwnd_, reinterpret_cast<HMENU>(static_cast<INT_PTR>(IDC_STATUS)), instance_, nullptr);
        SendMessageW(st, WM_SETFONT, reinterpret_cast<WPARAM>(section_font_), TRUE);
    }
    y += 22;

    progress_bar_ = CreateWindowExW(0, PROGRESS_CLASSW, nullptr,
        WS_CHILD | WS_VISIBLE,
        M, y, RX - M, 4,
        hwnd_, reinterpret_cast<HMENU>(static_cast<INT_PTR>(IDC_PROGRESS)), instance_, nullptr);
    SendMessageW(progress_bar_, PBM_SETRANGE32, 0, 1);
    SendMessageW(progress_bar_, PBM_SETPOS, 0, 0);
    y += 15 + 15;

    // ════════════════════════════════════════════════════════════════════════
    // RECENT ACTIVITY
    // ════════════════════════════════════════════════════════════════════════
    section_header(L"RECENT ACTIVITY", y);
    y += 30;

    // Client height depends on 800 outer -> ~761
    constexpr int kClientH       = 761;
    constexpr int kBottomMargin  = 20;
    constexpr int kBtnAreaH      = 40;
    const int listbox_bottom     = kClientH - kBtnAreaH - kBottomMargin;
    const int listbox_h          = listbox_bottom - y;

    activity_list_ = CreateWindowExW(WS_EX_CLIENTEDGE, L"LISTBOX", nullptr,
        WS_CHILD | WS_VISIBLE | WS_VSCROLL | WS_TABSTOP |
        LBS_NOINTEGRALHEIGHT | LBS_NOTIFY,
        M, y, RX - M, listbox_h > 100 ? listbox_h : 100,
        hwnd_, reinterpret_cast<HMENU>(static_cast<INT_PTR>(IDC_ACTIVITY)), instance_, nullptr);
    SendMessageW(activity_list_, WM_SETFONT, reinterpret_cast<WPARAM>(ui_font), TRUE);
    y += (listbox_h > 100 ? listbox_h : 100) + kBottomMargin;

    // ════════════════════════════════════════════════════════════════════════
    // BOTTOM BUTTONS
    // ════════════════════════════════════════════════════════════════════════
    
    // Version label
    HWND version_lbl = CreateWindowW(L"STATIC", L"WEBDAVSYNC V1.0",
        WS_CHILD | WS_VISIBLE | SS_NOPREFIX,
        M, y + 8, 150, 16,
        hwnd_, nullptr, instance_, nullptr);
    SendMessageW(version_lbl, WM_SETFONT, reinterpret_cast<WPARAM>(section_font_), TRUE);
    section_labels_.push_back(version_lbl); // Color it grey as well

    constexpr int kSaveBtnW  = 100;
    constexpr int kCloseBtnW = 100;
    const int btn_y = y;
    btn(IDC_SAVE,  L"SAVE",  RX - kSaveBtnW - kCloseBtnW - 10, btn_y, kSaveBtnW,  BH + 4, BS_DEFPUSHBUTTON);
    btn(IDC_CLOSE, L"CLOSE", RX - kCloseBtnW,                  btn_y, kCloseBtnW, BH + 4);
}

void App::LoadIntoControls() {
    SetControlText(IDC_WATCH_FOLDER, config_.watch_folder);
    SetControlText(IDC_WEBDAV_URL, config_.webdav_url);
    SetControlText(IDC_REMOTE_FOLDER, config_.remote_folder);
    SetControlText(IDC_USERNAME, config_.username);
    SetControlText(IDC_PASSWORD, config_.password);
    SetCheck(IDC_STARTUP, config_.start_with_windows);
    SetCheck(IDC_DOWNLOAD_REMOTE, config_.download_remote_changes);
}

void App::SaveFromControls() {
    config_.watch_folder = GetControlText(IDC_WATCH_FOLDER);
    config_.webdav_url = GetControlText(IDC_WEBDAV_URL);
    config_.remote_folder = GetControlText(IDC_REMOTE_FOLDER);
    config_.username = GetControlText(IDC_USERNAME);
    config_.password = GetControlText(IDC_PASSWORD);
    config_.start_with_windows = GetCheck(IDC_STARTUP);
    config_.download_remote_changes = GetCheck(IDC_DOWNLOAD_REMOTE);
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
        [this](SyncState state, const std::wstring& text, int completed, int total) {
            UpdateStatus(state, text, completed, total);
        });
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

void App::UpdateStatus(SyncState state, const std::wstring& text, int completed, int total) {
    sync_state_ = state;
    UpdateStatusLabel(text);
    UpdateProgress(completed, total);
    UpdateTrayIcon(state);
}

void App::UpdateStatusLabel(const std::wstring& text) {
    if (!hwnd_) {
        return;
    }
    SetControlText(IDC_STATUS, text);
}

void App::UpdateProgress(int completed, int total) {
    if (!progress_bar_) {
        return;
    }

    if (total <= 0) {
        SendMessageW(progress_bar_, PBM_SETRANGE32, 0, 1);
        SendMessageW(progress_bar_, PBM_SETPOS, 0, 0);
        return;
    }

    const int clamped_completed = std::clamp(completed, 0, total);
    SendMessageW(progress_bar_, PBM_SETRANGE32, 0, total);
    SendMessageW(progress_bar_, PBM_SETPOS, clamped_completed, 0);
}

void App::AppendActivity(const std::wstring& text) {
    if (!activity_list_) {
        return;
    }

    const int index = static_cast<int>(SendMessageW(activity_list_, LB_ADDSTRING, 0, reinterpret_cast<LPARAM>(text.c_str())));
    SendMessageW(activity_list_, LB_SETTOPINDEX, index, 0);

    constexpr int kMaxActivityItems = 100;
    const int count = static_cast<int>(SendMessageW(activity_list_, LB_GETCOUNT, 0, 0));
    if (count > kMaxActivityItems) {
        SendMessageW(activity_list_, LB_DELETESTRING, 0, 0);
    }
}

void App::UpdateTrayIcon(SyncState state) {
    if (!tray_added_) {
        return;
    }

    HICON icon = idle_icon_;
    switch (state) {
    case SyncState::Syncing:
        icon = syncing_icon_;
        break;
    case SyncState::Error:
        icon = error_icon_;
        break;
    case SyncState::Idle:
    default:
        icon = idle_icon_;
        break;
    }

    NOTIFYICONDATAW data{};
    data.cbSize = sizeof(data);
    data.hWnd = hwnd_;
    data.uID = kTrayIconId;
    data.uFlags = NIF_ICON | NIF_TIP;
    data.hIcon = icon;

    std::wstring tip = L"WebDavSync";
    HWND status = GetDlgItem(hwnd_, IDC_STATUS);
    if (status) {
        wchar_t buffer[96] = {};
        GetWindowTextW(status, buffer, static_cast<int>(_countof(buffer)));
        if (buffer[0] != L'\0') {
            tip += L" - ";
            tip += buffer;
        }
    }
    StringCchCopyW(data.szTip, _countof(data.szTip), tip.c_str());
    Shell_NotifyIconW(NIM_MODIFY, &data);
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
    data.hIcon = idle_icon_;
    StringCchCopyW(data.szTip, _countof(data.szTip), L"WebDavSync");
    tray_added_ = Shell_NotifyIconW(NIM_ADD, &data) == TRUE;
    if (tray_added_) {
        UpdateTrayIcon(sync_state_);
    }
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

    if (hwnd_) {
        PostMessageW(hwnd_, kActivityMessage, 0, reinterpret_cast<LPARAM>(new std::wstring(line)));
    }
}

void App::OpenLogFolder() {
    const std::wstring log_folder = JoinPath(GetExecutableDirectory(), L"logs");
    EnsureDirectory(log_folder);
    ShellExecuteW(hwnd_, L"open", log_folder.c_str(), nullptr, nullptr, SW_SHOWNORMAL);
}

void App::OpenWebDavUrl() {
    const std::wstring url = GetControlText(IDC_WEBDAV_URL);
    if (url.empty()) {
        MessageBoxW(hwnd_, L"WebDAV URL is required.", L"WebDavSync", MB_ICONWARNING);
        return;
    }

    const HINSTANCE result = ShellExecuteW(hwnd_, L"open", url.c_str(), nullptr, nullptr, SW_SHOWNORMAL);
    if (reinterpret_cast<INT_PTR>(result) <= 32) {
        MessageBoxW(hwnd_, L"Could not open the URL in the default browser.", L"WebDavSync", MB_ICONERROR);
    }
}

void App::BrowseForWatchFolder() {
    BROWSEINFOW info{};
    info.hwndOwner = hwnd_;
    info.lpszTitle = L"Select the local folder to sync";
    info.ulFlags = BIF_RETURNONLYFSDIRS | BIF_USENEWUI | BIF_VALIDATE;

    PIDLIST_ABSOLUTE selected = SHBrowseForFolderW(&info);
    if (!selected) {
        return;
    }

    wchar_t folder[MAX_PATH] = {};
    if (SHGetPathFromIDListW(selected, folder)) {
        SetControlText(IDC_WATCH_FOLDER, folder);
    }

    CoTaskMemFree(selected);
}

void App::BrowseRemoteFolder() {
    // First save current values from controls
    SaveFromControls();
    
    // Validate we have URL, username, password first
    if (config_.webdav_url.empty() || config_.username.empty() || config_.password.empty()) {
        MessageBoxW(hwnd_, L"Please enter WebDAV URL, Username, and Password first.", L"WebDavSync", MB_ICONWARNING);
        return;
    }
    
    std::wstring selected_folder;
    if (WebDavFolderDialog::Show(hwnd_, config_, selected_folder)) {
        SetControlText(IDC_REMOTE_FOLDER, selected_folder);
    }
}

void App::ConnectToServer() {
    // Save current values
    SaveFromControls();
    
    // Validate basic requirements
    if (config_.webdav_url.empty() || config_.username.empty() || config_.password.empty()) {
        UpdateStatus(SyncState::Error, L"WebDAV URL, Username, and Password are required", 0, 0);
        MessageBoxW(hwnd_, L"Please enter WebDAV URL, Username, and Password.", L"WebDavSync", MB_ICONWARNING);
        return;
    }
    
    UpdateStatus(SyncState::Idle, L"Connecting to server...", 0, 0);
    
    WebDavClient client(config_);
    std::wstring error_message;
    
    if (!client.TestConnection(error_message)) {
        UpdateConnectionStatus(false);
        UpdateStatus(SyncState::Error, L"Connection failed", 0, 0);
        MessageBoxW(hwnd_, error_message.c_str(), L"WebDavSync - Connection Failed", MB_ICONERROR);
        return;
    }
    
    // Connection successful
    is_connected_ = true;
    GetLocalTime(&connection_time_);
    UpdateConnectionStatus(true);
    UpdateStatus(SyncState::Idle, L"Connected successfully", 0, 0);
    Log(L"Connected to WebDAV server");
}

void App::UpdateConnectionStatus(bool connected) {
    HWND status_label = GetDlgItem(hwnd_, IDC_CONNECTION_STATUS);
    if (!status_label) {
        return;
    }
    
    is_connected_ = connected;
    
    if (connected) {
        wchar_t time_buffer[64] = {};
        SYSTEMTIME now;
        GetLocalTime(&now);
        StringCchPrintfW(time_buffer, _countof(time_buffer), L"%02d:%02d", now.wHour, now.wMinute);
        
        std::wstring status = L"● Connected since " + std::wstring(time_buffer);
        SetWindowTextW(status_label, status.c_str());
        
        // Set text color to green (using owner draw would be better, but this works for now)
        // For a visual dot, we use Unicode bullet character
    } else {
        SetWindowTextW(status_label, L"● Not connected");
    }
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
    if (config_.remote_folder.empty()) {
        error_message = L"Remote folder is required.";
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
    case IDC_BROWSE_FOLDER:
        BrowseForWatchFolder();
        break;
    case IDC_OPEN_WEBDAV_URL:
        OpenWebDavUrl();
        break;
    case IDC_BROWSE_REMOTE:
        BrowseRemoteFolder();
        break;
    case IDC_CONNECT:
        ConnectToServer();
        break;
    case IDC_SAVE: {
        SaveFromControls();
        std::wstring error_message;
        if (!ValidateConfig(error_message)) {
            UpdateStatus(SyncState::Error, error_message, 0, 0);
            MessageBoxW(hwnd_, error_message.c_str(), L"WebDavSync", MB_ICONWARNING);
            return;
        }

        UpdateStatus(SyncState::Idle, L"Validating connection...", 0, 0);
        WebDavClient client(config_);
        if (!client.TestConnection(error_message)) {
            UpdateStatus(SyncState::Error, error_message, 0, 0);
            MessageBoxW(hwnd_, error_message.c_str(), L"WebDavSync", MB_ICONERROR);
            return;
        }

        if (!SaveConfig(config_)) {
            UpdateStatus(SyncState::Error, L"Could not write config.json.", 0, 0);
            MessageBoxW(hwnd_, L"Could not write config.json.", L"WebDavSync", MB_ICONERROR);
            return;
        }

        ApplyStartupSetting();
        StopSync();
        StartSync();
        UpdateConnectionStatus(true);
        UpdateStatus(SyncState::Idle, L"Connected and watching for changes", 0, 0);
        break;
    }
    case IDC_CLOSE:
        ShowSettings(false);
        break;
    default:
        break;
    }
}

void App::HandleTrayAction(UINT action) {
    switch (action) {
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

LRESULT App::HandleMessage(HWND hwnd, UINT message, WPARAM wparam, LPARAM lparam) {
    switch (message) {
    case kActivityMessage: {
        std::unique_ptr<std::wstring> activity(reinterpret_cast<std::wstring*>(lparam));
        if (activity) {
            AppendActivity(*activity);
        }
        return 0;
    }
    case WM_CTLCOLORSTATIC: {
        HDC dc = reinterpret_cast<HDC>(wparam);
        HWND ctrl = reinterpret_cast<HWND>(lparam);
        SetBkMode(dc, TRANSPARENT);
        // Paint section labels in grey
        for (HWND sh : section_labels_) {
            if (ctrl == sh) {
                SetTextColor(dc, RGB(120, 120, 120));
                return reinterpret_cast<LRESULT>(section_label_brush_
                    ? section_label_brush_
                    : GetSysColorBrush(COLOR_WINDOW));
            }
        }
        return reinterpret_cast<LRESULT>(GetSysColorBrush(COLOR_WINDOW));
    }
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

    return DefWindowProcW(hwnd, message, wparam, lparam);
}
