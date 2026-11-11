//
//  SPVirtualDisplay.m
//
//  Port of BetterDummy's working recipe (waydabber/BetterDisplay, branch `opensource`,
//  BetterDummy/Model/Dummy.swift, MIT).
//

#import "include/SPVirtualDisplay.h"
#import <AppKit/AppKit.h>

#pragma mark - private SPI, re-declared from the runtime (see Tools/dumpvd.m)

@interface CGVirtualDisplayMode : NSObject
- (instancetype)initWithWidth:(unsigned int)width height:(unsigned int)height refreshRate:(double)rate;
@end

@interface CGVirtualDisplayDescriptor : NSObject
@property(retain, nonatomic) id queue;
@property(retain, nonatomic) NSString *name;
@property(nonatomic) CGPoint whitePoint, redPrimary, greenPrimary, bluePrimary;
@property(nonatomic) unsigned int maxPixelsWide, maxPixelsHigh;
@property(nonatomic) CGSize sizeInMillimeters;
@property(nonatomic) unsigned int serialNum, productID, vendorID;
@property(copy, nonatomic) id terminationHandler;
@end

@interface CGVirtualDisplaySettings : NSObject
@property(nonatomic) unsigned int hiDPI;
@property(retain, nonatomic) NSArray *modes;
@end

@interface CGVirtualDisplay : NSObject
- (instancetype)initWithDescriptor:(CGVirtualDisplayDescriptor *)descriptor;
- (BOOL)applySettings:(CGVirtualDisplaySettings *)settings;
@property(readonly, nonatomic) unsigned int displayID;
@end

#pragma mark -

@implementation SPVirtualDisplay {
    CGVirtualDisplay *_display;
}

- (nullable instancetype)initWithModeWidths:(NSArray<NSNumber *> *)widths
                                    heights:(NSArray<NSNumber *> *)heights
                                      hiDPI:(BOOL)hiDPI
                               serialNumber:(uint32_t)serialNumber
                                       name:(NSString *)name {
    self = [super init];
    if (!self) return nil;
    if (widths.count == 0 || widths.count != heights.count) return nil;

    // Deliberately NOT calling NSApplicationLoad() here. Initialising AppKit before the
    // display exists makes NSScreen cache a list without it, and that stale list then
    // misplaces every window we create. The caller brings AppKit up afterwards instead.
    //
    // **A list of sizes, of any shape, and macOS offers them all** (System Settings >
    // Displays, and CGDisplayCopyAllDisplayModes): 1280x720 up to 7680x2160 together, tried
    // on macOS 27.2, 2026-11-11. They need not be one aspect, as the ladder this was first
    // made with (BetterDummy's) was; a lone mode is still not reliably accepted.
    //
    // With hiDPI each size is offered at two pixels a point as well (and macOS adds sizes of
    // its own): the pixels allowed for must then be twice the largest, and the largest is
    // offered at two pixels a point only.
    uint32_t scale = hiDPI ? 2 : 1;
    uint32_t maxW = 0, maxH = 0;
    NSMutableArray *modes = [NSMutableArray array];
    for (NSUInteger i = 0; i < widths.count; i++) {
        uint32_t w = widths[i].unsignedIntValue, h = heights[i].unsignedIntValue;
        if (w < 320 || h < 200) continue;
        CGVirtualDisplayMode *m = [[CGVirtualDisplayMode alloc] initWithWidth:w height:h refreshRate:60.0];
        if (!m) continue;
        [modes addObject:m];
        if (w > maxW) maxW = w;
        if (h > maxH) maxH = h;
    }
    if (modes.count == 0) return nil;

    CGVirtualDisplayDescriptor *desc = [[CGVirtualDisplayDescriptor alloc] init];
    desc.queue = dispatch_get_global_queue(QOS_CLASS_USER_INTERACTIVE, 0);
    desc.name = name;
    // These are XYZ tristimulus values, NOT xy chromaticities — their Y components sum to
    // 1.000. Passing sRGB xy values here yields a degenerate colorspace and the display
    // publishes only a 1x1 mode, while every call still reports success.
    desc.whitePoint   = CGPointMake(0.950, 1.000);
    desc.redPrimary   = CGPointMake(0.454, 0.242);
    desc.greenPrimary = CGPointMake(0.353, 0.674);
    desc.bluePrimary  = CGPointMake(0.157, 0.084);
    // Must cover the largest mode, each way.
    desc.maxPixelsWide = maxW * scale;
    desc.maxPixelsHigh = maxH * scale;
    // Nominally a 16:9 panel of 27 inches: nothing is drawn to its size.
    desc.sizeInMillimeters = CGSizeMake(598, 336);
    // Stable across launches, so macOS remembers this display's resolution and where the
    // user put it. A random value here means rearranging the desktop every run.
    desc.serialNum = serialNumber;
    desc.productID = 0x0F10;
    desc.vendorID  = 0xF0F0;

    _display = [[CGVirtualDisplay alloc] initWithDescriptor:desc];
    if (!_display) return nil;

    CGVirtualDisplaySettings *settings = [[CGVirtualDisplaySettings alloc] init];
    settings.hiDPI = hiDPI ? 1 : 0;
    settings.modes = modes;
    if (![_display applySettings:settings]) return nil;

    return self;
}

