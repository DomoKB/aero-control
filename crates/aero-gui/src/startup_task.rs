//! Explicit per-user logon startup. No task is created just by constructing this type.
//!
//! Task Scheduler's COM API avoids shell parsing, passwords and temporary scripts.
//! Existing tasks are changed only after their source, principal, trigger and sole
//! executable action have been checked against this app and current user.

use std::path::{Path, PathBuf};

const TASK_SOURCE: &str = "aero-control/startup/v1";
const BACKGROUND_ARGUMENT: &str = "--background";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StartupStatus {
    pub enabled: bool,
    pub background: bool,
}

pub enum StartupTask {
    Mock {
        enabled: bool,
    },
    #[cfg(windows)]
    Real,
}

impl StartupTask {
    pub fn new(mock: bool) -> Self {
        #[cfg(windows)]
        if !mock {
            return Self::Real;
        }
        let _ = mock;
        Self::Mock { enabled: false }
    }

    pub fn status(&self) -> Result<StartupStatus, String> {
        match self {
            Self::Mock { enabled } => Ok(StartupStatus {
                enabled: *enabled,
                background: true,
            }),
            #[cfg(windows)]
            Self::Real => real::status(),
        }
    }

    /// Must be invoked only after an explicit user action; real calls can require elevation.
    pub fn set_enabled(&mut self, enabled: bool) -> Result<(), String> {
        match self {
            Self::Mock { enabled: value } => {
                *value = enabled;
                Ok(())
            }
            #[cfg(windows)]
            Self::Real => real::set_enabled(enabled),
        }
    }
}

fn task_name(sid: &str) -> Result<String, String> {
    if sid.len() > 184
        || !sid.starts_with("S-1-")
        || !sid[4..]
            .split('-')
            .all(|part| !part.is_empty() && part.bytes().all(|byte| byte.is_ascii_digit()))
    {
        return Err("Windows returned an invalid user SID.".into());
    }
    Ok(format!("AeroControl-{sid}"))
}

