#include "app.h"
#include <windows.h>

namespace {

constexpr wchar_t kSingleInstanceMutexName[] = L"Local\\WebDavSyncSingleInstance";
constexpr wchar_t kMainWindowClassName[] = L"WebDavSyncMainWindow";

void ShowExistingInstance() {
    HWND existing = FindWindowW(kMainWindowClassName, nullptr);
    if (!existing) {
        return;
    }

    if (IsIconic(existing)) {
        ShowWindow(existing, SW_RESTORE);
    } else {
        ShowWindow(existing, SW_SHOW);
    }
    SetForegroundWindow(existing);
}

} // namespace

int WINAPI wWinMain(HINSTANCE instance, HINSTANCE, PWSTR, int show_command) {
    HANDLE mutex = CreateMutexW(nullptr, FALSE, kSingleInstanceMutexName);
    if (!mutex) {
        return 1;
    }
    if (GetLastError() == ERROR_ALREADY_EXISTS) {
        ShowExistingInstance();
        CloseHandle(mutex);
        return 0;
    }

    App app(instance, show_command);
    const int exit_code = app.Run();
    CloseHandle(mutex);
    return exit_code;
}
