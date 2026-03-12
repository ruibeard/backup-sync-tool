#include "dpapi.h"

#include <windows.h>
#include <wincrypt.h>
#include <vector>

namespace {

bool EncodeBase64(const BYTE* data, DWORD size, std::wstring& output) {
    DWORD chars_needed = 0;
    if (!CryptBinaryToStringW(data, size, CRYPT_STRING_BASE64 | CRYPT_STRING_NOCRLF, nullptr, &chars_needed)) {
        return false;
    }

    std::vector<wchar_t> buffer(chars_needed);
    if (!CryptBinaryToStringW(data, size, CRYPT_STRING_BASE64 | CRYPT_STRING_NOCRLF, buffer.data(), &chars_needed)) {
        return false;
    }

    output.assign(buffer.data(), chars_needed - 1);
    return true;
}

bool DecodeBase64(const std::wstring& input, std::vector<BYTE>& output) {
    DWORD bytes_needed = 0;
    if (!CryptStringToBinaryW(input.c_str(), 0, CRYPT_STRING_BASE64, nullptr, &bytes_needed, nullptr, nullptr)) {
        return false;
    }

    output.resize(bytes_needed);
    if (!CryptStringToBinaryW(input.c_str(), 0, CRYPT_STRING_BASE64, output.data(), &bytes_needed, nullptr, nullptr)) {
        return false;
    }

    output.resize(bytes_needed);
    return true;
}

} // namespace

bool ProtectSecret(const std::wstring& plain_text, std::wstring& protected_base64) {
    DATA_BLOB input{};
    input.pbData = reinterpret_cast<BYTE*>(const_cast<wchar_t*>(plain_text.data()));
    input.cbData = static_cast<DWORD>(plain_text.size() * sizeof(wchar_t));

    DATA_BLOB output{};
    if (!CryptProtectData(&input, L"WebDavSync", nullptr, nullptr, nullptr, CRYPTPROTECT_UI_FORBIDDEN, &output)) {
        return false;
    }

    const bool ok = EncodeBase64(output.pbData, output.cbData, protected_base64);
    LocalFree(output.pbData);
    return ok;
}

bool UnprotectSecret(const std::wstring& protected_base64, std::wstring& plain_text) {
    std::vector<BYTE> bytes;
    if (!DecodeBase64(protected_base64, bytes)) {
        return false;
    }

    DATA_BLOB input{};
    input.pbData = bytes.data();
    input.cbData = static_cast<DWORD>(bytes.size());

    DATA_BLOB output{};
    if (!CryptUnprotectData(&input, nullptr, nullptr, nullptr, nullptr, CRYPTPROTECT_UI_FORBIDDEN, &output)) {
        return false;
    }

    plain_text.assign(reinterpret_cast<wchar_t*>(output.pbData), output.cbData / sizeof(wchar_t));
    LocalFree(output.pbData);
    return true;
}
