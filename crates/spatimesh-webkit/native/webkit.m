/* WebKit (WKWebView) behind the C interface every engine's native half provides
 * (crates/spatimesh-shim): SpatiMesh's engine on the Mac.
 *
 * Every tab is a WKWebView in a window of its own that nobody sees: borderless, transparent,
 * ignoring the mouse, kept out of screen capture, at the screen's origin (so that a point in
 * the window and on the screen are the same point, which synthesised scroll events need).
 * WebKit would stop drawing a page nobody can see, so its occlusion detection is switched off
 * (SPI, `_setWindowOcclusionDetectionEnabled:`, where the system has it). Pictures are taken by
 * WebKit itself (`takeSnapshotWithConfiguration:`), often while the page is busy and seldom
 * while it is not, and passed on only when they changed. Input is given to the view as the
 * NSEvents AppKit would have made. What WebKit would answer with a menu of its own -- a
 * <select>'s list, a context menu -- is taken from it: the list by a script in the page that
 * asks SpatiMesh instead, the context menu by refusing it.
 *
 * Written on Linux and compiled by CI's Mac job; not run by its author on a Mac. */

#import <AppKit/AppKit.h>
#import <WebKit/WebKit.h>
#import <objc/message.h>
#include <stdint.h>
#include <stdlib.h>
#include <string.h>

enum {
    EV_TITLE = 1,
    EV_URL,
    EV_PROGRESS,
    EV_HISTORY,
    EV_CURSOR,
    EV_SOUND,
    EV_OPEN_TAB,
    EV_CLOSE_TAB,
    EV_CRASHED,
    EV_FAILED,
    EV_CHOOSE,
    EV_DIALOG,
    EV_EXTENSION,
    EV_SCRIPT_RESULT,
    EV_MEDIA,
};

typedef struct {
    int kind;
    uint64_t tab;
    double number;
    int a, b;
    char *text;
    char *text2;
    int x, y, w, h;
} MeshEvent;

typedef struct Engine Engine;

void spatimesh_sound_playing(const pid_t *pids, size_t count);
static void push(Engine *e, int kind, uint64_t tab, double number, int a, int b, NSString *text, NSString *text2);
static void wake(void);

/* The page's own <select> lists, asked of SpatiMesh instead of shown by WebKit. */
static NSString *const SelectScript =
    @"(function(){"
     "function opts(s){var r=[];for(var i=0;i<s.options.length;i++){var o=s.options[i];"
     "r.push({label:o.label||o.text,enabled:!o.disabled&&!(o.parentElement&&o.parentElement.disabled),"
     "selected:o.selected,heading:false});}return r;}"
     "document.addEventListener('mousedown',function(ev){var s=ev.target.closest&&ev.target.closest('select');"
     "if(!s||s.multiple||s.size>1||s.disabled)return;ev.preventDefault();s.focus();"
     "window.__spatimeshSelect=s;var r=s.getBoundingClientRect();"
     "window.webkit.messageHandlers.spatimeshSelect.postMessage({options:opts(s),"
     "x:r.left,y:r.top,w:r.width,h:r.height,dpr:window.devicePixelRatio});},true);"
     "window.__spatimeshChoose=function(i){var s=window.__spatimeshSelect;window.__spatimeshSelect=null;"
     "if(!s||i<0||i>=s.options.length)return;if(s.selectedIndex!==i){s.selectedIndex=i;"
     "s.dispatchEvent(new Event('input',{bubbles:true}));s.dispatchEvent(new Event('change',{bubbles:true}));}};"
     "})();";

/* No context menu: WebKit's would be a real menu, modal, on a screen nobody looks at. */
@interface MeshWebView : WKWebView
@end

@implementation MeshWebView
- (NSMenu *)menuForEvent:(NSEvent *)event {
    return nil;
}
- (void)willOpenMenu:(NSMenu *)menu withEvent:(NSEvent *)event {
    [menu cancelTrackingWithoutAnimation];
}
@end

/* Extensions (docs/spatimesh.md, "Extensions"): one, as the window gave it (crates/spatimesh-shim):
 * its scheme, its content scripts' world, and the code that sets its API up in a content
 * script's world and in its own pages. */
@interface MeshExtension : NSObject
@property(nonatomic, copy) NSString *ident;
@property(nonatomic, copy) NSString *scheme;
@property(nonatomic, copy) NSString *world;
@property(nonatomic, copy) NSString *contentBoot;
@property(nonatomic, copy) NSString *pageBoot;
@end

@implementation MeshExtension
@end

/* One content-script entry of an extension's: when, where, for which pages (WebKit's own
 * patterns: none for every page), and its files' code. `guard` is JavaScript that is true where
 * the entry is wanted, for what the patterns cannot say exactly (`exact` NO). */
@interface MeshEntry : NSObject
@property(nonatomic, strong) MeshExtension *extension;
@property(nonatomic) int runAt;
@property(nonatomic) BOOL allFrames;
@property(nonatomic) BOOL mainWorld;
@property(nonatomic, copy) NSArray<NSString *> *allow;
@property(nonatomic, copy) NSArray<NSString *> *block;
@property(nonatomic, copy) NSString *guard;
@property(nonatomic) BOOL exact;
@property(nonatomic, strong) NSMutableArray<NSString *> *js;
@property(nonatomic, strong) NSMutableArray<NSString *> *css;
@end

@implementation MeshEntry
@end

/* What answers chrome-extension:// and moz-extension:// in every tab. */
@interface MeshSchemes : NSObject <WKURLSchemeHandler>
@end

/* WebKit's own match patterns for a user script (SPI, what Safari's extensions use), where the
 * system has them; where it has not, each script guards itself. */
@interface WKUserScript (SpatiMeshPatterns)
- (instancetype)_initWithSource:(NSString *)source
                  injectionTime:(WKUserScriptInjectionTime)injectionTime
               forMainFrameOnly:(BOOL)forMainFrameOnly
     includeMatchPatternStrings:(NSArray<NSString *> *)includeMatchPatternStrings
     excludeMatchPatternStrings:(NSArray<NSString *> *)excludeMatchPatternStrings
                  associatedURL:(NSURL *)associatedURL
                   contentWorld:(WKContentWorld *)contentWorld
  deferRunningUntilNotification:(BOOL)deferRunningUntilNotification;
@end

/* Extensions' addresses as secure ones, as Chrome's are (SPI, asked for only where it is there). */
@interface WKProcessPool (SpatiMeshSchemes)
- (void)_registerURLSchemeAsSecure:(NSString *)scheme;
- (void)_registerURLSchemeAsBypassingContentSecurityPolicy:(NSString *)scheme;
@end

@interface MeshTab : NSObject <WKNavigationDelegate, WKUIDelegate, WKScriptMessageHandler, WKDownloadDelegate>
@property(nonatomic) uint64_t ident;
@property(nonatomic) Engine *engine;
@property(nonatomic, strong) NSWindow *window;
@property(nonatomic, strong) MeshWebView *view;
/* Frames other than the top one that an extension's context speaks from, by world ("" the
 * page's own) and the frame's number in api.js: where code for every frame goes. */
@property(nonatomic, strong) NSMutableDictionary<NSString *, NSMutableDictionary<NSNumber *, WKFrameInfo *> *> *frames;
/* The worlds this tab's view has the extensions' message handler in. */
@property(nonatomic, strong) NSMutableArray<WKContentWorld *> *handlerWorlds;
@property(nonatomic) int width;
@property(nonatomic) int height;
@property(nonatomic) unsigned char *pixels;
@property(nonatomic) int pixelsWidth;
@property(nonatomic) int pixelsHeight;
@property(nonatomic) BOOL damaged;
@property(nonatomic) BOOL snapshotting;
@property(nonatomic) CFTimeInterval lastSnapshot;
@property(nonatomic) CFTimeInterval busyUntil;
@property(nonatomic) NSUInteger buttons;
@property(nonatomic) BOOL playing;
@property(nonatomic, copy) void (^alertDone)(void);
@property(nonatomic, copy) void (^confirmDone)(BOOL);
@property(nonatomic, copy) void (^promptDone)(NSString *);
@end

struct Engine {
    WKWebsiteDataStore *store;
    NSMutableDictionary<NSNumber *, MeshTab *> *tabs;
    MeshEvent *events;
    size_t count, capacity, head;
    MeshEvent current;
    BOOL woken;
    CFFileDescriptorRef watch;
    NSTimer *timer;
    /* The profile's extensions, their content scripts, and what serves their files. */
    NSMutableArray<MeshExtension *> *extensions;
    NSMutableArray<MeshEntry *> *entries;
    MeshSchemes *schemes;
    /* declarativeNetRequest: each extension's rules as WebKit's content blockers (JSON), and
     * those compiled, by extension. */
    NSMutableDictionary<NSString *, NSString *> *ruleSources;
    NSMutableDictionary<NSString *, WKContentRuleList *> *ruleLists;
    /* SpatiMesh's own script for every frame's every document (its media), and what the top
     * frame is given before it, to say things to the window with. */
    NSString *pageScript;
};

