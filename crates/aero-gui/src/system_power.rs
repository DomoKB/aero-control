//! Windows power plans and battery status. `--mock` keeps both in memory.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Plan {
    pub name: &'static str,
    guid: u128,
}

const PLANS: [Plan; 3] = [
    Plan {
        name: "Power saver",
        guid: 0xa1841308_3541_4fab_bc81_f71556f20b4a,
    },
    Plan {
        name: "Balanced",
        guid: 0x381b4222_f694_41f0_9685_ff5bb260df2e,
    },
    Plan {
        name: "High performance",
        guid: 0x8c5e7fda_e8bf_4a96_9a85_cf1bfa8bcc5c,
    },
];

#[derive(Clone, Copy, Debug)]
pub struct Battery {
    pub on_ac: Option<bool>,
    pub percent: Option<u8>,
    pub charging: bool,
}

pub enum SystemPower {
    Mock {
        active: Plan,
    },
    #[cfg(windows)]
    Real,
}

impl SystemPower {
    #[cfg(windows)]
    pub fn new(mock: bool) -> Self {
        if mock {
            Self::mock()
        } else {
            Self::Real
        }
    }

    #[cfg(not(windows))]
    pub fn new(_mock: bool) -> Self {
        Self::mock()
    }

    fn mock() -> Self {
        Self::Mock { active: PLANS[1] }
    }

    /// The standard plans that exist on this system.
    pub fn plans(&self) -> Vec<Plan> {
        match self {
            Self::Mock { .. } => PLANS.to_vec(),
            #[cfg(windows)]
            Self::Real => {
                let present = real::schemes();
                PLANS
                    .into_iter()
                    .filter(|plan| present.contains(&plan.guid))
                    .collect()
            }
        }
    }

    /// `Err` holds the active scheme's GUID when it is not one of the standard plans.
    pub fn active(&self) -> Result<Plan, String> {
        match self {
            Self::Mock { active } => Ok(*active),
            #[cfg(windows)]
            Self::Real => {
                let guid = real::active()?;
                PLANS
                    .into_iter()
                    .find(|plan| plan.guid == guid)
                    .ok_or_else(|| format!("a custom plan ({guid:032x})"))
            }
        }
    }

    pub fn set_active(&mut self, plan: Plan) -> Result<(), String> {
        match self {
            Self::Mock { active } => {
                *active = plan;
                Ok(())
            }
            #[cfg(windows)]
            Self::Real => real::set_active(plan.guid),
        }
    }

    pub fn battery(&self) -> Option<Battery> {
        match self {
            Self::Mock { .. } => Some(Battery {
                on_ac: Some(true),
                percent: Some(78),
                charging: true,
            }),
            #[cfg(windows)]
            Self::Real => real::battery(),
        }
    }
}

#[cfg(windows)]
mod real {
    use windows::core::GUID;
    use windows::Win32::Foundation::{LocalFree, ERROR_SUCCESS, HLOCAL};
    use windows::Win32::System::Power::{
        GetSystemPowerStatus, PowerEnumerate, PowerGetActiveScheme, PowerSetActiveScheme,
        ACCESS_SCHEME, SYSTEM_POWER_STATUS,
    };

    use super::Battery;

    pub fn schemes() -> Vec<u128> {
        let mut found = Vec::new();
        for index in 0.. {
            let mut guid = GUID::zeroed();
            let mut size = std::mem::size_of::<GUID>() as u32;
            let rc = unsafe {
                PowerEnumerate(
                    None,
                    None,
                    None,
                    ACCESS_SCHEME,
                    index,
                    Some((&mut guid as *mut GUID).cast()),
                    &mut size,
                )
            };
            if rc != ERROR_SUCCESS {
                break;
            }
            found.push(guid.to_u128());
        }
        found
    }

    pub fn active() -> Result<u128, String> {
        let mut ptr: *mut GUID = std::ptr::null_mut();
        let rc = unsafe { PowerGetActiveScheme(None, &mut ptr) };
        if rc != ERROR_SUCCESS || ptr.is_null() {
            return Err(format!("PowerGetActiveScheme failed ({})", rc.0));
        }
        let guid = unsafe { *ptr }.to_u128();
        unsafe { LocalFree(Some(HLOCAL(ptr.cast()))) };
        Ok(guid)
    }

    pub fn set_active(guid: u128) -> Result<(), String> {
        let guid = GUID::from_u128(guid);
        let rc = unsafe { PowerSetActiveScheme(None, Some(&guid)) };
        if rc == ERROR_SUCCESS {
            Ok(())
        } else {
            Err(format!("PowerSetActiveScheme failed ({})", rc.0))
        }
    }

    pub fn battery() -> Option<Battery> {
        let mut status = SYSTEM_POWER_STATUS::default();
        unsafe { GetSystemPowerStatus(&mut status) }.ok()?;
        Some(Battery {
            on_ac: match status.ACLineStatus {
                0 => Some(false),
                1 => Some(true),
                _ => None,
            },
            percent: (status.BatteryLifePercent <= 100).then_some(status.BatteryLifePercent),
            charging: status.BatteryFlag & 8 != 0,
        })
    }
}
