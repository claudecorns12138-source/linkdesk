// LinkDesk native macOS capture / experimental virtual display helper.
// Private CGVirtualDisplay interface research: Chromium's BSD-licensed test
// utility. See THIRD_PARTY_NOTICES.md. Screen capture uses public Apple APIs.
#import <Foundation/Foundation.h>
#import <CoreGraphics/CoreGraphics.h>
#import <ScreenCaptureKit/ScreenCaptureKit.h>
#import <VideoToolbox/VideoToolbox.h>
#import <CoreMedia/CoreMedia.h>
#import <CoreVideo/CoreVideo.h>
#include <signal.h>
#include <unistd.h>
#include <cerrno>
#include <climits>
#include "layout_guard.h"

@interface CGVirtualDisplayDescriptor : NSObject
@property(nonatomic) unsigned int vendorID, productID, serialNum, serialNumber;
@property(nonatomic) unsigned int maxPixelsWide, maxPixelsHigh;
@property(nonatomic) CGSize sizeInMillimeters;
@property(nonatomic) CGPoint redPrimary, greenPrimary, bluePrimary, whitePoint;
@property(strong, nonatomic) NSString *name;
@property(strong, nonatomic) id queue;
@end
@interface CGVirtualDisplayMode : NSObject
- (instancetype)initWithWidth:(unsigned int)width height:(unsigned int)height refreshRate:(double)rate;
@end
@interface CGVirtualDisplaySettings : NSObject
@property(nonatomic) unsigned int hiDPI;
@property(strong, nonatomic) NSArray *modes;
@end
@interface CGVirtualDisplay : NSObject
- (instancetype)initWithDescriptor:(CGVirtualDisplayDescriptor *)descriptor;
- (BOOL)applySettings:(CGVirtualDisplaySettings *)settings;
@property(readonly, nonatomic) unsigned int displayID;
@end

static CGVirtualDisplay *virtualDisplay;
static id captureOwner;
static NSDictionary *activeLayout;

static void record(uint8_t kind, NSData *data) {
    if (data.length > 8 * 1024 * 1024 - 1) return;
    uint32_t size = CFSwapInt32HostToBig((uint32_t)data.length + 1);
    flockfile(stdout);
    bool ok = fwrite(&size, 4, 1, stdout) == 1 && fwrite(&kind, 1, 1, stdout) == 1;
    if (ok && data.length) ok = fwrite(data.bytes, data.length, 1, stdout) == 1;
    if (fflush(stdout) != 0) ok = false;
    funlockfile(stdout);
    if (!ok) _exit(0);
}
static void fail(NSString *message) {
    record(3, [message dataUsingEncoding:NSUTF8StringEncoding]);
    exit(1);
}

static std::vector<linkdesk::DisplayState> readTopology(void) {
    CGDirectDisplayID displays[65]; uint32_t count=0;
    if (CGGetOnlineDisplayList(65,displays,&count)!=kCGErrorSuccess || !count || count>64)
        fail(@"Could not read the existing display layout safely.");
    std::vector<linkdesk::DisplayState> result;
    for (uint32_t i=0;i<count;i++) {
        auto id=displays[i];
        CGRect bounds=CGDisplayBounds(id);
        CGDisplayModeRef mode=CGDisplayCopyDisplayMode(id);
        if (!mode) fail(@"An existing display mode could not be read safely.");
        linkdesk::Rect rect{llround(bounds.origin.x),llround(bounds.origin.y),llround(bounds.size.width),llround(bounds.size.height)};
        if (!rect.valid()) { CGDisplayModeRelease(mode); fail(@"An existing display has unsupported bounds."); }
        result.push_back({id,rect,CGDisplayModeGetWidth(mode),CGDisplayModeGetHeight(mode),
            CGDisplayModeGetPixelWidth(mode),CGDisplayModeGetPixelHeight(mode),CGDisplayModeGetRefreshRate(mode),
            CGDisplayRotation(id),CGDisplayMirrorsDisplay(id),(bool)CGDisplayIsMain(id),(bool)CGDisplayIsActive(id)});
        CGDisplayModeRelease(mode);
    }
    return result;
}
static NSDictionary *rectDictionary(const linkdesk::Rect &r) {
    return @{ @"x":@(r.x), @"y":@(r.y), @"width":@(r.width), @"height":@(r.height) };
}
static NSArray *displayDictionaries(const std::vector<linkdesk::DisplayState> &displays) {
    NSMutableArray *result=[NSMutableArray array];
    for (const auto &d:displays)
        [result addObject:@{@"id":@(d.id),@"rect":rectDictionary(d.rect),@"main":@(d.main),@"active":@(d.active),@"mirrored_to":@(d.mirror)}];
    return result;
}

