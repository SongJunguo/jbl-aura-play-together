//! Exact, clean-room protocol routing for the supported device pair.
//!
//! Routing is deliberately based on the complete proven product descriptor.
//! A broad category such as `home_bt` is never sufficient by itself.

/// The protocol families proven for the exact supported devices.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeviceProtocolRoute {
    /// JBL OneOS control used by the Authentics 300.
    OneOs,
    /// Harman V4 advertisement and legacy AA command family.
    V4Aa,
}

/// The complete Bluetooth product tuple used for protocol routing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BluetoothProductDescriptor<'a> {
    pub pid: u16,
    pub category: &'a str,
    pub advertisement_format: &'a str,
}

const AUTHENTICS_300_MODEL: &str = "JBL Authentics 300";
const AURA_STUDIO_5_PID: u16 = 0x212d;
const HOME_BT_CATEGORY: &str = "home_bt";
const ADV_FORMAT_4: &str = "adv_format_4";

/// Selects the OneOS route only for the exact supported JBL model.
pub const fn route_jbl_model(model: &str) -> Option<DeviceProtocolRoute> {
    if const_str_eq(model, AUTHENTICS_300_MODEL) {
        Some(DeviceProtocolRoute::OneOs)
    } else {
        None
    }
}

/// Selects the V4/AA route only for the complete Aura Studio 5 tuple.
///
/// In particular, `category == "home_bt"` does not imply a V5 device.
pub const fn route_bluetooth_product(
    product: BluetoothProductDescriptor<'_>,
) -> Option<DeviceProtocolRoute> {
    if product.pid == AURA_STUDIO_5_PID
        && const_str_eq(product.category, HOME_BT_CATEGORY)
        && const_str_eq(product.advertisement_format, ADV_FORMAT_4)
    {
        Some(DeviceProtocolRoute::V4Aa)
    } else {
        None
    }
}

const fn const_str_eq(left: &str, right: &str) -> bool {
    let left = left.as_bytes();
    let right = right.as_bytes();
    if left.len() != right.len() {
        return false;
    }
    let mut index = 0;
    while index < left.len() {
        if left[index] != right[index] {
            return false;
        }
        index += 1;
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    fn aura_descriptor() -> BluetoothProductDescriptor<'static> {
        BluetoothProductDescriptor {
            pid: AURA_STUDIO_5_PID,
            category: HOME_BT_CATEGORY,
            advertisement_format: ADV_FORMAT_4,
        }
    }

    #[test]
    fn exact_authentics_300_routes_to_one_os() {
        assert_eq!(
            route_jbl_model("JBL Authentics 300"),
            Some(DeviceProtocolRoute::OneOs)
        );
    }

    #[test]
    fn jbl_model_routing_has_no_name_or_whitespace_heuristic() {
        for model in [
            "JBL Authentics 200",
            "JBL Authentics 500",
            "jbl authentics 300",
            "JBL Authentics 300 ",
            "Authentics 300",
            "",
        ] {
            assert_eq!(route_jbl_model(model), None, "unexpected route for {model}");
        }
    }

    #[test]
    fn exact_aura_tuple_routes_to_v4_aa() {
        assert_eq!(
            route_bluetooth_product(aura_descriptor()),
            Some(DeviceProtocolRoute::V4Aa)
        );
    }

    #[test]
    fn every_aura_tuple_component_is_required() {
        let cases = [
            BluetoothProductDescriptor {
                pid: 0x212c,
                ..aura_descriptor()
            },
            BluetoothProductDescriptor {
                category: "portable",
                ..aura_descriptor()
            },
            BluetoothProductDescriptor {
                advertisement_format: "adv_format_3",
                ..aura_descriptor()
            },
        ];
        for product in cases {
            assert_eq!(route_bluetooth_product(product), None);
        }
    }

    #[test]
    fn home_bt_category_alone_never_selects_a_protocol() {
        for product in [
            BluetoothProductDescriptor {
                pid: 0,
                category: HOME_BT_CATEGORY,
                advertisement_format: "",
            },
            BluetoothProductDescriptor {
                pid: 0xffff,
                category: HOME_BT_CATEGORY,
                advertisement_format: "adv_format_5",
            },
        ] {
            assert_eq!(route_bluetooth_product(product), None);
        }
    }

    #[test]
    fn tuple_strings_are_exact_and_case_sensitive() {
        for product in [
            BluetoothProductDescriptor {
                category: "Home_Bt",
                ..aura_descriptor()
            },
            BluetoothProductDescriptor {
                advertisement_format: "ADV_FORMAT_4",
                ..aura_descriptor()
            },
            BluetoothProductDescriptor {
                category: "home_bt ",
                ..aura_descriptor()
            },
        ] {
            assert_eq!(route_bluetooth_product(product), None);
        }
    }
}
