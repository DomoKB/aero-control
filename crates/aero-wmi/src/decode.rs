use serde::Serialize;

use crate::value::WmiOutput;

/// Byte-swap the low 16 bits. Matches `rol16` in `gigabyte-laptop-wmi`
/// (`convert_fan_rpm`), which undoes the embedded controller's big-endian tach.
pub fn swap_u16(value: u32) -> u32 {
    let low = (value & 0xffff) as u16;
    u32::from(low.rotate_left(8))
}

/// Which RPM interpretation to prefer when both byte orders look plausible.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum RpmOrder {
    /// Use whichever order falls in a real fan range. When both do, prefer the
    /// swapped value. That matches `gigabyte-laptop-wmi` for family strings
    /// other than `GIGABYTE GAMING`, which is the right default for an
    /// AERO 17 YB. Newer AERO boards (see that driver's issue 22) already
    /// return native order; pass [`RpmOrder::AutoPreferNative`] for those.
    #[default]
    AutoPreferSwap,
    AutoPreferNative,
    ForceSwap,
    ForceNative,
}

impl RpmOrder {
    pub fn parse(text: &str) -> Option<Self> {
        match text {
            "auto-swap" | "auto" => Some(Self::AutoPreferSwap),
            "auto-native" => Some(Self::AutoPreferNative),
            "swap" => Some(Self::ForceSwap),
            "native" => Some(Self::ForceNative),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RpmReading {
    pub raw: u32,
    pub swapped: u32,
    pub rpm: u32,
    pub ambiguous: bool,
    pub chose: &'static str,
}

/// Fans on this chassis sit well below 12_000 RPM. Values above that are the
/// usual signature of a byte-swapped tach (for example 2970 swapped is 39435).
const PLAUSIBLE_MAX: u32 = 12_000;

pub fn decode_rpm(raw: u32, order: RpmOrder) -> RpmReading {
    let swapped = swap_u16(raw);
    let raw_ok = raw <= PLAUSIBLE_MAX;
    let swap_ok = swapped <= PLAUSIBLE_MAX;
    let (rpm, ambiguous, chose) = match order {
        RpmOrder::ForceSwap => (swapped, false, "force-swap"),
        RpmOrder::ForceNative => (raw, false, "force-native"),
        RpmOrder::AutoPreferSwap | RpmOrder::AutoPreferNative => {
            let prefer_swap = matches!(order, RpmOrder::AutoPreferSwap);
            match (raw_ok, swap_ok) {
                (true, false) => (raw, false, "native-only-plausible"),
                (false, true) => (swapped, false, "swapped-only-plausible"),
                (true, true) if raw == swapped => (raw, false, "identical"),
                (true, true) if prefer_swap => (swapped, true, "ambiguous-prefer-swap"),
                (true, true) => (raw, true, "ambiguous-prefer-native"),
                (false, false) if prefer_swap => (swapped, true, "neither-plausible-prefer-swap"),
                (false, false) => (raw, true, "neither-plausible-prefer-native"),
            }
        }
    };
    RpmReading {
        raw,
        swapped,
        rpm,
        ambiguous,
        chose,
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct FanIndexReading {
    pub index: u8,
    pub temperature_c: Option<u8>,
    pub duty_raw: Option<u8>,
    /// `separate` when both out-params were present. `packed-duty-high` is the
    /// `aorus-laptop.c` GET layout (low byte temperature, high byte speed),
    /// used only when the firmware returns one integer instead of the two
    /// MOF out-params. `empty` means the call returned no usable number.
    pub encoding: &'static str,
    pub raw_properties: WmiOutput,
}

/// Decode `GetFanIndexValue`.
///
/// The Windows MOF (Aero 16 dump, lm-sensors issue 400) declares
/// `[out] uint8 Temperture` and `[out] uint8 Value`. Those win when both are
/// present. A single integer is decoded as the Linux driver's packing
/// (`temperature = low byte`, `speed = high byte`), not as `temp * 256 + duty`.
/// The scope mentioned the latter; the driver source does the former.
pub fn decode_fan_index(index: u8, output: &WmiOutput) -> FanIndexReading {
    let temp = output
        .first(&["Temperture", "Temperature", "Temp"])
        .and_then(crate::WmiNumber::as_u8);
    let duty = output
        .first(&["Value", "Speed", "Duty"])
        .and_then(crate::WmiNumber::as_u8);
    if let (Some(temperature_c), Some(duty_raw)) = (temp, duty) {
        return FanIndexReading {
            index,
            temperature_c: Some(temperature_c),
            duty_raw: Some(duty_raw),
            encoding: "separate",
            raw_properties: output.clone(),
        };
    }

    if let Some(packed) = output
        .first(&["Data", "ReturnValue"])
        .and_then(crate::WmiNumber::as_u32)
    {
        let temperature_c = (packed & 0xff) as u8;
        let duty_raw = ((packed >> 8) & 0xff) as u8;
        return FanIndexReading {
            index,
            temperature_c: Some(temperature_c),
            duty_raw: Some(duty_raw),
            encoding: "packed-duty-high",
            raw_properties: output.clone(),
        };
    }

    FanIndexReading {
        index,
        temperature_c: temp,
        duty_raw: duty,
        encoding: "empty",
        raw_properties: output.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::WmiNumber;

    #[test]
    fn rpm_keeps_a_native_value_whose_swap_is_impossible() {
        // 2970 RPM is 0x0B9A. Swapping it yields 39435, which is not a fan speed.
        // This is the Aero X16 counter-example from gigabyte-laptop-wmi issue 22.
        let reading = decode_rpm(2970, RpmOrder::AutoPreferSwap);
        assert_eq!(reading.rpm, 2970);
        assert_eq!(reading.swapped, 39435);
        assert!(!reading.ambiguous);
        assert_eq!(reading.chose, "native-only-plausible");
    }

    #[test]
    fn rpm_swaps_when_only_the_swapped_value_is_plausible() {
        let swapped_form = swap_u16(2970);
        assert_eq!(swapped_form, 39435);
        let reading = decode_rpm(swapped_form, RpmOrder::AutoPreferSwap);
        assert_eq!(reading.rpm, 2970);
        assert_eq!(reading.chose, "swapped-only-plausible");
    }

    #[test]
    fn rpm_zero_stays_zero() {
        let reading = decode_rpm(0, RpmOrder::AutoPreferSwap);
        assert_eq!(reading.rpm, 0);
        assert!(!reading.ambiguous);
    }

    #[test]
    fn ambiguous_rpm_follows_the_requested_preference() {
        // 0x0C1A = 3098, swap 0x1A0C = 6668. Both are plausible tach readings.
        let raw = 0x0c1a;
        let prefer_swap = decode_rpm(raw, RpmOrder::AutoPreferSwap);
        assert_eq!(prefer_swap.rpm, swap_u16(raw));
        assert!(prefer_swap.ambiguous);
        let prefer_native = decode_rpm(raw, RpmOrder::AutoPreferNative);
        assert_eq!(prefer_native.rpm, raw);
        assert!(prefer_native.ambiguous);
    }

    #[test]
    fn separate_fan_index_params_win_over_a_packed_integer() {
        let mut output = WmiOutput::new();
        output.insert("Temperture", WmiNumber::U8(70));
        output.insert("Value", WmiNumber::U8(183));
        output.insert("Data", WmiNumber::U16(0x1234));
        let reading = decode_fan_index(3, &output);
        assert_eq!(reading.temperature_c, Some(70));
        assert_eq!(reading.duty_raw, Some(183));
        assert_eq!(reading.encoding, "separate");
    }

    #[test]
    fn packed_fan_index_uses_driver_byte_order() {
        // aorus-laptop.c GET: temperature = low byte, speed = high byte.
        // 0xB746 => temp 0x46 = 70, duty 0xB7 = 183.
        let mut output = WmiOutput::new();
        output.insert("Data", WmiNumber::U16(0xb746));
        let reading = decode_fan_index(0, &output);
        assert_eq!(reading.temperature_c, Some(70));
        assert_eq!(reading.duty_raw, Some(183));
        assert_eq!(reading.encoding, "packed-duty-high");
    }
}
