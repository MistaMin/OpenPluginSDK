// SPDX-License-Identifier: MIT
// The Linux side of compat/CoreFoundation/CoreFoundation.h: what the VST3 proofs
// need to load a module and run the main loop. Proof builds only; the shell itself
// does not link this.
#include "CoreFoundation/CoreFoundation.h"
#include "pluginterfaces/base/ipluginbase.h"
#include "pluginterfaces/gui/iplugview.h"
#include <algorithm>
#include <chrono>
#include <cstdio>
#include <cstring>
#include <dlfcn.h>
#include <poll.h>
#include <string>
#include <vector>

using namespace Steinberg;

namespace {
enum class Kind { Url, Bundle };
struct Object {
    Kind kind;
};
using Clock = std::chrono::steady_clock;

// The run loop a host lends: timers and descriptor handlers, run by
// CFRunLoopRunInMode on the thread that calls it. Never destroyed or counted: it
// lives as long as the proof.
class RunLoop final : public Linux::IRunLoop {
public:
    tresult PLUGIN_API registerEventHandler(Linux::IEventHandler* handler, Linux::FileDescriptor fd) override {
        if (!handler || fd < 0) return kInvalidArgument;
        handlers_.push_back({handler, fd});
        return kResultOk;
    }
    tresult PLUGIN_API unregisterEventHandler(Linux::IEventHandler* handler) override {
        const auto end = std::remove_if(handlers_.begin(), handlers_.end(), [&](const Handler& h) { return h.handler == handler; });
        if (end == handlers_.end()) return kResultFalse;
        handlers_.erase(end, handlers_.end());
        return kResultOk;
    }
    tresult PLUGIN_API registerTimer(Linux::ITimerHandler* handler, Linux::TimerInterval milliseconds) override {
        if (!handler || milliseconds == 0) return kInvalidArgument;
        const auto interval = std::chrono::milliseconds(milliseconds);
        timers_.push_back({handler, interval, Clock::now() + interval});
        return kResultOk;
    }
    tresult PLUGIN_API unregisterTimer(Linux::ITimerHandler* handler) override {
        const auto end = std::remove_if(timers_.begin(), timers_.end(), [&](const Timer& t) { return t.handler == handler; });
        if (end == timers_.end()) return kResultFalse;
        timers_.erase(end, timers_.end());
        return kResultOk;
    }
    tresult PLUGIN_API queryInterface(const TUID iid, void** obj) override {
        if (FUnknownPrivate::iidEqual(iid, Linux::IRunLoop::iid) || FUnknownPrivate::iidEqual(iid, FUnknown::iid)) {
            *obj = static_cast<Linux::IRunLoop*>(this);
            return kResultOk;
        }
        *obj = nullptr;
        return kNoInterface;
    }
    uint32 PLUGIN_API addRef() override { return 1; }
    uint32 PLUGIN_API release() override { return 1; }

    // Waits up to `wait` for a timer to fall due or a descriptor to be readable, then
    // calls what is due. Returns whether a handler ran. A handler may register and
    // unregister: each is looked up again before it is called.
    bool turn(std::chrono::milliseconds wait) {
        const auto now = Clock::now();
        auto timeout = wait;
        for (const Timer& t : timers_) {
            const auto until = std::chrono::duration_cast<std::chrono::milliseconds>(t.due - now);
            timeout = std::min(timeout, std::max(until, std::chrono::milliseconds(0)));
        }
        std::vector<pollfd> fds;
        for (const Handler& h : handlers_) fds.push_back({h.fd, POLLIN, 0});
        ::poll(fds.data(), fds.size(), static_cast<int>(timeout.count()));
        bool ran = false;
        const std::vector<Handler> handlers = handlers_;
        for (size_t i = 0; i < handlers.size(); ++i) {
            if (!(fds[i].revents & (POLLIN | POLLHUP | POLLERR))) continue;
            const Handler& h = handlers[i];
            const bool live = std::any_of(handlers_.begin(), handlers_.end(), [&](const Handler& x) { return x.handler == h.handler && x.fd == h.fd; });
            if (!live) continue;
            h.handler->onFDIsSet(h.fd);
            ran = true;
        }
        const auto after = Clock::now();
        const std::vector<Timer> timers = timers_;
        for (const Timer& t : timers) {
            if (t.due > after) continue;
            const auto live = std::find_if(timers_.begin(), timers_.end(), [&](const Timer& x) { return x.handler == t.handler; });
            if (live == timers_.end()) continue;
            live->due = after + live->interval;
            t.handler->onTimer();
            ran = true;
        }
        return ran;
    }

private:
    struct Handler {
        Linux::IEventHandler* handler;
        Linux::FileDescriptor fd;
    };
    struct Timer {
        Linux::ITimerHandler* handler;
        std::chrono::milliseconds interval;
        Clock::time_point due;
    };
    std::vector<Handler> handlers_;
    std::vector<Timer> timers_;
};

RunLoop& run_loop() {
    static RunLoop loop;
    return loop;
}
}

