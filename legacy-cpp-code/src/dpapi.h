#pragma once

#include <string>

bool ProtectSecret(const std::wstring& plain_text, std::wstring& protected_base64);
bool UnprotectSecret(const std::wstring& protected_base64, std::wstring& plain_text);
