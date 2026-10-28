//! Rolling text log of every WMI call.
//!
//! Preferred directory is `%ProgramData%\AeroCtl\logs`. If that cannot be
//! created, the log sits in `logs` next to the executable.

use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

const MAX_BYTES: u64 = 1024 * 1024;
const KEEP: u32 = 3;

pub struct CallLog {
    path: PathBuf,
    file: File,
}

impl CallLog {
    pub fn open() -> Result<Self, String> {
        let dir = log_directory();
        fs::create_dir_all(&dir).map_err(|err| format!("create {}: {err}", dir.display()))?;
        Self::open_in(dir)
    }

    pub fn open_in(dir: impl AsRef<Path>) -> Result<Self, String> {
        let dir = dir.as_ref();
        fs::create_dir_all(dir).map_err(|err| format!("create {}: {err}", dir.display()))?;
        let path = dir.join("aero-control.log");
        let file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .map_err(|err| format!("open {}: {err}", path.display()))?;
        Ok(Self { path, file })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn record(&mut self, line: &str) {
        self.rotate_if_needed();
        let _ = writeln!(self.file, "{line}");
        let _ = self.file.flush();
    }

    fn rotate_if_needed(&mut self) {
        let len = fs::metadata(&self.path).map(|meta| meta.len()).unwrap_or(0);
        if len < MAX_BYTES {
            return;
        }
        rotate_files(&self.path);
        if let Ok(file) = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)
        {
            self.file = file;
        }
    }
}

pub fn log_directory() -> PathBuf {
    choose_log_directory(
        std::env::var_os("ProgramData").map(PathBuf::from),
        exe_dir(),
    )
}

pub fn choose_log_directory(program_data: Option<PathBuf>, exe_dir: PathBuf) -> PathBuf {
    if let Some(root) = program_data {
        let dir = root.join("AeroCtl").join("logs");
        if fs::create_dir_all(&dir).is_ok() {
            return dir;
        }
    }
    exe_dir.join("logs")
}

fn exe_dir() -> PathBuf {
    std::env::current_exe()
        .ok()
        .and_then(|path| path.parent().map(Path::to_path_buf))
        .unwrap_or_else(|| PathBuf::from("."))
}

fn rotate_files(path: &Path) {
    let _ = fs::remove_file(numbered(path, KEEP));
    for index in (1..KEEP).rev() {
        let _ = fs::rename(numbered(path, index), numbered(path, index + 1));
    }
    let _ = fs::rename(path, numbered(path, 1));
}

fn numbered(path: &Path, index: u32) -> PathBuf {
    let mut name = path.as_os_str().to_os_string();
    name.push(format!(".{index}"));
    PathBuf::from(name)
}

pub fn format_call(kind: &str, method: &str, params: &str, result: &str) -> String {
    format!("{} {kind} {method} {params} {result}", utc_now())
}

pub fn utc_now() -> String {
    let duration = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    format!(
        "{}.{:03}Z",
        utc_timestamp(duration.as_secs()),
        duration.subsec_millis()
    )
}

pub fn utc_timestamp(unix_secs: u64) -> String {
    let days = unix_secs / 86_400;
    let tod = unix_secs % 86_400;
    let hour = tod / 3600;
    let minute = (tod % 3600) / 60;
    let second = tod % 60;
    let (year, month, day) = civil_from_days(days as i64);
    format!("{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}")
}

fn civil_from_days(days: i64) -> (i64, u32, u32) {
    // Howard Hinnant's civil_from_days, shifted so day 0 is 1970-01-01.
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = if month <= 2 { y + 1 } else { y };
    (year, month as u32, day as u32)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unix_epoch_formats_as_utc() {
        assert_eq!(utc_timestamp(0), "1970-01-01T00:00:00");
        assert_eq!(utc_timestamp(86_400), "1970-01-02T00:00:00");
    }

    #[test]
    fn program_data_is_preferred_when_it_can_be_created() {
        let root = std::env::temp_dir().join(format!(
            "aero-log-{}-{}",
            std::process::id(),
            utc_timestamp(0)
        ));
        let _ = fs::remove_dir_all(&root);
        let dir = choose_log_directory(Some(root.clone()), PathBuf::from("/tmp/exe"));
        assert_eq!(dir, root.join("AeroCtl").join("logs"));
        let mut log = CallLog::open_in(&dir).unwrap();
        log.record(&format_call("GET", "getCpuTemp", "", "OK Data=61"));
        let text = fs::read_to_string(dir.join("aero-control.log")).unwrap();
        assert!(text.contains("GET getCpuTemp"));
        let _ = fs::remove_dir_all(&root);
    }
}
