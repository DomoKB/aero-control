//! Confirmed fan state and the request currently waiting for verification.

use aero_core::{CurvePoint, CurvePointToml, FanMode, Profile};

#[derive(Default)]
pub(super) struct ApplyState {
    pub active_mode: Option<FanMode>,
    pub pending_mode: Option<FanMode>,
    pub applied_curve: Vec<CurvePoint>,
    pub pending_curve: Option<Vec<CurvePoint>>,
    pub applied_fixed_raw: Option<u8>,
    pub pending_fixed_raw: Option<u8>,
    pub pending_temporary: bool,
}

impl ApplyState {
    pub fn from_readback(state: &crate::fan_state::RecognizedFanState) -> Self {
        Self {
            active_mode: Some(state.mode),
            applied_fixed_raw: state.fixed_raw,
            ..Default::default()
        }
    }

    pub fn reconcile_readback(
        &mut self,
        state: &Result<crate::fan_state::RecognizedFanState, crate::fan_state::FanStateError>,
    ) {
        if self.pending_mode.is_some() {
            return;
        }
        match state {
            Ok(state) => *self = Self::from_readback(state),
            // A locally uploaded/verified curve is known even though the YB
            // cannot distinguish its step flags from capped automatic control.
            Err(crate::fan_state::FanStateError::AmbiguousStep)
                if self.active_mode == Some(FanMode::Custom) => {}
            Err(_) => *self = Self::default(),
        }
    }

    /// Progress, sensor and power events have no fan completion result.
    pub fn complete(&mut self, result: Option<bool>) -> Option<Profile> {
        let succeeded = result?;
        let Some(mode) = self.pending_mode.take() else {
            return None;
        };
        let temporary = std::mem::take(&mut self.pending_temporary);
        let curve = self.pending_curve.take();
        let fixed_raw = self.pending_fixed_raw.take();
        if succeeded {
            self.active_mode = Some(mode);
            self.applied_fixed_raw = fixed_raw;
            if let Some(curve) = curve {
                self.applied_curve = curve;
            }
        } else {
            // Recovery can itself fail. Neither the mode nor curve is confirmed.
            self.active_mode = None;
            self.applied_curve.clear();
            self.applied_fixed_raw = None;
        }
        if succeeded && !temporary {
            self.fan_profile("Last fan settings".into())
        } else {
            None
        }
    }

