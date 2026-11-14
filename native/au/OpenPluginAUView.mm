// SPDX-License-Identifier: MIT
// Cocoa view for the Audio Unit shell. The host asks the factory class for a
// view; the view owns the plugin's op_editor_* editor and keeps it in step
// with the Audio Unit on a timer: the parameter values, and which engine is
// live (the Audio Unit recreates its engine on Initialize).
#include "op_editor.h"
#include "OpenPluginAUBridge.h"
#import <AudioToolbox/AudioToolbox.h>
#import <AudioUnit/AUCocoaUIView.h>
#import <Cocoa/Cocoa.h>

// Objective-C class names are global in the host process, so they carry the
// plugin's name and version (set by the build).
@interface OP_AU_CONTAINER_CLASS : NSView {
    AudioUnit unit_;
    void* instance_;
    void* editor_;
    void* engine_;
    NSTimer* timer_;
}
- (instancetype)initWithAudioUnit:(AudioUnit)unit;
@end

namespace {
void notify(AudioUnit unit, AudioUnitEventType type, uint32_t index) {
    AudioUnitEvent event{};
    event.mEventType = type;
    event.mArgument.mParameter.mAudioUnit = unit;
    event.mArgument.mParameter.mParameterID = index;
    event.mArgument.mParameter.mScope = kAudioUnitScope_Global;
    event.mArgument.mParameter.mElement = 0;
    AUEventListenerNotify(nullptr, nullptr, &event);
}
void beginEdit(void* context, uint32_t index) {
    notify(static_cast<AudioUnit>(context), kAudioUnitEvent_BeginParameterChangeGesture, index);
}
void performEdit(void* context, uint32_t index, double plain) {
    AudioUnitParameter parameter{static_cast<AudioUnit>(context), index, kAudioUnitScope_Global, 0};
    // Sets the value and tells every listener, the host's automation included.
    AUParameterSet(nullptr, nullptr, &parameter, static_cast<AudioUnitParameterValue>(plain), 0);
}
void endEdit(void* context, uint32_t index) {
    notify(static_cast<AudioUnit>(context), kAudioUnitEvent_EndParameterChangeGesture, index);
}
}

@implementation OP_AU_CONTAINER_CLASS
- (instancetype)initWithAudioUnit:(AudioUnit)unit {
    uint32_t width = 0;
    uint32_t height = 0;
    op_editor_size(&width, &height);
    self = [super initWithFrame:NSMakeRect(0, 0, width, height)];
    if (!self) return nil;
    unit_ = unit;
    UInt32 size = sizeof(instance_);
    if (AudioUnitGetProperty(unit, kOpenPluginAUProperty_Instance, kAudioUnitScope_Global, 0, &instance_, &size) != noErr)
        instance_ = nullptr;
    OpEditorHost host{unit, beginEdit, performEdit, endEdit};
    editor_ = op_editor_create(&host);
    if (!editor_) {
        [self release];
        return nil;
    }
    [self sync];
    op_editor_attach(editor_, self);
    return self;
}
- (BOOL)isFlipped {
    return YES;
}
// The timer retains its target, so it only runs while the view is in a window.
- (void)viewDidMoveToWindow {
    [super viewDidMoveToWindow];
    [timer_ invalidate];
    timer_ = nil;
    if ([self window]) {
        timer_ = [NSTimer timerWithTimeInterval:1.0 / 20.0 target:self selector:@selector(sync) userInfo:nil repeats:YES];
        [[NSRunLoop currentRunLoop] addTimer:timer_ forMode:NSRunLoopCommonModes];
    }
}
- (void)sync {
    if (!editor_) return;
    void* engine = op_au_engine(instance_);
    // An Audio Unit with no engine is uninitialized: a host that renders it gets silence.
    if (!engine && engine_) NSLog(@"%s: the Audio Unit has no engine (uninitialized)", OP_AU_VIEW_CLASS_NAME);
    if (engine && !engine_ && editor_) NSLog(@"%s: the Audio Unit has an engine again", OP_AU_VIEW_CLASS_NAME);
    if (engine != engine_) {
        engine_ = engine;
        op_editor_bind(editor_, engine_);
    }
    // A failed render is silent in the host. Say so in the log.
    if (const int32_t error = op_au_take_error(instance_))
        NSLog(@"%s: the Audio Unit failed to initialize or render (status %d)", OP_AU_VIEW_CLASS_NAME, error);
    const uint32_t count = op_au_param_count(instance_);
    for (uint32_t i = 0; i < count; ++i) {
        float value = 0.f;
        if (op_au_param(instance_, i, &value)) op_editor_set_plain(editor_, i, value);
    }
}
- (void)dealloc {
    [timer_ invalidate];
    if (editor_) op_editor_destroy(editor_);
    [super dealloc];
}
@end

@interface OP_AU_VIEW_CLASS : NSObject <AUCocoaUIBase>
@end

@implementation OP_AU_VIEW_CLASS
- (unsigned)interfaceVersion {
    return 0;
}
- (NSView*)uiViewForAudioUnit:(AudioUnit)unit withSize:(NSSize)size {
    (void)size;
    return [[[OP_AU_CONTAINER_CLASS alloc] initWithAudioUnit:unit] autorelease];
}
@end
