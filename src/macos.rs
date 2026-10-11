//! Minimal Objective-C/AX FFI. All AppKit calls stay on the main thread.
#![allow(unsafe_op_in_unsafe_fn)]
use std::{
    ffi::{CStr, CString, c_char, c_void},
    ptr,
};
type Id = *mut c_void;
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Point {
    pub x: f64,
    pub y: f64,
}
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Size {
    pub width: f64,
    pub height: f64,
}
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Rect {
    pub origin: Point,
    pub size: Size,
}
#[repr(C)]
#[derive(Clone, Copy, Default)]
struct Range {
    location: isize,
    length: isize,
}
#[link(name = "AppKit", kind = "framework")]
unsafe extern "C" {}
#[link(name = "ApplicationServices", kind = "framework")]
unsafe extern "C" {
    fn AXIsProcessTrusted() -> bool;
    fn AXUIElementCreateApplication(pid: i32) -> Id;
    fn AXUIElementCopyAttributeValue(e: Id, a: Id, v: *mut Id) -> i32;
    fn AXUIElementCopyParameterizedAttributeValue(e: Id, a: Id, p: Id, v: *mut Id) -> i32;
    fn AXUIElementSetMessagingTimeout(e: Id, timeout: f32) -> i32;
    fn AXValueGetValue(v: Id, kind: i32, out: *mut c_void) -> bool;
    fn AXValueCreate(kind: i32, value: *const c_void) -> Id;
}
#[link(name = "CoreFoundation", kind = "framework")]
unsafe extern "C" {
    fn CFRelease(value: Id);
    fn CFEqual(a: Id, b: Id) -> bool;
}
#[link(name = "objc")]
unsafe extern "C" {
    fn objc_getClass(name: *const c_char) -> Id;
    fn sel_registerName(name: *const c_char) -> Id;
    fn objc_msgSend();
    #[cfg(target_arch = "x86_64")]
    fn objc_msgSend_stret();
}
unsafe fn class(s: &str) -> Id {
    objc_getClass(CString::new(s).unwrap().as_ptr())
}
unsafe fn sel(s: &str) -> Id {
    sel_registerName(CString::new(s).unwrap().as_ptr())
}
unsafe fn m0<R>(o: Id, s: &str) -> R {
    let f: unsafe extern "C" fn(Id, Id) -> R = std::mem::transmute(objc_msgSend as *const ());
    f(o, sel(s))
}
unsafe fn m1<A, R>(o: Id, s: &str, a: A) -> R {
    let f: unsafe extern "C" fn(Id, Id, A) -> R = std::mem::transmute(objc_msgSend as *const ());
    f(o, sel(s), a)
}
unsafe fn m2<A, B, R>(o: Id, s: &str, a: A, b: B) -> R {
    let f: unsafe extern "C" fn(Id, Id, A, B) -> R = std::mem::transmute(objc_msgSend as *const ());
    f(o, sel(s), a, b)
}
unsafe fn m4<A, B, C, D, R>(o: Id, s: &str, a: A, b: B, c: C, d: D) -> R {
    let f: unsafe extern "C" fn(Id, Id, A, B, C, D) -> R =
        std::mem::transmute(objc_msgSend as *const ());
    f(o, sel(s), a, b, c, d)
}
unsafe fn ns(s: &str) -> Id {
    let clean = s.replace('\0', "�");
    m1(
        class("NSString"),
        "stringWithUTF8String:",
        CString::new(clean).unwrap().as_ptr(),
    )
}
unsafe fn string(o: Id) -> Option<String> {
    if o.is_null() {
        return None;
    }
    let p: *const c_char = m0(o, "UTF8String");
    if p.is_null() {
        None
    } else {
        Some(CStr::from_ptr(p).to_string_lossy().into_owned())
    }
}
unsafe fn rect(o: Id, s: &str) -> Rect {
    #[cfg(target_arch = "aarch64")]
    {
        m0(o, s)
    }
    #[cfg(target_arch = "x86_64")]
    {
        let mut r = Rect::default();
        let f: unsafe extern "C" fn(*mut Rect, Id, Id) =
            std::mem::transmute(objc_msgSend_stret as *const ());
        f(&mut r, o, sel(s));
        r
    }
}
unsafe fn attr(e: Id, name: &str) -> Option<Id> {
    let mut v = ptr::null_mut();
    if AXUIElementCopyAttributeValue(e, ns(name), &mut v) == 0 && !v.is_null() {
        Some(v)
    } else {
        None
    }
}
struct Owned(Id);
impl Drop for Owned {
    fn drop(&mut self) {
        unsafe { CFRelease(self.0) }
    }
}
pub struct Pool(Id);
impl Pool {
    pub fn new() -> Self {
        unsafe { Self(m0::<Id>(class("NSAutoreleasePool"), "new")) }
    }
}
impl Drop for Pool {
    fn drop(&mut self) {
        unsafe { m0::<()>(self.0, "drain") }
    }
}
#[derive(Clone, Debug)]
pub struct Geometry {
    pub pane: String,
    pub caret: Rect,
}
struct StatusItem(Id);
impl Drop for StatusItem {
    fn drop(&mut self) {
        unsafe {
            let bar = m0::<Id>(class("NSStatusBar"), "systemStatusBar");
            m1::<_, ()>(bar, "removeStatusItem:", self.0);
            m0::<()>(self.0, "release");
        }
    }
}
// NSTextAlignment uses iOS values on Apple Silicon.
#[cfg(target_arch = "aarch64")]
const ALIGN_CENTER: isize = 1;
#[cfg(target_arch = "aarch64")]
const ALIGN_RIGHT: isize = 2;
#[cfg(target_arch = "x86_64")]
const ALIGN_CENTER: isize = 2;
#[cfg(target_arch = "x86_64")]
const ALIGN_RIGHT: isize = 1;
const MAX_ROWS: usize = 7;
const PAD: f64 = 5.;
const ROW: f64 = 26.;
const FOOT: f64 = 26.;
// ponytail: fixed advance of the 13pt monospaced system font; measure with NSFont if sizes become configurable.
const CHAR_W: f64 = 8.;
struct Row {
    bg: Id,
    glyph: Id,
    label: Id,
    desc: Id,
}
unsafe fn rgb(hex: u32, alpha: f64) -> Id {
    let c = |shift: u32| ((hex >> shift) & 0xff) as f64 / 255.;
    m4(
        class("NSColor"),
        "colorWithSRGBRed:green:blue:alpha:",
        c(16),
        c(8),
        c(0),
        alpha,
    )
}
unsafe fn mono(size: f64, weight: f64) -> Id {
    m2(
        class("NSFont"),
        "monospacedSystemFontOfSize:weight:",
        size,
        weight,
    )
}
unsafe fn subview(parent: Id, cls: Id) -> Id {
    let v = m1(m0::<Id>(cls, "alloc"), "initWithFrame:", Rect::default());
    m1::<_, ()>(parent, "addSubview:", v);
    v
}
/// Gives `view` a rounded, filled layer and returns the layer.
unsafe fn layered(view: Id, color: Id, radius: f64) -> Id {
    m1::<_, ()>(view, "setWantsLayer:", true);
    let layer = m0::<Id>(view, "layer");
    m1::<_, ()>(layer, "setCornerRadius:", radius);
    m1::<_, ()>(layer, "setBackgroundColor:", m0::<Id>(color, "CGColor"));
    layer
}
unsafe fn text_field(parent: Id, font: Id) -> Id {
    let f = subview(parent, class("NSTextField"));
    m1::<_, ()>(f, "setEditable:", false);
    m1::<_, ()>(f, "setSelectable:", false);
    m1::<_, ()>(f, "setBezeled:", false);
    m1::<_, ()>(f, "setDrawsBackground:", false);
    m1::<_, ()>(f, "setFont:", font);
    m1::<_, ()>(f, "setMaximumNumberOfLines:", 1isize);
    m1::<_, ()>(f, "setLineBreakMode:", 4usize); // truncate tail
    m1::<_, ()>(m0::<Id>(f, "cell"), "setUsesSingleLineMode:", true);
    f
}
fn kind_style(kind: &str) -> (&'static str, u32) {
    match kind {
        "directory" => ("▸", 0x78aaff),
        "file" => ("≡", 0xa7afbc),
        "flag" => ("−", 0x62c6d8),
        "value" => ("◆", 0xb998ff),
        "ref" => ("⎇", 0xf2a65e),
        _ => ("›", 0x86d08a),
    }
}
/// Footer text such as `⇥ insert   ⌃N ⌃P move   ⌃G close` from zsh `bindkey` notation.
pub fn key_hints([next, prev, accept, dismiss]: &[String; 4]) -> String {
    format!(
        "{} insert   {} {} move   {} close",
        key_label(accept),
        key_label(next),
        key_label(prev),
        key_label(dismiss)
    )
}
fn key_label(key: &str) -> String {
    let mut out = String::new();
    let mut rest = key;
    while let Some(c) = rest.chars().next() {
        rest = if let Some(r) = rest.strip_prefix("^[").or(rest.strip_prefix("\\e")) {
            out.push('⌥');
            r
        } else if let Some(r) = rest.strip_prefix("^I").or(rest.strip_prefix("\\t")) {
            out.push('⇥');
            r
        } else if let Some(r) = rest.strip_prefix('^')
            && let Some(c) = r.chars().next()
        {
            out.push('⌃');
            out.extend(c.to_uppercase());
            &r[c.len_utf8()..]
        } else {
            out.extend(c.to_uppercase());
            &rest[c.len_utf8()..]
        };
    }
    out
}
pub struct Desktop {
    app: Id,
    status_item: Option<StatusItem>,
    panel: Id,
    rows: Vec<Row>,
    separator: Id,
    footer: Id,
    hints: Id,
    script: Id,
    pub reason: String,
    last_focus: Option<Owned>,
    last_pid: Option<i32>,
    visible: bool,
    last_text: String,
    last_frame: Option<Rect>,
}
impl Desktop {
    pub fn new() -> Self {
        unsafe {
            let app = m0(class("NSApplication"), "sharedApplication");
            let _ = m1::<_, bool>(app, "setActivationPolicy:", 2isize);
            m0::<()>(app, "finishLaunching");
            let panel = m4(
                m0::<Id>(class("NSPanel"), "alloc"),
                "initWithContentRect:styleMask:backing:defer:",
                Rect::default(),
                128usize,
                2usize,
                false,
            );
            m1::<_, ()>(panel, "setLevel:", 3isize);
            m1::<_, ()>(panel, "setHidesOnDeactivate:", false);
            m1::<_, ()>(panel, "setBecomesKeyOnlyIfNeeded:", true);
            m1::<_, ()>(panel, "setIgnoresMouseEvents:", true);
            m1::<_, ()>(panel, "setCollectionBehavior:", 273usize);
            m1::<_, ()>(panel, "setHasShadow:", true);
            // Fixed dark palette matching the terminal; rounded corners come from the content layer.
            m1::<_, ()>(panel, "setOpaque:", false);
            m1::<_, ()>(
                panel,
                "setBackgroundColor:",
                m0::<Id>(class("NSColor"), "clearColor"),
            );
            let content = m0::<Id>(panel, "contentView");
            let root = layered(content, rgb(0x1f2229, 1.), 9.);
            m1::<_, ()>(root, "setMasksToBounds:", true);
            m1::<_, ()>(root, "setBorderWidth:", 1f64);
            m1::<_, ()>(
                root,
                "setBorderColor:",
                m0::<Id>(rgb(0x343944, 1.), "CGColor"),
            );
            let rows = (0..MAX_ROWS)
                .map(|_| {
                    let bg = subview(content, class("NSView"));
                    layered(bg, rgb(0x2e4a8f, 1.), 5.);
                    let glyph =
                        text_field(content, m1(class("NSFont"), "systemFontOfSize:", 11f64));
                    m1::<_, ()>(glyph, "setAlignment:", ALIGN_CENTER);
                    layered(glyph, rgb(0, 0.), 4.);
                    let label = text_field(content, mono(13., 0.23));
                    let desc = text_field(content, mono(12., 0.));
                    Row {
                        bg,
                        glyph,
                        label,
                        desc,
                    }
                })
                .collect();
            let separator = subview(content, class("NSView"));
            layered(separator, rgb(0x30343e, 1.), 0.);
            let footer = text_field(content, m1(class("NSFont"), "systemFontOfSize:", 11f64));
            m1::<_, ()>(footer, "setTextColor:", rgb(0x7a8190, 1.));
            let hints = text_field(content, m1(class("NSFont"), "systemFontOfSize:", 11f64));
            m1::<_, ()>(hints, "setTextColor:", rgb(0x7a8190, 1.));
            m1::<_, ()>(hints, "setAlignment:", ALIGN_RIGHT);
            let script = m1(
                m0::<Id>(class("NSAppleScript"), "alloc"),
                "initWithSource:",
                ns(
                    "with timeout of 1 seconds\ntell application \"iTerm2\" to get unique ID of current session of current window\nend timeout",
                ),
            );
            Self {
                app,
                status_item: None,
                panel,
                rows,
                separator,
                footer,
                hints,
                script,
                reason: String::new(),
                last_focus: None,
                last_pid: None,
                visible: false,
                last_text: String::new(),
                last_frame: None,
            }
        }
    }
    pub fn show_status_item(&mut self) {
        if self.status_item.is_some() {
            return;
        }
        unsafe {
            // Accessory apps can show menus without appearing in the Dock.
            let _ = m1::<_, bool>(self.app, "setActivationPolicy:", 1isize);
            let bar = m0::<Id>(class("NSStatusBar"), "systemStatusBar");
            let item = m1::<_, Id>(bar, "statusItemWithLength:", -2f64);
            // NSStatusBar does not retain its items. Keep this alive with Desktop.
            self.status_item = Some(StatusItem(m0(item, "retain")));
            let button = m0::<Id>(item, "button");
            let icon = m2::<_, _, Id>(
                class("NSImage"),
                "imageWithSystemSymbolName:accessibilityDescription:",
                ns("terminal"),
                ns("Riga"),
            );
            if icon.is_null() {
                m1::<_, ()>(button, "setTitle:", ns("R"));
            } else {
                m1::<_, ()>(icon, "setTemplate:", true);
                m1::<_, ()>(button, "setImage:", icon);
            }
            m1::<_, ()>(button, "setToolTip:", ns("Riga — running"));
            m1::<_, ()>(button, "setAccessibilityLabel:", ns("Riga"));

            let menu = m0::<Id>(class("NSMenu"), "new");
            m1::<_, ()>(menu, "setAutoenablesItems:", false);
            let status = m0::<Id>(class("NSMenuItem"), "new");
            m1::<_, ()>(status, "setTitle:", ns("Riga is running"));
            m1::<_, ()>(status, "setEnabled:", false);
            m1::<_, ()>(menu, "addItem:", status);
            m0::<()>(status, "release");
            m1::<_, ()>(
                menu,
                "addItem:",
                m0::<Id>(class("NSMenuItem"), "separatorItem"),
            );
            let quit = m0::<Id>(class("NSMenuItem"), "new");
            m1::<_, ()>(quit, "setTitle:", ns("Quit Riga"));
            m1::<_, ()>(quit, "setTarget:", self.app);
            m1::<_, ()>(quit, "setAction:", sel("terminate:"));
            m1::<_, ()>(menu, "addItem:", quit);
            m0::<()>(quit, "release");
            m1::<_, ()>(item, "setMenu:", menu);
            m0::<()>(menu, "release");
        }
    }
    pub fn trusted() -> bool {
        unsafe { AXIsProcessTrusted() }
    }
    unsafe fn front() -> Option<(Id, i32)> {
        let workspace = m0::<Id>(class("NSWorkspace"), "sharedWorkspace");
        let front = m0::<Id>(workspace, "frontmostApplication");
        if string(m0(front, "bundleIdentifier")).as_deref() != Some("com.googlecode.iterm2") {
            return None;
        }
        Some((front, m0(front, "processIdentifier")))
    }
    unsafe fn pane(&self) -> Option<String> {
        let mut error: Id = ptr::null_mut();
        let value: Id = m1(self.script, "executeAndReturnError:", &mut error as *mut Id);
        if !error.is_null() || value.is_null() {
            None
        } else {
            string(m0(value, "stringValue"))
        }
    }
    pub fn probe(&mut self) -> Option<Geometry> {
        // Keep a verified panel stable while refreshing the same AX element.
        let result = unsafe { self.probe_inner() };
        match result {
            Ok(g) => {
                self.reason = "AX caret and pane identity verified".into();
                Some(g)
            }
            Err(e) => {
                self.hide();
                self.last_focus = None;
                self.last_pid = None;
                self.reason = e.into();
                None
            }
        }
    }
    unsafe fn probe_inner(&mut self) -> Result<Geometry, &'static str> {
        if !Self::trusted() {
            return Err("Accessibility permission missing");
        }
        let (_, pid) = Self::front().ok_or("iTerm2 is not frontmost")?;
        let app = Owned(AXUIElementCreateApplication(pid));
        AXUIElementSetMessagingTimeout(app.0, 0.05);
        let focused =
            Owned(attr(app.0, "AXFocusedUIElement").ok_or("AXFocusedUIElement unavailable")?);
        if self.last_pid != Some(pid)
            || self
                .last_focus
                .as_ref()
                .is_none_or(|previous| !CFEqual(previous.0, focused.0))
        {
            self.hide();
        }
        let role = Owned(attr(focused.0, "AXRole").ok_or("AXRole unavailable")?);
        if string(role.0).as_deref() != Some("AXTextArea") {
            return Err("Focused AX element is not terminal text");
        }
        let pane = self
            .pane()
            .ok_or("iTerm2 Automation permission or session identity unavailable")?;
        let selected =
            Owned(attr(focused.0, "AXSelectedTextRange").ok_or("AXSelectedTextRange unavailable")?);
        let mut range = Range::default();
        if !AXValueGetValue(selected.0, 4, &mut range as *mut _ as *mut c_void) || range.length != 0
        {
            return Err("Terminal selection is not an insertion caret");
        }
        let mut value = ptr::null_mut();
        let mut caret = Rect::default();
        let mut found = false;
        for len in [0, 1] {
            range.length = len;
            let param = Owned(AXValueCreate(4, &range as *const _ as *const c_void));
            if AXUIElementCopyParameterizedAttributeValue(
                focused.0,
                ns("AXBoundsForRange"),
                param.0,
                &mut value,
            ) == 0
                && !value.is_null()
            {
                let v = Owned(value);
                if AXValueGetValue(v.0, 3, &mut caret as *mut _ as *mut c_void)
                    && caret.size.height > 0.0
                {
                    found = true;
                    break;
                }
            }
        }
        if !found || !caret.origin.x.is_finite() || !caret.origin.y.is_finite() {
            return Err("AXBoundsForRange did not return a usable caret");
        }
        let focused_after =
            Owned(attr(app.0, "AXFocusedUIElement").ok_or("Focus changed during AX query")?);
        if !CFEqual(focused.0, focused_after.0)
            || Self::front().map(|x| x.1) != Some(pid)
            || self.pane().as_deref() != Some(&pane)
        {
            return Err("Pane or foreground changed during query");
        }
        self.last_focus = Some(focused);
        self.last_pid = Some(pid);
        Ok(Geometry { pane, caret })
    }
    pub fn hide(&mut self) {
        if self.visible {
            unsafe {
                m1::<_, ()>(self.panel, "orderOut:", ptr::null_mut::<c_void>());
            }
            self.visible = false;
        }
    }
    pub fn show(
        &mut self,
        geometry: &Geometry,
        items: &[riga::completion::Suggestion],
        selected: usize,
        hints: &str,
    ) {
        unsafe {
            let screens = m0::<Id>(class("NSScreen"), "screens");
            let count: usize = m0(screens, "count");
            if count == 0 {
                return;
            }
            let first = m1::<_, Id>(screens, "objectAtIndex:", 0usize);
            let top = rect(first, "frame").size.height;
            let x = geometry.caret.origin.x;
            let y = top - geometry.caret.origin.y - geometry.caret.size.height;
            let mut bounds = rect(first, "visibleFrame");
            for i in 0..count {
                let screen = m1::<_, Id>(screens, "objectAtIndex:", i);
                let frame = rect(screen, "frame");
                if x >= frame.origin.x
                    && x < frame.origin.x + frame.size.width
                    && y >= frame.origin.y
                    && y < frame.origin.y + frame.size.height
                {
                    bounds = rect(screen, "visibleFrame");
                    break;
                }
            }
            let offset = selected.saturating_sub(MAX_ROWS - 1);
            let rows: Vec<_> = items
                .iter()
                .enumerate()
                .skip(offset)
                .take(MAX_ROWS)
                .collect();
            let h = PAD + rows.len() as f64 * ROW + 1. + FOOT;
            let w = 520f64.min(bounds.size.width);
            let clean = |s: &str| -> String {
                s.chars()
                    .map(|c| if c.is_control() { '�' } else { c })
                    .collect()
            };
            // Size the name column over every item so it doesn't jump while scrolling.
            let longest = items
                .iter()
                .map(|s| s.label.chars().count())
                .max()
                .unwrap_or(0);
            let lw = longest.clamp(4, 22) as f64 * CHAR_W + 12.; // + text field insets
            let footer = if items.len() > MAX_ROWS {
                format!(
                    "{} suggestions · {}/{}",
                    items.len(),
                    selected + 1,
                    items.len()
                )
            } else if items.len() == 1 {
                "1 suggestion".into()
            } else {
                format!("{} suggestions", items.len())
            };
            let mut text = format!("{w}\0{footer}\0{hints}");
            for (i, s) in &rows {
                text += &format!(
                    "\0{}\0{}\0{}\0{}",
                    *i == selected,
                    s.kind,
                    s.label,
                    s.description
                );
            }
            if text != self.last_text {
                let frame = |x: f64, y: f64, width: f64, height: f64| Rect {
                    origin: Point { x, y },
                    size: Size { width, height },
                };
                for (slot, row) in self.rows.iter().enumerate() {
                    let views = [row.bg, row.glyph, row.label, row.desc];
                    let Some((i, s)) = rows.get(slot) else {
                        for v in views {
                            m1::<_, ()>(v, "setHidden:", true);
                        }
                        continue;
                    };
                    let on = *i == selected;
                    let top = h - PAD - (slot + 1) as f64 * ROW;
                    let (glyph, color) = kind_style(s.kind);
                    let lx = PAD + 34.;
                    let dx = lx + lw + 10.;
                    m1::<_, ()>(row.bg, "setFrame:", frame(PAD, top, w - 2. * PAD, ROW));
                    m1::<_, ()>(row.glyph, "setFrame:", frame(PAD + 6., top + 3., 18., 18.));
                    m1::<_, ()>(row.label, "setFrame:", frame(lx, top + 4., lw, 17.));
                    m1::<_, ()>(
                        row.desc,
                        "setFrame:",
                        frame(dx, top + 4., (w - PAD - 6. - dx).max(0.), 17.),
                    );
                    for v in views {
                        m1::<_, ()>(v, "setHidden:", false);
                    }
                    m1::<_, ()>(row.bg, "setHidden:", !on);
                    m1::<_, ()>(row.glyph, "setStringValue:", ns(glyph));
                    m1::<_, ()>(row.glyph, "setTextColor:", rgb(color, 1.));
                    m1::<_, ()>(
                        m0::<Id>(row.glyph, "layer"),
                        "setBackgroundColor:",
                        m0::<Id>(rgb(color, 0.16), "CGColor"),
                    );
                    m1::<_, ()>(row.label, "setStringValue:", ns(&clean(&s.label)));
                    m1::<_, ()>(
                        row.label,
                        "setTextColor:",
                        rgb(if on { 0xffffff } else { 0xc9cdd5 }, 1.),
                    );
                    m1::<_, ()>(row.desc, "setStringValue:", ns(&clean(&s.description)));
                    m1::<_, ()>(
                        row.desc,
                        "setTextColor:",
                        rgb(if on { 0xc4d3ff } else { 0x858c9a }, 1.),
                    );
                }
                m1::<_, ()>(
                    self.separator,
                    "setFrame:",
                    frame(PAD + 6., FOOT, w - 2. * PAD - 12., 1.),
                );
                m1::<_, ()>(
                    self.footer,
                    "setFrame:",
                    frame(PAD + 6., 6., w - 2. * PAD - 12., 14.),
                );
                m1::<_, ()>(self.footer, "setStringValue:", ns(&footer));
                m1::<_, ()>(
                    self.hints,
                    "setFrame:",
                    frame(PAD + 6., 6., w - 2. * PAD - 12., 14.),
                );
                m1::<_, ()>(self.hints, "setStringValue:", ns(hints));
                self.last_text = text;
            }
            let py = if y - h - 4. >= bounds.origin.y {
                y - h - 4.
            } else {
                (y + geometry.caret.size.height + 4.).min(bounds.origin.y + bounds.size.height - h)
            };
            let frame = Rect {
                origin: Point {
                    x: x.max(bounds.origin.x)
                        .min(bounds.origin.x + bounds.size.width - w),
                    y: py,
                },
                size: Size {
                    width: w,
                    height: h,
                },
            };
            if self.last_frame != Some(frame) {
                m2::<_, _, ()>(self.panel, "setFrame:display:", frame, true);
                // The shadow follows the rounded content, so recompute it when the size changes.
                m0::<()>(self.panel, "invalidateShadow");
                self.last_frame = Some(frame);
            }
            if !self.visible {
                m0::<()>(self.panel, "orderFrontRegardless");
                self.visible = true;
            }
        }
    }
    pub fn pump(&self) {
        unsafe {
            let date = m0::<Id>(class("NSDate"), "distantPast");
            for _ in 0..32 {
                let event: Id = m4(
                    self.app,
                    "nextEventMatchingMask:untilDate:inMode:dequeue:",
                    usize::MAX,
                    date,
                    ns("kCFRunLoopDefaultMode"),
                    true,
                );
                if event.is_null() {
                    break;
                }
                m1::<_, ()>(self.app, "sendEvent:", event);
            }
            m0::<()>(self.app, "updateWindows");
        }
    }
}
#[cfg(test)]
mod tests {
    #[test]
    fn key_labels_follow_bindkey_notation() {
        let keys = ["^N", "^P", "^I", "^G"].map(String::from);
        assert_eq!(super::key_hints(&keys), "⇥ insert   ⌃N ⌃P move   ⌃G close");
        assert_eq!(super::key_label("^[j"), "⌥J");
        assert_eq!(super::key_label("\\ek"), "⌥K");
        assert_eq!(super::key_label("^X^N"), "⌃X⌃N");
        assert_eq!(super::key_label("\\t"), "⇥");
    }
}
