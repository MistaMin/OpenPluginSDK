// SPDX-License-Identifier: MIT
// Loads the unsigned bundle in this process. Not a Pro Tools host test.
#include <CoreFoundation/CoreFoundation.h>
#include <cstdio>
#include <cstring>
int main(int argc, char** argv) {
    if (argc != 2) return 1;
    CFStringRef path = CFStringCreateWithCString(nullptr, argv[1], kCFStringEncodingUTF8);
    CFURLRef url = CFURLCreateWithFileSystemPath(nullptr, path, kCFURLPOSIXPathStyle, true);
    CFRelease(path);
    CFBundleRef bundle = CFBundleCreate(nullptr, url);
    CFRelease(url);
    if (!bundle) return 2;
    if (!CFBundleLoadExecutable(bundle)) {
        CFRelease(bundle);
        return 3;
    }
    const char* exports[] = {"ACFRegisterPlugin", "ACFRegisterComponent", "ACFGetClassFactory", "ACFCanUnloadNow", "ACFStartup", "ACFShutdown", "ACFGetSDKVersion"};
    for (const char* name : exports) {
        CFStringRef symbol = CFStringCreateWithCString(nullptr, name, kCFStringEncodingASCII);
        void* pointer = CFBundleGetFunctionPointerForName(bundle, symbol);
        CFRelease(symbol);
        if (!pointer) {
            std::fprintf(stderr, "Missing %s\n", name);
            CFRelease(bundle);
            return 4;
        }
    }
    using Version = int32_t (*)(uint64_t*);
    uint64_t version = 0;
    auto getVersion = reinterpret_cast<Version>(CFBundleGetFunctionPointerForName(bundle, CFSTR("ACFGetSDKVersion")));
    if (!getVersion || getVersion(&version) != 0 || version == 0) {
        CFRelease(bundle);
        return 5;
    }
    CFRelease(bundle);
    std::printf("Unsigned AAX bundle loaded. Seven ACF exports and SDK version %llu. Not a Pro Tools test: PASS\n", static_cast<unsigned long long>(version));
    return 0;
}