static CGDirectDisplayID createVirtualDisplay(uint32_t anchor, NSString *side, const linkdesk::Rect &placement) {
    // Take the baseline before creating anything. Do not identify or reconfigure
    // Sidecar using private Continuity APIs; treat all existing screens as owned
    // by macOS/the user, and keep their complete display states unchanged.
    const auto baseline=readTopology();
    if (!linkdesk::placementValid(baseline,anchor,side.UTF8String,placement))
        fail(@"The selected display edge is stale or occupied. Refresh the layout and choose another side; existing screens were not moved.");
    Class descriptorClass = NSClassFromString(@"CGVirtualDisplayDescriptor");
    Class displayClass = NSClassFromString(@"CGVirtualDisplay");
    Class modeClass = NSClassFromString(@"CGVirtualDisplayMode");
    Class settingsClass = NSClassFromString(@"CGVirtualDisplaySettings");
    if (!descriptorClass || !displayClass || !modeClass || !settingsClass)
        fail(@"This macOS version does not expose the experimental virtual-display interface.");
    CGVirtualDisplayDescriptor *d = [[descriptorClass alloc] init];
    d.name = @"LinkDesk Extended Display";
    d.vendorID = 0x4c44;
    d.productID = 1;
    unsigned int serial = arc4random_uniform(UINT32_MAX - 1) + 1;
    d.serialNum = serial;
    if ([d respondsToSelector:@selector(setSerialNumber:)]) d.serialNumber = serial;
    d.maxPixelsWide = 1920; d.maxPixelsHigh = 1080;
    d.sizeInMillimeters = CGSizeMake(509, 286);
    d.redPrimary = CGPointMake(0.64, 0.33);
    d.greenPrimary = CGPointMake(0.30, 0.60);
    d.bluePrimary = CGPointMake(0.15, 0.06);
    d.whitePoint = CGPointMake(0.3127, 0.3290);
    d.queue = dispatch_get_global_queue(QOS_CLASS_USER_INTERACTIVE, 0);
    virtualDisplay = [[displayClass alloc] initWithDescriptor:d];
    if (!virtualDisplay) fail(@"macOS refused to create the experimental display.");
    CGVirtualDisplaySettings *settings = [[settingsClass alloc] init];
    settings.hiDPI = 0;
    settings.modes = @[[[modeClass alloc] initWithWidth:1920 height:1080 refreshRate:60]];
    if (![virtualDisplay applySettings:settings]) fail(@"macOS refused the virtual display mode.");
    CGDirectDisplayID display = virtualDisplay.displayID;
    if (!display) fail(@"The virtual display has no display ID.");
    bool online=false;
    for (int attempt=0;attempt<40;attempt++) {
        CGDisplayModeRef mode=CGDisplayCopyDisplayMode(display);
        online=CGDisplayIsOnline(display) && mode;
        if (mode) CGDisplayModeRelease(mode);
        if (online) break;
        usleep(50000);
    }
    if (!online) fail(@"The owned virtual display did not come online.");
    if (!linkdesk::existingUnchanged(baseline,readTopology(),display))
        fail(@"An existing display changed while the virtual display was being added. LinkDesk stopped its own extension without reconfiguring Sidecar.");
    // Configure only the newly created display; leave existing Sidecar intact.
    CGDisplayConfigRef config = NULL;
    if (CGBeginDisplayConfiguration(&config) == kCGErrorSuccess) {
        CGError mirror = CGConfigureDisplayMirrorOfDisplay(config, display, kCGNullDirectDisplay);
        CGError origin = CGConfigureDisplayOrigin(config, display, (int32_t)placement.x, (int32_t)placement.y);
        if (mirror == kCGErrorSuccess && origin == kCGErrorSuccess) {
            if (CGCompleteDisplayConfiguration(config, kCGConfigureForSession) != kCGErrorSuccess)
                fail(@"macOS refused the independent desktop layout.");
        } else {
            CGCancelDisplayConfiguration(config);
            fail(@"Could not detach the virtual display from mirroring.");
        }
    } else fail(@"Could not begin the independent display configuration.");
    if (CGDisplayMirrorsDisplay(display) != kCGNullDirectDisplay)
        fail(@"The requested independent display is still mirrored; extension was not started.");
    for (int attempt=0;attempt<40;attempt++) {
        CGRect bounds=CGDisplayBounds(display);
        linkdesk::Rect actual{llround(bounds.origin.x),llround(bounds.origin.y),llround(bounds.size.width),llround(bounds.size.height)};
        if (actual==placement && CGDisplayIsActive(display)) break;
        usleep(50000);
    }
    const auto finalTopology=readTopology();
    if (!linkdesk::existingUnchanged(baseline,finalTopology,display))
        fail(@"An existing display position, mode or mirror state changed. LinkDesk stopped only its own extension; Sidecar was not reconfigured.");
    auto owned=std::find_if(finalTopology.begin(),finalTopology.end(),[&](const auto &d){return d.id==display;});
    if (owned==finalTopology.end() || !(owned->rect==placement) || !owned->active || owned->mirror)
        fail(@"macOS did not apply the requested adjacent independent display. Extension was not started.");
    activeLayout=@{@"display_id":@(display),@"placement":@{@"anchor_id":@(anchor),@"side":side,@"rect":rectDictionary(placement)},
        @"existing":displayDictionaries(baseline),@"existing_preserved":@YES};
    return display;
}

