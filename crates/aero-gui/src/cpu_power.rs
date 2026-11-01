//! Intel XTU bridge. Runs outside the UI thread; no CPU writes during polling.
use aero_core::CpuPowerPreset;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CpuPowerSnapshot {
    pub pl1_mw: u32,
    pub pl2_mw: u32,
    available: u8,
}

impl CpuPowerSnapshot {
    pub fn supports(&self, preset: CpuPowerPreset) -> bool {
        CpuPowerPreset::ALL
            .iter()
            .position(|item| *item == preset)
            .is_some_and(|index| self.available & (1 << index) != 0)
    }

    pub fn preset(&self) -> Option<CpuPowerPreset> {
        CpuPowerPreset::ALL
            .into_iter()
            .find(|preset| preset.limits_mw() == (self.pl1_mw, self.pl2_mw))
    }
}

pub struct CpuPower {
    mock: bool,
    supported_model: bool,
    elevated: bool,
    current: CpuPowerSnapshot,
}

impl CpuPower {
    pub fn new(mock: bool, supported_model: bool, elevated: bool) -> Self {
        Self {
            mock,
            supported_model,
            elevated,
            current: CpuPowerSnapshot {
                pl1_mw: 52_000,
                pl2_mw: 107_000,
                available: 127,
            },
        }
    }

    pub fn read(&self) -> Result<CpuPowerSnapshot, String> {
        self.check_model()?;
        if self.mock {
            return Ok(self.current.clone());
        }
        platform_read_or_apply(None)
    }

    pub fn apply(&mut self, preset: CpuPowerPreset) -> Result<CpuPowerSnapshot, String> {
        self.check_model()?;
        if !self.mock && !self.elevated {
            return Err("CPU power changes require administrator privileges.".into());
        }
        if self.mock {
            (self.current.pl1_mw, self.current.pl2_mw) = preset.limits_mw();
            return self.read();
        }
        let snapshot = platform_read_or_apply(Some(preset))?;
        if (snapshot.pl1_mw, snapshot.pl2_mw) != preset.limits_mw() {
            return Err("CPU power readback mismatch; current limits are unconfirmed. Refresh before retrying.".into());
        }
        Ok(snapshot)
    }

    fn check_model(&self) -> Result<(), String> {
        if self.supported_model {
            Ok(())
        } else {
            Err("No verified CPU power presets are available for the detected model.".into())
        }
    }
}

fn parse_snapshot(text: &str) -> Result<CpuPowerSnapshot, String> {
    let mut lines = text.lines().filter(|line| !line.trim().is_empty());
    let line = lines
        .next()
        .ok_or("Intel XTU returned no CPU power readings.")?;
    if lines.next().is_some() {
        return Err("Unexpected Intel XTU response.".into());
    }
    let fields: Vec<_> = line.trim().split('|').collect();
    if fields.len() != 4 || fields[0] != "AERO_CPU_V1" {
        return Err("Invalid Intel XTU response.".into());
    }
    let number = |index: usize| {
        fields[index]
            .parse::<u32>()
            .map_err(|_| "Invalid Intel XTU numeric reading.".to_string())
    };
    let (pl1_mw, pl2_mw, available) = (number(1)?, number(2)?, number(3)?);
    if !(1..=500_000).contains(&pl1_mw) || !(1..=500_000).contains(&pl2_mw) || available > 127 {
        return Err("Intel XTU returned out-of-range CPU power readings.".into());
    }
    Ok(CpuPowerSnapshot {
        pl1_mw,
        pl2_mw,
        available: available as u8,
    })
}

#[cfg(not(windows))]
fn platform_read_or_apply(_: Option<CpuPowerPreset>) -> Result<CpuPowerSnapshot, String> {
    Err("Intel XTU CPU power control is available only on Windows.".into())
}

