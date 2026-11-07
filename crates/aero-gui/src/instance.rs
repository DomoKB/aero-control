//! One controller per Windows session. A second interactive launch opens the
//! existing controller; a repeated background launch leaves it undisturbed.

#[cfg(windows)]
pub use native::Instance;

#[cfg(not(windows))]
pub struct Instance;

#[cfg(not(windows))]
impl Instance {
    pub fn acquire(_background: bool, _mock: bool) -> Result<Option<Self>, String> {
        Ok(Some(Self))
    }

    pub fn take_show_request(&self) -> bool {
        false
    }
}

#[cfg(windows)]
mod native {
    use windows::core::PCWSTR;
    use windows::Win32::Foundation::{
        CloseHandle, HANDLE, WAIT_ABANDONED, WAIT_OBJECT_0, WAIT_TIMEOUT,
    };
    use windows::Win32::System::Threading::{
        CreateEventW, CreateMutexW, ReleaseMutex, SetEvent, WaitForSingleObject,
    };

    struct Handle(HANDLE);

    impl Drop for Handle {
        fn drop(&mut self) {
            let _ = unsafe { CloseHandle(self.0) };
        }
    }

    pub struct Instance {
        mutex: Handle,
        show: Handle,
    }

    impl Instance {
        pub fn acquire(background: bool, mock: bool) -> Result<Option<Self>, String> {
            // Local names are scoped to the interactive Windows session. Mock
            // sessions never take ownership of the real hardware controller.
            let name = if mock {
                "Local\\AeroControl-Mock-v1"
            } else {
                "Local\\AeroControl-v1"
            };
            Self::acquire_named(name, background)
        }

        fn acquire_named(name: &str, background: bool) -> Result<Option<Self>, String> {
            let mutex_name: Vec<_> = name.encode_utf16().chain(Some(0)).collect();
            let event_name: Vec<_> = format!("{name}-Show")
                .encode_utf16()
                .chain(Some(0))
                .collect();
            let mutex = Handle(
                unsafe { CreateMutexW(None, false, PCWSTR(mutex_name.as_ptr())) }.map_err(
                    |error| format!("Could not protect against a second controller: {error}"),
                )?,
            );
            let show = Handle(
                unsafe { CreateEventW(None, false, false, PCWSTR(event_name.as_ptr())) }
                    .map_err(|error| format!("Could not prepare window activation: {error}"))?,
            );
            match unsafe { WaitForSingleObject(mutex.0, 0) } {
                WAIT_OBJECT_0 | WAIT_ABANDONED => Ok(Some(Self { mutex, show })),
                WAIT_TIMEOUT => {
                    if !background {
                        unsafe { SetEvent(show.0) }.map_err(|error| {
                            format!("Could not open the running controller: {error}")
                        })?;
                    }
                    Ok(None)
                }
                _ => Err(format!(
                    "Could not check the running controller: {}",
                    windows::core::Error::from_win32()
                )),
            }
        }

        pub fn take_show_request(&self) -> bool {
            unsafe { WaitForSingleObject(self.show.0, 0) == WAIT_OBJECT_0 }
        }
    }

    impl Drop for Instance {
        fn drop(&mut self) {
            // Instance remains on the main thread, which acquired the mutex.
            let _ = unsafe { ReleaseMutex(self.mutex.0) };
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn repeated_launches_share_one_controller_and_only_manual_launch_opens_it() {
            let name = format!("Local\\AeroControl-Test-{}", std::process::id());
            let first = Instance::acquire_named(&name, true).unwrap().unwrap();
            for background in [true, false] {
                let name = name.clone();
                std::thread::spawn(move || {
                    assert!(Instance::acquire_named(&name, background)
                        .unwrap()
                        .is_none());
                })
                .join()
                .unwrap();
                assert_eq!(first.take_show_request(), !background);
                assert!(!first.take_show_request());
            }
            drop(first);
            assert!(Instance::acquire_named(&name, true).unwrap().is_some());
        }
    }
}
