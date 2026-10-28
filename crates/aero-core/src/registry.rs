//! Read-only access to `HKLM\SOFTWARE\Gigabyte\SmartManager\CustomFanTable`.
//!
//! This module does not import or call any registry write API.
#![allow(unsafe_code)]

use windows::{
    core::w,
    Win32::Foundation::{ERROR_MORE_DATA, ERROR_SUCCESS},
    Win32::System::Registry::{
        RegCloseKey, RegOpenKeyExW, RegQueryValueExW, HKEY, HKEY_LOCAL_MACHINE, KEY_READ,
        KEY_WOW64_64KEY,
    },
};

pub fn read_custom_fan_table() -> Result<Vec<u8>, String> {
    unsafe {
        read_value(w!("SOFTWARE\\Gigabyte\\SmartManager"), w!("CustomFanTable")).or_else(|_| {
            read_value(
                w!("SOFTWARE\\WOW6432Node\\Gigabyte\\SmartManager"),
                w!("CustomFanTable"),
            )
        })
    }
}

unsafe fn read_value(
    subkey: windows::core::PCWSTR,
    value: windows::core::PCWSTR,
) -> Result<Vec<u8>, String> {
    let mut key = HKEY::default();
    let opened = RegOpenKeyExW(
        HKEY_LOCAL_MACHINE,
        subkey,
        0,
        KEY_READ | KEY_WOW64_64KEY,
        &mut key,
    );
    if opened != ERROR_SUCCESS {
        return Err(format!("RegOpenKeyExW failed ({opened:?})"));
    }
    let result = query(key, value);
    let _ = RegCloseKey(key);
    result
}

unsafe fn query(key: HKEY, value: windows::core::PCWSTR) -> Result<Vec<u8>, String> {
    let mut size = 0u32;
    let status = RegQueryValueExW(key, value, None, None, None, Some(&mut size));
    if status != ERROR_SUCCESS && status != ERROR_MORE_DATA {
        return Err(format!("RegQueryValueExW size query failed ({status:?})"));
    }
    let mut buffer = vec![0u8; size as usize];
    let status = RegQueryValueExW(
        key,
        value,
        None,
        None,
        Some(buffer.as_mut_ptr()),
        Some(&mut size),
    );
    if status != ERROR_SUCCESS {
        return Err(format!("RegQueryValueExW failed ({status:?})"));
    }
    buffer.truncate(size as usize);
    Ok(buffer)
}
