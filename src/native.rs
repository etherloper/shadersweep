//! Windows-specific presentation helpers: executable icons and the window title bar.

use std::ffi::c_void;
use std::mem;
use std::os::windows::ffi::OsStrExt;
use std::path::Path;
use std::ptr;
use std::sync::Arc;

use gpui::{RenderImage, Window};
use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use smallvec::smallvec;
use windows_sys::Win32::Foundation::HWND;
use windows_sys::Win32::Graphics::Dwm::{
    DWMWA_CAPTION_COLOR, DWMWA_USE_IMMERSIVE_DARK_MODE, DwmSetWindowAttribute,
};
use windows_sys::Win32::Graphics::Gdi::{
    BI_RGB, BITMAP, BITMAPINFO, BITMAPINFOHEADER, CreateCompatibleDC, DIB_RGB_COLORS, DeleteDC,
    DeleteObject, GetDIBits, GetObjectW,
};
use windows_sys::Win32::UI::Shell::SHDefExtractIconW;
use windows_sys::Win32::UI::WindowsAndMessaging::{DestroyIcon, GetIconInfo, HICON, ICONINFO};

/// Loads the main icon embedded in an executable as a BGRA image, `size` pixels square.
pub fn exe_icon(path: &Path, size: u32) -> Option<Arc<RenderImage>> {
    let wide: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
    let mut icon: HICON = ptr::null_mut();
    // SAFETY: `wide` is NUL-terminated; on success we own `icon` and destroy it below.
    let status = unsafe { SHDefExtractIconW(wide.as_ptr(), 0, 0, &mut icon, ptr::null_mut(), size) };
    if status != 0 || icon.is_null() {
        return None;
    }
    // SAFETY: `icon` is a valid icon handle until DestroyIcon.
    let image = unsafe { icon_to_bgra(icon) };
    unsafe { DestroyIcon(icon) };
    let (width, height, pixels) = image?;
    let buffer = image::RgbaImage::from_raw(width, height, pixels)?;
    Some(Arc::new(RenderImage::new(smallvec![image::Frame::new(buffer)])))
}

/// Loads an image file (Steam's cached `.jpg`/`.ico` artwork) as a BGRA icon.
pub fn image_file_icon(path: &Path) -> Option<Arc<RenderImage>> {
    let mut rgba = image::open(path).ok()?.to_rgba8();
    // GPUI stores images as BGRA.
    for pixel in rgba.pixels_mut() {
        pixel.0.swap(0, 2);
    }
    Some(Arc::new(RenderImage::new(smallvec![image::Frame::new(rgba)])))
}

/// Reads the colour bitmap of an icon as top-down BGRA rows (GPUI's native layout).
unsafe fn icon_to_bgra(icon: HICON) -> Option<(u32, u32, Vec<u8>)> {
    unsafe {
        let mut info: ICONINFO = mem::zeroed();
        if GetIconInfo(icon, &mut info) == 0 {
            return None;
        }
        let cleanup = |info: &ICONINFO| {
            if !info.hbmColor.is_null() {
                DeleteObject(info.hbmColor);
            }
            if !info.hbmMask.is_null() {
                DeleteObject(info.hbmMask);
            }
        };
        if info.hbmColor.is_null() {
            cleanup(&info);
            return None; // monochrome icon; the monogram fallback looks better anyway
        }

        let mut bitmap: BITMAP = mem::zeroed();
        let got = GetObjectW(
            info.hbmColor,
            mem::size_of::<BITMAP>() as i32,
            (&mut bitmap as *mut BITMAP).cast::<c_void>(),
        );
        let (width, height) = (bitmap.bmWidth, bitmap.bmHeight);
        if got == 0 || width <= 0 || height <= 0 {
            cleanup(&info);
            return None;
        }

        let mut header: BITMAPINFO = mem::zeroed();
        header.bmiHeader = BITMAPINFOHEADER {
            biSize: mem::size_of::<BITMAPINFOHEADER>() as u32,
            biWidth: width,
            biHeight: -height, // negative = top-down rows
            biPlanes: 1,
            biBitCount: 32,
            biCompression: BI_RGB,
            ..mem::zeroed()
        };
        let mut pixels = vec![0u8; width as usize * height as usize * 4];
        let dc = CreateCompatibleDC(ptr::null_mut());
        let rows = GetDIBits(
            dc,
            info.hbmColor,
            0,
            height as u32,
            pixels.as_mut_ptr().cast(),
            &mut header,
            DIB_RGB_COLORS,
        );
        DeleteDC(dc);
        cleanup(&info);
        if rows == 0 {
            return None;
        }
        // Old 24-bit icons carry no alpha channel; treat them as fully opaque.
        let (bgra, _) = pixels.as_chunks_mut::<4>();
        if bgra.iter().all(|px| px[3] == 0) {
            bgra.iter_mut().for_each(|px| px[3] = 255);
        }
        Some((width as u32, height as u32, pixels))
    }
}

/// Matches the native title bar to the app theme: light or dark caption text and the app
/// background as the caption colour (Windows 11).
pub fn style_title_bar(window: &Window, background: u32, dark: bool) {
    // `Window` has an inherent `window_handle()` returning a GPUI handle; we need the raw one.
    let Ok(handle) = HasWindowHandle::window_handle(window) else { return };
    let RawWindowHandle::Win32(win32) = handle.as_raw() else { return };
    let hwnd = win32.hwnd.get() as HWND;
    let dark = i32::from(dark);
    // COLORREF is 0x00BBGGRR.
    let caption: u32 = ((background & 0xff) << 16) | (background & 0xff00) | ((background >> 16) & 0xff);
    // SAFETY: `hwnd` is this process's live window; attribute sizes match their types.
    // Failures (older Windows) are harmless and leave the default title bar.
    unsafe {
        DwmSetWindowAttribute(
            hwnd,
            DWMWA_USE_IMMERSIVE_DARK_MODE as u32,
            (&dark as *const i32).cast(),
            mem::size_of::<i32>() as u32,
        );
        DwmSetWindowAttribute(
            hwnd,
            DWMWA_CAPTION_COLOR as u32,
            (&caption as *const u32).cast(),
            mem::size_of::<u32>() as u32,
        );
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn extracts_an_icon_from_explorer() {
        let windir = std::env::var("WINDIR").unwrap_or_else(|_| r"C:\Windows".into());
        let icon = super::exe_icon(&std::path::Path::new(&windir).join("explorer.exe"), 48)
            .expect("explorer.exe has an icon");
        let size = icon.size(0);
        assert_eq!((size.width.0, size.height.0), (48, 48));
    }
}
