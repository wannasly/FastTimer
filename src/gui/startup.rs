use fasttimer::{Result, timer::Case};
use std::{
    ffi::{OsStr, OsString},
    io,
    os::windows::ffi::{OsStrExt, OsStringExt},
    path::{Path, PathBuf},
};
use windows::{
    Win32::{
        System::Com::{
            CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED, CoCreateInstance, CoInitializeEx,
            CoTaskMemFree, CoUninitialize, IPersistFile,
        },
        UI::Shell::{
            FOLDERID_Startup, IShellLinkW, KF_FLAG_DEFAULT, SHGetKnownFolderPath, ShellLink,
        },
    },
    core::{Interface, PCWSTR},
};

struct Apartment;

impl Apartment {
    fn initialize() -> windows::core::Result<Self> {
        unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED).ok()? };
        Ok(Self)
    }
}

impl Drop for Apartment {
    fn drop(&mut self) {
        unsafe { CoUninitialize() };
    }
}

pub(super) fn wide(s: impl AsRef<OsStr>) -> Vec<u16> {
    s.as_ref().encode_wide().chain([0]).collect()
}

pub(super) fn shortcut_path() -> Result<PathBuf> {
    // The shell owns the allocation until CoTaskMemFree; copy it before freeing.
    let path = unsafe { SHGetKnownFolderPath(&FOLDERID_Startup, KF_FLAG_DEFAULT, None)? };
    let folder = unsafe { OsString::from_wide(path.as_wide()) };
    unsafe { CoTaskMemFree(Some(path.0.cast())) };
    Ok(PathBuf::from(folder).join("FastTimer.lnk"))
}

pub(super) fn enable(shortcut: &Path, case: Case) -> Result<()> {
    let target = std::env::current_exe()?;
    let target = wide(target);
    let args = wide(format!("--hidden --request {case}"));
    let description = wide("FastTimer timer resolution utility");
    let path = wide(shortcut);
    // This thread owns the apartment and all COM objects through Save.
    unsafe {
        let _apartment = Apartment::initialize()?;
        let link: IShellLinkW = CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER)?;
        link.SetPath(PCWSTR(target.as_ptr()))?;
        link.SetArguments(PCWSTR(args.as_ptr()))?;
        link.SetDescription(PCWSTR(description.as_ptr()))?;
        link.cast::<IPersistFile>()?
            .Save(PCWSTR(path.as_ptr()), true)?;
    }
    Ok(())
}

pub(super) fn disable(shortcut: &Path) -> io::Result<()> {
    match std::fs::remove_file(shortcut) {
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        result => result,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};
    use windows::Win32::System::Com::STGM_READ;

    #[test]
    fn startup_shortcut_stores_the_hidden_preset() -> Result<()> {
        let stamp = SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos();
        let path =
            std::env::temp_dir().join(format!("fasttimer-test-{}-{stamp}.lnk", std::process::id()));
        struct Fixture(PathBuf);
        impl Drop for Fixture {
            fn drop(&mut self) {
                let _ = disable(&self.0);
            }
        }
        let fixture = Fixture(path);
        enable(&fixture.0, Case::Native("0.5100".parse()?))?;
        let path = wide(&fixture.0);
        let mut arguments = [0u16; 256];
        {
            let _apartment = Apartment::initialize()?;
            unsafe {
                let link: IShellLinkW = CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER)?;
                link.cast::<IPersistFile>()?
                    .Load(PCWSTR(path.as_ptr()), STGM_READ)?;
                link.GetArguments(&mut arguments)?;
            }
        }
        let length = arguments
            .iter()
            .position(|&unit| unit == 0)
            .unwrap_or(arguments.len());
        assert_eq!(
            String::from_utf16_lossy(&arguments[..length]),
            "--hidden --request 0.5100"
        );
        disable(&fixture.0)?;
        assert!(!fixture.0.try_exists()?);
        Ok(())
    }
}