static Engine *the_engine;
static void apply_rule_lists(MeshTab *tab);

static char *copy_string(NSString *s) {
    const char *utf8 = s ? s.UTF8String : "";
    return strdup(utf8 ? utf8 : "");
}

static void push(Engine *e, int kind, uint64_t tab, double number, int a, int b, NSString *text, NSString *text2) {
    if (e->count == e->capacity) {
        size_t capacity = e->capacity ? e->capacity * 2 : 64;
        MeshEvent *grown = calloc(capacity, sizeof(MeshEvent));
        for (size_t i = 0; i < e->count; i++)
            grown[i] = e->events[(e->head + i) % e->capacity];
        free(e->events);
        e->events = grown;
        e->capacity = capacity;
        e->head = 0;
    }
    MeshEvent *ev = &e->events[(e->head + e->count) % e->capacity];
    memset(ev, 0, sizeof *ev);
    ev->kind = kind;
    ev->tab = tab;
    ev->number = number;
    ev->a = a;
    ev->b = b;
    ev->text = text ? copy_string(text) : NULL;
    ev->text2 = text2 ? copy_string(text2) : NULL;
    e->count++;
    wake();
}

/* Break the pump's wait: something to tell, or the window's socket has something. */
static void wake(void) {
    if (!the_engine)
        return;
    the_engine->woken = YES;
    NSEvent *nudge = [NSEvent otherEventWithType:NSEventTypeApplicationDefined location:NSZeroPoint modifierFlags:0 timestamp:0
                                     windowNumber:0 context:nil subtype:77 data1:0 data2:0];
    [NSApp postEvent:nudge atStart:NO];
}

static CFTimeInterval now(void) {
    return [[NSProcessInfo processInfo] systemUptime];
}

@implementation MeshTab

- (void)dealloc {
    free(_pixels);
}

- (void)busy {
    self.busyUntil = now() + 2.0;
}

- (void)observeValueForKeyPath:(NSString *)keyPath ofObject:(id)object change:(NSDictionary *)change context:(void *)context {
    WKWebView *v = self.view;
    if ([keyPath isEqualToString:@"title"]) {
        push(self.engine, EV_TITLE, self.ident, 0, 0, 0, v.title ?: @"", nil);
    } else if ([keyPath isEqualToString:@"URL"]) {
        push(self.engine, EV_URL, self.ident, 0, 0, 0, v.URL.absoluteString ?: @"", nil);
        push(self.engine, EV_HISTORY, self.ident, 0, v.canGoBack, v.canGoForward, nil, nil);
    } else if ([keyPath isEqualToString:@"estimatedProgress"] || [keyPath isEqualToString:@"loading"]) {
        push(self.engine, EV_PROGRESS, self.ident, v.estimatedProgress, v.loading, 0, nil, nil);
        [self busy];
    } else if ([keyPath isEqualToString:@"canGoBack"] || [keyPath isEqualToString:@"canGoForward"]) {
        push(self.engine, EV_HISTORY, self.ident, 0, v.canGoBack, v.canGoForward, nil, nil);
    }
}

/* ---- navigation ---- */

- (void)webView:(WKWebView *)webView decidePolicyForNavigationAction:(WKNavigationAction *)action decisionHandler:(void (^)(WKNavigationActionPolicy))decide {
    BOOL link = action.navigationType == WKNavigationTypeLinkActivated;
    BOOL background = action.buttonNumber == 2 || (action.modifierFlags & NSEventModifierFlagCommand);
    if (link && background && action.request.URL) {
        push(self.engine, EV_OPEN_TAB, self.ident, 0, 1, 0, action.request.URL.absoluteString, nil);
        decide(WKNavigationActionPolicyCancel);
        return;
    }
    decide(WKNavigationActionPolicyAllow);
}

- (void)webView:(WKWebView *)webView decidePolicyForNavigationResponse:(WKNavigationResponse *)response decisionHandler:(void (^)(WKNavigationResponsePolicy))decide {
    if (!response.canShowMIMEType) {
        if (@available(macOS 11.3, *)) {
            decide(WKNavigationResponsePolicyDownload);
            return;
        }
    }
    decide(WKNavigationResponsePolicyAllow);
}

- (void)webView:(WKWebView *)webView navigationResponse:(WKNavigationResponse *)navigationResponse didBecomeDownload:(WKDownload *)download API_AVAILABLE(macos(11.3)) {
    download.delegate = self;
}

- (void)webView:(WKWebView *)webView navigationAction:(WKNavigationAction *)navigationAction didBecomeDownload:(WKDownload *)download API_AVAILABLE(macos(11.3)) {
    download.delegate = self;
}

- (void)download:(WKDownload *)download decideDestinationUsingResponse:(NSURLResponse *)response suggestedFilename:(NSString *)name completionHandler:(void (^)(NSURL *))done API_AVAILABLE(macos(11.3)) {
    NSURL *folder = [[NSFileManager defaultManager] URLsForDirectory:NSDownloadsDirectory inDomains:NSUserDomainMask].firstObject;
    NSString *base = name.length ? name.lastPathComponent : @"download";
    NSURL *target = [folder URLByAppendingPathComponent:base];
    for (int n = 2; [[NSFileManager defaultManager] fileExistsAtPath:target.path] && n < 1000; n++) {
        NSString *stem = base.stringByDeletingPathExtension;
        NSString *ext = base.pathExtension;
        NSString *numbered = ext.length ? [NSString stringWithFormat:@"%@ %d.%@", stem, n, ext] : [NSString stringWithFormat:@"%@ %d", stem, n];
        target = [folder URLByAppendingPathComponent:numbered];
    }
    done(target);
}

- (void)webView:(WKWebView *)webView didFailProvisionalNavigation:(WKNavigation *)navigation withError:(NSError *)error {
    [self failed:error];
}

- (void)webView:(WKWebView *)webView didFailNavigation:(WKNavigation *)navigation withError:(NSError *)error {
    [self failed:error];
}

- (void)failed:(NSError *)error {
    /* Stopped, or turned into a download: not failures anyone needs telling about. */
    if ([error.domain isEqualToString:NSURLErrorDomain] && error.code == NSURLErrorCancelled)
        return;
    if ([error.domain isEqualToString:@"WebKitErrorDomain"] && error.code == 102)
        return;
    NSString *url = error.userInfo[NSURLErrorFailingURLStringErrorKey] ?: @"";
    push(self.engine, EV_FAILED, self.ident, 0, 0, 0, url, error.localizedDescription ?: @"it could not be loaded");
}

- (void)webViewWebContentProcessDidTerminate:(WKWebView *)webView {
    push(self.engine, EV_CRASHED, self.ident, 0, 0, 0, @"The page's process stopped", nil);
}

/* ---- what would otherwise be a window, a menu or a panel of WebKit's own ---- */

- (WKWebView *)webView:(WKWebView *)webView createWebViewWithConfiguration:(WKWebViewConfiguration *)configuration
           forNavigationAction:(WKNavigationAction *)action windowFeatures:(WKWindowFeatures *)features {
    NSString *url = action.request.URL.absoluteString ?: @"about:blank";
    push(self.engine, EV_OPEN_TAB, self.ident, 0, 0, 0, url, nil);
    return nil;
}

- (void)webViewDidClose:(WKWebView *)webView {
    push(self.engine, EV_CLOSE_TAB, self.ident, 0, 0, 0, nil, nil);
}

- (void)webView:(WKWebView *)webView runJavaScriptAlertPanelWithMessage:(NSString *)message initiatedByFrame:(WKFrameInfo *)frame completionHandler:(void (^)(void))done {
    [self answerAll];
    self.alertDone = done;
    push(self.engine, EV_DIALOG, self.ident, 0, 0, 0, message, @"");
}

- (void)webView:(WKWebView *)webView runJavaScriptConfirmPanelWithMessage:(NSString *)message initiatedByFrame:(WKFrameInfo *)frame completionHandler:(void (^)(BOOL))done {
    [self answerAll];
    self.confirmDone = done;
    push(self.engine, EV_DIALOG, self.ident, 0, 1, 0, message, @"");
}

- (void)webView:(WKWebView *)webView runJavaScriptTextInputPanelWithPrompt:(NSString *)prompt defaultText:(NSString *)text initiatedByFrame:(WKFrameInfo *)frame completionHandler:(void (^)(NSString *))done {
    [self answerAll];
    self.promptDone = done;
    push(self.engine, EV_DIALOG, self.ident, 0, 2, 0, prompt, text ?: @"");
}

- (void)webView:(WKWebView *)webView runOpenPanelWithParameters:(WKOpenPanelParameters *)parameters initiatedByFrame:(WKFrameInfo *)frame completionHandler:(void (^)(NSArray<NSURL *> *))done {
    done(nil);
}

