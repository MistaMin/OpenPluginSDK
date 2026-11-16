// SPDX-License-Identifier: MIT
// Loads the unsigned plug-in in this process, from where an AAX host looks for it
// on Windows: <Name>.aaxplugin\Contents\x64\<Name>.aaxplugin. Not a Pro Tools host test.
#ifndef WIN32_LEAN_AND_MEAN
#define WIN32_LEAN_AND_MEAN
#endif
#include <windows.h>
#include <cstdint>
#include <cstdio>
#include <string>

int wmain(int argc, wchar_t** argv) {
    if (argc != 2) return 1;
    std::wstring bundle = argv[1];
    while (!bundle.empty() && (bundle.back() == L'\\' || bundle.back() == L'/')) bundle.pop_back();
    const size_t slash = bundle.find_last_of(L"\\/");
    const std::wstring name = slash == std::wstring::npos ? bundle : bundle.substr(slash + 1);
    const std::wstring path = bundle + L"\\Contents\\x64\\" + name;
    if (GetFileAttributesW(path.c_str()) == INVALID_FILE_ATTRIBUTES) {
        std::fprintf(stderr, "No Contents\\x64\\%ls in the bundle\n", name.c_str());
        return 2;
    }
    HMODULE module = LoadLibraryW(path.c_str());
    if (!module) {
        std::fprintf(stderr, "LoadLibrary failed: %lu\n", GetLastError());
        return 3;
    }
    const char* exports[] = {"ACFRegisterPlugin", "ACFRegisterComponent", "ACFGetClassFactory", "ACFCanUnloadNow", "ACFStartup", "ACFShutdown", "ACFGetSDKVersion"};
    for (const char* symbol : exports) {
        if (!GetProcAddress(module, symbol)) {
            std::fprintf(stderr, "Missing %s\n", symbol);
            FreeLibrary(module);
            return 4;
        }
    }
    using Version = int32_t(__stdcall*)(uint64_t*);
    uint64_t version = 0;
    auto getVersion = reinterpret_cast<Version>(reinterpret_cast<void*>(GetProcAddress(module, "ACFGetSDKVersion")));
    if (!getVersion || getVersion(&version) != 0 || version == 0) {
        FreeLibrary(module);
        return 5;
    }
    FreeLibrary(module);
    std::printf("Unsigned AAX bundle loaded. Seven ACF exports and SDK version %llu. Not a Pro Tools test: PASS\n", static_cast<unsigned long long>(version));
    return 0;
}