    pub fn fan_profile(&self, name: String) -> Option<Profile> {
        let mode = self.active_mode?;
        let mut profile = aero_core::builtin_config().profile("Normal")?.clone();
        profile.name = name;
        profile.fan_mode = mode;
        profile.notes = "Saved from confirmed fan settings in aero-control.".into();
        if mode == FanMode::Fixed {
            profile.fixed_duty_raw = Some(self.applied_fixed_raw?);
            profile.expert_mode = self.applied_fixed_raw.is_some_and(|raw| raw < 80);
        }
        if mode == FanMode::Custom {
            if self.applied_curve.is_empty() {
                return None;
            }
            profile.curve = self
                .applied_curve
                .iter()
                .map(|point| CurvePointToml {
                    temp_c: point.temp_c,
                    duty_percent: Some(aero_core::raw_to_percent(point.duty_raw)),
                    duty_raw: Some(point.duty_raw),
                })
                .collect();
        }
        Some(profile)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn startup_readback_highlights_the_mode_without_scheduling_a_write() {
        let state = ApplyState::from_readback(&crate::fan_state::RecognizedFanState {
            mode: FanMode::Gaming,
            fixed_raw: None,
            note: None,
        });
        assert_eq!(state.active_mode, Some(FanMode::Gaming));
        assert_eq!(state.pending_mode, None);
        assert_eq!(state.pending_curve, None);
        assert_eq!(state.pending_fixed_raw, None);
    }

    #[test]
    fn gpu_override_reconciles_or_invalidates_the_highlight() {
        let mut state = ApplyState::from_readback(&crate::fan_state::RecognizedFanState {
            mode: FanMode::Normal,
            fixed_raw: None,
            note: None,
        });
        state.reconcile_readback(&Ok(crate::fan_state::RecognizedFanState {
            mode: FanMode::Quiet,
            fixed_raw: None,
            note: None,
        }));
        assert_eq!(state.active_mode, Some(FanMode::Quiet));
        state.reconcile_readback(&Err("Mixed GPU configuration".into()));
        assert_eq!(state.active_mode, None);
    }

    #[test]
    fn ambiguous_readback_preserves_only_a_locally_confirmed_custom_curve() {
        let mut state = ApplyState {
            active_mode: Some(FanMode::Custom),
            applied_curve: requested_curve(),
            ..Default::default()
        };
        state.reconcile_readback(&Err(crate::fan_state::FanStateError::AmbiguousStep));
        assert_eq!(state.active_mode, Some(FanMode::Custom));
        assert_eq!(state.applied_curve, requested_curve());
        state.reconcile_readback(&Err("Getter failed".into()));
        assert_eq!(state.active_mode, None);
        state.active_mode = Some(FanMode::Custom);
        state.pending_mode = Some(FanMode::Gaming);
        state.reconcile_readback(&Ok(crate::fan_state::RecognizedFanState {
            mode: FanMode::Normal,
            fixed_raw: None,
            note: None,
        }));
        assert_eq!(state.active_mode, Some(FanMode::Custom));
        assert_eq!(state.pending_mode, Some(FanMode::Gaming));
    }

    fn requested_curve() -> Vec<CurvePoint> {
        aero_core::factory_curve().points().to_vec()
    }

    #[test]
    fn progress_does_not_commit_a_pending_upload() {
        let previous = aero_core::quiet_curve().points().to_vec();
        let mut state = ApplyState {
            active_mode: Some(FanMode::Normal),
            pending_mode: Some(FanMode::Custom),
            applied_curve: previous.clone(),
            pending_curve: Some(requested_curve()),
            ..Default::default()
        };
        assert!(state.complete(None).is_none());
        assert_eq!(state.active_mode, Some(FanMode::Normal));
        assert_eq!(state.pending_mode, Some(FanMode::Custom));
        assert_eq!(state.applied_curve, previous);
    }

    #[test]
    fn successful_upload_commits_the_requested_curve() {
        let curve = requested_curve();
        let mut state = ApplyState {
            pending_mode: Some(FanMode::Custom),
            pending_curve: Some(curve.clone()),
            ..Default::default()
        };
        let remembered = state.complete(Some(true)).unwrap();
        assert_eq!(remembered.fan_curve().unwrap().points(), curve);
        assert_eq!(remembered.cpu_power_preset, None);
        assert_eq!(remembered.charge_policy, None);
        assert_eq!(state.active_mode, Some(FanMode::Custom));
        assert_eq!(state.applied_curve, curve);
        assert!(state.pending_mode.is_none());
        assert!(state.pending_curve.is_none());
    }

    #[test]
    fn failed_upload_invalidates_custom_state_and_allows_explicit_retry() {
        let mut state = ApplyState {
            active_mode: Some(FanMode::Custom),
            pending_mode: Some(FanMode::Custom),
            applied_curve: aero_core::quiet_curve().points().to_vec(),
            pending_curve: Some(requested_curve()),
            ..Default::default()
        };
        assert!(state.complete(Some(false)).is_none());
        assert!(state.active_mode.is_none());
        assert!(state.pending_mode.is_none());
        assert!(state.pending_curve.is_none());
        assert!(state.applied_curve.is_empty());
    }

    #[test]
    fn fixed_duty_memory_uses_verified_raw_bytes_and_preserves_expert_floor() {
        let mut state = ApplyState {
            active_mode: Some(FanMode::Fixed),
            applied_fixed_raw: Some(114),
            pending_mode: Some(FanMode::Fixed),
            pending_fixed_raw: Some(79),
            ..Default::default()
        };
        let profile = state.complete(Some(true)).unwrap();
        assert_eq!(profile.fan_mode, FanMode::Fixed);
        assert_eq!(profile.fixed_duty_raw, Some(79));
        assert!(profile.expert_mode);
        assert!(crate::profile_store::validate_profile(&profile).is_ok());
        assert!(state.complete(Some(true)).is_none());
    }

    #[test]
    fn max_test_completion_updates_the_display_without_replacing_the_preference() {
        let mut state = ApplyState {
            active_mode: Some(FanMode::Gaming),
            pending_mode: Some(FanMode::Normal),
            pending_temporary: true,
            ..Default::default()
        };
        assert!(state.complete(None).is_none());
        assert!(state.pending_temporary);
        assert!(state.complete(Some(true)).is_none());
        assert_eq!(state.active_mode, Some(FanMode::Normal));
        assert!(!state.pending_temporary);
        state.pending_mode = Some(FanMode::Quiet);
        assert_eq!(state.complete(Some(true)).unwrap().fan_mode, FanMode::Quiet);
    }

    #[test]
    fn a_readback_without_a_local_command_never_becomes_a_saved_preference() {
        let mut state = ApplyState::from_readback(&crate::fan_state::RecognizedFanState {
            mode: FanMode::Normal,
            fixed_raw: None,
            note: None,
        });
        assert!(state.complete(None).is_none());
        assert!(state.complete(Some(true)).is_none());
        state.active_mode = Some(FanMode::Custom);
        assert!(state.fan_profile("Unknown curve".into()).is_none());
    }
}