/* A question still waiting when another comes is answered "no". */
- (void)answerAll {
    if (self.alertDone) {
        self.alertDone();
        self.alertDone = nil;
    }
    if (self.confirmDone) {
        self.confirmDone(NO);
        self.confirmDone = nil;
    }
    if (self.promptDone) {
        self.promptDone(nil);
        self.promptDone = nil;
    }
}

/* An extension's context speaking (api.js's JSON). A frame other than the top one is kept, by
 * its world and api.js's number for it, for code that is to reach every frame. */
- (void)extensionSaid:(NSString *)text frame:(WKFrameInfo *)frame world:(WKContentWorld *)world {
    if (frame && !frame.isMainFrame) {
        NSData *data = [text dataUsingEncoding:NSUTF8StringEncoding];
        id call = data ? [NSJSONSerialization JSONObjectWithData:data options:0 error:nil] : nil;
        NSNumber *number = [call isKindOfClass:[NSDictionary class]] ? call[@"f"] : nil;
        if ([number isKindOfClass:[NSNumber class]]) {
            NSString *name = world.name ?: @"";
            if (!self.frames)
                self.frames = [NSMutableDictionary dictionary];
            NSMutableDictionary<NSNumber *, WKFrameInfo *> *of = self.frames[name];
            if (!of) {
                of = [NSMutableDictionary dictionary];
                self.frames[name] = of;
            }
            if ([call[@"c"] isEqual:@"gone"])
                [of removeObjectForKey:number];
            else
                of[number] = frame;
        }
    }
    push(self.engine, EV_EXTENSION, self.ident, 0, 0, 0, text, nil);
}

/* A new page in the top frame: the frames of the one before are gone. */
- (void)webView:(WKWebView *)webView didCommitNavigation:(WKNavigation *)navigation {
    [self.frames removeAllObjects];
}

- (void)userContentController:(WKUserContentController *)controller didReceiveScriptMessage:(WKScriptMessage *)message {
    if ([message.name isEqualToString:@"spatimeshMedia"]) {
        if ([message.body isKindOfClass:[NSString class]] && message.frameInfo.isMainFrame)
            push(self.engine, EV_MEDIA, self.ident, 0, 0, 0, message.body, nil);
        return;
    }
    if ([message.name isEqualToString:@"spatimeshExt"]) {
        if ([message.body isKindOfClass:[NSString class]])
            [self extensionSaid:message.body frame:message.frameInfo world:message.world];
        return;
    }
    if (![message.name isEqualToString:@"spatimeshSelect"] || ![message.body isKindOfClass:[NSDictionary class]])
        return;
    NSDictionary *body = message.body;
    NSArray *options = body[@"options"];
    if (![options isKindOfClass:[NSArray class]])
        return;
    /* The list's lines go as text: label, then flags, a line each, read back by the Rust side
     * through spatimesh_choice. */
    objc_setAssociatedObject(self, "choices", options, OBJC_ASSOCIATION_RETAIN);
    double dpr = [body[@"dpr"] doubleValue] > 0 ? [body[@"dpr"] doubleValue] : 1.0;
    double scale = self.window.backingScaleFactor > 0 ? self.window.backingScaleFactor : 1.0;
    double zoom = self.view.pageZoom > 0 ? self.view.pageZoom : 1.0;
    (void)dpr;
    MeshEvent *last;
    push(self.engine, EV_CHOOSE, self.ident, 0, (int)options.count, 0, nil, nil);
    last = &self.engine->events[(self.engine->head + self.engine->count - 1) % self.engine->capacity];
    last->x = (int)([body[@"x"] doubleValue] * zoom * scale);
    last->y = (int)([body[@"y"] doubleValue] * zoom * scale);
    last->w = (int)([body[@"w"] doubleValue] * zoom * scale);
    last->h = (int)([body[@"h"] doubleValue] * zoom * scale);
}

/* ---- pictures ---- */

- (void)snapshotIfDue {
    if (self.snapshotting)
        return;
    CFTimeInterval t = now();
    CFTimeInterval interval = t < self.busyUntil ? 1.0 / 30.0 : 0.5;
    if (t - self.lastSnapshot < interval)
        return;
    self.lastSnapshot = t;
    self.snapshotting = YES;
    WKSnapshotConfiguration *config = [[WKSnapshotConfiguration alloc] init];
    config.rect = self.view.bounds;
    if (@available(macOS 10.15, *))
        config.afterScreenUpdates = NO;
    int width = self.width, height = self.height;
    __weak MeshTab *weakSelf = self;
    [self.view takeSnapshotWithConfiguration:config completionHandler:^(NSImage *image, NSError *error) {
        MeshTab *tab = weakSelf;
        if (!tab)
            return;
        tab.snapshotting = NO;
        if (!image)
            return;
        CGImageRef cg = [image CGImageForProposedRect:NULL context:nil hints:nil];
        if (!cg)
            return;
        size_t bytes = (size_t)width * (size_t)height * 4;
        unsigned char *fresh = malloc(bytes);
        if (!fresh)
            return;
        memset(fresh, 0, bytes);
        CGColorSpaceRef space = CGColorSpaceCreateWithName(kCGColorSpaceSRGB);
        CGContextRef context = CGBitmapContextCreate(fresh, (size_t)width, (size_t)height, 8, (size_t)width * 4, space,
                                                     kCGImageAlphaPremultipliedFirst | kCGBitmapByteOrder32Little);
        CGColorSpaceRelease(space);
        if (!context) {
            free(fresh);
            return;
        }
        CGContextDrawImage(context, CGRectMake(0, 0, width, height), cg);
        CGContextRelease(context);
        /* Passed on only if it changed: most snapshots of a page at rest do not. */
        if (tab.pixels && tab.pixelsWidth == width && tab.pixelsHeight == height && memcmp(tab.pixels, fresh, bytes) == 0) {
            free(fresh);
            return;
        }
        free(tab.pixels);
        tab.pixels = fresh;
        tab.pixelsWidth = width;
        tab.pixelsHeight = height;
        tab.damaged = YES;
        wake();
    }];
}

/* The processes WebKit plays this tab's sound from, as the web view says by SPI (the GPU
 * process; the page's own, where it plays there), to be tapped for the room (sound.m). */
- (void)soundToTheRoom {
    pid_t pids[2] = {0, 0};
    size_t n = 0;
    for (NSString *name in @[ @"_gpuProcessIdentifier", @"_webProcessIdentifier" ]) {
        SEL selector = NSSelectorFromString(name);
        if ([self.view respondsToSelector:selector])
            pids[n++] = ((pid_t (*)(id, SEL))objc_msgSend)(self.view, selector);
    }
    static BOOL told;
    if (n == 0 && !told) {
        told = YES;
        fprintf(stderr, "spatimesh-webkit: this WebKit does not say which process plays a page's sound; it plays on the Mac\n");
    }
    spatimesh_sound_playing(pids, n);
}

- (void)checkSound {
    if (@available(macOS 12.0, *)) {
        __weak MeshTab *weakSelf = self;
        [self.view requestMediaPlaybackState:^(WKMediaPlaybackState state) {
            MeshTab *tab = weakSelf;
            if (!tab)
                return;
            BOOL playing = state == WKMediaPlaybackStatePlaying;
            if (playing)
                [tab soundToTheRoom];
            if (playing != tab.playing) {
                tab.playing = playing;
                push(tab.engine, EV_SOUND, tab.ident, 0, playing, 0, nil, nil);
            }
            if (playing)
                [tab busy];
        }];
    }
}

@end

/* ---- extensions (docs/spatimesh.md, "Extensions") ---------------------------------------- *
 *
 * The window's runtime answers everything; here are only the bridge (a script message handler
 * named spatimeshExt, in each extension's own content world and in the pages' world), the
 * content scripts and style sheets as WebKit's user scripts, the extensions' files (served by
 * the Rust side, crates/spatimesh-shim), and code run in a tab's frames when asked. */

typedef int (*ServeFn)(const char *url, const char *initiator, const char **mime, unsigned char **bytes, size_t *len);
typedef void (*ServeFreeFn)(unsigned char *bytes, size_t len);
static ServeFn serve_fn;
static ServeFreeFn serve_free_fn;

static NSString *text_of(const char *s) {
    return (s ? [NSString stringWithUTF8String:s] : nil) ?: @"";
}

/* `value` as JSON ("null" for what JSON cannot say). */
static NSString *json_of(id value) {
    if (!value || ![NSJSONSerialization isValidJSONObject:@[ value ]])
        return @"null";
    NSData *data = [NSJSONSerialization dataWithJSONObject:@[ value ] options:0 error:nil];
    NSString *array = data ? [[NSString alloc] initWithData:data encoding:NSUTF8StringEncoding] : nil;
    return array.length > 2 ? [array substringWithRange:NSMakeRange(1, array.length - 2)] : @"null";
}

