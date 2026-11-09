use super::startup::wide;
use std::io;
use windows_sys::Win32::{
    Foundation::{HWND, RECT},
    Graphics::Gdi::*,
    UI::Controls::{DRAWITEMSTRUCT, ODS_DISABLED, ODS_FOCUS, ODS_SELECTED},
};

const fn gray(value: u32) -> u32 {
    value * 0x010101
}

const BACKGROUND: u32 = gray(32);
const FIELD: u32 = gray(25);
const TEXT: u32 = gray(235);
const MUTED: u32 = gray(176);

struct Gdi(HGDIOBJ);

impl Gdi {
    fn new(handle: HGDIOBJ) -> io::Result<Self> {
        if handle.is_null() {
            Err(io::Error::last_os_error())
        } else {
            Ok(Self(handle))
        }
    }

    fn brush(color: u32) -> io::Result<Self> {
        Self::new(unsafe { CreateSolidBrush(color) })
    }
}

impl Drop for Gdi {
    fn drop(&mut self) {
        unsafe { DeleteObject(self.0) };
    }
}

pub const fn scale(dpi: u32, value: i32) -> i32 {
    (value * dpi as i32 + 48) / 96
}

pub struct Theme {
    pub dpi: u32,
    background: Gdi,
    field: Gdi,
    line: Gdi,
    button: Gdi,
    pressed: Gdi,
    outline: Gdi,
    focus: Gdi,
    font: Gdi,
}

impl Theme {
    pub fn new(dpi: u32) -> io::Result<Self> {
        let family = wide("Segoe UI");
        let font = unsafe {
            CreateFontW(
                -scale(dpi, 13),
                0,
                0,
                0,
                FW_NORMAL as i32,
                0,
                0,
                0,
                DEFAULT_CHARSET.into(),
                OUT_DEFAULT_PRECIS.into(),
                CLIP_DEFAULT_PRECIS.into(),
                CLEARTYPE_QUALITY.into(),
                DEFAULT_PITCH.into(),
                family.as_ptr(),
            )
        };
        Ok(Self {
            dpi,
            font: Gdi::new(font)?,
            background: Gdi::brush(BACKGROUND)?,
            field: Gdi::brush(FIELD)?,
            line: Gdi::brush(gray(64))?,
            button: Gdi::brush(gray(49))?,
            pressed: Gdi::brush(gray(67))?,
            outline: Gdi::brush(gray(100))?,
            focus: Gdi::brush(gray(195))?,
        })
    }

    pub fn font(&self) -> HFONT {
        self.font.0
    }

    pub fn paint(&self, hwnd: HWND) {
        let mut ps = PAINTSTRUCT::default();
        unsafe {
            let dc = BeginPaint(hwnd, &mut ps);
            FillRect(dc, &ps.rcPaint, self.background.0);
            for y in [76, 138] {
                let top = scale(self.dpi, y);
                let rect = RECT {
                    left: scale(self.dpi, 12),
                    top,
                    right: scale(self.dpi, 420),
                    bottom: top + 1,
                };
                FillRect(dc, &rect, self.line.0);
            }
            EndPaint(hwnd, &ps);
        }
    }

    pub fn colors(&self, dc: HDC, field: bool, secondary: bool) -> isize {
        let (background, brush) = if field {
            (FIELD, self.field.0)
        } else {
            (BACKGROUND, self.background.0)
        };
        unsafe {
            SetTextColor(dc, if secondary { MUTED } else { TEXT });
            SetBkColor(dc, background);
            SetBkMode(dc, OPAQUE as i32);
        }
        brush as isize
    }

    // Keep the native button's focus, click and accessibility behavior.
    pub fn button(&self, item: &DRAWITEMSTRUCT, label: &str) {
        let (fill, ink) = if item.itemState & ODS_DISABLED != 0 {
            (self.background.0, gray(137))
        } else if item.itemState & ODS_SELECTED != 0 {
            (self.pressed.0, TEXT)
        } else {
            (self.button.0, TEXT)
        };
        let edge = if item.itemState & ODS_FOCUS != 0 {
            self.focus.0
        } else {
            self.outline.0
        };
        let text = wide(label);
        let mut rect = item.rcItem;
        unsafe {
            FillRect(item.hDC, &rect, fill);
            FrameRect(item.hDC, &rect, edge);
            let old = SelectObject(item.hDC, self.font.0);
            SetBkMode(item.hDC, TRANSPARENT as i32);
            SetTextColor(item.hDC, ink);
            DrawTextW(
                item.hDC,
                text.as_ptr(),
                -1,
                &mut rect,
                DT_CENTER | DT_VCENTER | DT_SINGLELINE | DT_NOPREFIX,
            );
            if !old.is_null() {
                SelectObject(item.hDC, old);
            }
        }
    }
}
