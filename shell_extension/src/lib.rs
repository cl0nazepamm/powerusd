#![allow(non_snake_case)]

use std::ffi::{c_void, OsStr, OsString};
use std::mem::{size_of, zeroed};
use std::os::windows::ffi::{OsStrExt, OsStringExt};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::ptr::null_mut;
use std::sync::atomic::{AtomicPtr, AtomicU32, Ordering};
use std::sync::Mutex;

use windows::core::{implement, w, Error, GUID, HRESULT, Interface, IUnknown, PCWSTR, PWSTR};
use windows::Win32::Foundation::{
    BOOL, CLASS_E_CLASSNOTAVAILABLE, E_FAIL, E_NOTIMPL, E_POINTER, HANDLE, HINSTANCE, HWND,
    HMODULE, LPARAM, LRESULT, MAX_PATH, RECT, S_FALSE, S_OK, WPARAM,
};
use windows::Win32::Graphics::Gdi::{
    CreateDIBSection, BITMAPINFO, BITMAPINFOHEADER, BI_RGB, DIB_RGB_COLORS, HBITMAP,
};
use windows::Win32::System::Com::{IClassFactory, IClassFactory_Impl};
use windows::Win32::System::LibraryLoader::GetModuleFileNameW;
use windows::Win32::System::Ole::{
    IObjectWithSite, IObjectWithSite_Impl, IOleWindow, IOleWindow_Impl,
};
use windows::Win32::System::Registry::{
    RegCloseKey, RegCreateKeyExW, RegDeleteTreeW, RegOpenKeyExW, RegSetValueExW, HKEY,
    HKEY_CURRENT_USER, KEY_READ, KEY_WRITE, REG_DWORD, REG_OPTION_NON_VOLATILE, REG_SZ,
};
use windows::Win32::System::Threading::GetCurrentProcessId;
use windows::Win32::UI::Input::KeyboardAndMouse::{GetFocus, SetFocus};
use windows::Win32::UI::Shell::{
    IPreviewHandler, IPreviewHandler_Impl, IPreviewHandlerVisuals, IPreviewHandlerVisuals_Impl,
    IThumbnailProvider, IThumbnailProvider_Impl, WTS_ALPHATYPE, WTSAT_ARGB,
};
use windows::Win32::UI::Shell::PropertiesSystem::{
    IInitializeWithFile, IInitializeWithFile_Impl,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, IsWindow, RegisterClassW, SetWindowLongPtrW,
    SetWindowPos, CREATESTRUCTW, CS_HREDRAW, CS_VREDRAW, GWLP_USERDATA, HMENU, SWP_NOACTIVATE,
    SWP_NOZORDER, WINDOW_EX_STYLE, WM_CREATE, WM_DESTROY, WNDCLASSW, WS_CHILD,
    WS_CLIPCHILDREN, WS_CLIPSIBLINGS, WS_VISIBLE,
};

const CLSID_POWERUSD_THUMBNAIL: GUID =
    GUID::from_u128(0xc5d7a75f_95bc_4bd2_9d5d_4df5c78b68f1);
const CLSID_POWERUSD_PREVIEW: GUID =
    GUID::from_u128(0xd2251698_70f2_4770_8ba8_4d1ea4c7e7a6);
const CLSID_POWERUSD_THUMBNAIL_STR: &str = "{C5D7A75F-95BC-4BD2-9D5D-4DF5C78B68F1}";
const CLSID_POWERUSD_PREVIEW_STR: &str = "{D2251698-70F2-4770-8BA8-4D1EA4C7E7A6}";
const IID_ICLASSFACTORY: GUID = GUID::from_u128(0x00000001_0000_0000_c000_000000000046);
const EXTENSIONS: &[&str] = &[".usd", ".usda", ".usdc", ".usdz"];

static DLL_REFCOUNT: AtomicU32 = AtomicU32::new(0);
static MODULE_HANDLE: AtomicPtr<c_void> = AtomicPtr::new(null_mut());

fn error(hr: HRESULT) -> Error {
    Error::from(hr)
}

fn null_hwnd() -> HWND {
    HWND(null_mut())
}

fn null_hmenu() -> HMENU {
    HMENU(null_mut())
}

fn null_hinstance() -> HINSTANCE {
    HINSTANCE(null_mut())
}

