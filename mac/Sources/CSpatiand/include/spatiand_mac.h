// The room in the glasses: the Deck's compositor, as a library. See crates/spatiand-mac, whose
// src/mac/ffi.rs is this file's other half; keep the two in step.
//
// The app owns what is AppKit's -- the window on the glasses' screen, the Mac's pointer and
// keyboard, screen capture, the menu -- and tells the compositor about it with the calls below.
// The compositor runs on threads of its own and asks things of the app through one callback,
// which may arrive on any of them: copy what is needed and return.
#ifndef SPATIAND_MAC_H
#define SPATIAND_MAC_H

#include <CoreFoundation/CoreFoundation.h>
#include <IOSurface/IOSurfaceRef.h>
#include <stdbool.h>
#include <stddef.h>
#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

// What the compositor asks of the app, or tells it.
enum {
    SP_RETURN_TO_DESKTOP = 1, // leave the room: give the Mac its pointer and keyboard back
    SP_SYSTEM_SETTINGS = 2,   // open a pane of System Settings in the room; text is "wifi" or "bluetooth"
    SP_LAUNCH = 3,            // open an application and bring its windows in; text is what sp_app gave
    SP_HAPTIC = 4,            // a tick under the finger; a is 0 click, 1 tick, 2 alert
    SP_SAVED = 5,             // a screenshot or a video was saved at text
    SP_NOTICE_PRESSED = 6,    // the notification on show was pressed
    SP_WINDOW_MOTION = 10,    // the pointer is at (a, b) in window id's own pixels
    SP_WINDOW_LEAVE = 11,     // the pointer left window id
    SP_WINDOW_BUTTON = 12,    // button a (0x110 left, 0x111 right, 0x112 middle) went down (b = 1) or up
    SP_WINDOW_SCROLL = 13,    // scrolled by (a across, b down); a wheel notch is 15
    SP_WINDOW_KEY = 14,       // key a (a Mac virtual key code, -1 for none; c is evdev's) went down (b = 1) or up
    SP_WINDOW_FOCUS = 15,     // the keyboard is window id's now, or nobody's of this Mac's (id 0)
    SP_WINDOW_RESIZE = 16,    // window id should be a by b pixels
    SP_WINDOW_CLOSE = 17,     // window id should close
    SP_WINDOW_SHOWN = 18,     // window id is on show in the room: its picture is wanted
    SP_WINDOW_HIDDEN = 19,
    SP_RECORD_START = 20,     // film what the glasses show, from now
    SP_RECORD_STOP = 21,      // and stop, and save it
    SP_AUDIO_LIST = 22,       // say what sound devices there are (sp_audio_devices_begin...)
    SP_AUDIO_CHOOSE = 23,     // use device id: the microphone if a is 1, else the output    // window id has been put away: no picture is needed
    SP_MENU_OPEN = 24,        // say the menus of window id's application, from the top (sp_menu_begin...)
    SP_MENU_CHOOSE = 25,      // row a of the menu last said was chosen
    SP_MENU_BACK = 26,        // up a level, or out from the top
    // 2D mode (sp_desk): the session's windows shown on the Mac's desktop.
    SP_DESK_OPEN = 30,        // show window id, a by b pixels; text is its title, "\n", its app id
    SP_DESK_PICTURE = 31,     // window id's newest picture is the global IOSurface numbered a
    SP_DESK_TITLE = 32,       // window id's title is text
    SP_DESK_SIZE = 33,        // window id is a by b pixels now
    SP_DESK_CLOSE = 34,       // window id has closed, or the glasses have it now
    SP_CLIPBOARD = 35,        // the session's clipboard to the pasteboard; text is {"host": {"Clipboard": ...}}
};

typedef void (*sp_asked_fn)(void *user, int32_t what, uint32_t id, double a, double b, double c,
                            const char *text);

// Start the session, once. Returns at once. Everything below may be called before it.
void sp_begin(sp_asked_fn callback, void *user);
void sp_end(void);
bool sp_running(void);

// The CALayer on the glasses' screen to draw into, its size in pixels (twice as wide as high
// and more is two eyes side by side), and the CGDirectDisplayID it is on, whose refresh paces
// the frames (0 for the Mac's own display). The layer is retained.
void sp_glasses(void *layer, int32_t width, int32_t height, uint32_t display);
// The glasses' screen has gone. Returns once nothing is drawing into its layer.
void sp_glasses_gone(void);
// Whether the glasses' sensors are to be held at all.
void sp_hold_glasses(bool hold);

// The Mac's pointer: travel in points (+y down), the buttons down (1 primary, 2 secondary,
// 4 middle), scrolling in wheel notches (+ right, + up).
void sp_pointer(float dx, float dy, int32_t buttons, float wheel_right, float wheel_up);
// Whether the room has the Mac's pointer. Without it there is no pointer in the room.
void sp_pointer_held(bool held);
void sp_pointer_speed(float speed);
// Two fingers spreading (above 1) or closing, since the last call.
void sp_pinch(float scale);
// A key of the Mac's keyboard, by its virtual key code; or by its evdev code.
void sp_key(uint16_t mac_code, bool pressed);
void sp_key_evdev(uint32_t code, bool pressed);
// One of the Deck's buttons: 0 STEAM (settings), 1 ... (launcher), 2 A, 3 B, 4 X, 5 Y, 6 up,
// 7 down, 8 left, 9 right, 10 menu, 11 view.
void sp_control(int32_t control, bool pressed);
void sp_recentre(void);
// Close the window that has the keyboard.
void sp_close_window(void);
void sp_screenshot(void);