@interface LinkDeskCapture : NSObject <SCStreamOutput, SCStreamDelegate> {
    VTCompressionSessionRef _encoder;
    uint64_t _sequence;
}
@property(strong) SCStream *stream;
@property(nonatomic) uint32_t width, height;
- (void)begin:(SCDisplay *)display;
- (void)encoded:(CMSampleBufferRef)sample;
@end

static void compressed(void *ref, void *, OSStatus status, VTEncodeInfoFlags, CMSampleBufferRef sample) {
    if (status != noErr || !sample) return;
    [(__bridge LinkDeskCapture *)ref encoded:sample];
}

@implementation LinkDeskCapture
- (void)begin:(SCDisplay *)display {
    double scale = MIN(1.0, MIN(1920.0 / display.width, 1080.0 / display.height));
    self.width = ((uint32_t)(display.width * scale)) & ~1u;
    self.height = ((uint32_t)(display.height * scale)) & ~1u;
    NSDictionary *spec = @{(__bridge NSString *)kVTVideoEncoderSpecification_EnableHardwareAcceleratedVideoEncoder:@YES};
    OSStatus status = VTCompressionSessionCreate(kCFAllocatorDefault, self.width, self.height,
        kCMVideoCodecType_H264, (__bridge CFDictionaryRef)spec, NULL, NULL,
        compressed, (__bridge void *)self, &_encoder);
    if (status != noErr || !_encoder) fail(@"H.264 encoder initialization failed.");
    NSDictionary *properties = @{
        (__bridge NSString *)kVTCompressionPropertyKey_RealTime:@YES,
        (__bridge NSString *)kVTCompressionPropertyKey_AllowFrameReordering:@NO,
        (__bridge NSString *)kVTCompressionPropertyKey_ProfileLevel:(__bridge NSString *)kVTProfileLevel_H264_Baseline_AutoLevel,
        (__bridge NSString *)kVTCompressionPropertyKey_AverageBitRate:@8000000,
        (__bridge NSString *)kVTCompressionPropertyKey_ExpectedFrameRate:@30,
        (__bridge NSString *)kVTCompressionPropertyKey_MaxKeyFrameInterval:@30,
        (__bridge NSString *)kVTCompressionPropertyKey_MaxKeyFrameIntervalDuration:@1
    };
    if (VTSessionSetProperties(_encoder, (__bridge CFDictionaryRef)properties) != noErr)
        fail(@"H.264 low-latency configuration failed.");
    VTCompressionSessionPrepareToEncodeFrames(_encoder);
    SCStreamConfiguration *config = [SCStreamConfiguration new];
    config.width = self.width; config.height = self.height;
    config.minimumFrameInterval = CMTimeMake(1,30);
    config.queueDepth = 3;
    config.pixelFormat = kCVPixelFormatType_420YpCbCr8BiPlanarVideoRange;
    config.showsCursor = YES;
    SCContentFilter *filter = [[SCContentFilter alloc] initWithDisplay:display excludingWindows:@[]];
    self.stream = [[SCStream alloc] initWithFilter:filter configuration:config delegate:self];
    NSError *error = nil;
    BOOL added = [self.stream addStreamOutput:self type:SCStreamOutputTypeScreen
        sampleHandlerQueue:dispatch_queue_create("org.linkdesk.capture", DISPATCH_QUEUE_SERIAL) error:&error];
    if (!added) fail(@"Could not attach the native screen capture stream.");
    [self.stream startCaptureWithCompletionHandler:^(NSError *error) {
        if (error) fail(@"Screen capture could not start. Check Screen Recording permission and relaunch LinkDesk.");
    }];
}
- (void)stream:(SCStream *)stream didStopWithError:(NSError *)error {
    fail(@"Screen capture stopped. Reconnect after checking display and permission state.");
}
- (void)stream:(SCStream *)stream didOutputSampleBuffer:(CMSampleBufferRef)sample ofType:(SCStreamOutputType)type {
    if (type != SCStreamOutputTypeScreen || !CMSampleBufferIsValid(sample)) return;
    CVImageBufferRef image = CMSampleBufferGetImageBuffer(sample);
    if (!image) return;
    CFArrayRef attachments = CMSampleBufferGetSampleAttachmentsArray(sample, false);
    if (attachments && CFArrayGetCount(attachments)) {
        NSDictionary *info = (__bridge NSDictionary *)CFArrayGetValueAtIndex(attachments,0);
        NSNumber *status = info[SCStreamFrameInfoStatus];
        if (status && status.integerValue != SCFrameStatusComplete) return;
    }
    OSStatus result = VTCompressionSessionEncodeFrame(_encoder, image,
        CMSampleBufferGetPresentationTimeStamp(sample), kCMTimeInvalid, NULL, NULL, NULL);
    if (result != noErr) fail(@"H.264 frame encoding failed.");
}
- (void)encoded:(CMSampleBufferRef)sample {
    if (!CMSampleBufferDataIsReady(sample)) return;
    CFArrayRef attachments = CMSampleBufferGetSampleAttachmentsArray(sample, false);
    bool keyframe = true;
    if (attachments && CFArrayGetCount(attachments)) {
        CFDictionaryRef info = (CFDictionaryRef)CFArrayGetValueAtIndex(attachments,0);
        keyframe = !CFDictionaryContainsKey(info,kCMSampleAttachmentKey_NotSync);
    }
    NSMutableData *annex = [NSMutableData data];
    const uint8_t start[] = {0,0,0,1};
    CMFormatDescriptionRef format = CMSampleBufferGetFormatDescription(sample);
    if (keyframe) {
        size_t count = 0;
        if (CMVideoFormatDescriptionGetH264ParameterSetAtIndex(format,0,NULL,NULL,&count,NULL) != noErr) return;
        for (size_t i=0;i<count;i++) {
            const uint8_t *bytes = NULL; size_t size = 0;
            if (CMVideoFormatDescriptionGetH264ParameterSetAtIndex(format,i,&bytes,&size,NULL,NULL) != noErr) return;
            [annex appendBytes:start length:4];[annex appendBytes:bytes length:size];
        }
    }
    CMBlockBufferRef buffer = CMSampleBufferGetDataBuffer(sample);
    size_t size = CMBlockBufferGetDataLength(buffer);
    if (size > 8*1024*1024-1024) return;
    NSMutableData *avcc = [NSMutableData dataWithLength:size];
    if (CMBlockBufferCopyDataBytes(buffer,0,size,avcc.mutableBytes) != noErr) return;
    const uint8_t *bytes = (const uint8_t *)avcc.bytes;
    size_t offset = 0;
    while (offset + 4 <= size) {
        uint32_t n = ((uint32_t)bytes[offset]<<24)|((uint32_t)bytes[offset+1]<<16)|((uint32_t)bytes[offset+2]<<8)|bytes[offset+3];
        offset += 4;
        if (n > size-offset) return;
        [annex appendBytes:start length:4];[annex appendBytes:bytes+offset length:n];offset+=n;
    }
    if (offset != size || annex.length == 0) return;
    NSMutableData *packet = [NSMutableData data];
    uint64_t sequence = CFSwapInt64HostToBig(_sequence++);
    uint32_t w = CFSwapInt32HostToBig(self.width), h = CFSwapInt32HostToBig(self.height);
    uint8_t key = keyframe ? 1 : 0;
    [packet appendBytes:&sequence length:8];[packet appendBytes:&w length:4];
    [packet appendBytes:&h length:4];[packet appendBytes:&key length:1];[packet appendData:annex];
    record(2,packet);
}
- (void)dealloc {
    if (_encoder) {VTCompressionSessionInvalidate(_encoder);CFRelease(_encoder);}
}
@end