fn null_hmodule() -> HMODULE {
    HMODULE(null_mut())
}

fn null_handle() -> HANDLE {
    HANDLE(null_mut())
}

fn null_hkey() -> HKEY {
    HKEY(null_mut())
}

#[derive(Clone, Copy)]
enum ClassKind {
    Thumbnail,
    Preview,
}

#[implement(IClassFactory)]
struct ClassFactory {
    kind: ClassKind,
}

impl IClassFactory_Impl for ClassFactory_Impl {
    fn CreateInstance(
        &self,
        punkouter: Option<&IUnknown>,
        riid: *const GUID,
        ppvobject: *mut *mut c_void,
    ) -> windows::core::Result<()> {
        if punkouter.is_some() {
            return Err(error(HRESULT(0x80040110u32 as i32)));
        }
        unsafe {
            *ppvobject = null_mut();
        }

        let unknown: IUnknown = match self.kind {
            ClassKind::Thumbnail => ThumbnailProvider::new().into(),
            ClassKind::Preview => PreviewHandler::new().into(),
        };

        unsafe { unknown.query(riid, ppvobject).ok() }
    }

    fn LockServer(&self, flock: BOOL) -> windows::core::Result<()> {
        if flock.as_bool() {
            DLL_REFCOUNT.fetch_add(1, Ordering::SeqCst);
        } else {
            DLL_REFCOUNT.fetch_sub(1, Ordering::SeqCst);
        }
        Ok(())
    }
}

#[implement(IInitializeWithFile, IThumbnailProvider)]
struct ThumbnailProvider {
    file_path: Mutex<Option<PathBuf>>,
}

impl ThumbnailProvider {
    fn new() -> Self {
        DLL_REFCOUNT.fetch_add(1, Ordering::SeqCst);
        Self {
            file_path: Mutex::new(None),
        }
    }
}

impl Drop for ThumbnailProvider {
    fn drop(&mut self) {
        DLL_REFCOUNT.fetch_sub(1, Ordering::SeqCst);
    }
}

impl IInitializeWithFile_Impl for ThumbnailProvider_Impl {
    fn Initialize(&self, pszFilePath: &PCWSTR, _grfMode: u32) -> windows::core::Result<()> {
        let path = pcwstr_to_path(*pszFilePath).ok_or_else(|| error(E_POINTER))?;
        *self.file_path.lock().unwrap() = Some(path);
        Ok(())
    }
}

impl IThumbnailProvider_Impl for ThumbnailProvider_Impl {
    fn GetThumbnail(
        &self,
        cx: u32,
        phbmp: *mut HBITMAP,
        pdwAlpha: *mut WTS_ALPHATYPE,
    ) -> windows::core::Result<()> {
        if phbmp.is_null() {
            return Err(error(E_POINTER));
        }
        unsafe {
            *phbmp = HBITMAP(null_mut());
        }

        let file = self
            .file_path
            .lock()
            .unwrap()
            .clone()
            .ok_or_else(|| error(E_FAIL))?;
        let thumb = render_thumbnail(&file, cx.clamp(16, 2048))?;
        let bitmap = load_png_as_hbitmap(&thumb)?;
        let _ = std::fs::remove_file(thumb);

        unsafe {
            *phbmp = bitmap;
            if !pdwAlpha.is_null() {
                *pdwAlpha = WTSAT_ARGB;
            }
        }
        Ok(())
    }
}

#[implement(
    IInitializeWithFile,
    IObjectWithSite,
    IOleWindow,
    IPreviewHandler,
    IPreviewHandlerVisuals
)]
struct PreviewHandler {
    file_path: Mutex<Option<PathBuf>>,
    site: Mutex<Option<IUnknown>>,
    hwnd_parent: Mutex<HWND>,
    hwnd_host: Mutex<HWND>,
    rect: Mutex<RECT>,
    child: Mutex<Option<Child>>,
}

impl PreviewHandler {
    fn new() -> Self {
        DLL_REFCOUNT.fetch_add(1, Ordering::SeqCst);
        Self {
            file_path: Mutex::new(None),
            site: Mutex::new(None),
            hwnd_parent: Mutex::new(null_hwnd()),
            hwnd_host: Mutex::new(null_hwnd()),
            rect: Mutex::new(RECT::default()),
            child: Mutex::new(None),
        }
    }

