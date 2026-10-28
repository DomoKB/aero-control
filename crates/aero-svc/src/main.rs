fn main() {
    eprintln!(
        "aero-svc does not install a Windows service yet.\n\
         Fan applies already go through the firmware command sequences in aero-core.\n\
         The named pipe and the service host are milestone M6.\n\
         Until then, run aero-gui (mock) or aero-probe."
    );
    std::process::exit(2);
}