@implementation MeshSchemes

/* A file of an extension's, for the page that asked: the request's Referer or Origin, else the
 * tab's page (unless the request is for the tab's own new page, which the window asked for). */
- (void)webView:(WKWebView *)webView startURLSchemeTask:(id<WKURLSchemeTask>)task {
    NSURLRequest *request = task.request;
    NSString *initiator = [request valueForHTTPHeaderField:@"Referer"] ?: [request valueForHTTPHeaderField:@"Origin"];
    if (!initiator && ![request.URL isEqual:request.mainDocumentURL])
        initiator = webView.URL.absoluteString;
    const char *mime = "text/plain";
    unsigned char *bytes = NULL;
    size_t len = 0;
    const char *url = request.URL.absoluteString.UTF8String;
    const char *from = (initiator ?: @"").UTF8String;
    int status = serve_fn ? serve_fn(url ? url : "", from ? from : "", &mime, &bytes, &len) : 404;
    NSData *body = len ? [NSData dataWithBytes:bytes length:len] : [NSData data];
    if (serve_free_fn)
        serve_free_fn(bytes, len);
    NSDictionary<NSString *, NSString *> *headers = @{
        @"Content-Type" : text_of(mime),
        @"Content-Length" : [NSString stringWithFormat:@"%lu", (unsigned long)body.length],
        /* Who may read it was decided as it was served (only what is web accessible, to pages
         * not the extension's own). */
        @"Access-Control-Allow-Origin" : @"*",
        @"Cache-Control" : @"no-cache",
    };
    NSHTTPURLResponse *response = [[NSHTTPURLResponse alloc] initWithURL:request.URL statusCode:status HTTPVersion:@"HTTP/1.1" headerFields:headers];
    [task didReceiveResponse:response];
    [task didReceiveData:body];
    [task didFinish];
}

- (void)webView:(WKWebView *)webView stopURLSchemeTask:(id<WKURLSchemeTask>)task {
}

@end

/* Whether this WebKit has match patterns for user scripts (said once, in the log). */
static BOOL patterns_work(void) {
    static int known = -1;
    if (known < 0) {
        SEL patterned = @selector(_initWithSource:injectionTime:forMainFrameOnly:includeMatchPatternStrings:excludeMatchPatternStrings:associatedURL:contentWorld:deferRunningUntilNotification:);
        known = [WKUserScript instancesRespondToSelector:patterned] ? 1 : 0;
        fprintf(stderr, "spatimesh-webkit: extensions' content scripts are matched %s\n",
                known ? "by WebKit's own patterns" : "by a check in each script");
    }
    return known == 1;
}

/* A user script for the pages `allow` lets in and `block` keeps out (WebKit's patterns) where
 * WebKit can say, for every page where it cannot. */
static WKUserScript *user_script(NSString *code, WKUserScriptInjectionTime when, BOOL main_only, NSArray<NSString *> *allow,
                                 NSArray<NSString *> *block, WKContentWorld *world) {
    if ((allow.count || block.count) && patterns_work())
        return [[WKUserScript alloc] _initWithSource:code
                                       injectionTime:when
                                    forMainFrameOnly:main_only
                          includeMatchPatternStrings:allow
                          excludeMatchPatternStrings:block
                                       associatedURL:nil
                                        contentWorld:world
                       deferRunningUntilNotification:NO];
    return [[WKUserScript alloc] initWithSource:code injectionTime:when forMainFrameOnly:main_only inContentWorld:world];
}

/* Extensions' addresses in a new tab's configuration: served by the shim, and (once) secure
 * ones, as Chrome's are, where WebKit lets that be said. */
static void register_schemes(WKWebViewConfiguration *config, Engine *e) {
    static BOOL secured;
    for (NSString *scheme in @[ @"chrome-extension", @"moz-extension" ]) {
        if ([WKWebView handlesURLScheme:scheme])
            continue;
        [config setURLSchemeHandler:e->schemes forURLScheme:scheme];
        if (secured)
            continue;
        WKProcessPool *pool = config.processPool;
        if ([pool respondsToSelector:@selector(_registerURLSchemeAsSecure:)])
            [pool _registerURLSchemeAsSecure:scheme];
        if ([pool respondsToSelector:@selector(_registerURLSchemeAsBypassingContentSecurityPolicy:)])
            [pool _registerURLSchemeAsBypassingContentSecurityPolicy:scheme];
    }
    secured = YES;
}

/* The <select> script, the profile's extensions' scripts and style sheets, and the handlers
 * their contexts speak to, into a tab's view in place of what was there: pages loaded from now
 * on have them. */
static void apply_extensions(MeshTab *tab) {
    Engine *e = tab.engine;
    WKUserContentController *c = tab.view.configuration.userContentController;
    for (WKContentWorld *world in tab.handlerWorlds)
        [c removeScriptMessageHandlerForName:@"spatimeshExt" contentWorld:world];
    tab.handlerWorlds = [NSMutableArray array];
    [c removeAllUserScripts];
    [c addUserScript:[[WKUserScript alloc] initWithSource:SelectScript injectionTime:WKUserScriptInjectionTimeAtDocumentStart forMainFrameOnly:YES]];
    if (e->pageScript.length) {
        /* Before anything of the page's, in every frame; the top frame first given its way out,
         * kept where the page's own scripts cannot replace it. */
        NSString *code = [NSString stringWithFormat:
            @"if(window.top===window){(function(){var h=window.webkit&&window.webkit.messageHandlers&&window.webkit.messageHandlers.spatimeshMedia;"
             "if(h){var p=h.postMessage.bind(h);window.__spatimeshMediaPost=function(s){p(s);};}})();}\n%@", e->pageScript];
        [c addUserScript:[[WKUserScript alloc] initWithSource:code injectionTime:WKUserScriptInjectionTimeAtDocumentStart forMainFrameOnly:NO]];
    }
    if (!e->extensions.count)
        return;
    WKContentWorld *page = [WKContentWorld pageWorld];
    [c addScriptMessageHandler:tab contentWorld:page name:@"spatimeshExt"];
    [tab.handlerWorlds addObject:page];
    for (MeshExtension *x in e->extensions) {
        WKContentWorld *world = [WKContentWorld worldWithName:x.world];
        [c addScriptMessageHandler:tab contentWorld:world name:@"spatimeshExt"];
        [tab.handlerWorlds addObject:world];
        /* Its own pages, in every frame they are in: their API in the pages' own world. */
        NSString *prefix = [NSString stringWithFormat:@"%@://%@/", x.scheme, x.ident];
        NSString *code = [NSString stringWithFormat:@"if(location.href.indexOf(%@)===0){%@}", json_of(prefix), x.pageBoot];
        [c addUserScript:user_script(code, WKUserScriptInjectionTimeAtDocumentStart, NO, @[ [prefix stringByAppendingString:@"*"] ], nil, page)];
    }
    for (MeshEntry *n in e->entries) {
        BOOL main_only = !n.allFrames;
        WKUserScriptInjectionTime when = n.runAt == 0 ? WKUserScriptInjectionTimeAtDocumentStart : WKUserScriptInjectionTimeAtDocumentEnd;
        WKContentWorld *world = n.mainWorld ? page : [WKContentWorld worldWithName:n.extension.world];
        /* Where WebKit's patterns cannot say exactly which pages, or it has none, the code checks. */
        BOOL checked = !(n.exact && patterns_work());
        for (NSString *css in n.css) {
            /* Style sheets before anything is shown, whenever the scripts go, as in Chrome. */
            NSString *code = [NSString stringWithFormat:@"%@(function(css){try{var s=new CSSStyleSheet();s.replaceSync(css);"
                                                         "document.adoptedStyleSheets=document.adoptedStyleSheets.concat([s]);}catch(e){}})(%@);",
                                                        checked ? [NSString stringWithFormat:@"if(%@)", n.guard] : @"", json_of(css)];
            [c addUserScript:user_script(code, WKUserScriptInjectionTimeAtDocumentStart, main_only, n.allow, n.block, world)];
        }
        /* Its API first, in its world (a script in the page's own world has none). */
        if (!n.mainWorld) {
            NSString *boot = checked ? [NSString stringWithFormat:@"if(%@){%@}", n.guard, n.extension.contentBoot] : n.extension.contentBoot;
            [c addUserScript:user_script(boot, when, main_only, n.allow, n.block, world)];
        }
        for (NSString *file in n.js) {
            /* Each file a script of its own, as in Chrome: its top-level names shared with the
             * others in the world. Where it is not wanted, it stops at once. */
            NSString *code = checked ? [NSString stringWithFormat:@"if(!(%@))throw new Error('not for this page');\n%@", n.guard, file] : file;
            [c addUserScript:user_script(code, when, main_only, n.allow, n.block, world)];
        }
    }
}

