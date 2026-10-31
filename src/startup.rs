use std::path::PathBuf;

use windows::core::PCWSTR;
use windows::Win32::Foundation::ERROR_FILE_NOT_FOUND;
use windows::Win32::System::Registry::{
    RegCloseKey, RegCreateKeyExW, RegDeleteValueW, RegOpenKeyExW, RegQueryValueExW, RegSetValueExW,
    HKEY, HKEY_CURRENT_USER, KEY_QUERY_VALUE, KEY_SET_VALUE, REG_OPTION_NON_VOLATILE, REG_SZ,
};

const RUN_KEY: &str = "Software\\Microsoft\\Windows\\CurrentVersion\\Run";
const VALUE_NAME: &str = "real-rs";

fn wide(value: &str) -> Vec<u16> {
    value.encode_utf16().chain(std::iter::once(0)).collect()
}

fn current_executable() -> Result<PathBuf, String> {
    let path = std::env::current_exe()
        .map_err(|error| format!("could not locate the executable: {error}"))?;

    Ok(
        match path.to_str().and_then(|text| text.strip_prefix(r"\\?\")) {
            Some(stripped) => PathBuf::from(stripped),
            None => path,
        },
    )
}

pub fn command() -> Result<String, String> {
    Ok(format!(
        "\"{}\" --tray",
        release_path(current_executable()?).display()
    ))
}

fn release_path(path: PathBuf) -> PathBuf {
    let Some(parent) = path.parent() else {
        return path;
    };

    let is_deps = parent
        .file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name.eq_ignore_ascii_case("deps"));

    if !is_deps {
        return path;
    }

    let (Some(root), Some(name)) = (parent.parent(), path.file_name()) else {
        return path;
    };

    let candidate = root.join(name);

    if candidate.is_file() {
        candidate
    } else {
        path
    }
}

fn stored_target(stored: &str) -> Option<PathBuf> {
    let rest = stored.strip_prefix('"')?;
    let end = rest.find('"')?;
    Some(PathBuf::from(&rest[..end]))
}

pub fn is_enabled() -> bool {
    let Some(stored) = read_value(VALUE_NAME) else {
        return false;
    };

    stored_target(&stored).is_some_and(|path| path.is_file())
        && command().is_ok_and(|expected| expected == stored)
}

pub fn set_enabled(enabled: bool) -> Result<(), String> {
    if enabled {
        install(VALUE_NAME)
    } else {
        uninstall(VALUE_NAME)
    }
}

fn install(value: &str) -> Result<(), String> {
    let command = command()?;
    let key_name = wide(RUN_KEY);
    let value_name = wide(value);
    let data: Vec<u16> = command.encode_utf16().chain(std::iter::once(0)).collect();
    let bytes = unsafe {
        std::slice::from_raw_parts(
            data.as_ptr().cast::<u8>(),
            data.len() * std::mem::size_of::<u16>(),
        )
    };

    unsafe {
        let mut key = HKEY::default();
        let status = RegCreateKeyExW(
            HKEY_CURRENT_USER,
            PCWSTR(key_name.as_ptr()),
            None,
            PCWSTR::null(),
            REG_OPTION_NON_VOLATILE,
            KEY_SET_VALUE | KEY_QUERY_VALUE,
            None,
            &mut key,
            None,
        );

        if status.0 != 0 {
            return Err(format!(
                "could not open the startup registry key (error {})",
                status.0
            ));
        }

        let status = RegSetValueExW(key, PCWSTR(value_name.as_ptr()), None, REG_SZ, Some(bytes));
        let _ = RegCloseKey(key);

        if status.0 != 0 {
            return Err(format!(
                "could not write the startup registry value (error {})",
                status.0
            ));
        }
    }

    Ok(())
}

fn uninstall(value: &str) -> Result<(), String> {
    let key_name = wide(RUN_KEY);
    let value_name = wide(value);

    unsafe {
        let mut key = HKEY::default();
        let status = RegOpenKeyExW(
            HKEY_CURRENT_USER,
            PCWSTR(key_name.as_ptr()),
            None,
            KEY_SET_VALUE,
            &mut key,
        );

        if status.0 != 0 {
            return if status.0 == ERROR_FILE_NOT_FOUND.0 {
                Ok(())
            } else {
                Err(format!(
                    "could not open the startup registry key (error {})",
                    status.0
                ))
            };
        }

        let status = RegDeleteValueW(key, PCWSTR(value_name.as_ptr()));
        let _ = RegCloseKey(key);

        if status.0 != 0 && status.0 != ERROR_FILE_NOT_FOUND.0 {
            return Err(format!(
                "could not remove the startup registry value (error {})",
                status.0
            ));
        }
    }

    Ok(())
}

fn read_value(value: &str) -> Option<String> {
    let key_name = wide(RUN_KEY);
    let value_name = wide(value);

    unsafe {
        let mut key = HKEY::default();
        let status = RegOpenKeyExW(
            HKEY_CURRENT_USER,
            PCWSTR(key_name.as_ptr()),
            None,
            KEY_QUERY_VALUE,
            &mut key,
        );

        if status.0 != 0 {
            return None;
        }

        let mut size = 0u32;
        let status = RegQueryValueExW(
            key,
            PCWSTR(value_name.as_ptr()),
            None,
            None,
            None,
            Some(&mut size),
        );

        if status.0 != 0 || size == 0 {
            let _ = RegCloseKey(key);
            return None;
        }

        let mut buffer = vec![0u8; size as usize];
        let status = RegQueryValueExW(
            key,
            PCWSTR(value_name.as_ptr()),
            None,
            None,
            Some(buffer.as_mut_ptr()),
            Some(&mut size),
        );
        let _ = RegCloseKey(key);

        if status.0 != 0 {
            return None;
        }

        let units: Vec<u16> = buffer
            .chunks_exact(2)
            .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
            .take_while(|unit| *unit != 0)
            .collect();

        String::from_utf16(&units).ok()
    }
}
