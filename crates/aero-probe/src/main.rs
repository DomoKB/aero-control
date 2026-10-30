use std::env;
use std::process::ExitCode;

use aero_probe::run_probe;
#[cfg(windows)]
use aero_wmi::FixedPacer;
use aero_wmi::{MockAcpi, NoPacer, RpmOrder, SkuSource};

fn main() -> ExitCode {
    let mut mock = false;
    let mut pace = true;
    let mut diag = false;
    let mut rpm_order = RpmOrder::AutoPreferSwap;
    let mut args = env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--mock" => mock = true,
            "--diag" => diag = true,
            "--no-pace" => pace = false,
            "--rpm-order" => {
                let Some(value) = args.next() else {
                    eprintln!("--rpm-order needs auto-swap, auto-native, swap, or native");
                    return ExitCode::from(2);
                };
                let Some(order) = RpmOrder::parse(&value) else {
                    eprintln!("unknown --rpm-order {value}");
                    return ExitCode::from(2);
                };
                rpm_order = order;
            }
            "--help" | "-h" => {
                print_help();
                return ExitCode::SUCCESS;
            }
            other => {
                eprintln!("unknown argument {other}");
                print_help();
                return ExitCode::from(2);
            }
        }
    }

    if mock {
        let mut device = MockAcpi::aero_17_yb_fixture();
        let _ = device.read_sku();
        let report = run_probe(&mut device, &mut NoPacer, rpm_order, diag);
        return finish(report);
    }

    #[cfg(windows)]
    {
        let mut reader = match aero_wmi::WindowsReader::connect() {
            Ok(reader) => reader,
            Err(err) => {
                eprintln!("could not open the GB_WMIACPI_Get firmware block: {err}");
                eprintln!("run this from an elevated PowerShell, or pass --mock");
                return ExitCode::from(1);
            }
        };
        if pace {
            let report = run_probe(&mut reader, &mut FixedPacer::standard(), rpm_order, diag);
            return finish(report);
        }
        let report = run_probe(&mut reader, &mut NoPacer, rpm_order, diag);
        return finish(report);
    }

    #[cfg(not(windows))]
    {
        let _ = (pace, diag);
        eprintln!("this build has no Windows WMI backend.");
        eprintln!("pass --mock to dump the AERO 17 YB fixture, or build aero-probe on Windows.");
        ExitCode::from(2)
    }
}

fn finish(report: aero_probe::ProbeReport) -> ExitCode {
    let missing = !report.mof_get.present;
    let failed = report
        .getters
        .iter()
        .filter(|call| call.error.is_some())
        .count();
    eprintln!(
        "aero-probe: setters_invoked={} getters={} failed={} mof_get={}",
        report.setters_invoked,
        report.getters.len(),
        failed,
        report.mof_get.present
    );
    match serde_json::to_string_pretty(&report) {
        Ok(json) => println!("{json}"),
        Err(err) => {
            eprintln!("could not encode the report: {err}");
            return ExitCode::from(1);
        }
    }
    if report.setters_invoked != 0 {
        eprintln!("internal error: the probe invoked a setter");
        return ExitCode::from(1);
    }
    if missing {
        eprintln!("GB_WMIACPI_Get firmware block was not found on this model");
        return ExitCode::from(4);
    }
    ExitCode::SUCCESS
}

fn print_help() {
    eprintln!(
        "aero-probe — getter-only GB_WMIACPI dump\n\
         \n\
         aero-probe [--mock] [--diag] [--no-pace] [--rpm-order auto-swap|auto-native|swap|native]\n\
         \n\
         On Windows, run from an elevated shell. The probe never calls a setter.\n\
         --diag adds each method's in/out layout and the instance name.\n\
         Real hardware calls are spaced 500 ms apart unless --no-pace is set.\n\
         --mock works on Linux and does not touch WMI."
    );
}
