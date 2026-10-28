/// Whether this process can call `GB_WMIACPI_Set`.
///
/// The executable manifest asks for `requireAdministrator`. `cargo run` from
/// a non-elevated shell does not always embed that manifest, so the window
/// also checks the token and stays read-only when the check fails.
#[cfg(windows)]
pub fn is_elevated() -> bool {
    unsafe { windows::Win32::UI::Shell::IsUserAnAdmin().as_bool() }
}

#[cfg(not(windows))]
pub fn is_elevated() -> bool {
    false
}