// Game controllers. Buttons: 0 A, 1 B, 2 X, 3 Y, 4 L1, 5 R1, 6 L2, 7 R2, 8 select, 9 start,
// 10 guide, 11 left stick, 12 right stick, 13 up, 14 down, 15 left, 16 right. Sticks and the
// touchpad are -1..1 with +y up, triggers 0..1, the gyro degrees a second about X, Y and Z.
void sp_pad_button(int32_t pad, int32_t button, bool down);
void sp_pad_axes(int32_t pad, float lx, float ly, float rx, float ry, float lt, float rt);
void sp_pad_touch(int32_t pad, float x, float y, bool touched, bool clicked);
void sp_pad_gyro(int32_t pad, float x, float y, float z);
void sp_pad_gone(int32_t pad);
// The rumble a game asked for since the last call, strong << 16 | weak, or -1 for none.
int64_t sp_pad_rumble(void);

// This Mac's applications, for the launcher: begin, one sp_app each, end. Before sp_begin.
void sp_apps_begin(void);
void sp_app(const char *name, const char *open_with, const char *icon_png_path);
void sp_apps_end(void);

// A computer this Mac has paired with, for the room to connect to. Before sp_begin.
void sp_paired_host(const char *address, const char *fingerprint);

// How many pixels a point of this Mac's screen is: 2 on a Retina display.
/// Where the room's pointer is, each from -1 to 1 across the view. For tracing.
void sp_pointer_where(float *x, float *y);
// This Mac's sound devices, for the room's settings: begin, each one, end, from one thread.
void sp_audio_devices_begin(void);
void sp_audio_device(uint32_t id, const char *name, bool input, bool current);
void sp_audio_devices_end(void);
void sp_display_scale(double scale);
/// Whether macOS has allowed the microphone: it is not opened for a host until it has.
void sp_microphone_allowed(bool allowed);

// A window of this Mac's, for the room. id is the app's own and never 0; the size is its
// picture's, in pixels. hidden lists it without showing it: it is in the room's list of windows
// until it is brought out, from there or with sp_window_show, and SP_WINDOW_SHOWN says when.
void sp_window_open(uint32_t id, const char *bundle, const char *title, uint32_t width,
                    uint32_t height, bool hidden);
void sp_window_show(uint32_t id);
// Its newest picture, an IOSurfaceRef, held for as long as it is shown.
// x, y, width, height: the part of the surface that is the window, in pixels; a width of 0 is all of it.
void sp_window_picture(uint32_t id, const void *surface, uint32_t x, uint32_t y, uint32_t width, uint32_t height);
/// Whether a window's picture has parts that are nothing, to be seen through.
void sp_window_clear(uint32_t id, bool on);
/// A level of an application's menu: begin with what it is called, each row, end. And done with.
void sp_menu_begin(const char *title);
void sp_menu_row(const char *label, const char *detail);
void sp_menu_end(void);
void sp_menu_close(void);
void sp_window_title(uint32_t id, const char *title);
void sp_window_close(uint32_t id);

// A notification to show under the clock: its sender on the first line and what it says on
// the second, or "" for none. SP_NOTICE_PRESSED says when the wearer presses it.
void sp_notice(const char *text);

// Play to this Core Audio device; 0 for the system's default output.
void sp_audio_output(int32_t device);
// Sound one of this Mac's applications made: interleaved 16-bit at 48 kHz. Placed at its window.
void sp_app_sound(const char *bundle, const int16_t *pcm, uint32_t frames, uint32_t channels);

// A line saying how the session is. Returns its length.
size_t sp_status(char *into, size_t size);

// 2D mode: Spatiand's own applications as windows on the Mac's desktop. With sp_desk on and no
// glasses' screen given, the session shows nothing itself and says each of its windows with
// SP_DESK_*; what is done to the Mac window comes back here, in the window's own pixels from its
// top left. Off, the glasses have the same windows.
void sp_desk(bool on);
// The picture an application handed over as `number` (SP_DESK_PICTURE's), in private: see
// crates/spatiand-surfaces. NULL if there is none.
IOSurfaceRef _Nullable spatiand_surfaces_open(uint32_t number) CF_RETURNS_RETAINED;
// Open one of Spatiand's own applications by name ("SpatiMesh").
void sp_desk_launch(const char *name);
// The names of Spatiand's own applications that are here, one a line.
void sp_desk_apps(char *buffer, size_t capacity);
void sp_desk_motion(uint32_t id, double x, double y);
void sp_desk_leave(uint32_t id);
void sp_desk_button(uint32_t id, uint32_t button, bool pressed); // 0x110 left, 0x111 right, 0x112 middle
void sp_desk_scroll(uint32_t id, double dx, double dy, bool wheel); // pixels across and down; a notch is 15
void sp_desk_key(uint32_t id, uint16_t mac_code, bool pressed);
void sp_desk_focus(uint32_t id, bool focused);
void sp_desk_pinch(uint32_t id, float scale);
void sp_desk_resize(uint32_t id, int32_t width, int32_t height);
void sp_desk_close(uint32_t id);

// The Mac's pasteboard is a peer of the session's clipboard, as a host is: what it has to say,
// as the 2D link says it to a host ({"Clipboard": {"Offer"|"Want"|"Data": ...}}). The session's
// side comes as SP_CLIPBOARD.
void sp_clipboard_said(const char *json);

#ifdef __cplusplus
}
#endif

#endif
