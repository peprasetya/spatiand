//
//  SPVirtualDisplay.h — the HoloFrame canvas, as a macOS virtual display.
//
//  Wraps the private CGVirtualDisplay SPI. Everything about why this is shaped the way it
//  is — the XYZ primaries, the mode ladder, and the mirroring trap — is in
//  virtualDisplay.md at the repo root. Read that before changing anything here.
//

#import <Foundation/Foundation.h>
#import <CoreGraphics/CoreGraphics.h>

NS_ASSUME_NONNULL_BEGIN

@interface SPVirtualDisplay : NSObject

/// Create a screen that offers each of the sizes given (`widths[i]` x `heights[i]`, in
/// points) for macOS's Displays settings to choose among; they need not be of one shape.
///
/// `hiDPI` NO gives one point per pixel — the most desktop real estate, and a quarter of
/// the capture bandwidth of the 2x equivalent. YES has macOS offer each size at two pixels
/// a point as well.
///
/// `serialNumber` must be **stable across launches**. macOS identifies a display by
/// vendor/product/serial and keys its remembered resolution and arrangement off that, so a
/// random serial makes the user rearrange their desktop every single run.
///
/// Returns nil if the display could not be created. The canvas lives exactly as long as
/// this object.
- (nullable instancetype)initWithModeWidths:(NSArray<NSNumber *> *)widths
                                    heights:(NSArray<NSNumber *> *)heights
                                      hiDPI:(BOOL)hiDPI
                               serialNumber:(uint32_t)serialNumber
                                       name:(NSString *)name;

/// 0 until the display registers.
@property(readonly, nonatomic) CGDirectDisplayID displayID;

/// Pixel size of the mode currently selected, or 0x0 if the display is not active.
@property(readonly, nonatomic) CGSize currentPixelSize;

/// The same in points: half the pixels in a mode at two pixels a point.
@property(readonly, nonatomic) CGSize currentPointSize;

/// Select the mode of this size in points, at one pixel a point or (`hiDPI`) two. macOS
/// otherwise leaves the choice to the user in System Settings, and to begin with lands on
/// a small one. NO if the display offers no such mode (yet: its modes are not there the
/// moment it exists).
- (BOOL)selectWidth:(uint32_t)width height:(uint32_t)height hiDPI:(BOOL)hiDPI;

/// YES if any online display is mirroring another.
///
/// Check this before trusting anything about a display's mode: a mirroring display reports
/// the resolution of the display it mirrors, is absent from the active list, and silently
/// ignores CGConfigureDisplayWithDisplayMode. Misreading that state cost this project
/// hours.
+ (BOOL)anyDisplayIsMirroring;

/// Release every online display from mirroring, in one configuration transaction.
///
/// Releasing only the main display is not enough — the others keep mirroring it.
+ (BOOL)disableAllMirroring;

@end

NS_ASSUME_NONNULL_END