/* A script's end, for the window: its value, or what went wrong. */
static void answered(Engine *e, uint64_t tab, uint64_t request, id result, NSError *error) {
    BOOL unsupported = error && [error.domain isEqualToString:WKErrorDomain] && error.code == WKErrorJavaScriptResultTypeIsUnsupported;
    if (error && !unsupported) {
        NSString *why = error.userInfo[@"WKJavaScriptExceptionMessage"] ?: error.localizedDescription;
        push(e, EV_SCRIPT_RESULT, tab, (double)request, 0, 0, why ?: @"the script failed", nil);
        return;
    }
    push(e, EV_SCRIPT_RESULT, tab, (double)request, 1, 0, json_of(unsupported ? nil : result), nil);
}

/* ---- the engine ---- */

const char *spatimesh_version(void) {
    static char version[96];
    NSString *os = [[NSProcessInfo processInfo] operatingSystemVersionString];
    snprintf(version, sizeof version, "WebKit (macOS %s)", os.UTF8String ?: "");
    return version;
}

static void on_socket(CFFileDescriptorRef fd, CFOptionFlags flags, void *info) {
    wake();
    CFFileDescriptorEnableCallBacks(fd, kCFFileDescriptorReadCallBack);
}

/* A UUID made from a profile's id, the same every time: its website data store's name. */
static NSUUID *uuid_of(const char *id) {
    uint8_t bytes[16] = {0};
    uint64_t h1 = 0xcbf29ce484222325ull, h2 = 0x84222325cbf29ce4ull;
    for (const char *p = id; *p; p++) {
        h1 = (h1 ^ (uint8_t)*p) * 0x100000001b3ull;
        h2 = (h2 ^ (uint8_t)*p) * 0x100000001b3ull + 7;
    }
    memcpy(bytes, &h1, 8);
    memcpy(bytes + 8, &h2, 8);
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    return [[NSUUID alloc] initWithUUIDBytes:bytes];
}

/* `data` is the profile's id here (WebKit keeps a store's files itself), NULL for the private
 * profile, which keeps everything in memory. */
Engine *spatimesh_new(const char *data, const char *cache) {
    (void)cache;
    @autoreleasepool {
        [NSApplication sharedApplication];
        [NSApp setActivationPolicy:NSApplicationActivationPolicyProhibited];
        [NSApp finishLaunching];
        Engine *e = calloc(1, sizeof(Engine));
        e->tabs = [NSMutableDictionary dictionary];
        e->extensions = [NSMutableArray array];
        e->entries = [NSMutableArray array];
        e->schemes = [[MeshSchemes alloc] init];
        e->ruleSources = [NSMutableDictionary dictionary];
        e->ruleLists = [NSMutableDictionary dictionary];
        if (!data) {
            e->store = [WKWebsiteDataStore nonPersistentDataStore];
        } else if (@available(macOS 14.0, *)) {
            NSString *path = [NSString stringWithUTF8String:data];
            e->store = [WKWebsiteDataStore dataStoreForIdentifier:uuid_of(path.lastPathComponent.UTF8String)];
        } else {
            e->store = [WKWebsiteDataStore defaultDataStore];
        }
        the_engine = e;
        e->timer = [NSTimer scheduledTimerWithTimeInterval:1.0 / 60.0 repeats:YES block:^(NSTimer *timer) {
            for (MeshTab *tab in the_engine->tabs.allValues)
                [tab snapshotIfDue];
        }];
        [NSTimer scheduledTimerWithTimeInterval:1.0 repeats:YES block:^(NSTimer *timer) {
            for (MeshTab *tab in the_engine->tabs.allValues)
                [tab checkSound];
        }];
        return e;
    }
}

void spatimesh_watch(Engine *e, int fd) {
    if (e->watch) {
        CFFileDescriptorInvalidate(e->watch);
        CFRelease(e->watch);
    }
    e->watch = CFFileDescriptorCreate(NULL, fd, false, on_socket, NULL);
    CFFileDescriptorEnableCallBacks(e->watch, kCFFileDescriptorReadCallBack);
    CFRunLoopSourceRef source = CFFileDescriptorCreateRunLoopSource(NULL, e->watch, 0);
    CFRunLoopAddSource(CFRunLoopGetMain(), source, kCFRunLoopCommonModes);
    CFRelease(source);
}

static MeshTab *find(Engine *e, uint64_t id) {
    return e->tabs[@(id)];
}

/* The window's size in points for a picture of `width` by `height` pixels. */
static NSSize points(NSWindow *window, int width, int height) {
    CGFloat scale = window.backingScaleFactor > 0 ? window.backingScaleFactor : 1.0;
    return NSMakeSize(width / scale, height / scale);
}

int spatimesh_open(Engine *e, uint64_t id, const char *uri, int width, int height, double zoom) {
    @autoreleasepool {
        if (find(e, id))
            return 0;
        MeshTab *tab = [[MeshTab alloc] init];
        tab.ident = id;
        tab.engine = e;
        tab.width = MAX(width, 1);
        tab.height = MAX(height, 1);
        NSWindow *window = [[NSWindow alloc] initWithContentRect:NSMakeRect(0, 0, 100, 100) styleMask:NSWindowStyleMaskBorderless
                                                         backing:NSBackingStoreBuffered defer:NO];
        window.releasedWhenClosed = NO;
        window.alphaValue = 0.0;
        window.opaque = NO;
        window.ignoresMouseEvents = YES;
        window.sharingType = NSWindowSharingNone;
        window.hasShadow = NO;
        window.acceptsMouseMovedEvents = YES;
        [window setContentSize:points(window, tab.width, tab.height)];
        [window setFrameOrigin:NSZeroPoint];
        WKWebViewConfiguration *config = [[WKWebViewConfiguration alloc] init];
        config.websiteDataStore = e->store;
        register_schemes(config, e);
        [config.userContentController addScriptMessageHandler:tab name:@"spatimeshSelect"];
        [config.userContentController addScriptMessageHandler:tab name:@"spatimeshMedia"];
        MeshWebView *view = [[MeshWebView alloc] initWithFrame:NSMakeRect(0, 0, window.contentView.bounds.size.width, window.contentView.bounds.size.height) configuration:config];
        view.autoresizingMask = NSViewWidthSizable | NSViewHeightSizable;
        view.navigationDelegate = tab;
        view.UIDelegate = tab;
        if (@available(macOS 11.0, *))
            view.pageZoom = zoom > 0.1 ? zoom : 1.0;
        SEL occlusion = NSSelectorFromString(@"_setWindowOcclusionDetectionEnabled:");
        if ([view respondsToSelector:occlusion])
            ((void (*)(WKWebView *, SEL, BOOL))objc_msgSend)(view, occlusion, NO);
        window.contentView = view;
        [window orderBack:nil];
        [window makeFirstResponder:view];
        tab.window = window;
        tab.view = view;
        /* The <select> script, and the profile's extensions from the first page on. */
        apply_extensions(tab);
        apply_rule_lists(tab);
        for (NSString *key in @[ @"title", @"URL", @"estimatedProgress", @"loading", @"canGoBack", @"canGoForward" ])
            [view addObserver:tab forKeyPath:key options:NSKeyValueObservingOptionNew context:NULL];
        e->tabs[@(id)] = tab;
        [tab busy];
        NSString *address = uri && *uri ? [NSString stringWithUTF8String:uri] : @"about:blank";
        NSURL *url = [NSURL URLWithString:address];
        if (url)
            [view loadRequest:[NSURLRequest requestWithURL:url]];
        return 1;
    }
}

static void forget(MeshTab *tab) {
    [tab answerAll];
    for (NSString *key in @[ @"title", @"URL", @"estimatedProgress", @"loading", @"canGoBack", @"canGoForward" ])
        [tab.view removeObserver:tab forKeyPath:key];
    /* The controller holds its handlers, the tab among them: let go of them all. */
    [tab.view.configuration.userContentController removeAllScriptMessageHandlers];
    tab.view.navigationDelegate = nil;
    tab.view.UIDelegate = nil;
    [tab.window orderOut:nil];
    [tab.window close];
}

void spatimesh_close(Engine *e, uint64_t id) {
    @autoreleasepool {
        MeshTab *tab = find(e, id);
        if (tab) {
            forget(tab);
            [e->tabs removeObjectForKey:@(id)];
        }
    }
}

void spatimesh_load(Engine *e, uint64_t id, const char *uri) {
    @autoreleasepool {
        MeshTab *tab = find(e, id);
        NSURL *url = uri ? [NSURL URLWithString:[NSString stringWithUTF8String:uri]] : nil;
        if (tab && url) {
            [tab.view loadRequest:[NSURLRequest requestWithURL:url]];
            [tab busy];
        }
    }
}

void spatimesh_back(Engine *e, uint64_t id) {
    @autoreleasepool {
        [find(e, id).view goBack];
    }
}

void spatimesh_forward(Engine *e, uint64_t id) {
    @autoreleasepool {
        [find(e, id).view goForward];
    }
}