    fn ensure_host(&self) -> windows::core::Result<HWND> {
        let hwnd = *self.hwnd_host.lock().unwrap();
        if !hwnd.0.is_null() && unsafe { IsWindow(hwnd).as_bool() } {
            return Ok(hwnd);
        }

        let parent = *self.hwnd_parent.lock().unwrap();
        if parent.0.is_null() {
            return Err(error(E_FAIL));
        }

        register_host_class();
        let rect = *self.rect.lock().unwrap();
        let hwnd_host = unsafe {
            CreateWindowExW(
                WINDOW_EX_STYLE(0),
                w!("PowerUSDShellPreviewHost"),
                w!("PowerUSD Preview"),
                WS_CHILD | WS_VISIBLE | WS_CLIPCHILDREN | WS_CLIPSIBLINGS,
                rect.left,
                rect.top,
                (rect.right - rect.left).max(1),
                (rect.bottom - rect.top).max(1),
                parent,
                null_hmenu(),
                null_hinstance(),
                None,
            )
        }?;

        if hwnd_host.0.is_null() {
            return Err(error(E_FAIL));
        }

        *self.hwnd_host.lock().unwrap() = hwnd_host;
        Ok(hwnd_host)
    }

    fn stop_child(&self) {
        if let Some(mut child) = self.child.lock().unwrap().take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

impl Drop for PreviewHandler {
    fn drop(&mut self) {
        self.stop_child();
        let hwnd = *self.hwnd_host.lock().unwrap();
        if !hwnd.0.is_null() {
            unsafe {
                let _ = DestroyWindow(hwnd);
            }
        }
        DLL_REFCOUNT.fetch_sub(1, Ordering::SeqCst);
    }
}

impl IInitializeWithFile_Impl for PreviewHandler_Impl {
    fn Initialize(&self, pszFilePath: &PCWSTR, _grfMode: u32) -> windows::core::Result<()> {
        let path = pcwstr_to_path(*pszFilePath).ok_or_else(|| error(E_POINTER))?;
        *self.file_path.lock().unwrap() = Some(path);
        Ok(())
    }
}

impl IObjectWithSite_Impl for PreviewHandler_Impl {
    fn SetSite(&self, punkSite: Option<&IUnknown>) -> windows::core::Result<()> {
        *self.site.lock().unwrap() = punkSite.cloned();
        Ok(())
    }

    fn GetSite(&self, riid: *const GUID, ppvSite: *mut *mut c_void) -> windows::core::Result<()> {
        unsafe {
            *ppvSite = null_mut();
        }
        let site = self
            .site
            .lock()
            .unwrap()
            .clone()
            .ok_or_else(|| error(E_FAIL))?;
        unsafe { site.query(riid, ppvSite).ok() }
    }
}

impl IOleWindow_Impl for PreviewHandler_Impl {
    fn GetWindow(&self) -> windows::core::Result<HWND> {
        self.ensure_host()
    }

    fn ContextSensitiveHelp(&self, _fEnterMode: BOOL) -> windows::core::Result<()> {
        Err(error(E_NOTIMPL))
    }
}

impl IPreviewHandler_Impl for PreviewHandler_Impl {
    fn SetWindow(&self, hwnd: HWND, prc: *const RECT) -> windows::core::Result<()> {
        if prc.is_null() {
            return Err(error(E_POINTER));
        }
        *self.hwnd_parent.lock().unwrap() = hwnd;
        *self.rect.lock().unwrap() = unsafe { *prc };
        self.SetRect(prc)
    }

    fn SetRect(&self, prc: *const RECT) -> windows::core::Result<()> {
        if prc.is_null() {
            return Err(error(E_POINTER));
        }
        let rect = unsafe { *prc };
        *self.rect.lock().unwrap() = rect;
        let hwnd = *self.hwnd_host.lock().unwrap();
        if !hwnd.0.is_null() {
            unsafe {
                let _ = SetWindowPos(
                    hwnd,
                    null_hwnd(),
                    rect.left,
                    rect.top,
                    (rect.right - rect.left).max(1),
                    (rect.bottom - rect.top).max(1),
                    SWP_NOZORDER | SWP_NOACTIVATE,
                );
            }
        }
        Ok(())
    }

    fn DoPreview(&self) -> windows::core::Result<()> {
        self.stop_child();
        let file = self
            .file_path
            .lock()
            .unwrap()
            .clone()
            .ok_or_else(|| error(E_FAIL))?;
        let hwnd_host = self.ensure_host()?;
        let child = spawn_powerusd(&[
            OsString::from("--preview-child"),
            OsString::from((hwnd_host.0 as usize).to_string()),
            OsString::from("--file"),
            file.into_os_string(),
        ])?;
        *self.child.lock().unwrap() = Some(child);
        Ok(())
    }

    fn Unload(&self) -> windows::core::Result<()> {
        self.stop_child();
        let hwnd = *self.hwnd_host.lock().unwrap();
        if !hwnd.0.is_null() {
            unsafe {
                let _ = DestroyWindow(hwnd);
            }
            *self.hwnd_host.lock().unwrap() = null_hwnd();
        }
        Ok(())
    }

    fn SetFocus(&self) -> windows::core::Result<()> {
        let hwnd = self.ensure_host()?;
        unsafe {
            let _ = SetFocus(hwnd);
        }
        Ok(())
    }

    fn QueryFocus(&self) -> windows::core::Result<HWND> {
        Ok(unsafe { GetFocus() })
    }

    fn TranslateAccelerator(&self, _pmsg: *const windows::Win32::UI::WindowsAndMessaging::MSG) -> windows::core::Result<()> {
        Ok(())
    }
}

impl IPreviewHandlerVisuals_Impl for PreviewHandler_Impl {
    fn SetBackgroundColor(&self, _color: windows::Win32::Foundation::COLORREF) -> windows::core::Result<()> {
        Ok(())
    }

    fn SetFont(&self, _plf: *const windows::Win32::Graphics::Gdi::LOGFONTW) -> windows::core::Result<()> {
        Ok(())
    }

    fn SetTextColor(&self, _color: windows::Win32::Foundation::COLORREF) -> windows::core::Result<()> {
        Ok(())
    }
}

#[no_mangle]
pub extern "system" fn DllMain(
    hinst: HINSTANCE,
    reason: u32,
    _reserved: *mut c_void,
) -> BOOL {
    const DLL_PROCESS_ATTACH: u32 = 1;
    if reason == DLL_PROCESS_ATTACH {
        MODULE_HANDLE.store(hinst.0, Ordering::SeqCst);
    }
    BOOL(1)
}

#[no_mangle]
pub extern "system" fn DllCanUnloadNow() -> HRESULT {
    if DLL_REFCOUNT.load(Ordering::SeqCst) == 0 {
        S_OK
    } else {
        S_FALSE
    }
}

#[no_mangle]
pub unsafe extern "system" fn DllGetClassObject(
    rclsid: *const GUID,
    riid: *const GUID,
    ppv: *mut *mut c_void,
) -> HRESULT {
    if rclsid.is_null() || riid.is_null() || ppv.is_null() {
        return E_POINTER;
    }
    *ppv = null_mut();

    let kind = if *rclsid == CLSID_POWERUSD_THUMBNAIL {
        ClassKind::Thumbnail
    } else if *rclsid == CLSID_POWERUSD_PREVIEW {
        ClassKind::Preview
    } else {
        return CLASS_E_CLASSNOTAVAILABLE;
    };

    if *riid != IID_ICLASSFACTORY && *riid != IUnknown::IID {
        return windows::Win32::Foundation::E_NOINTERFACE;
    }

    let factory: IClassFactory = ClassFactory { kind }.into();
    factory.query(riid, ppv)
}

#[no_mangle]
pub extern "system" fn DllRegisterServer() -> HRESULT {
    match register_server() {
        Ok(()) => S_OK,
        Err(e) => e.code(),
    }
}

#[no_mangle]
pub extern "system" fn DllUnregisterServer() -> HRESULT {
    match unregister_server() {
        Ok(()) => S_OK,
        Err(e) => e.code(),
    }
}

fn render_thumbnail(file: &Path, size: u32) -> windows::core::Result<PathBuf> {
    let out = std::env::temp_dir().join(format!(
        "powerusd_thumb_{}_{}.png",
        unsafe { GetCurrentProcessId() },
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis()
    ));

    let mut child = spawn_powerusd(&[
        OsString::from("--thumbnail"),
        file.as_os_str().to_os_string(),
        OsString::from("--out"),
        out.as_os_str().to_os_string(),
        OsString::from("--size"),
        OsString::from(size.to_string()),
    ])?;

    let status = child.wait().map_err(|_| error(E_FAIL))?;
    if !status.success() || !out.exists() {
        return Err(error(E_FAIL));
    }

    Ok(out)
}

fn spawn_powerusd(args: &[OsString]) -> windows::core::Result<Child> {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;

    let exe = find_powerusd_exe().ok_or_else(|| error(E_FAIL))?;
    Command::new(exe)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .creation_flags(CREATE_NO_WINDOW)
        .spawn()
        .map_err(|_| error(E_FAIL))
}

fn find_powerusd_exe() -> Option<PathBuf> {
    if let Ok(path) = std::env::var("POWERUSD_EXE") {
        let path = PathBuf::from(path);
        if path.exists() {
            return Some(path);
        }
    }

    if let Some(path) = read_registry_string(
        HKEY_CURRENT_USER,
        "Software\\PowerUSD\\ShellExtension",
        "PowerUsdExe",
    ) {
        let path = PathBuf::from(path);
        if path.exists() {
            return Some(path);
        }
    }

    let dll_dir = module_path().and_then(|p| p.parent().map(Path::to_path_buf))?;
    let sibling = dll_dir.join("powerusd.exe");
    if sibling.exists() {
        return Some(sibling);
    }

    let dev_tree = dll_dir
        .parent()
        .and_then(Path::parent)
        .and_then(Path::parent)
        .map(|p| p.join("target\\release\\powerusd.exe"));
    if let Some(path) = dev_tree {
        if path.exists() {
            return Some(path);
        }
    }

    Some(PathBuf::from("powerusd.exe"))
}

fn load_png_as_hbitmap(path: &Path) -> windows::core::Result<HBITMAP> {
    let img = image::open(path)
        .map_err(|_| error(E_FAIL))?
        .to_rgba8();
    let width = img.width() as i32;
    let height = img.height() as i32;

    let bmi = BITMAPINFO {
        bmiHeader: BITMAPINFOHEADER {
            biSize: size_of::<BITMAPINFOHEADER>() as u32,
            biWidth: width,
            biHeight: -height,
            biPlanes: 1,
            biBitCount: 32,
            biCompression: BI_RGB.0,
            ..unsafe { zeroed() }
        },
        ..unsafe { zeroed() }
    };

    let mut bits: *mut c_void = null_mut();
    let hbitmap = unsafe {
        CreateDIBSection(
            None,
            &bmi,
            DIB_RGB_COLORS,
            &mut bits,
            null_handle(),
            0,
        )
    }?;
    if hbitmap.0.is_null() || bits.is_null() {
        return Err(error(E_FAIL));
    }

    let dst =
        unsafe { std::slice::from_raw_parts_mut(bits as *mut u8, (width * height * 4) as usize) };
    for (src, dst) in img.as_raw().chunks_exact(4).zip(dst.chunks_exact_mut(4)) {
        dst[0] = src[2];
        dst[1] = src[1];
        dst[2] = src[0];
        dst[3] = src[3];
    }

    Ok(hbitmap)
}

fn register_server() -> windows::core::Result<()> {
    let module = module_path().ok_or_else(|| error(E_FAIL))?;
    let module = module.to_string_lossy().to_string();
    let powerusd = find_powerusd_exe()
        .unwrap_or_else(|| PathBuf::from("powerusd.exe"))
        .to_string_lossy()
        .to_string();

    write_registry_string(
        HKEY_CURRENT_USER,
        "Software\\PowerUSD\\ShellExtension",
        "PowerUsdExe",
        &powerusd,
    )?;

    register_clsid(CLSID_POWERUSD_THUMBNAIL, "PowerUSD Thumbnail Provider", &module)?;
    register_clsid(CLSID_POWERUSD_PREVIEW, "PowerUSD Preview Handler", &module)?;

    let thumbnail_iid = "{e357fccd-a995-4576-b01f-234630154e96}";
    let preview_iid = "{8895b1c6-b41f-4c1c-a562-0d564250836f}";
    for ext in EXTENSIONS {
        write_registry_string(
            HKEY_CURRENT_USER,
            &format!("Software\\Classes\\{}\\shellex\\{}", ext, thumbnail_iid),
            "",
            CLSID_POWERUSD_THUMBNAIL_STR,
        )?;
        write_registry_string(
            HKEY_CURRENT_USER,
            &format!("Software\\Classes\\{}\\shellex\\{}", ext, preview_iid),
            "",
            CLSID_POWERUSD_PREVIEW_STR,
        )?;
    }

    write_registry_string(
        HKEY_CURRENT_USER,
        "Software\\Microsoft\\Windows\\CurrentVersion\\PreviewHandlers",
        CLSID_POWERUSD_PREVIEW_STR,
        "PowerUSD Preview Handler",
    )?;
    write_registry_string(
        HKEY_CURRENT_USER,
        "Software\\Microsoft\\Windows\\CurrentVersion\\Shell Extensions\\Approved",
        CLSID_POWERUSD_THUMBNAIL_STR,
        "PowerUSD Thumbnail Provider",
    )?;
    write_registry_string(
        HKEY_CURRENT_USER,
        "Software\\Microsoft\\Windows\\CurrentVersion\\Shell Extensions\\Approved",
        CLSID_POWERUSD_PREVIEW_STR,
        "PowerUSD Preview Handler",
    )?;

    Ok(())
}

fn unregister_server() -> windows::core::Result<()> {
    let _ = delete_registry_tree(
        HKEY_CURRENT_USER,
        &format!("Software\\Classes\\CLSID\\{}", CLSID_POWERUSD_THUMBNAIL_STR),
    );
    let _ = delete_registry_tree(
        HKEY_CURRENT_USER,
        &format!("Software\\Classes\\CLSID\\{}", CLSID_POWERUSD_PREVIEW_STR),
    );

    let thumbnail_iid = "{e357fccd-a995-4576-b01f-234630154e96}";
    let preview_iid = "{8895b1c6-b41f-4c1c-a562-0d564250836f}";
    for ext in EXTENSIONS {
        let _ = delete_registry_tree(
            HKEY_CURRENT_USER,
            &format!("Software\\Classes\\{}\\shellex\\{}", ext, thumbnail_iid),
        );
        let _ = delete_registry_tree(
            HKEY_CURRENT_USER,
            &format!("Software\\Classes\\{}\\shellex\\{}", ext, preview_iid),
        );
    }
    Ok(())
}

fn register_clsid(clsid: GUID, name: &str, module: &str) -> windows::core::Result<()> {
    let clsid_string = if clsid == CLSID_POWERUSD_THUMBNAIL {
        CLSID_POWERUSD_THUMBNAIL_STR
    } else {
        CLSID_POWERUSD_PREVIEW_STR
    };
    let clsid_key = format!("Software\\Classes\\CLSID\\{}", clsid_string);
    write_registry_string(HKEY_CURRENT_USER, &clsid_key, "", name)?;
    write_registry_string(
        HKEY_CURRENT_USER,
        &format!("{}\\InProcServer32", clsid_key),
        "",
        module,
    )?;
    write_registry_string(
        HKEY_CURRENT_USER,
        &format!("{}\\InProcServer32", clsid_key),
        "ThreadingModel",
        "Apartment",
    )?;

    write_registry_dword(HKEY_CURRENT_USER, &clsid_key, "DisableProcessIsolation", 1)?;
    write_registry_dword(HKEY_CURRENT_USER, &clsid_key, "DisableLowILProcessIsolation", 1)?;

    Ok(())
}

fn write_registry_string(
    root: HKEY,
    subkey: &str,
    name: &str,
    value: &str,
) -> windows::core::Result<()> {
    let subkey = wide(subkey);
    let name = wide(name);
    let value = wide(value);
    let mut key = null_hkey();
    unsafe {
        RegCreateKeyExW(
            root,
            PCWSTR(subkey.as_ptr()),
            0,
            PWSTR(null_mut()),
            REG_OPTION_NON_VOLATILE,
            KEY_WRITE,
            None,
            &mut key,
            None,
        )
        .ok()?;
        let bytes = std::slice::from_raw_parts(
            value.as_ptr() as *const u8,
            value.len() * size_of::<u16>(),
        );
        RegSetValueExW(key, PCWSTR(name.as_ptr()), 0, REG_SZ, Some(bytes)).ok()?;
        RegCloseKey(key).ok()?;
    }
    Ok(())
}

fn write_registry_dword(
    root: HKEY,
    subkey: &str,
    name: &str,
    value: u32,
) -> windows::core::Result<()> {
    let subkey = wide(subkey);
    let name = wide(name);
    let mut key = null_hkey();
    unsafe {
        RegCreateKeyExW(
            root,
            PCWSTR(subkey.as_ptr()),
            0,
            PWSTR(null_mut()),
            REG_OPTION_NON_VOLATILE,
            KEY_WRITE,
            None,
            &mut key,
            None,
        )
        .ok()?;
        RegSetValueExW(
            key,
            PCWSTR(name.as_ptr()),
            0,
            REG_DWORD,
            Some(&value.to_le_bytes()),
        )
        .ok()?;
        RegCloseKey(key).ok()?;
    }
    Ok(())
}

fn read_registry_string(root: HKEY, subkey: &str, name: &str) -> Option<String> {
    let subkey = wide(subkey);
    let name = wide(name);
    let mut key = null_hkey();
    unsafe {
        if RegOpenKeyExW(root, PCWSTR(subkey.as_ptr()), 0, KEY_READ, &mut key)
            .ok()
            .is_err()
        {
            return None;
        }
        let mut buf = vec![0u16; MAX_PATH as usize * 4];
        let mut bytes = (buf.len() * size_of::<u16>()) as u32;
        let status = windows::Win32::System::Registry::RegQueryValueExW(
            key,
            PCWSTR(name.as_ptr()),
            None,
            None,
            Some(buf.as_mut_ptr() as *mut u8),
            Some(&mut bytes),
        );
        let _ = RegCloseKey(key);
        status.ok().ok()?;
        let len = (bytes as usize / size_of::<u16>()).saturating_sub(1);
        Some(OsString::from_wide(&buf[..len]).to_string_lossy().to_string())
    }
}

fn delete_registry_tree(root: HKEY, subkey: &str) -> windows::core::Result<()> {
    let subkey = wide(subkey);
    unsafe { RegDeleteTreeW(root, PCWSTR(subkey.as_ptr())).ok() }
}

fn module_path() -> Option<PathBuf> {
    let mut buf = vec![0u16; MAX_PATH as usize * 4];
    let module = MODULE_HANDLE.load(Ordering::SeqCst);
    let hmodule = if module.is_null() {
        null_hmodule()
    } else {
        HMODULE(module)
    };
    let len = unsafe { GetModuleFileNameW(hmodule, &mut buf) } as usize;
    if len == 0 {
        return None;
    }
    Some(PathBuf::from(OsString::from_wide(&buf[..len])))
}

fn pcwstr_to_path(value: PCWSTR) -> Option<PathBuf> {
    if value.is_null() {
        return None;
    }
    let s = unsafe { value.to_string().ok()? };
    Some(PathBuf::from(s))
}

fn wide(value: &str) -> Vec<u16> {
    OsStr::new(value).encode_wide().chain(std::iter::once(0)).collect()
}

fn register_host_class() {
    unsafe extern "system" fn wnd_proc(
        hwnd: HWND,
        msg: u32,
        wparam: WPARAM,
        lparam: LPARAM,
    ) -> LRESULT {
        match msg {
            WM_CREATE => {
                let createstruct = lparam.0 as *const CREATESTRUCTW;
                if !createstruct.is_null() {
                    SetWindowLongPtrW(hwnd, GWLP_USERDATA, (*createstruct).lpCreateParams as isize);
                }
                LRESULT(0)
            }
            WM_DESTROY => {
                SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0);
                LRESULT(0)
            }
            _ => DefWindowProcW(hwnd, msg, wparam, lparam),
        }
    }

    static REGISTERED: AtomicU32 = AtomicU32::new(0);
    if REGISTERED.swap(1, Ordering::SeqCst) != 0 {
        return;
    }

    let class = WNDCLASSW {
        style: CS_HREDRAW | CS_VREDRAW,
        lpfnWndProc: Some(wnd_proc),
        hInstance: null_hinstance(),
        lpszClassName: w!("PowerUSDShellPreviewHost"),
        ..unsafe { zeroed() }
    };
    unsafe {
        let _ = RegisterClassW(&class);
    }
}
