//! Method names and signatures.
//!
//! Getter and setter signatures are taken from the `GB_WMIACPI_Get` /
//! `GB_WMIACPI_Set` MOF published for a Gigabyte Aero 16 (SKU P86YE5) in
//! lm-sensors issue 400. Parameter *names* are the MOF identifiers (`Data`,
//! `Temperture`), not the `Description("...")` strings.
//!
//! `SetFanCustomStatus` and `GetSuperQuiet` are not in that dump. The scope
//! names the custom-cap setter, so they are listed and probed. Calls to the
//! setter are optional and skipped when the firmware has no such method.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MethodSpec {
    pub name: &'static str,
    pub inputs: &'static [&'static str],
    pub outputs: &'static [&'static str],
}

/// Methods aero-control will not call. `TurnOffFan` exists in the MOF and
/// can stop the fans; it is not part of any mode script.
pub const NEVER_CALL: &[&str] = &["TurnOffFan"];

macro_rules! methods {
    ($(($name:literal, [$($in:literal),* $(,)?], [$($out:literal),* $(,)?])),* $(,)?) => {
        &[
            $(
                MethodSpec {
                    name: $name,
                    inputs: &[$($in),*],
                    outputs: &[$($out),*],
                },
            )*
        ]
    };
}

pub static GETTERS: &[MethodSpec] = methods![
    ("GetCPUFanDuty", [], ["Data"]),
    ("GetGPUFanDuty", [], ["Data"]),
    ("GetBatteryCount", [], ["Data"]),
    ("GetMaxCharge", [], ["Data"]),
    ("GetPEGorSG", [], ["Data"]),
    ("GetNvPowerConfig", [], ["Data"]),
    ("GetThermalData", [], ["Thermal1", "Thermal2", "Thermal3"]),
    ("GetNvThermalTarget", [], ["Data"]),
    ("CheckHeavyLoading", [], ["Status"]),
    (
        "GetLightBar",
        ["Index"],
        ["Status", "Level", "Red", "Green", "Blue"]
    ),
    (
        "GetDeepFan",
        [],
        [
            "Temperature0",
            "Temperature1",
            "Temperature2",
            "Temperature3",
            "Temperature4",
            "Speed0",
            "Speed1",
            "Speed2",
            "Speed3",
            "Speed4"
        ]
    ),
    ("GetBatteryHealth", [], ["Data"]),
    ("GetFanHealth", [], ["Data"]),
    (
        "GetPowerOnTime",
        [],
        ["Year", "Month", "Day", "Hour", "Minute"]
    ),
    ("GetChargePolicy", [], ["Data"]),
    ("GetChargeStop", [], ["Data"]),
    ("GetStepFanStatus", [], ["Data"]),
    ("GetFanIndexValue", ["Index"], ["Temperture", "Value"]),
    ("GetVRStatus", [], ["Data"]),
    ("GetFixedFanStatus", [], ["Data"]),
    ("GetFixedFanSpeed", [], ["Data"]),
    ("IsDeviceExist", [], ["Data"]),
    ("getBattCyc1", [], ["Data"]),
    ("getBattCyc", [], ["Data"]),
    ("GetFanPWMStatus", [], ["Data"]),
    ("GetFanAdjustStatus", [], ["Data"]),
    ("GetAutoFanStatus", [], ["Data"]),
    ("GetSleepUSBCharge", [], ["Data"]),
    ("GetHibernationUSBCharge", [], ["Data"]),
    ("CheckUCFSupport", [], ["Data"]),
    ("GetFanSpeed", [], ["Data"]),
    ("GetCamera2", [], ["Data"]),
    ("GetTouchScreenSupport", [], ["Data"]),
    ("GetAIBoostStatus", [], ["Data"]),
    (
        "GetFirstDate",
        [],
        ["Year", "Month", "Day", "Hour", "Minute"]
    ),
    ("GetEcValueBoostStatus", [], ["Data"]),
    ("GetWhisperMode", [], ["Data"]),
    ("GetBrightness", [], ["Data"]),
    ("GetBluetooth", [], ["Data"]),
    ("GetWiFi", [], ["Data"]),
    ("GetW35G", [], ["Data"]),
    ("GetBirightnessOff", [], ["Data"]),
    ("GetCamera", [], ["Data"]),
    ("GetTouchPad", [], ["Data"]),
    ("GetWinkeyBlocking", [], ["Data"]),
    ("GetTurboMode", [], ["Data"]),
    ("GetSmartCharge", [], ["Data"]),
    ("CheckSmartCharge", [], ["Data"]),
    ("CheckSmartTurbo", [], ["Data"]),
    ("GetUSB30", [], ["Data"]),
    ("CheckUSB30", [], ["Data"]),
    ("GetDockingStatus", [], ["Data"]),
    ("GetTouchScreen", [], ["Data"]),
    ("GetSmartTurbo", [], ["Data"]),
    ("GetLid1Status", [], ["Data"]),
    ("GetKeyboardMatrix", [], ["Data"]),
    ("GetSmartTurboStatus", [], ["Data"]),
    ("GetGSensorStatus", [], ["Data"]),
    ("GetOnboardLANStatus", [], ["Data"]),
    ("CheckDocking", [], ["Data"]),
    ("GetKeyBoardBackLight", [], ["Data"]),
    ("QueryLightSensor", [], ["Data"]),
    ("QueryThermalSensor", [], ["Data"]),
    ("GetSmartCool", [], ["Data"]),
    ("GetLightSensorVersion", [], ["Data"]),
    (
        "GetLightSensorValue",
        [],
        ["Byte0", "Byte1", "Byte2", "Byte3"]
    ),
    ("Check3GModule", [], ["Data"]),
    ("getCpuTemp", [], ["Data"]),
    ("getGpuTemp1", [], ["Data"]),
    ("getGpuTemp2", [], ["Data"]),
    ("getRpm1", [], ["Data"]),
    ("getRpm2", [], ["Data"]),
    ("GetPEG2orSG2", [], ["Data"]),
    ("GetDynamicBoostStatus", [], ["Data"]),
    // Not in the Aero 16 MOF. Probed so an AERO 17 YB that has it shows up.
    ("GetSuperQuiet", [], ["Data"]),
    ("GetFanCustomStatus", [], ["Data"]),
];