void spatimesh_reload(Engine *e, uint64_t id) {
    @autoreleasepool {
        MeshTab *tab = find(e, id);
        [tab.view reload];
        [tab busy];
    }
}

void spatimesh_stop(Engine *e, uint64_t id) {
    @autoreleasepool {
        [find(e, id).view stopLoading];
    }
}

void spatimesh_resize(Engine *e, uint64_t id, int width, int height, double zoom) {
    @autoreleasepool {
        MeshTab *tab = find(e, id);
        if (!tab)
            return;
        tab.width = MAX(width, 1);
        tab.height = MAX(height, 1);
        [tab.window setContentSize:points(tab.window, tab.width, tab.height)];
        [tab.window setFrameOrigin:NSZeroPoint];
        if (@available(macOS 11.0, *)) {
            if (zoom > 0.1)
                tab.view.pageZoom = zoom;
        }
        [tab busy];
    }
}

void spatimesh_mute(Engine *e, uint64_t id, int muted) {
    @autoreleasepool {
        MeshTab *tab = find(e, id);
        /* WebKit's own mute (`_setPageMuted:`, SPI); where it has not got it, the tab plays on. */
        SEL mute = NSSelectorFromString(@"_setPageMuted:");
        if (tab && [tab.view respondsToSelector:mute])
            ((void (*)(WKWebView *, SEL, NSUInteger))objc_msgSend)(tab.view, mute, muted ? 1 : 0);
    }
}

void spatimesh_set_window(Engine *e, uint64_t id, uint32_t window) {
    /* Where the tab's sound goes: docs/spatimesh.md, "Sound". */
    (void)e;
    (void)id;
    (void)window;
}

/* ---- input ---- */

static NSEventModifierFlags flags_of(int modifiers) {
    NSEventModifierFlags flags = 0;
    if (modifiers & 1) flags |= NSEventModifierFlagShift;
    if (modifiers & 2) flags |= NSEventModifierFlagControl;
    if (modifiers & 4) flags |= NSEventModifierFlagOption;
    if (modifiers & 8) flags |= NSEventModifierFlagCommand;
    return flags;
}

/* A point in the window, from pixels from the top left of the page's picture. */
static NSPoint location(MeshTab *tab, double x, double y) {
    CGFloat scale = tab.window.backingScaleFactor > 0 ? tab.window.backingScaleFactor : 1.0;
    NSRect content = tab.window.contentView.bounds;
    return NSMakePoint(x / scale, content.size.height - y / scale);
}

static NSUInteger mask_of(int button) {
    return button == 1 ? 1 : button == 3 ? 2 : button == 2 ? 4 : 0;
}

void spatimesh_pointer(Engine *e, uint64_t id, double x, double y, int button, int pressed, int modifiers) {
    @autoreleasepool {
        MeshTab *tab = find(e, id);
        if (!tab)
            return;
        [tab busy];
        NSPoint at = location(tab, x, y);
        NSEventModifierFlags flags = flags_of(modifiers);
        NSTimeInterval t = now();
        NSInteger window = tab.window.windowNumber;
        if (button == 0) {
            NSEventType type = NSEventTypeMouseMoved;
            if (tab.buttons & 1) type = NSEventTypeLeftMouseDragged;
            else if (tab.buttons & 2) type = NSEventTypeRightMouseDragged;
            else if (tab.buttons & 4) type = NSEventTypeOtherMouseDragged;
            NSEvent *ev = [NSEvent mouseEventWithType:type location:at modifierFlags:flags timestamp:t windowNumber:window context:nil
                                          eventNumber:0 clickCount:0 pressure:tab.buttons ? 1.0 : 0.0];
            switch (type) {
            case NSEventTypeLeftMouseDragged: [tab.view mouseDragged:ev]; break;
            case NSEventTypeRightMouseDragged: [tab.view rightMouseDragged:ev]; break;
            case NSEventTypeOtherMouseDragged: [tab.view otherMouseDragged:ev]; break;
            default: [tab.view mouseMoved:ev]; break;
            }
            return;
        }
        if (button == 4 || button == 5) {
            if (pressed) {
                if (button == 4) [tab.view goBack];
                else [tab.view goForward];
            }
            return;
        }
        /* Clicks in quick succession at one place count as a double or a triple click. */
        static NSTimeInterval last_down;
        static NSPoint last_at;
        static NSInteger clicks;
        if (pressed) {
            BOOL near = fabs(at.x - last_at.x) < 4 && fabs(at.y - last_at.y) < 4;
            clicks = (near && t - last_down < [NSEvent doubleClickInterval]) ? clicks + 1 : 1;
            last_down = t;
            last_at = at;
        }
        NSEventType type;
        switch (button) {
        case 3: type = pressed ? NSEventTypeRightMouseDown : NSEventTypeRightMouseUp; break;
        case 2: type = pressed ? NSEventTypeOtherMouseDown : NSEventTypeOtherMouseUp; break;
        default: type = pressed ? NSEventTypeLeftMouseDown : NSEventTypeLeftMouseUp; break;
        }
        NSEvent *ev = [NSEvent mouseEventWithType:type location:at modifierFlags:flags timestamp:t windowNumber:window context:nil
                                      eventNumber:0 clickCount:MAX(clicks, 1) pressure:pressed ? 1.0 : 0.0];
        if (pressed)
            tab.buttons |= mask_of(button);
        else
            tab.buttons &= ~mask_of(button);
        switch (type) {
        case NSEventTypeLeftMouseDown: [tab.view mouseDown:ev]; break;
        case NSEventTypeLeftMouseUp: [tab.view mouseUp:ev]; break;
        case NSEventTypeRightMouseDown: [tab.view rightMouseDown:ev]; break;
        case NSEventTypeRightMouseUp: [tab.view rightMouseUp:ev]; break;
        case NSEventTypeOtherMouseDown: [tab.view otherMouseDown:ev]; break;
        default: [tab.view otherMouseUp:ev]; break;
        }
    }
}

void spatimesh_leave(Engine *e, uint64_t id) {
    @autoreleasepool {
        MeshTab *tab = find(e, id);
        if (!tab)
            return;
        tab.buttons = 0;
        NSEvent *ev = [NSEvent enterExitEventWithType:NSEventTypeMouseExited location:NSMakePoint(-1, -1) modifierFlags:0 timestamp:now()
                                         windowNumber:tab.window.windowNumber context:nil eventNumber:0 trackingNumber:0 userData:NULL];
        [tab.view mouseExited:ev];
    }
}

void spatimesh_scroll(Engine *e, uint64_t id, double x, double y, double dx, double dy, int modifiers) {
    @autoreleasepool {
        MeshTab *tab = find(e, id);
        if (!tab)
            return;
        [tab busy];
        CGFloat scale = tab.window.backingScaleFactor > 0 ? tab.window.backingScaleFactor : 1.0;
        CGEventRef cg = CGEventCreateScrollWheelEvent2(NULL, kCGScrollEventUnitPixel, 2, (int32_t)(-dy / scale), (int32_t)(-dx / scale), 0);
        if (!cg)
            return;
        /* The window is at the screen's origin, so the window's point is the screen's, in
         * Core Graphics' terms counted from the top of the main display. */
        NSPoint at = location(tab, x, y);
        CGFloat screen_height = NSScreen.screens.firstObject.frame.size.height;
        CGEventSetLocation(cg, CGPointMake(at.x, screen_height - at.y));
        CGEventSetFlags(cg, (CGEventFlags)flags_of(modifiers));
        NSEvent *ev = [NSEvent eventWithCGEvent:cg];
        CFRelease(cg);
        if (ev)
            [tab.view scrollWheel:ev];
    }
}

