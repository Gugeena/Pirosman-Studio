//! Start-up checks that must run before Tauri creates a window.
//!
//! The UI is drawn by the Microsoft Edge WebView2 Runtime. It ships with
//! Windows 11 and current Windows 10, but stripped or offline machines may lack
//! it, in which case window creation fails and the app would just vanish. We
//! detect the runtime the way Microsoft documents (EdgeUpdate client `pv`
//! registry values) and offer the official installer instead.

/// Evergreen Bootstrapper download (Microsoft's official short link).
#[cfg(windows)]
pub const WEBVIEW2_DOWNLOAD: &str = "https://go.microsoft.com/fwlink/p/?LinkId=2124703";

#[cfg(windows)]
mod win {
    use windows_sys::Win32::Foundation::ERROR_SUCCESS;
    use windows_sys::Win32::System::Registry::{RegGetValueW, HKEY, HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, RRF_RT_REG_SZ};
    use windows_sys::Win32::UI::Shell::ShellExecuteW;
    use windows_sys::Win32::UI::WindowsAndMessaging::{MessageBoxW, IDYES, MB_ICONERROR, MB_ICONWARNING, MB_OK, MB_SETFOREGROUND, MB_YESNO, SW_SHOWNORMAL};

    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(std::iter::once(0)).collect()
    }

    const CLIENT: &str = r"Microsoft\EdgeUpdate\Clients\{F3017226-FE2A-4295-8BDF-00C3A9A7E4C5}";

    fn read_pv(root: HKEY, key: &str) -> Option<String> {
        let key = wide(key);
        let name = wide("pv");
        let mut buf = [0u16; 64];
        let mut len = std::mem::size_of_val(&buf) as u32;
        // SAFETY: valid null-terminated strings and an output buffer of `len` bytes.
        let rc = unsafe { RegGetValueW(root, key.as_ptr(), name.as_ptr(), RRF_RT_REG_SZ, std::ptr::null_mut(), buf.as_mut_ptr().cast(), &mut len) };
        if rc != ERROR_SUCCESS {
            return None;
        }
        let n = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
        Some(String::from_utf16_lossy(&buf[..n]))
    }

    /// Installed WebView2 Runtime version, if any (per-machine or per-user).
    pub fn webview2_version() -> Option<String> {
        [
            (HKEY_LOCAL_MACHINE, format!(r"SOFTWARE\WOW6432Node\{CLIENT}")),
            (HKEY_LOCAL_MACHINE, format!(r"SOFTWARE\{CLIENT}")),
            (HKEY_CURRENT_USER, format!(r"Software\{CLIENT}")),
        ]
        .into_iter()
        .filter_map(|(root, key)| read_pv(root, &key))
        .find(|v| !v.is_empty() && v != "0.0.0.0")
    }

    pub fn ask(title: &str, text: &str) -> bool {
        let (t, x) = (wide(title), wide(text));
        // SAFETY: plain modal message box with null-terminated strings.
        unsafe { MessageBoxW(std::ptr::null_mut(), x.as_ptr(), t.as_ptr(), MB_YESNO | MB_ICONWARNING | MB_SETFOREGROUND) == IDYES }
    }

    pub fn error(title: &str, text: &str) {
        let (t, x) = (wide(title), wide(text));
        // SAFETY: as above.
        unsafe { MessageBoxW(std::ptr::null_mut(), x.as_ptr(), t.as_ptr(), MB_OK | MB_ICONERROR | MB_SETFOREGROUND) };
    }

    pub fn open_url(url: &str) {
        let (op, u) = (wide("open"), wide(url));
        // SAFETY: ShellExecuteW with null-terminated verb and target.
        unsafe { ShellExecuteW(std::ptr::null_mut(), op.as_ptr(), u.as_ptr(), std::ptr::null(), std::ptr::null(), SW_SHOWNORMAL) };
    }
}

/// Returns false when the app must not continue (WebView2 missing).
pub fn check() -> bool {
    #[cfg(windows)]
    {
        // A fixed-version runtime supplied by the user also works.
        if std::env::var_os("WEBVIEW2_BROWSER_EXECUTABLE_FOLDER").is_some() || std::env::var_os("PIROSMAN_SKIP_WEBVIEW2_CHECK").is_some() {
            return true;
        }
        // PIROSMAN_TEST_NO_WEBVIEW2 forces the "missing" path for QA.
        if win::webview2_version().is_none() || std::env::var_os("PIROSMAN_TEST_NO_WEBVIEW2").is_some() {
            let yes = win::ask(
                "Pirosman Studio needs Microsoft Edge WebView2",
                "Pirosman Studio draws its interface with the Microsoft Edge WebView2 Runtime, which is not installed on this PC.\n\n\
                 Open Microsoft's download now? Run the downloaded \"MicrosoftEdgeWebview2Setup.exe\" (about 2 MB, free), \
                 then start Pirosman Studio again.\n\n\
                 Offline PC? Get the \"Evergreen Standalone Installer\" from\nhttps://developer.microsoft.com/microsoft-edge/webview2/",
            );
            if yes {
                win::open_url(WEBVIEW2_DOWNLOAD);
            }
            return false;
        }
    }
    true
}

/// Release builds have no console, so a panic would otherwise close the app
/// without a word. Show it in a message box first.
pub fn install_panic_hook() {
    let default = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let text = format!("Pirosman Studio hit an internal error and may need to close.\n\n{info}\n\nSave your work if the window is still open.");
        #[cfg(windows)]
        win::error("Pirosman Studio", &text);
        #[cfg(not(windows))]
        eprintln!("{text}");
        default(info);
    }));
}

/// Shown when Tauri fails to start, instead of exiting silently.
pub fn fatal(err: &dyn std::fmt::Display) {
    let text = format!(
        "Pirosman Studio could not start.\n\n{err}\n\n\
         If this mentions WebView2, install or repair the Microsoft Edge WebView2 Runtime \
         (https://developer.microsoft.com/microsoft-edge/webview2/) and try again. \
         If another Pirosman Studio window is already open, close it first."
    );
    #[cfg(windows)]
    win::error("Pirosman Studio", &text);
    #[cfg(not(windows))]
    eprintln!("{text}");
}
