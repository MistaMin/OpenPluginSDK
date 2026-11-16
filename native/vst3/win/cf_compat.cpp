// SPDX-License-Identifier: MIT
// The Windows side of compat/CoreFoundation/CoreFoundation.h and compat/pthread.h:
// what the VST3 proofs need to load a module and run the main loop. Proof builds
// only; the shell itself does not link this.
#ifndef WIN32_LEAN_AND_MEAN
#define WIN32_LEAN_AND_MEAN
#endif
#ifndef NOMINMAX
#define NOMINMAX
#endif
#include <windows.h>
#include "CoreFoundation/CoreFoundation.h"
#include "pthread.h"
#include <cstring>
#include <string>

namespace {
enum class Kind { Url, Bundle };
struct Object {
    Kind kind;
};
const DWORD gLoadThread = GetCurrentThreadId();

std::wstring widen(const std::string& utf8) {
    if (utf8.empty()) return {};
    const int length = MultiByteToWideChar(CP_UTF8, MB_ERR_INVALID_CHARS, utf8.data(), static_cast<int>(utf8.size()), nullptr, 0);
    // The path came through main's argv, which is in the process code page unless
    // the executable's manifest asks for UTF-8.
    const UINT page = length > 0 ? CP_UTF8 : CP_ACP;
    const int size = MultiByteToWideChar(page, 0, utf8.data(), static_cast<int>(utf8.size()), nullptr, 0);
    std::wstring wide(static_cast<size_t>(size > 0 ? size : 0), L'\0');
    if (size > 0) MultiByteToWideChar(page, 0, utf8.data(), static_cast<int>(utf8.size()), wide.data(), size);
    return wide;
}

#if defined(_M_ARM64EC)
const wchar_t* const kArchFolder = L"arm64ec-win";
#elif defined(_M_ARM64)
const wchar_t* const kArchFolder = L"arm64-win";
#elif defined(_M_X64)
const wchar_t* const kArchFolder = L"x86_64-win";
#else
const wchar_t* const kArchFolder = L"x86-win";
#endif
}

struct OpWinCFURL {
    Object header{Kind::Url};
    std::wstring path;
};
struct OpWinCFBundle {
    Object header{Kind::Bundle};
    std::wstring path;
    HMODULE module = nullptr;
};

namespace {
bool callInitDll(CFBundleRef bundle) {
    if (!bundle || !bundle->module) return false;
    auto init = reinterpret_cast<bool (*)()>(GetProcAddress(bundle->module, "InitDll"));
    return init && init();
}
}

extern "C" {
const CFRunLoopMode kCFRunLoopDefaultMode = "kCFRunLoopDefaultMode";
void (*op_win_module_loaded)(void* module) = nullptr;

CFURLRef CFURLCreateFromFileSystemRepresentation(CFAllocatorRef, const UInt8* utf8, CFIndex length, Boolean) {
    if (!utf8 || length <= 0) return nullptr;
    auto* url = new OpWinCFURL;
    url->path = widen(std::string(reinterpret_cast<const char*>(utf8), static_cast<size_t>(length)));
    for (wchar_t& c : url->path) {
        if (c == L'/') c = L'\\';
    }
    while (!url->path.empty() && url->path.back() == L'\\') url->path.pop_back();
    if (url->path.empty()) {
        delete url;
        return nullptr;
    }
    return url;
}

CFBundleRef CFBundleCreate(CFAllocatorRef, CFURLRef url) {
    if (!url) return nullptr;
    const DWORD attributes = GetFileAttributesW(url->path.c_str());
    if (attributes == INVALID_FILE_ATTRIBUTES || !(attributes & FILE_ATTRIBUTE_DIRECTORY)) return nullptr;
    auto* bundle = new OpWinCFBundle;
    bundle->path = url->path;
    return bundle;
}

Boolean CFBundleLoadExecutable(CFBundleRef bundle) {
    if (!bundle) return 0;
    if (bundle->module) return 1;
    const size_t slash = bundle->path.find_last_of(L'\\');
    const std::wstring name = slash == std::wstring::npos ? bundle->path : bundle->path.substr(slash + 1);
    const std::wstring file = bundle->path + L"\\Contents\\" + kArchFolder + L"\\" + name;
    wchar_t full[32768];
    const DWORD length = GetFullPathNameW(file.c_str(), static_cast<DWORD>(sizeof(full) / sizeof(full[0])), full, nullptr);
    if (length == 0 || length >= sizeof(full) / sizeof(full[0])) return 0;
    bundle->module = LoadLibraryExW(full, nullptr, LOAD_WITH_ALTERED_SEARCH_PATH);
    if (!bundle->module) return 0;
    if (op_win_module_loaded) op_win_module_loaded(bundle->module);
    return 1;
}

void* CFBundleGetFunctionPointerForName(CFBundleRef bundle, CFStringRef name) {
    if (!bundle || !bundle->module || !name) return nullptr;
    // A proof calls bundleEntry(bundle) and bundleExit(): InitDll() and ExitDll() here.
    if (std::strcmp(name, "bundleEntry") == 0) {
        if (!GetProcAddress(bundle->module, "InitDll")) return nullptr;
        return reinterpret_cast<void*>(static_cast<bool (*)(CFBundleRef)>(callInitDll));
    }
    if (std::strcmp(name, "bundleExit") == 0) name = "ExitDll";
    return reinterpret_cast<void*>(GetProcAddress(bundle->module, name));
}

void CFRelease(CFTypeRef object) {
    if (!object) return;
    if (static_cast<const Object*>(object)->kind == Kind::Url) {
        delete static_cast<const OpWinCFURL*>(object);
    } else {
        // The library stays loaded, as a bundle's code does on macOS.
        delete static_cast<const OpWinCFBundle*>(object);
    }
}

int32_t CFRunLoopRunInMode(CFRunLoopMode, CFTimeInterval seconds, Boolean returnAfterSourceHandled) {
    constexpr int32_t kTimedOut = 3;
    constexpr int32_t kHandledSource = 4;
    const ULONGLONG start = GetTickCount64();
    const ULONGLONG span = seconds > 0 ? static_cast<ULONGLONG>(seconds * 1000.0 + 0.5) : 0;
    bool handled = false;
    for (;;) {
        MSG message;
        while (PeekMessageW(&message, nullptr, 0, 0, PM_REMOVE)) {
            TranslateMessage(&message);
            DispatchMessageW(&message);
            handled = true;
        }
        if (handled && returnAfterSourceHandled) return kHandledSource;
        const ULONGLONG spent = GetTickCount64() - start;
        if (spent >= span) return handled ? kHandledSource : kTimedOut;
        MsgWaitForMultipleObjectsEx(0, nullptr, static_cast<DWORD>(span - spent), QS_ALLINPUT, MWMO_INPUTAVAILABLE);
    }
}

int pthread_main_np(void) { return GetCurrentThreadId() == gLoadThread ? 1 : 0; }
}
