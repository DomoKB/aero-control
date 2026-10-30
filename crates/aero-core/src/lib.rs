//! Fan policy for aero-control.
//!
//! Nothing in this crate polls temperatures to decide a fan duty. Curves and
//! mode switches are written to the embedded controller, which runs them
//! itself. Sensor reads exist so the UI and the probe can display them, and
//! so a drift check can compare the chip's table with the one we uploaded.
//!
//! On a failed write, a panicked apply, or a readback mismatch, the sequences
//! here restore the firmware's automatic fan mode.

#![deny(unsafe_code)]

mod apply;
mod curve;
mod import_gcc;
mod mode;
mod power;
mod preset;
mod profile;
mod scale;

#[cfg(windows)]
#[allow(unsafe_code)]
mod registry;

pub use apply::{
    apply_mode, apply_profile, curve_drift, reason_for, restore_firmware_auto, upload_curve,
    ApplyError, ApplyReason, ApplyReport, PowerEvent, SlotDiff,
};
pub use curve::{
    constrain_drag, duty_at, insert_point, issues, planned_slots, remove_point, stair_polyline,
    CurveIssue, CurvePoint, FanCurve, FAN_CURVE_SLOTS, HIGH_TEMP_C, LAST_MIN_PERCENT,
    LAST_TEMP_MAX, MAX_POINTS, MIN_POINTS,
};
pub use import_gcc::{parse_custom_fan_table_auto, parse_custom_fan_table_text, GccImport};
pub use mode::{
    custom_prefix, fan_point_writes, gaming_script, normal_script, quiet_script, script_for,
    FanMode, ModeScript, PlannedWrite,
};
pub use power::{set_charge_policy, set_nv_power_config, ChargePolicy};
pub use preset::{
    builtin_config, curve_from_percent, factory_curve, gcc_floor_raw, presets, Preset, PresetKind,
    FACTORY_YB_CHIP_RAW, FACTORY_YB_PERCENT,
};
pub use profile::{Config, CurvePointToml, Profile};
pub use scale::{clamp_fixed_duty, percent_to_raw, raw_to_percent, DUTY_MAX, FIXED_FLOOR_PERCENT};

#[cfg(windows)]
pub use import_gcc::read_gcc_custom_fan_table;
