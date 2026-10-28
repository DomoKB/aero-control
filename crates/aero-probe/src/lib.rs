//! Getter-only capability dump.
//!
//! This package depends on `aero-wmi` with the `writer` feature disabled, so
//! the setter trait is not compiled in. [`run_probe`] also refuses to call a
//! name that looks like a setter. It never writes a fan duty.

use std::collections::BTreeMap;

use aero_wmi::{
    decode_fan_index, decode_rpm, is_setter_name, AcpiDiag, AcpiReader, ClassStatus,
    FanIndexReading, MethodSignature, MethodSpec, Pacer, RpmOrder, RpmReading, SkuInfo, SkuSource,
    WmiNumber, WmiOutput, FAN_CURVE_SLOTS, GETTERS, GET_CLASS, SETTERS, SET_CLASS,
};
use serde::Serialize;

#[derive(Debug, Clone, Serialize)]
pub struct ProbeReport {
    pub setters_invoked: usize,
    pub mof_get: ClassReport,
    pub mof_set: ClassReport,
    pub sku: SkuReport,
    pub getters: Vec<GetterCall>,
    pub fan_flags: BTreeMap<String, Option<u32>>,
    pub rpm1: Option<RpmReading>,
    pub rpm2: Option<RpmReading>,
    pub cpu_temp_c: Option<u32>,
    pub gpu_temp_c: Option<u32>,
    pub gpu_temp2_c: Option<u32>,
    pub curve: Vec<FanIndexReading>,
    pub setters_not_called: Vec<&'static str>,
    /// Populated by `--diag`. Signatures only; setters are not executed.
    pub diag: Vec<MethodSignature>,
    pub note: &'static str,
}