struct OpLinuxCFURL {
    Object header{Kind::Url};
    std::string path;
};
struct OpLinuxCFBundle {
    Object header{Kind::Bundle};
    std::string path;
    void* module = nullptr;
};

namespace {
// ModuleEntry, then what a host does next: the factory is given the host context,
// which here is the run loop alone.
bool callModuleEntry(CFBundleRef bundle) {
    if (!bundle || !bundle->module) return false;
    auto entry = reinterpret_cast<bool (*)(void*)>(dlsym(bundle->module, "ModuleEntry"));
    if (!entry || !entry(bundle->module)) return false;
    auto getFactory = reinterpret_cast<IPluginFactory* (*)()>(dlsym(bundle->module, "GetPluginFactory"));
    if (IPluginFactory* factory = getFactory ? getFactory() : nullptr) {
        IPluginFactory3* factory3 = nullptr;
        if (factory->queryInterface(IPluginFactory3::iid, reinterpret_cast<void**>(&factory3)) == kResultOk && factory3) {
            factory3->setHostContext(static_cast<Linux::IRunLoop*>(&run_loop()));
            factory3->release();
        }
        factory->release();
    }
    return true;
}
}

Linux::IRunLoop* op_linux_run_loop() { return &run_loop(); }

extern "C" {
const CFRunLoopMode kCFRunLoopDefaultMode = "kCFRunLoopDefaultMode";

CFURLRef CFURLCreateFromFileSystemRepresentation(CFAllocatorRef, const UInt8* utf8, CFIndex length, Boolean) {
    if (!utf8 || length <= 0) return nullptr;
    auto* url = new OpLinuxCFURL;
    url->path.assign(reinterpret_cast<const char*>(utf8), static_cast<size_t>(length));
    while (url->path.size() > 1 && url->path.back() == '/') url->path.pop_back();
    return url;
}

CFBundleRef CFBundleCreate(CFAllocatorRef, CFURLRef url) {
    if (!url) return nullptr;
    auto* bundle = new OpLinuxCFBundle;
    bundle->path = url->path;
    return bundle;
}

Boolean CFBundleLoadExecutable(CFBundleRef bundle) {
    if (!bundle) return 0;
    if (bundle->module) return 1;
    // `<folder>/<Name>.vst3` holds `Contents/<machine>-linux/<Name>.so`.
    const size_t slash = bundle->path.find_last_of('/');
    std::string name = slash == std::string::npos ? bundle->path : bundle->path.substr(slash + 1);
    const size_t dot = name.rfind('.');
    if (dot != std::string::npos) name.resize(dot);
    const std::string file = bundle->path + "/Contents/" OP_VST3_ARCH "/" + name + ".so";
    bundle->module = dlopen(file.c_str(), RTLD_NOW | RTLD_LOCAL);
    if (!bundle->module) std::fprintf(stderr, "cannot load %s: %s\n", file.c_str(), dlerror());
    return bundle->module ? 1 : 0;
}

void* CFBundleGetFunctionPointerForName(CFBundleRef bundle, CFStringRef name) {
    if (!bundle || !bundle->module || !name) return nullptr;
    // A proof calls bundleEntry(bundle) and bundleExit(): ModuleEntry(handle) and
    // ModuleExit() here.
    if (std::strcmp(name, "bundleEntry") == 0) {
        if (!dlsym(bundle->module, "ModuleEntry")) return nullptr;
        return reinterpret_cast<void*>(static_cast<bool (*)(CFBundleRef)>(callModuleEntry));
    }
    if (std::strcmp(name, "bundleExit") == 0) name = "ModuleExit";
    return dlsym(bundle->module, name);
}

void CFRelease(CFTypeRef object) {
    if (!object) return;
    if (static_cast<const Object*>(object)->kind == Kind::Url) {
        delete static_cast<const OpLinuxCFURL*>(object);
    } else {
        // The library stays loaded, as a bundle's code does on macOS.
        delete static_cast<const OpLinuxCFBundle*>(object);
    }
}

int32_t CFRunLoopRunInMode(CFRunLoopMode, CFTimeInterval seconds, Boolean returnAfterSourceHandled) {
    constexpr int32_t kTimedOut = 3;
    constexpr int32_t kHandledSource = 4;
    const auto start = Clock::now();
    const auto span = std::chrono::milliseconds(seconds > 0 ? static_cast<long long>(seconds * 1000.0 + 0.5) : 0);
    bool handled = false;
    for (;;) {
        const auto spent = std::chrono::duration_cast<std::chrono::milliseconds>(Clock::now() - start);
        const auto left = spent < span ? span - spent : std::chrono::milliseconds(0);
        if (run_loop().turn(left)) handled = true;
        if (handled && returnAfterSourceHandled) return kHandledSource;
        if (Clock::now() - start >= span) return handled ? kHandledSource : kTimedOut;
    }
}
}
