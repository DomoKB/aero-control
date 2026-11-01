//! Detected identity is separate from the command set supported by this build.

use aero_wmi::SkuInfo;

pub fn read(mock: bool) -> SkuInfo {
    #[cfg(windows)]
    if !mock {
        return aero_wmi::read_system_sku();
    }
    let _ = mock;
    use aero_wmi::SkuSource;
    aero_wmi::MockAcpi::aero_17_yb_fixture()
        .read_sku()
        .expect("the in-memory identity fixture is available")
}

fn useful(value: Option<&str>) -> Option<&str> {
    value.map(str::trim).filter(|value| {
        !value.is_empty()
            && !value.chars().any(char::is_control)
            && !matches!(
                value.to_ascii_uppercase().as_str(),
                "DEFAULT STRING" | "TO BE FILLED BY O.E.M." | "SYSTEM PRODUCT NAME" | "UNKNOWN"
            )
    })
}

pub fn model_name(identity: &SkuInfo) -> &str {
    useful(identity.model.as_deref())
        .or_else(|| useful(identity.system_sku.as_deref()))
        .unwrap_or("Unknown model")
}

pub fn has_yb_command_set(identity: &SkuInfo) -> bool {
    [identity.model.as_deref(), identity.system_sku.as_deref()]
        .into_iter()
        .filter_map(useful)
        .map(|value| {
            value
                .chars()
                .filter(|ch| !ch.is_ascii_whitespace() && *ch != '-')
                .collect::<String>()
                .to_ascii_uppercase()
        })
        .any(|value| matches!(value.as_str(), "AERO17YB" | "P77YB"))
}

pub fn support_note(identity: &SkuInfo) -> String {
    let model = model_name(identity);
    if has_yb_command_set(identity) {
        format!("Fan and firmware power commands are available for {model}. Physical behaviour still needs hardware validation.")
    } else {
        format!("No verified fan or firmware power command set is available for {model}. Those writes are disabled; Windows system controls remain available.")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn other_models_keep_their_name_without_inheriting_yb_commands() {
        let mut identity = read(true);
        identity.model = Some("AERO 15 XE4".into());
        identity.system_sku = Some("P65XE4".into());
        assert_eq!(model_name(&identity), "AERO 15 XE4");
        assert!(support_note(&identity).contains("AERO 15 XE4"));
        assert!(!support_note(&identity).contains("AERO 17 YB"));
        assert!(!has_yb_command_set(&identity));
    }

    #[test]
    fn known_model_and_sku_are_recognized_exactly() {
        let mut identity = read(true);
        assert!(has_yb_command_set(&identity));
        identity.model = Some("AERO".into());
        identity.system_sku = Some("P77YB".into());
        assert!(has_yb_command_set(&identity));
        identity.system_sku = Some("P77YB-other".into());
        assert!(!has_yb_command_set(&identity));
    }

    #[test]
    fn missing_identity_is_not_labelled_as_the_target_laptop() {
        let mut identity = read(true);
        identity.model = Some("To Be Filled By O.E.M.".into());
        identity.system_sku = None;
        assert_eq!(model_name(&identity), "Unknown model");
        assert!(support_note(&identity).contains("Unknown model"));
        assert!(!has_yb_command_set(&identity));
    }
}