fn xml_escape(text: &str) -> Result<String, String> {
    if text.chars().any(|ch| {
        ((ch as u32) < 0x20 && !matches!(ch, '\t' | '\r' | '\n'))
            || matches!(ch, '\u{fffe}' | '\u{ffff}')
    }) {
        return Err(
            "The startup path contains a character Task Scheduler XML cannot represent.".into(),
        );
    }
    Ok(text
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;"))
}

fn task_xml(executable: &Path, sid: &str) -> Result<String, String> {
    let _ = task_name(sid)?;
    let command = executable
        .to_str()
        .ok_or("The startup executable path is not valid Unicode.")?;
    let parent = executable
        .parent()
        .and_then(Path::to_str)
        .ok_or("The executable has no valid working directory.")?;
    let command = xml_escape(command)?;
    let parent = xml_escape(parent)?;
    let sid = xml_escape(sid)?;
    Ok(format!(
        r#"<?xml version="1.0"?>
<Task version="1.2" xmlns="http://schemas.microsoft.com/windows/2004/02/mit/task">
  <RegistrationInfo><Description>Restore aero-control settings in the background when this user signs in.</Description><Source>{TASK_SOURCE}</Source></RegistrationInfo>
  <Triggers><LogonTrigger><Enabled>true</Enabled><UserId>{sid}</UserId></LogonTrigger></Triggers>
  <Principals><Principal id="CurrentUser"><UserId>{sid}</UserId><LogonType>InteractiveToken</LogonType><RunLevel>HighestAvailable</RunLevel></Principal></Principals>
  <Settings><MultipleInstancesPolicy>IgnoreNew</MultipleInstancesPolicy><DisallowStartIfOnBatteries>false</DisallowStartIfOnBatteries><StopIfGoingOnBatteries>false</StopIfGoingOnBatteries><AllowHardTerminate>true</AllowHardTerminate><StartWhenAvailable>true</StartWhenAvailable><RunOnlyIfNetworkAvailable>false</RunOnlyIfNetworkAvailable><AllowStartOnDemand>true</AllowStartOnDemand><Enabled>true</Enabled><Hidden>false</Hidden><WakeToRun>false</WakeToRun><ExecutionTimeLimit>PT0S</ExecutionTimeLimit></Settings>
  <Actions Context="CurrentUser"><Exec><Command>{command}</Command><Arguments>{BACKGROUND_ARGUMENT}</Arguments><WorkingDirectory>{parent}</WorkingDirectory></Exec></Actions>
</Task>"#
    ))
}

struct TaskIdentity {
    source: String,
    principal_sid: String,
    trigger_sid: String,
    executable: PathBuf,
    arguments: String,
    working_directory: PathBuf,
    interactive_logon: bool,
    highest_available: bool,
    trigger_enabled: bool,
}

fn validate_identity(identity: &TaskIdentity, executable: &Path, sid: &str) -> Result<(), String> {
    if identity.source != TASK_SOURCE
        || identity.principal_sid != sid
        || identity.trigger_sid != sid
        || identity.executable != executable
        || !matches!(identity.arguments.trim(), "" | BACKGROUND_ARGUMENT)
        || executable.parent() != Some(identity.working_directory.as_path())
        || !identity.interactive_logon
        || !identity.highest_available
        || !identity.trigger_enabled
    {
        Err("An existing startup task does not match this executable and user. It was not changed. Disable it using the original app location or inspect it in Task Scheduler.".into())
    } else {
        Ok(())
    }
}

#[cfg(windows)]
mod real {
    use windows::core::{Interface, BSTR, PCWSTR, PWSTR};
    use windows::Win32::Foundation::{CloseHandle, LocalFree, HANDLE, HLOCAL, VARIANT_BOOL};
    use windows::Win32::Security::Authorization::ConvertSidToStringSidW;
    use windows::Win32::Security::{
        GetTokenInformation, LookupAccountNameW, SidTypeUser, TokenUser, PSID, SID_NAME_USE,
        TOKEN_QUERY, TOKEN_USER,
    };
    use windows::Win32::System::Com::{
        CoCreateInstance, CoInitializeEx, CoUninitialize, CLSCTX_INPROC_SERVER,
        COINIT_MULTITHREADED,
    };
    use windows::Win32::System::TaskScheduler::{
        IExecAction, ILogonTrigger, IRegisteredTask, ITaskFolder, ITaskService, TaskScheduler,
        TASK_CREATE, TASK_LOGON_INTERACTIVE_TOKEN, TASK_RUNLEVEL_HIGHEST, TASK_UPDATE,
    };
    use windows::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};
    use windows::Win32::System::Variant::VARIANT;

    use super::*;

    struct ComGuard(bool);
    impl ComGuard {
        fn new() -> Result<Self, String> {
            let result = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) };
            if result.is_ok() {
                Ok(Self(true))
            } else if result.0 == 0x80010106_u32 as i32 {
                Ok(Self(false))
            } else {
                Err(format!(
                    "Task Scheduler COM initialization failed: {result}"
                ))
            }
        }
    }
    impl Drop for ComGuard {
        fn drop(&mut self) {
            if self.0 {
                unsafe { CoUninitialize() };
            }
        }
    }

    struct Token(HANDLE);
    impl Drop for Token {
        fn drop(&mut self) {
            let _ = unsafe { CloseHandle(self.0) };
        }
    }
    struct SidText(PWSTR);
    impl Drop for SidText {
        fn drop(&mut self) {
            unsafe { LocalFree(Some(HLOCAL(self.0 .0.cast()))) };
        }
    }

    fn current_sid() -> Result<String, String> {
        let mut token = HANDLE::default();
        unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) }
            .map_err(|error| format!("Could not read the current user token: {error}"))?;
        let token = Token(token);
        let mut required = 0;
        let first = unsafe { GetTokenInformation(token.0, TokenUser, None, 0, &mut required) };
        if required == 0 || required > 1_048_576 {
            return Err(format!(
                "Could not size the current user token: {:?}",
                first.err()
            ));
        }
        let mut storage = vec![0usize; (required as usize).div_ceil(std::mem::size_of::<usize>())];
        unsafe {
            GetTokenInformation(
                token.0,
                TokenUser,
                Some(storage.as_mut_ptr().cast()),
                required,
                &mut required,
            )
        }
        .map_err(|error| format!("Could not read the current user SID: {error}"))?;
        let user = unsafe { &*storage.as_ptr().cast::<TOKEN_USER>() };
        let mut sid = PWSTR::null();
        unsafe { ConvertSidToStringSidW(user.User.Sid, &mut sid) }
            .map_err(|error| format!("Could not convert the user SID: {error}"))?;
        let sid = SidText(sid);
        unsafe { sid.0.to_string() }
            .map_err(|error| format!("Could not decode the user SID: {error}"))
    }

    fn current_executable() -> Result<PathBuf, String> {
        let path = std::env::current_exe()
            .map_err(|error| format!("Could not locate aero-control: {error}"))?
            .canonicalize()
            .map_err(|error| format!("Could not resolve the executable path: {error}"))?;
        // This task must never point at a network share.
        let text = path
            .to_str()
            .ok_or("The executable path is not valid Unicode.")?;
        if text.starts_with(r"\\?\UNC\") || (text.starts_with(r"\\") && !text.starts_with(r"\\?\"))
        {
            return Err("Logon startup requires an executable on a local drive.".into());
        }
        Ok(path)
    }

    /// Task Scheduler can return a SID, a local name or DOMAIN\\name for the
    /// same account. Resolve names through Windows before ownership checks;
    /// never treat an unqualified username alone as proof of task ownership.
    fn account_sid(account: &str) -> Result<String, String> {
        if account.starts_with("S-1-") {
            // Literal SIDs must still equal the actual current token SID in
            // validate_identity; malformed/different literals cannot match.
            return Ok(account.to_owned());
        }
        if account.is_empty() || account.len() > 4096 || account.contains('\0') {
            return Err("The startup task contains an invalid Windows account.".into());
        }
        let name: Vec<_> = account.encode_utf16().chain(Some(0)).collect();
        let mut sid_bytes = 0;
        let mut domain_chars = 0;
        let mut kind = SID_NAME_USE::default();
        let sized = unsafe {
            LookupAccountNameW(
                PCWSTR::null(),
                PCWSTR(name.as_ptr()),
                None,
                &mut sid_bytes,
                None,
                &mut domain_chars,
                &mut kind,
            )
        };
        if sid_bytes == 0 || sid_bytes > 1024 || domain_chars > 32768 {
            return Err(format!(
                "Could not resolve the startup task's Windows account: {:?}",
                sized.err()
            ));
        }
        let mut sid_storage =
            vec![0usize; (sid_bytes as usize).div_ceil(std::mem::size_of::<usize>())];
        let mut domain = vec![0u16; (domain_chars as usize).max(1)];
        let sid = PSID(sid_storage.as_mut_ptr().cast());
        unsafe {
            LookupAccountNameW(
                PCWSTR::null(),
                PCWSTR(name.as_ptr()),
                Some(sid),
                &mut sid_bytes,
                Some(PWSTR(domain.as_mut_ptr())),
                &mut domain_chars,
                &mut kind,
            )
        }
        .map_err(|error| {
            format!("Could not resolve the startup task's Windows account: {error}")
        })?;
        if kind != SidTypeUser {
            return Err("The startup task account is not a Windows user.".into());
        }
        let mut text = PWSTR::null();
        unsafe { ConvertSidToStringSidW(sid, &mut text) }
            .map_err(|error| format!("Could not convert the startup task account SID: {error}"))?;
        let text = SidText(text);
        unsafe { text.0.to_string() }
            .map_err(|error| format!("Could not decode the startup task account SID: {error}"))
    }

    fn folder() -> Result<ITaskFolder, String> {
        let service: ITaskService =
            unsafe { CoCreateInstance(&TaskScheduler, None, CLSCTX_INPROC_SERVER) }
                .map_err(|error| format!("Task Scheduler is unavailable: {error}"))?;
        let empty = VARIANT::default();
        unsafe { service.Connect(&empty, &empty, &empty, &empty) }
            .map_err(|error| format!("Could not connect to the local Task Scheduler: {error}"))?;
        unsafe { service.GetFolder(&BSTR::from("\\")) }
            .map_err(|error| format!("Could not access the task folder: {error}"))
    }

    fn find(folder: &ITaskFolder, name: &str) -> Result<Option<IRegisteredTask>, String> {
        match unsafe { folder.GetTask(&BSTR::from(name)) } {
            Ok(task) => Ok(Some(task)),
            Err(error) if error.code().0 == 0x80070002_u32 as i32 => Ok(None),
            Err(error) => Err(format!("Could not inspect the startup task: {error}")),
        }
    }

    fn identity(task: &IRegisteredTask) -> Result<TaskIdentity, String> {
        let get = || -> windows::core::Result<TaskIdentity> {
            let definition = unsafe { task.Definition()? };
            let registration = unsafe { definition.RegistrationInfo()? };
            let mut source = BSTR::new();
            unsafe { registration.Source(&mut source)? };
            let principal = unsafe { definition.Principal()? };
            let mut principal_sid = BSTR::new();
            let mut logon = TASK_LOGON_INTERACTIVE_TOKEN;
            let mut run_level = TASK_RUNLEVEL_HIGHEST;
            unsafe {
                principal.UserId(&mut principal_sid)?;
                principal.LogonType(&mut logon)?;
                principal.RunLevel(&mut run_level)?;
            }
            let actions = unsafe { definition.Actions()? };
            let mut count = 0;
            unsafe { actions.Count(&mut count)? };
            if count != 1 {
                return Err(windows::core::Error::from_hresult(windows::core::HRESULT(
                    0x80070057_u32 as i32,
                )));
            }
            let action: IExecAction = unsafe { actions.get_Item(1)? }.cast()?;
            let mut path = BSTR::new();
            let mut arguments = BSTR::new();
            let mut directory = BSTR::new();
            unsafe {
                action.Path(&mut path)?;
                action.Arguments(&mut arguments)?;
                action.WorkingDirectory(&mut directory)?;
            }
            let triggers = unsafe { definition.Triggers()? };
            unsafe { triggers.Count(&mut count)? };
            if count != 1 {
                return Err(windows::core::Error::from_hresult(windows::core::HRESULT(
                    0x80070057_u32 as i32,
                )));
            }
            let trigger: ILogonTrigger = unsafe { triggers.get_Item(1)? }.cast()?;
            let mut trigger_sid = BSTR::new();
            let mut trigger_enabled = VARIANT_BOOL(0);
            unsafe {
                trigger.UserId(&mut trigger_sid)?;
                trigger.Enabled(&mut trigger_enabled)?;
            }
            Ok(TaskIdentity {
                source: source.to_string(),
                principal_sid: principal_sid.to_string(),
                trigger_sid: trigger_sid.to_string(),
                executable: PathBuf::from(path.to_string()),
                arguments: arguments.to_string(),
                working_directory: PathBuf::from(directory.to_string()),
                interactive_logon: logon == TASK_LOGON_INTERACTIVE_TOKEN,
                highest_available: run_level == TASK_RUNLEVEL_HIGHEST,
                trigger_enabled: trigger_enabled.0 != 0,
            })
        };
        let mut identity = get().map_err(|error| format!("The existing task is not a recognized aero-control startup task and was not changed: {error}"))?;
        identity.principal_sid = account_sid(&identity.principal_sid)?;
        identity.trigger_sid = account_sid(&identity.trigger_sid)?;
        identity.executable = identity.executable.canonicalize().map_err(|error| {
            format!("The existing task executable cannot be verified and was not changed: {error}")
        })?;
        identity.working_directory = identity.working_directory.canonicalize().map_err(|error| format!("The existing task working directory cannot be verified and was not changed: {error}"))?;
        Ok(identity)
    }

    #[test]
    #[ignore = "Requires the interactive user's Windows account environment"]
    fn scheduler_account_name_and_sid_identify_the_same_user() {
        let expected = current_sid().unwrap();
        let user = std::env::var("USERNAME").unwrap();
        let domain = std::env::var("USERDOMAIN").unwrap();
        for account in [
            expected.clone(),
            user.clone(),
            format!("{domain}\\{user}"),
            format!("{domain}\\{user}").to_lowercase(),
        ] {
            assert_eq!(account_sid(&account).unwrap(), expected);
        }
        assert!(account_sid("").is_err());
        assert!(account_sid("user\0hidden").is_err());
        assert!(account_sid(&format!(
            "{domain}\\AeroControlMissingUser{}",
            std::process::id()
        ))
        .is_err());
    }

    pub fn status() -> Result<StartupStatus, String> {
        let _com = ComGuard::new()?;
        let sid = current_sid()?;
        let executable = current_executable()?;
        let folder = folder()?;
        let Some(task) = find(&folder, &task_name(&sid)?)? else {
            return Ok(StartupStatus {
                enabled: false,
                background: true,
            });
        };
        let identity = identity(&task)?;
        validate_identity(&identity, &executable, &sid)?;
        unsafe { task.Enabled() }
            .map(|value| StartupStatus {
                enabled: value.0 != 0,
                background: identity.arguments.trim() == BACKGROUND_ARGUMENT,
            })
            .map_err(|error| format!("Could not read startup task state: {error}"))
    }

    #[test]
    #[ignore = "Requires a registered task and AERO_TEST_STARTUP_EXE pointing to its executable"]
    fn registered_background_task_passes_the_real_ownership_check() {
        let _com = ComGuard::new().unwrap();
        let sid = current_sid().unwrap();
        let executable = PathBuf::from(std::env::var_os("AERO_TEST_STARTUP_EXE").unwrap())
            .canonicalize()
            .unwrap();
        let task = find(&folder().unwrap(), &task_name(&sid).unwrap())
            .unwrap()
            .unwrap();
        let identity = identity(&task).unwrap();
        validate_identity(&identity, &executable, &sid).unwrap();
        assert_eq!(identity.arguments, BACKGROUND_ARGUMENT);
        assert!(unsafe { task.Enabled() }.unwrap().0 != 0);
    }

    pub fn set_enabled(enabled: bool) -> Result<(), String> {
        let _com = ComGuard::new()?;
        let sid = current_sid()?;
        let executable = current_executable()?;
        let folder = folder()?;
        let name = task_name(&sid)?;
        let task = if let Some(task) = find(&folder, &name)? {
            let identity = identity(&task)?;
            validate_identity(&identity, &executable, &sid)?;
            if enabled && identity.arguments.trim().is_empty() {
                // Upgrade only the verified legacy action, and only on an
                // explicit enable/update request. Preserve all other settings.
                let update = || -> windows::core::Result<IRegisteredTask> {
                    let definition = unsafe { task.Definition()? };
                    let action: IExecAction =
                        unsafe { definition.Actions()?.get_Item(1)? }.cast()?;
                    unsafe { action.SetArguments(&BSTR::from(BACKGROUND_ARGUMENT))? };
                    let empty = VARIANT::default();
                    unsafe {
                        folder.RegisterTaskDefinition(
                            &BSTR::from(name.as_str()),
                            &definition,
                            TASK_UPDATE.0,
                            &empty,
                            &empty,
                            TASK_LOGON_INTERACTIVE_TOKEN,
                            &empty,
                        )
                    }
                };
                update().map_err(|error| format!("Could not update background startup: {error}"))?
            } else {
                task
            }
        } else if !enabled {
            return Ok(());
        } else {
            let empty = VARIANT::default();
            let xml = task_xml(&executable, &sid)?;
            unsafe {
                folder.RegisterTask(
                    &BSTR::from(name),
                    &BSTR::from(xml),
                    TASK_CREATE.0,
                    &empty,
                    &empty,
                    TASK_LOGON_INTERACTIVE_TOKEN,
                    &empty,
                )
            }
            .map_err(|error| {
                format!(
                    "Could not create logon startup. Run aero-control as Administrator: {error}"
                )
            })?
        };
        // Never delete or change unrelated tasks.
        unsafe { task.SetEnabled(VARIANT_BOOL(if enabled { -1 } else { 0 })) }
            .map_err(|error| format!("Could not change logon startup: {error}"))?;
        let confirmed = unsafe { task.Enabled() }
            .map_err(|error| format!("Could not verify logon startup: {error}"))?;
        let identity = identity(&task)?;
        validate_identity(&identity, &executable, &sid)?;
        if (confirmed.0 != 0) == enabled
            && (!enabled || identity.arguments.trim() == BACKGROUND_ARGUMENT)
        {
            Ok(())
        } else {
            Err("Task Scheduler did not confirm the requested startup state.".into())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mock_startup_is_explicit_and_reversible() {
        let mut task = StartupTask::new(true);
        assert!(!task.status().unwrap().enabled);
        task.set_enabled(true).unwrap();
        assert_eq!(
            task.status().unwrap(),
            StartupStatus {
                enabled: true,
                background: true
            }
        );
        task.set_enabled(false).unwrap();
        assert!(!task.status().unwrap().enabled);
    }

    #[test]
    fn xml_escapes_path_and_uses_interactive_current_user_logon() {
        let xml = task_xml(Path::new("C:/A&B/<app>/aero-control.exe"), "S-1-5-21-1000").unwrap();
        assert!(xml.contains("C:/A&amp;B/&lt;app&gt;/aero-control.exe"));
        assert_eq!(xml.matches("<UserId>S-1-5-21-1000</UserId>").count(), 2);
        assert!(xml.contains("<LogonType>InteractiveToken</LogonType>"));
        assert!(xml.contains("<RunLevel>HighestAvailable</RunLevel>"));
        assert!(xml.contains("<Arguments>--background</Arguments>"));
        assert!(!xml.contains("Password"));
        assert!(task_name("S-1-5-21-1000")
            .unwrap()
            .starts_with("AeroControl-S-1-"));
        assert!(task_name("S-1-5</UserId>").is_err());
        assert!(xml_escape("bad\u{0001}path").is_err());
    }

    #[test]
    fn unrelated_or_changed_task_actions_are_rejected() {
        let executable = Path::new("C:/Aero/aero-control.exe");
        let sid = "S-1-5-21-1000";
        let mut identity = TaskIdentity {
            source: TASK_SOURCE.into(),
            principal_sid: sid.into(),
            trigger_sid: sid.into(),
            executable: executable.into(),
            arguments: String::new(),
            working_directory: executable.parent().unwrap().into(),
            interactive_logon: true,
            highest_available: true,
            trigger_enabled: true,
        };
        assert!(validate_identity(&identity, executable, sid).is_ok());
        identity.arguments = "--background".into();
        assert!(validate_identity(&identity, executable, sid).is_ok());
        identity.arguments = "--background --mock".into();
        assert!(validate_identity(&identity, executable, sid).is_err());
        identity.arguments = "--unrelated".into();
        assert!(validate_identity(&identity, executable, sid).is_err());
        identity.arguments.clear();
        identity.source = "another app".into();
        assert!(validate_identity(&identity, executable, sid).is_err());
        identity.source = TASK_SOURCE.into();
        identity.principal_sid = "S-1-5-21-2000".into();
        assert!(validate_identity(&identity, executable, sid).is_err());
        identity.principal_sid = sid.into();
        identity.trigger_sid = "S-1-5-21-2000".into();
        assert!(validate_identity(&identity, executable, sid).is_err());
        identity.trigger_sid = sid.into();
        identity.executable = PathBuf::from("C:/Other/another.exe");
        assert!(validate_identity(&identity, executable, sid).is_err());
    }
}
