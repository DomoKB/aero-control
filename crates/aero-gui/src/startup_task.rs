//! Explicit per-user logon startup. No task is created just by constructing this type.
//!
//! Task Scheduler's COM API avoids shell parsing, passwords and temporary scripts.
//! Existing tasks are changed only after their source, principal, trigger and sole
//! executable action have been checked against this app and current user.

use std::path::{Path, PathBuf};

const TASK_SOURCE: &str = "aero-control/startup/v1";

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

    pub fn enabled(&self) -> Result<bool, String> {
        match self {
            Self::Mock { enabled } => Ok(*enabled),
            #[cfg(windows)]
            Self::Real => real::enabled(),
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
  <RegistrationInfo><Description>Start aero-control when this user logs on.</Description><Source>{TASK_SOURCE}</Source></RegistrationInfo>
  <Triggers><LogonTrigger><Enabled>true</Enabled><UserId>{sid}</UserId></LogonTrigger></Triggers>
  <Principals><Principal id="CurrentUser"><UserId>{sid}</UserId><LogonType>InteractiveToken</LogonType><RunLevel>HighestAvailable</RunLevel></Principal></Principals>
  <Settings><MultipleInstancesPolicy>IgnoreNew</MultipleInstancesPolicy><DisallowStartIfOnBatteries>false</DisallowStartIfOnBatteries><StopIfGoingOnBatteries>false</StopIfGoingOnBatteries><AllowHardTerminate>true</AllowHardTerminate><StartWhenAvailable>true</StartWhenAvailable><RunOnlyIfNetworkAvailable>false</RunOnlyIfNetworkAvailable><AllowStartOnDemand>true</AllowStartOnDemand><Enabled>true</Enabled><Hidden>false</Hidden><WakeToRun>false</WakeToRun><ExecutionTimeLimit>PT0S</ExecutionTimeLimit></Settings>
  <Actions Context="CurrentUser"><Exec><Command>{command}</Command><WorkingDirectory>{parent}</WorkingDirectory></Exec></Actions>
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
        || !identity.arguments.trim().is_empty()
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
    use windows::core::{Interface, BSTR, PWSTR};
    use windows::Win32::Foundation::{CloseHandle, LocalFree, HANDLE, HLOCAL, VARIANT_BOOL};
    use windows::Win32::Security::Authorization::ConvertSidToStringSidW;
    use windows::Win32::Security::{GetTokenInformation, TokenUser, TOKEN_QUERY, TOKEN_USER};
    use windows::Win32::System::Com::{
        CoCreateInstance, CoInitializeEx, CoUninitialize, CLSCTX_INPROC_SERVER,
        COINIT_MULTITHREADED,
    };
    use windows::Win32::System::TaskScheduler::{
        IExecAction, ILogonTrigger, IRegisteredTask, ITaskFolder, ITaskService, TaskScheduler,
        TASK_CREATE, TASK_LOGON_INTERACTIVE_TOKEN, TASK_RUNLEVEL_HIGHEST,
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
        identity.executable = identity.executable.canonicalize().map_err(|error| {
            format!("The existing task executable cannot be verified and was not changed: {error}")
        })?;
        identity.working_directory = identity.working_directory.canonicalize().map_err(|error| format!("The existing task working directory cannot be verified and was not changed: {error}"))?;
        Ok(identity)
    }

    pub fn enabled() -> Result<bool, String> {
        let _com = ComGuard::new()?;
        let sid = current_sid()?;
        let executable = current_executable()?;
        let folder = folder()?;
        let Some(task) = find(&folder, &task_name(&sid)?)? else {
            return Ok(false);
        };
        validate_identity(&identity(&task)?, &executable, &sid)?;
        unsafe { task.Enabled() }
            .map(|value| value.0 != 0)
            .map_err(|error| format!("Could not read startup task state: {error}"))
    }

    pub fn set_enabled(enabled: bool) -> Result<(), String> {
        let _com = ComGuard::new()?;
        let sid = current_sid()?;
        let executable = current_executable()?;
        let folder = folder()?;
        let name = task_name(&sid)?;
        let task = if let Some(task) = find(&folder, &name)? {
            validate_identity(&identity(&task)?, &executable, &sid)?;
            task
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
        // Do not replace or delete a task definition; only toggle this verified task.
        unsafe { task.SetEnabled(VARIANT_BOOL(if enabled { -1 } else { 0 })) }
            .map_err(|error| format!("Could not change logon startup: {error}"))?;
        let confirmed = unsafe { task.Enabled() }
            .map_err(|error| format!("Could not verify logon startup: {error}"))?;
        if (confirmed.0 != 0) == enabled {
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
        assert!(!task.enabled().unwrap());
        task.set_enabled(true).unwrap();
        assert!(task.enabled().unwrap());
        task.set_enabled(false).unwrap();
        assert!(!task.enabled().unwrap());
    }

    #[test]
    fn xml_escapes_path_and_uses_interactive_current_user_logon() {
        let xml = task_xml(Path::new("C:/A&B/<app>/aero-control.exe"), "S-1-5-21-1000").unwrap();
        assert!(xml.contains("C:/A&amp;B/&lt;app&gt;/aero-control.exe"));
        assert_eq!(xml.matches("<UserId>S-1-5-21-1000</UserId>").count(), 2);
        assert!(xml.contains("<LogonType>InteractiveToken</LogonType>"));
        assert!(xml.contains("<RunLevel>HighestAvailable</RunLevel>"));
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
        identity.arguments = "--unrelated".into();
        assert!(validate_identity(&identity, executable, sid).is_err());
        identity.arguments.clear();
        identity.source = "another app".into();
        assert!(validate_identity(&identity, executable, sid).is_err());
        identity.source = TASK_SOURCE.into();
        identity.trigger_sid = "S-1-5-21-2000".into();
        assert!(validate_identity(&identity, executable, sid).is_err());
        identity.trigger_sid = sid.into();
        identity.executable = PathBuf::from("C:/Other/another.exe");
        assert!(validate_identity(&identity, executable, sid).is_err());
    }
}
