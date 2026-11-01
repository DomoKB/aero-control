//! Confirmed fan state and the request currently waiting for verification.

use aero_core::{CurvePoint, FanMode};

#[derive(Default)]
pub(super) struct ApplyState {
    pub active_mode: Option<FanMode>,
    pub pending_mode: Option<FanMode>,
    pub applied_curve: Vec<CurvePoint>,
    pub pending_curve: Option<Vec<CurvePoint>>,
    pub applied_fixed_raw: Option<u8>,
    pub pending_fixed_raw: Option<u8>,
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
    pub fn complete(&mut self, result: Option<bool>) {
        let Some(succeeded) = result else { return };
        let Some(mode) = self.pending_mode.take() else {
            return;
        };
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
        state.complete(None);
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
        state.complete(Some(true));
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
        state.complete(Some(false));
        assert!(state.active_mode.is_none());
        assert!(state.pending_mode.is_none());
        assert!(state.pending_curve.is_none());
        assert!(state.applied_curve.is_empty());
    }
}