- (CGDirectDisplayID)displayID {
    return _display ? _display.displayID : 0;
}

- (CGSize)currentPixelSize {
    CGDirectDisplayID did = self.displayID;
    if (!did) return CGSizeZero;
    CGDisplayModeRef m = CGDisplayCopyDisplayMode(did);
    if (!m) return CGSizeZero;
    CGSize s = CGSizeMake(CGDisplayModeGetPixelWidth(m), CGDisplayModeGetPixelHeight(m));
    CGDisplayModeRelease(m);
    return s;
}

- (CGSize)currentPointSize {
    CGDirectDisplayID did = self.displayID;
    if (!did) return CGSizeZero;
    CGDisplayModeRef m = CGDisplayCopyDisplayMode(did);
    if (!m) return CGSizeZero;
    CGSize s = CGSizeMake(CGDisplayModeGetWidth(m), CGDisplayModeGetHeight(m));
    CGDisplayModeRelease(m);
    return s;
}

- (BOOL)selectWidth:(uint32_t)width height:(uint32_t)height hiDPI:(BOOL)hiDPI {
    CGDirectDisplayID did = self.displayID;
    if (!did) return NO;
    // The modes at two pixels a point are listed only when asked for.
    NSDictionary *options = @{(__bridge NSString *)kCGDisplayShowDuplicateLowResolutionModes: @YES};
    CFArrayRef modes = CGDisplayCopyAllDisplayModes(did, (__bridge CFDictionaryRef)options);
    if (!modes) return NO;

    CGDisplayModeRef found = NULL;
    for (CFIndex i = 0; i < CFArrayGetCount(modes); i++) {
        CGDisplayModeRef m = (CGDisplayModeRef)CFArrayGetValueAtIndex(modes, i);
        if (CGDisplayModeGetWidth(m) != width || CGDisplayModeGetHeight(m) != height) continue;
        if ((CGDisplayModeGetPixelWidth(m) != CGDisplayModeGetWidth(m)) != (hiDPI ? true : false)) continue;
        found = m;
        if (CGDisplayModeIsUsableForDesktopGUI(m)) break;
    }
    BOOL ok = NO;
    if (found) {
        CGDisplayConfigRef config;
        if (CGBeginDisplayConfiguration(&config) == kCGErrorSuccess) {
            CGConfigureDisplayWithDisplayMode(config, did, found, NULL);
            ok = CGCompleteDisplayConfiguration(config, kCGConfigurePermanently) == kCGErrorSuccess;
        }
    }
    CFRelease(modes);
    return ok;
}

+ (BOOL)anyDisplayIsMirroring {
    uint32_t n = 0;
    CGDirectDisplayID ids[32];
    if (CGGetOnlineDisplayList(32, ids, &n) != kCGErrorSuccess) return NO;
    for (uint32_t i = 0; i < n; i++) {
        if (CGDisplayMirrorsDisplay(ids[i]) != kCGNullDirectDisplay) return YES;
    }
    return NO;
}

+ (BOOL)disableAllMirroring {
    uint32_t n = 0;
    CGDirectDisplayID ids[32];
    if (CGGetOnlineDisplayList(32, ids, &n) != kCGErrorSuccess) return NO;
    CGDisplayConfigRef config;
    if (CGBeginDisplayConfiguration(&config) != kCGErrorSuccess) return NO;
    for (uint32_t i = 0; i < n; i++) {
        CGConfigureDisplayMirrorOfDisplay(config, ids[i], kCGNullDirectDisplay);
    }
    return CGCompleteDisplayConfiguration(config, kCGConfigurePermanently) == kCGErrorSuccess;
}

@end