int main(int argc, const char *argv[]) {
    @autoreleasepool {
        signal(SIGPIPE,SIG_IGN);
        if (argc==2 && !strcmp(argv[1],"list-displays")) {
            NSData *data=[NSJSONSerialization dataWithJSONObject:@{@"displays":displayDictionaries(readTopology())} options:0 error:nil];
            if (data.length>4096) fail(@"Too many displays for this development version's layout controls.");
            record(1,data);
            return 0;
        }
        bool extending=argc==10 && !strcmp(argv[1],"extend");
        bool mirroring=argc==2 && !strcmp(argv[1],"mirror");
        uint32_t anchor=0; NSString *side=nil; linkdesk::Rect placement{0,0,1920,1080};
        if (extending) {
            if (strcmp(argv[2],"--anchor") || strcmp(argv[4],"--side") || strcmp(argv[6],"--x") || strcmp(argv[8],"--y"))
                fail(@"Invalid extension layout arguments.");
            auto parse=[](const char *text)->int64_t {
                char *end=nullptr; errno=0; long long value=strtoll(text,&end,10);
                if (errno || !*text || *end) fail(@"Invalid extension layout coordinate.");
                return value;
            };
            int64_t value=parse(argv[3]);
            if (value<=0 || value>UINT32_MAX) fail(@"Invalid anchor display.");
            anchor=(uint32_t)value;
            side=[NSString stringWithUTF8String:argv[5]];
            if (![@[@"right",@"left",@"above",@"below"] containsObject:side]) fail(@"Invalid extension side.");
            placement.x=parse(argv[7]); placement.y=parse(argv[9]);
            if (!placement.valid()) fail(@"Invalid extension bounds.");
        }
        if (!extending && !mirroring) fail(@"Expected mirror, list-displays, or extend with an explicit adjacent layout.");
        if (!CGPreflightScreenCaptureAccess() && !CGRequestScreenCaptureAccess())
            fail(@"Screen Recording permission is required. Grant it to LinkDesk and relaunch.");
        CGDirectDisplayID target = extending ? createVirtualDisplay(anchor,side,placement) : CGMainDisplayID();
        // The display server can need a moment before exposing a newly attached display.
        for (int attempt=0;attempt<20;attempt++) {
            __block bool done = false;
            __block SCDisplay *selected = nil;
            [SCShareableContent getShareableContentExcludingDesktopWindows:NO onScreenWindowsOnly:NO
                completionHandler:^(SCShareableContent *content,NSError *error) {
                    for (SCDisplay *display in content.displays) if (display.displayID==target) selected=display;
                    done=true;
                }];
            NSDate *deadline = [NSDate dateWithTimeIntervalSinceNow:3];
            while (!done && deadline.timeIntervalSinceNow>0)
                [[NSRunLoop currentRunLoop] runUntilDate:[NSDate dateWithTimeIntervalSinceNow:0.01]];
            if (selected) {
                LinkDeskCapture *capture = [LinkDeskCapture new];captureOwner=capture;
                [capture begin:selected];
                NSMutableDictionary *ready=[@{@"width":@(capture.width),@"height":@(capture.height),@"display_id":@(target),@"experimental":@(extending)} mutableCopy];
                if (activeLayout) ready[@"layout"]=activeLayout;
                record(1,[NSJSONSerialization dataWithJSONObject:ready options:0 error:nil]);
                [[NSRunLoop mainRunLoop] run];
                return 0;
            }
            usleep(100000);
        }
        fail(@"The requested display did not become available to ScreenCaptureKit.");
    }
}