pub static SETTERS: &[MethodSpec] = methods![
    ("SetGPUFanDuty", ["Data"], ["DataOut"]),
    ("SetSuperQuiet", ["Data"], []),
    ("SetChargePolicy", ["Data"], ["DataOut"]),
    ("SetChargeStop", ["Data"], ["DataOut"]),
    ("SetCurrentFanStep", ["Data"], ["DataOut"]),
    ("SetStepFanStatus", ["Data"], ["DataOut"]),
    ("SetFanIndexValue", ["Index", "Temperture", "Value"], []),
    ("SetFixedFanStatus", ["Data"], ["DataOut"]),
    ("SetFixedFanSpeed", ["Data"], ["DataOut"]),
    ("SetFanAdjustStatus", ["Data"], ["DataOut"]),
    ("SetAutoFanStatus", ["Data"], ["DataOut"]),
    ("SetFanSpeed", ["Data"], ["DataOut"]),
    ("SetAIBoostStatus", ["Data"], ["DataOut"]),
    ("SetWhisperMode", ["Data"], ["DataOut"]),
    ("SetNvPowerConfig", ["Data"], []),
    ("SetNvThermalTarget", ["Data"], []),
    ("SetDynamicBoostStatus", ["Data"], []),
    ("SetPEGorSG", ["Data"], []),
    (
        "SetDeepFan",
        [
            "Temperature0",
            "Temperature1",
            "Temperature2",
            "Temperature3",
            "Temperature4",
            "Speed0",
            "Speed1",
            "Speed2",
            "Speed3",
            "Speed4"
        ],
        []
    ),
    ("TurnOffFan", ["Data"], ["DataOut"]),
    // Named by the scope's curve-accuracy notes. Absent from the Aero 16 MOF.
    ("SetFanCustomStatus", ["Data"], ["DataOut"]),
];

pub fn find_getter(name: &str) -> Option<&'static MethodSpec> {
    GETTERS.iter().find(|spec| spec.name == name)
}

pub fn find_setter(name: &str) -> Option<&'static MethodSpec> {
    SETTERS.iter().find(|spec| spec.name == name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fan_index_keeps_the_mof_temperature_typo() {
        let spec = find_getter("GetFanIndexValue").unwrap();
        assert_eq!(spec.inputs, &["Index"]);
        assert_eq!(spec.outputs, &["Temperture", "Value"]);
        let set = find_setter("SetFanIndexValue").unwrap();
        assert_eq!(set.inputs, &["Index", "Temperture", "Value"]);
    }

    #[test]
    fn simple_setters_take_a_parameter_named_data() {
        for name in [
            "SetAutoFanStatus",
            "SetFixedFanStatus",
            "SetFixedFanSpeed",
            "SetGPUFanDuty",
            "SetCurrentFanStep",
            "SetStepFanStatus",
            "SetSuperQuiet",
            "SetFanAdjustStatus",
        ] {
            let spec = find_setter(name).unwrap();
            assert_eq!(spec.inputs, &["Data"], "{name}");
        }
    }

    #[test]
    fn turn_off_fan_is_listed_and_forbidden() {
        assert!(SETTERS.iter().any(|spec| spec.name == "TurnOffFan"));
        assert!(NEVER_CALL.contains(&"TurnOffFan"));
    }
}
