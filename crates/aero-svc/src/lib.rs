//! Service-side command dispatch.
//!
//! The Windows service host (`windows-service`), the named pipe, and the
//! power-broadcast window are not wired. [`dispatch`] is the piece those will
//! call: it checks `protocol_version` and runs the same apply engine as the GUI.
//! Fan commands still go to the firmware. Nothing here polls temperatures to
//! pick a duty.

use aero_core::{
    apply_mode, apply_profile, restore_firmware_auto, upload_curve, ApplyError, ApplyReason,
    Config, FanCurve, FanMode,
};
use aero_proto::{Request, RequestBody, Response, PROTOCOL_VERSION};
use aero_wmi::{AcpiWriter, Pacer};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum ServiceError {
    #[error("the named-pipe service host is not implemented yet (milestone M6)")]
    PipeNotImplemented,
}

/// Placeholder for the LocalSystem pipe server. It does not listen.
pub fn serve_named_pipe() -> Result<(), ServiceError> {
    Err(ServiceError::PipeNotImplemented)
}

pub fn dispatch<W: AcpiWriter, P: Pacer>(
    writer: &mut W,
    pacer: &mut P,
    config: &Config,
    request: &Request,
) -> Response {
    if request.protocol_version != PROTOCOL_VERSION {
        return Response::failure(format!(
            "protocol {} is not supported (this service speaks {PROTOCOL_VERSION})",
            request.protocol_version
        ));
    }
    match &request.body {
        RequestBody::GetStatus => {
            Response::success("status reads belong to the GUI; the service does not stream sensors")
        }
        RequestBody::ApplyProfile { name } => match config.profile(name) {
            Some(profile) => match apply_profile(writer, pacer, profile, ApplyReason::User) {
                Ok(report) => {
                    Response::success(format!("applied {name} in {} attempt(s)", report.attempts))
                }
                Err(err) => Response::failure(err.to_string()),
            },
            None => Response::failure(format!("unknown profile {name}")),
        },
        RequestBody::SetFanMode {
            mode,
            fixed_duty_raw,
        } => {
            if *mode == FanMode::Custom {
                return Response::failure("custom mode is UploadCurve");
            }
            match apply_mode(writer, pacer, *mode, fixed_duty_raw.unwrap_or(0), false) {
                Ok(_) => Response::success(format!("fan mode {mode:?}")),
                Err(err) => Response::failure(err.to_string()),
            }
        }
        RequestBody::UploadCurve { points } => match FanCurve::try_new(points.clone()) {
            Ok(curve) => match upload_curve(writer, pacer, &curve) {
                Ok(report) => Response::success(format!(
                    "uploaded {} points in {} attempt(s)",
                    points.len(),
                    report.attempts
                )),
                Err(err) => Response::failure(err.to_string()),
            },
            Err(issues) => Response::failure(format!(
                "curve rejected: {}",
                issues
                    .iter()
                    .map(|issue| issue.message.as_str())
                    .collect::<Vec<_>>()
                    .join("; ")
            )),
        },
        RequestBody::RestoreAuto => match restore_firmware_auto(writer, pacer) {
            Ok(()) => Response::success("firmware automatic mode restored"),
            Err(err) => Response::failure(err.to_string()),
        },
    }
}

/// `ApplyError` is named so a mismatched service build fails here, not at the pipe.
#[allow(dead_code)]
fn _error_is_shared(err: ApplyError) -> String {
    err.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use aero_proto::Request;
    use aero_wmi::{MockAcpi, NoPacer};

    #[test]
    fn a_wrong_protocol_version_does_not_touch_the_ec() {
        let mut mock = MockAcpi::aero_17_yb_fixture();
        let mut request = Request::new(aero_proto::RequestBody::RestoreAuto);
        request.protocol_version = 99;
        let response = dispatch(
            &mut mock,
            &mut NoPacer,
            &aero_core::builtin_config(),
            &request,
        );
        assert!(!response.ok);
        assert!(mock.calls.is_empty());
    }

    #[test]
    fn dispatch_uploads_a_curve_through_the_mock() {
        let mut mock = MockAcpi::aero_17_yb_fixture();
        let curve = aero_core::curve_from_percent(&[(85, 50), (90, 100)]);
        let request = Request::new(aero_proto::RequestBody::UploadCurve {
            points: curve.points().to_vec(),
        });
        let response = dispatch(
            &mut mock,
            &mut NoPacer,
            &aero_core::builtin_config(),
            &request,
        );
        assert!(response.ok, "{}", response.message);
        assert_eq!(mock.calls_named("SetFanIndexValue").count(), 15);
    }

    #[test]
    fn the_pipe_server_is_still_a_stub() {
        assert!(serve_named_pipe().is_err());
    }
}