/* Linux's key numbers to the Mac's (kVK_*), for the keys a page cares about. */
static unsigned short mac_key(int code) {
    static const short table[128] = {
        [1] = 0x35, [2] = 0x12, [3] = 0x13, [4] = 0x14, [5] = 0x15, [6] = 0x17, [7] = 0x16, [8] = 0x1A, [9] = 0x1C, [10] = 0x19,
        [11] = 0x1D, [12] = 0x1B, [13] = 0x18, [14] = 0x33, [15] = 0x30, [16] = 0x0C, [17] = 0x0D, [18] = 0x0E, [19] = 0x0F,
        [20] = 0x11, [21] = 0x10, [22] = 0x20, [23] = 0x22, [24] = 0x1F, [25] = 0x23, [26] = 0x21, [27] = 0x1E, [28] = 0x24,
        [29] = 0x3B, [30] = 0x00, [31] = 0x01, [32] = 0x02, [33] = 0x03, [34] = 0x05, [35] = 0x04, [36] = 0x26, [37] = 0x28,
        [38] = 0x25, [39] = 0x29, [40] = 0x27, [41] = 0x32, [42] = 0x38, [43] = 0x2A, [44] = 0x06, [45] = 0x07, [46] = 0x08,
        [47] = 0x09, [48] = 0x0B, [49] = 0x2D, [50] = 0x2E, [51] = 0x2B, [52] = 0x2F, [53] = 0x2C, [54] = 0x3C, [56] = 0x3A,
        [57] = 0x31, [58] = 0x39, [59] = 0x7A, [60] = 0x78, [61] = 0x63, [62] = 0x76, [63] = 0x60, [64] = 0x61, [65] = 0x62,
        [66] = 0x64, [67] = 0x65, [68] = 0x6D, [87] = 0x67, [88] = 0x6F, [97] = 0x3E, [100] = 0x3D, [102] = 0x73, [103] = 0x7E,
        [104] = 0x74, [105] = 0x7B, [106] = 0x7C, [107] = 0x77, [108] = 0x7D, [109] = 0x79, [110] = 0x72, [111] = 0x75,
        [125] = 0x37, [126] = 0x36,
    };
    if (code < 0 || code >= 128)
        return 0xFFFF;
    /* 0 is A's own number, so the table cannot say "none" with it: A is told apart by code. */
    if (table[code] == 0 && code != 30)
        return 0xFFFF;
    return (unsigned short)table[code];
}

/* What a key types on a Mac: its character, or the function-key character AppKit gives
 * keys that type nothing. */
static NSString *characters_of(int key, uint32_t ch, int code) {
    unichar c = 0;
    switch (key) {
    case 1: c = '\r'; break;
    case 2: c = 0x1b; break;
    case 3: c = NSDeleteCharacter; break;
    case 4: c = NSDeleteFunctionKey; break;
    case 5: c = '\t'; break;
    case 6: c = ' '; break;
    case 7: c = NSLeftArrowFunctionKey; break;
    case 8: c = NSRightArrowFunctionKey; break;
    case 9: c = NSUpArrowFunctionKey; break;
    case 10: c = NSDownArrowFunctionKey; break;
    case 11: c = NSHomeFunctionKey; break;
    case 12: c = NSEndFunctionKey; break;
    case 13: c = NSPageUpFunctionKey; break;
    case 14: c = NSPageDownFunctionKey; break;
    case 15: {
        uint32_t scalar = ch;
        return [[NSString alloc] initWithBytes:&scalar length:4 encoding:NSUTF32LittleEndianStringEncoding] ?: @"";
    }
    default:
        if (code >= 59 && code <= 68) c = NSF1FunctionKey + (code - 59);
        else if (code == 87) c = NSF11FunctionKey;
        else if (code == 88) c = NSF12FunctionKey;
        break;
    }
    return c ? [NSString stringWithCharacters:&c length:1] : @"";
}

void spatimesh_key(Engine *e, uint64_t id, int key, uint32_t ch, int code, int pressed, int modifiers, const char *text) {
    @autoreleasepool {
        MeshTab *tab = find(e, id);
        if (!tab)
            return;
        [tab busy];
        NSString *plain = characters_of(key, ch, code);
        NSString *typed = (text && *text) ? [NSString stringWithUTF8String:text] : plain;
        unsigned short keyCode = mac_key(code);
        BOOL modifier = code == 42 || code == 54 || code == 29 || code == 97 || code == 56 || code == 100 || code == 125 || code == 126;
        NSEventModifierFlags flags = flags_of(modifiers);
        NSTimeInterval t = now();
        if (modifier) {
            NSEvent *ev = [NSEvent keyEventWithType:NSEventTypeFlagsChanged location:NSZeroPoint modifierFlags:flags timestamp:t
                                       windowNumber:tab.window.windowNumber context:nil characters:@"" charactersIgnoringModifiers:@""
                                          isARepeat:NO keyCode:keyCode == 0xFFFF ? 0 : keyCode];
            [tab.view flagsChanged:ev];
            return;
        }
        NSEvent *ev = [NSEvent keyEventWithType:pressed ? NSEventTypeKeyDown : NSEventTypeKeyUp location:NSZeroPoint modifierFlags:flags
                                      timestamp:t windowNumber:tab.window.windowNumber context:nil characters:typed
                    charactersIgnoringModifiers:plain isARepeat:NO keyCode:keyCode == 0xFFFF ? 0 : keyCode];
        if (pressed)
            [tab.view keyDown:ev];
        else
            [tab.view keyUp:ev];
    }
}

void spatimesh_focus(Engine *e, uint64_t id, int focused) {
    @autoreleasepool {
        MeshTab *tab = find(e, id);
        if (!tab)
            return;
        if (focused) {
            [tab.window makeFirstResponder:tab.view];
            [tab.window makeKeyWindow];
        } else {
            [tab.window makeFirstResponder:nil];
        }
    }
}

/* ---- <select> lists and script dialogs ---- */

const char *spatimesh_choice(Engine *e, uint64_t id, int index, int *flags) {
    MeshTab *tab = find(e, id);
    NSArray *choices = tab ? objc_getAssociatedObject(tab, "choices") : nil;
    if (!choices || index < 0 || (NSUInteger)index >= choices.count)
        return NULL;
    NSDictionary *c = choices[(NSUInteger)index];
    *flags = ([c[@"enabled"] boolValue] ? 1 : 0) | ([c[@"selected"] boolValue] ? 2 : 0) | ([c[@"heading"] boolValue] ? 4 : 0);
    NSString *label = [c[@"label"] isKindOfClass:[NSString class]] ? c[@"label"] : @"";
    /* A copy that lives until the next line is asked for: the Rust side copies it at once. */
    static char *last;
    free(last);
    last = copy_string(label);
    return last;
}

void spatimesh_choose(Engine *e, uint64_t id, int index) {
    @autoreleasepool {
        MeshTab *tab = find(e, id);
        if (!tab)
            return;
        NSString *script = [NSString stringWithFormat:@"window.__spatimeshChoose && window.__spatimeshChoose(%d)", index];
        [tab.view evaluateJavaScript:script completionHandler:nil];
    }
}

void spatimesh_answer(Engine *e, uint64_t id, int accepted, const char *text) {
    @autoreleasepool {
        MeshTab *tab = find(e, id);
        if (!tab)
            return;
        if (tab.alertDone) {
            tab.alertDone();
            tab.alertDone = nil;
        }
        if (tab.confirmDone) {
            tab.confirmDone(accepted != 0);
            tab.confirmDone = nil;
        }
        if (tab.promptDone) {
            tab.promptDone(accepted ? [NSString stringWithUTF8String:text ? text : ""] : nil);
            tab.promptDone = nil;
        }
    }
}

/* ---- the loop and the pictures ---- */

static BOOL any_damaged(Engine *e) {
    for (MeshTab *tab in e->tabs.allValues)
        if (tab.damaged)
            return YES;
    return NO;
}

void spatimesh_pump(Engine *e, int timeout_ms) {
    @autoreleasepool {
        e->woken = NO;
        NSDate *until = [NSDate dateWithTimeIntervalSinceNow:MAX(timeout_ms, 0) / 1000.0];
        for (;;) {
            NSEvent *ev = [NSApp nextEventMatchingMask:NSEventMaskAny untilDate:until inMode:NSDefaultRunLoopMode dequeue:YES];
            if (!ev)
                break;
            if (!(ev.type == NSEventTypeApplicationDefined && ev.subtype == 77))
                [NSApp sendEvent:ev];
            if (e->woken || e->count > 0 || any_damaged(e))
                break;
        }
    }
}

int spatimesh_next_event(Engine *e, MeshEvent *out) {
    free(e->current.text);
    free(e->current.text2);
    memset(&e->current, 0, sizeof e->current);
    if (e->count == 0)
        return 0;
    e->current = e->events[e->head];
    e->head = (e->head + 1) % e->capacity;
    e->count--;
    *out = e->current;
    return 1;
}

int spatimesh_damage(Engine *e, uint64_t id, int *x, int *y, int *w, int *h) {
    MeshTab *tab = find(e, id);
    if (!tab || !tab.damaged || !tab.pixels)
        return 0;
    *x = 0;
    *y = 0;
    *w = MIN(tab.pixelsWidth, tab.width);
    *h = MIN(tab.pixelsHeight, tab.height);
    return 1;
}

int spatimesh_copy(Engine *e, uint64_t id, unsigned char *pixels, int width, int height) {
    MeshTab *tab = find(e, id);
    if (!tab || !tab.damaged || !tab.pixels)
        return 0;
    int rows = MIN(height, tab.pixelsHeight);
    int across = MIN(width, tab.pixelsWidth);
    for (int row = 0; row < rows; row++)
        memcpy(pixels + (size_t)row * (size_t)width * 4, tab.pixels + (size_t)row * (size_t)tab.pixelsWidth * 4, (size_t)across * 4);
    tab.damaged = NO;
    return 1;
}