#[cfg(windows)]
fn platform_read_or_apply(preset: Option<CpuPowerPreset>) -> Result<CpuPowerSnapshot, String> {
    use std::io::Read;
    use std::os::windows::process::CommandExt;
    use std::process::{Command, Stdio};
    use std::thread;
    use std::time::{Duration, Instant};

    // Only compile-time numeric preset values are interpolated into the script.
    let mut script = String::new();
    if let Some(preset) = preset {
        let (pl1, pl2) = preset.limits_mw();
        script = format!("$RequestedPl1 = [decimal]({pl1} / 1000.0); $RequestedPl2 = [decimal]({pl2} / 1000.0);\n");
    }
    script.push_str(include_str!("cpu_power.ps1"));
    let root =
        std::env::var_os("SystemRoot").ok_or("The Windows system directory is unavailable.")?;
    let shell_dir = std::path::PathBuf::from(root).join("System32/WindowsPowerShell/v1.0");
    let executable = shell_dir.join("powershell.exe");
    let mut child = Command::new(executable)
        .env("PSModulePath", shell_dir.join("Modules"))
        .args([
            "-NoLogo",
            "-NoProfile",
            "-NonInteractive",
            "-Command",
            &script,
        ])
        .creation_flags(0x08000000) // CREATE_NO_WINDOW, also in debug builds.
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| format!("Could not start the Intel XTU bridge: {error}"))?;
    let stdout = child
        .stdout
        .take()
        .ok_or("Intel XTU output pipe unavailable.")?;
    let stderr = child
        .stderr
        .take()
        .ok_or("Intel XTU error pipe unavailable.")?;
    let capture = |mut pipe: Box<dyn Read + Send>| {
        thread::spawn(move || {
            let mut bytes = Vec::new();
            // Drain output even after the storage limit, so a verbose SDK cannot deadlock.
            let mut buffer = [0u8; 4096];
            loop {
                match pipe.read(&mut buffer) {
                    Ok(0) | Err(_) => break,
                    Ok(count) => {
                        let keep = count.min(65_536usize.saturating_sub(bytes.len()));
                        bytes.extend_from_slice(&buffer[..keep]);
                    }
                }
            }
            String::from_utf8_lossy(&bytes).into_owned()
        })
    };
    let output_thread = capture(Box::new(stdout));
    let error_thread = capture(Box::new(stderr));
    let deadline = Instant::now() + Duration::from_secs(30);
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if Instant::now() < deadline => thread::sleep(Duration::from_millis(50)),
            other => {
                let _ = child.kill();
                let _ = child.wait();
                let _ = output_thread.join();
                let _ = error_thread.join();
                return Err(match other {
                    Err(error) => format!("Could not wait for the Intel XTU bridge: {error}. CPU limits are unconfirmed."),
                    _ => "Intel XTU timed out. CPU limits are unconfirmed; refresh before retrying.".into(),
                });
            }
        }
    };
    let output = output_thread
        .join()
        .map_err(|_| "Intel XTU output capture failed.")?;
    let errors = error_thread
        .join()
        .map_err(|_| "Intel XTU error capture failed.")?;
    if !status.success() {
        return Err(if errors.trim().is_empty() {
            "Intel XTU failed without a diagnostic.".into()
        } else {
            errors.trim().to_string()
        });
    }
    parse_snapshot(&output)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mock_presets_have_exact_readback_and_no_automatic_apply() {
        let mut backend = CpuPower::new(true, true, false);
        assert_eq!(
            backend.read().unwrap().preset(),
            Some(CpuPowerPreset::Level3)
        );
        for preset in CpuPowerPreset::ALL {
            let snapshot = backend.apply(preset).unwrap();
            assert_eq!((snapshot.pl1_mw, snapshot.pl2_mw), preset.limits_mw());
            assert!(snapshot.supports(preset));
        }
    }

    #[test]
    fn unsupported_model_never_starts_the_bridge() {
        let mut backend = CpuPower::new(false, false, true);
        assert!(backend.read().is_err());
        assert!(backend.apply(CpuPowerPreset::Level1).is_err());
    }

    #[test]
    fn non_elevated_apply_fails_before_starting_the_bridge() {
        assert!(CpuPower::new(false, true, false)
            .apply(CpuPowerPreset::Level1)
            .is_err());
    }

    #[test]
    fn protocol_rejects_missing_malformed_extra_and_out_of_range_readings() {
        for text in [
            "",
            "52000|107000|127",
            "AERO_CPU_V1|NaN|107000|127",
            "AERO_CPU_V1|0|107000|127",
            "AERO_CPU_V1|52000|107000|128",
            "AERO_CPU_V1|52000|107000|127\nextra",
        ] {
            assert!(parse_snapshot(text).is_err(), "{text}");
        }
        let snapshot = parse_snapshot("AERO_CPU_V1|45000|56250|64\r\n").unwrap();
        assert_eq!(snapshot.preset(), Some(CpuPowerPreset::Battery));
        assert!(snapshot.supports(CpuPowerPreset::Battery));
        assert!(!snapshot.supports(CpuPowerPreset::Level2));
    }
}