#[derive(Debug, Clone, Serialize)]
pub struct ClassReport {
    pub name: &'static str,
    pub present: bool,
    pub detail: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct SkuReport {
    pub info: Option<SkuInfo>,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct GetterCall {
    pub method: String,
    pub params: Vec<(String, u8)>,
    pub error: Option<String>,
    pub properties: BTreeMap<String, WmiNumber>,
}

pub fn run_probe<R, P>(
    reader: &mut R,
    pacer: &mut P,
    rpm_order: RpmOrder,
    diag: bool,
) -> ProbeReport
where
    R: AcpiReader + SkuSource + AcpiDiag,
    P: Pacer,
{
    let mof_get = class_report(reader, GET_CLASS);
    let mof_set = class_report(reader, SET_CLASS);
    let sku = match reader.read_sku() {
        Ok(info) => SkuReport {
            info: Some(info),
            error: None,
        },
        Err(err) => SkuReport {
            info: None,
            error: Some(err.to_string()),
        },
    };

    let mut signatures = Vec::new();
    if diag {
        for spec in GETTERS {
            signatures.push(read_signature(reader, GET_CLASS, spec.name));
        }
        for spec in SETTERS {
            signatures.push(read_signature(reader, SET_CLASS, spec.name));
        }
    }

    let mut getters = Vec::new();
    let mut started = false;
    for spec in GETTERS {
        if spec.name == "GetFanIndexValue" {
            for index in 0..FAN_CURVE_SLOTS {
                call_getter(reader, pacer, &mut started, spec, index as u8, &mut getters);
            }
        } else {
            call_getter(reader, pacer, &mut started, spec, 0, &mut getters);
        }
    }

    let setters_invoked = getters
        .iter()
        .filter(|call| is_setter_name(&call.method))
        .count();

    ProbeReport {
        setters_invoked,
        mof_get,
        mof_set,
        sku,
        rpm1: number_of(&getters, "getRpm1").map(|raw| decode_rpm(raw, rpm_order)),
        rpm2: number_of(&getters, "getRpm2").map(|raw| decode_rpm(raw, rpm_order)),
        cpu_temp_c: number_of(&getters, "getCpuTemp"),
        gpu_temp_c: number_of(&getters, "getGpuTemp1"),
        gpu_temp2_c: number_of(&getters, "getGpuTemp2"),
        fan_flags: fan_flags(&getters),
        curve: curve_from(&getters),
        getters,
        setters_not_called: SETTERS.iter().map(|spec| spec.name).collect(),
        diag: signatures,
        note: "getter only; calls use the instance path and NULL in-params when the signature has none",
    }
}

fn read_signature<R: AcpiDiag>(reader: &mut R, class: &str, method: &str) -> MethodSignature {
    match reader.method_signature(class, method) {
        Ok(signature) => signature,
        Err(err) => MethodSignature {
            class: class.to_string(),
            method: method.to_string(),
            instance_path: String::new(),
            in_params: Vec::new(),
            out_params: vec![aero_wmi::ParamField {
                name: format!("error: {err}"),
                cim_type: 0,
                cim_type_name: "empty".to_string(),
            }],
        },
    }
}

fn class_report<R: AcpiReader>(reader: &mut R, class: &'static str) -> ClassReport {
    match reader.class_status(class) {
        Ok(ClassStatus::Present) => ClassReport {
            name: class,
            present: true,
            detail: None,
        },
        Ok(ClassStatus::Missing { detail }) => ClassReport {
            name: class,
            present: false,
            detail: Some(detail),
        },
        Err(err) => ClassReport {
            name: class,
            present: false,
            detail: Some(err.to_string()),
        },
    }
}

fn call_getter<R: AcpiReader, P: Pacer>(
    reader: &mut R,
    pacer: &mut P,
    started: &mut bool,
    spec: &MethodSpec,
    index: u8,
    out: &mut Vec<GetterCall>,
) {
    if is_setter_name(spec.name) {
        out.push(GetterCall {
            method: spec.name.to_string(),
            params: Vec::new(),
            error: Some("refused: setter".to_string()),
            properties: BTreeMap::new(),
        });
        return;
    }
    if *started {
        pacer.pause();
    }
    *started = true;
    let params: Vec<(&str, u8)> = spec
        .inputs
        .iter()
        .map(|name| (*name, if *name == "Index" { index } else { 0 }))
        .collect();
    let recorded = params
        .iter()
        .map(|(name, value)| ((*name).to_string(), *value))
        .collect();
    match reader.get(spec.name, &params) {
        Ok(output) => out.push(GetterCall {
            method: spec.name.to_string(),
            params: recorded,
            error: None,
            properties: output.properties,
        }),
        Err(err) => out.push(GetterCall {
            method: spec.name.to_string(),
            params: recorded,
            error: Some(err.to_string()),
            properties: BTreeMap::new(),
        }),
    }
}

fn number_of(calls: &[GetterCall], method: &str) -> Option<u32> {
    let call = calls
        .iter()
        .find(|call| call.method == method && call.error.is_none())?;
    call.properties.get("Data").and_then(|value| value.as_u32())
}

fn fan_flags(calls: &[GetterCall]) -> BTreeMap<String, Option<u32>> {
    [
        "GetAutoFanStatus",
        "GetFixedFanStatus",
        "GetStepFanStatus",
        "GetFanAdjustStatus",
        "GetSuperQuiet",
        "GetFanCustomStatus",
        "GetFixedFanSpeed",
        "GetCPUFanDuty",
        "GetGPUFanDuty",
    ]
    .into_iter()
    .map(|name| (name.to_string(), number_of(calls, name)))
    .collect()
}

fn curve_from(calls: &[GetterCall]) -> Vec<FanIndexReading> {
    calls
        .iter()
        .filter(|call| call.method == "GetFanIndexValue")
        .enumerate()
        .map(|(index, call)| {
            let mut output = WmiOutput::new();
            for (name, value) in &call.properties {
                output.insert(name, *value);
            }
            decode_fan_index(index as u8, &output)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use aero_wmi::{MockAcpi, NoPacer};

    #[test]
    fn probe_never_calls_a_setter_and_decodes_swapped_rpm() {
        let mut mock = MockAcpi::aero_17_yb_fixture();
        let report = run_probe(&mut mock, &mut NoPacer, RpmOrder::AutoPreferSwap, false);
        assert!(report.diag.is_empty());
        assert_eq!(report.setters_invoked, 0);
        assert!(mock.calls.iter().all(|call| !is_setter_name(&call.method)));
        assert!(mock.calls.iter().all(|call| call.class == GET_CLASS));
        let expected = GETTERS
            .iter()
            .map(|spec| {
                if spec.name == "GetFanIndexValue" {
                    15
                } else {
                    1
                }
            })
            .sum::<usize>();
        assert_eq!(mock.calls.len(), expected);
        assert_eq!(report.rpm1.as_ref().unwrap().rpm, 3200);
        assert_eq!(report.rpm2.as_ref().unwrap().rpm, 2800);
        assert_eq!(report.cpu_temp_c, Some(61));
        assert_eq!(report.curve[0].temperature_c, Some(45));
        assert_eq!(report.curve[0].duty_raw, Some(80));
        assert_eq!(report.curve[5].temperature_c, Some(0));
        assert_eq!(report.curve[5].duty_raw, Some(0));
        assert!(report.mof_get.present);
        assert!(report.setters_not_called.contains(&"TurnOffFan"));
        assert!(report.setters_not_called.contains(&"SetFanIndexValue"));
        assert_eq!(
            report.sku.info.unwrap().model.as_deref(),
            Some("AERO 17 YB")
        );
        assert!(report.fan_flags.get("GetAutoFanStatus").unwrap().is_some());
    }

    #[test]
    fn diag_lists_signatures_without_calling_setters() {
        let mut mock = MockAcpi::aero_17_yb_fixture();
        let report = run_probe(&mut mock, &mut NoPacer, RpmOrder::AutoPreferSwap, true);
        assert_eq!(report.setters_invoked, 0);
        assert!(mock.calls.iter().all(|call| !is_setter_name(&call.method)));
        let cpu = report
            .diag
            .iter()
            .find(|sig| sig.method == "getCpuTemp")
            .unwrap();
        assert!(cpu.in_params.is_empty());
        assert!(cpu.instance_path.contains("mock:"));
        let index = report
            .diag
            .iter()
            .find(|sig| sig.method == "GetFanIndexValue")
            .unwrap();
        assert_eq!(index.in_params[0].name, "Index");
        assert!(report
            .diag
            .iter()
            .any(|sig| sig.method == "SetFanIndexValue"));
    }
}