void spatimesh_run(Engine *e, uint64_t id, const char *script) {
    @autoreleasepool {
        MeshTab *tab = find(e, id);
        if (tab && script)
            [tab.view evaluateJavaScript:[NSString stringWithUTF8String:script] completionHandler:nil];
    }
}

/* ---- extensions: what the shim gives and asks ---- */

/* What answers chrome-extension:// and moz-extension://: the shim's, handed over at start. */
void spatimesh_ext_server(ServeFn serve, ServeFreeFn free_bytes) {
    serve_fn = serve;
    serve_free_fn = free_bytes;
}

/* The profile's extensions' declarativeNetRequest rules on a tab's view, in place of what was
 * there. */
static void apply_rule_lists(MeshTab *tab) {
    WKUserContentController *c = tab.view.configuration.userContentController;
    [c removeAllContentRuleLists];
    for (WKContentRuleList *list in tab.engine->ruleLists.allValues)
        [c addContentRuleList:list];
}

/* A name for a list that is the same for the same rules (FNV-1a of them): two engines, of two
 * profiles, sharing the store never take each other's. */
static NSString *rule_list_name(NSString *ident, NSString *source) {
    uint64_t h = 1469598103934665603ULL;
    for (const unsigned char *p = (const unsigned char *)source.UTF8String; p && *p; p++)
        h = (h ^ *p) * 1099511628211ULL;
    return [NSString stringWithFormat:@"spatimesh.%@.%016llx", ident, (unsigned long long)h];
}

/* declarativeNetRequest: each extension's rules as WebKit's content blockers, compiled (in the
 * background) and put on every tab's view; an extension's that are not in the new set taken
 * off. Pages loaded from then on have them. */
static void compile_rule_lists(Engine *e) {
    BOOL dropped = NO;
    for (NSString *ident in e->ruleLists.allKeys)
        if (!e->ruleSources[ident]) {
            [e->ruleLists removeObjectForKey:ident];
            dropped = YES;
        }
    if (dropped)
        for (MeshTab *tab in e->tabs.allValues)
            apply_rule_lists(tab);
    WKContentRuleListStore *store = [WKContentRuleListStore defaultStore];
    for (NSString *ident in e->ruleSources.allKeys) {
        NSString *source = e->ruleSources[ident];
        [store compileContentRuleListForIdentifier:rule_list_name(ident, source) encodedContentRuleList:source
                                 completionHandler:^(WKContentRuleList *list, NSError *error) {
                                     if (!list) {
                                         fprintf(stderr, "spatimesh: an extension's declarativeNetRequest rules could not be made WebKit's: %s\n",
                                                 error.localizedDescription.UTF8String ?: "?");
                                         return;
                                     }
                                     /* Rules changed or gone since they were asked for are not put on. */
                                     if (![e->ruleSources[ident] isEqualToString:source])
                                         return;
                                     e->ruleLists[ident] = list;
                                     for (MeshTab *tab in e->tabs.allValues)
                                         apply_rule_lists(tab);
                                 }];
    }
}

/* What a context's api.js hands its calls to: the handler in its own world. */
const char *spatimesh_ext_bridge(void) {
    return "(function(){var h=window.webkit.messageHandlers.spatimeshExt;return function(s){h.postMessage(s);};})()";
}

void spatimesh_ext_clear(Engine *e) {
    e->extensions = [NSMutableArray array];
    e->entries = [NSMutableArray array];
    e->ruleSources = [NSMutableDictionary dictionary];
}

/* What a profile's extensions' rules make of a request is WebKit's to decide here: no asking. */
typedef int (*DecideFn)(const char *url, const char *initiator, int kind, const char *method, char *redirect, size_t cap);
void spatimesh_ext_decider(DecideFn decide) {
    (void)decide;
}

/* An extension's rules, as WebKit's content blockers (JSON): compiled when the set is whole. */
void spatimesh_ext_rules(Engine *e, const char *ident, const char *webkit) {
    @autoreleasepool {
        e->ruleSources[text_of(ident)] = text_of(webkit);
    }
}

void spatimesh_ext_add(Engine *e, const char *ident, const char *scheme, const char *world, const char *content_boot, const char *page_boot) {
    @autoreleasepool {
        MeshExtension *x = [[MeshExtension alloc] init];
        x.ident = text_of(ident);
        x.scheme = text_of(scheme);
        x.world = text_of(world);
        x.contentBoot = text_of(content_boot);
        x.pageBoot = text_of(page_boot);
        [e->extensions addObject:x];
    }
}

/* "a\nb" as a list; nil for "". */
static NSArray<NSString *> *lines_of(const char *text) {
    return text && *text ? [text_of(text) componentsSeparatedByString:@"\n"] : nil;
}

void spatimesh_ext_entry(Engine *e, const char *ident, int run_at, int all_frames, int main_world, const char *allow, const char *block, const char *guard,
                         int exact) {
    @autoreleasepool {
        NSString *wanted = text_of(ident);
        MeshExtension *x = nil;
        for (MeshExtension *each in e->extensions)
            if ([each.ident isEqualToString:wanted])
                x = each;
        if (!x)
            return;
        MeshEntry *n = [[MeshEntry alloc] init];
        n.extension = x;
        n.runAt = run_at;
        n.allFrames = all_frames != 0;
        n.mainWorld = main_world != 0;
        n.allow = lines_of(allow);
        n.block = lines_of(block);
        n.guard = guard && *guard ? text_of(guard) : @"true";
        n.exact = exact != 0;
        n.js = [NSMutableArray array];
        n.css = [NSMutableArray array];
        [e->entries addObject:n];
    }
}

/* A file of the entry added last. */
void spatimesh_ext_js(Engine *e, const char *code) {
    @autoreleasepool {
        [e->entries.lastObject.js addObject:text_of(code)];
    }
}

void spatimesh_ext_css(Engine *e, const char *code) {
    @autoreleasepool {
        [e->entries.lastObject.css addObject:text_of(code)];
    }
}

/* The set is whole: every tab has it for the pages it loads from now on. */
void spatimesh_ext_done(Engine *e) {
    @autoreleasepool {
        for (MeshTab *tab in e->tabs.allValues)
            apply_extensions(tab);
        compile_rule_lists(e);
    }
}

/* Code run in a tab's top frame, in `world` ("" the page's own), the extension's API set up
 * there first if `boot`; with `all_frames`, in the other frames its contexts have spoken from
 * too. The top frame's result is told if there is a `request`: in the page's own world, its
 * value once it settles (the code is then one expression), as Chrome's executeScript has it. */
/* SpatiMesh's own script for every frame's every document, from the next document on; the
 * pages open now have it put in again for theirs. */
void spatimesh_set_page_script(Engine *e, const char *code) {
    @autoreleasepool {
        e->pageScript = code ? [NSString stringWithUTF8String:code] : nil;
        for (MeshTab *tab in e->tabs.allValues)
            apply_extensions(tab);
    }
}

void spatimesh_run_script(Engine *e, uint64_t ident, const char *world, int all_frames, const char *code, uint64_t request, int boot) {
    @autoreleasepool {
        MeshTab *tab = find(e, ident);
        if (!tab) {
            if (request)
                push(e, EV_SCRIPT_RESULT, ident, (double)request, 0, 0, @"No such tab", nil);
            return;
        }
        NSString *name = text_of(world);
        WKContentWorld *in = name.length ? [WKContentWorld worldWithName:name] : [WKContentWorld pageWorld];
        NSString *source = text_of(code);
        if (boot && name.length) {
            for (MeshExtension *x in e->extensions) {
                if ([x.world isEqualToString:name]) {
                    source = [NSString stringWithFormat:@"%@\n%@", x.contentBoot, source];
                    break;
                }
            }
        }
        if (request && !name.length) {
            [tab.view callAsyncJavaScript:[NSString stringWithFormat:@"return (%@\n);", source]
                                arguments:nil
                                  inFrame:nil
                           inContentWorld:in
                        completionHandler:^(id result, NSError *error) {
                          answered(e, ident, request, result, error);
                        }];
        } else {
            [tab.view evaluateJavaScript:source
                                 inFrame:nil
                          inContentWorld:in
                       completionHandler:^(id result, NSError *error) {
                         if (request)
                             answered(e, ident, request, result, error);
                       }];
        }
        if (!all_frames)
            return;
        NSMutableDictionary<NSNumber *, WKFrameInfo *> *of = tab.frames[name];
        __weak NSMutableDictionary<NSNumber *, WKFrameInfo *> *kept = of;
        for (NSNumber *key in of.allKeys) {
            [tab.view evaluateJavaScript:source
                                 inFrame:of[key]
                          inContentWorld:in
                       completionHandler:^(id result, NSError *error) {
                         /* A frame that is gone is forgotten. */
                         if (error && [error.domain isEqualToString:WKErrorDomain] && error.code == WKErrorJavaScriptInvalidFrameTarget)
                             [kept removeObjectForKey:key];
                       }];
        }
    }
}
