//! Closed Authentics 300 seven-band preset EQ model.

use serde::Serialize;
use serde_json::{Number, Value};

use crate::error::JblError;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum EqPresetTarget {
    Signature,
    Vocal,
    Energetic,
    Chill,
}

impl EqPresetTarget {
    fn name(self) -> &'static str {
        match self {
            Self::Signature => "JBL SIGNATURE",
            Self::Vocal => "VOCAL",
            Self::Energetic => "ENERGETIC",
            Self::Chill => "CHILL",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EqPresetWriteResult {
    AlreadyAtTarget(EqPresetTarget),
    Applied(EqPresetTarget),
    RejectedByDevice(EqPresetTarget),
    TargetObservedAfterUnknownWrite(EqPresetTarget),
    PostconditionFailed(Option<EqPresetTarget>),
    RejectedBeforeSend(JblError),
    OutcomeUnknown(JblError),
}

#[derive(Clone)]
pub(crate) struct EqCatalog {
    active: Option<EqPresetTarget>,
    entries: Vec<EqEntry>,
}

#[derive(Clone)]
struct EqEntry {
    target: Option<EqPresetTarget>,
    id: String,
    fs: Vec<Number>,
    gain: Vec<Number>,
}

#[derive(Serialize)]
struct ActiveEqPayload<'a> {
    active_eq_id: &'a str,
    band: u8,
    eq_payload: EqPayload<'a>,
}

#[derive(Serialize)]
struct EqPayload<'a> {
    gain: &'a [Number],
    fs: &'a [Number],
}

impl EqCatalog {
    pub(crate) fn active(&self) -> Option<EqPresetTarget> {
        self.active
    }

    pub(crate) fn mutation_body(&self, target: EqPresetTarget) -> Result<String, JblError> {
        let entry = self
            .entries
            .iter()
            .find(|entry| entry.target == Some(target))
            .ok_or(JblError::EqPresetInvalid)?;
        let payload = serde_json::to_string(&ActiveEqPayload {
            active_eq_id: &entry.id,
            band: 7,
            eq_payload: EqPayload {
                gain: &entry.gain,
                fs: &entry.fs,
            },
        })
        .map_err(|_| JblError::EqPresetInvalid)?;
        Ok(format!("command=setActiveEQ&payload={payload}"))
    }
}

pub(crate) fn parse_eq_feature(response: &Value) -> Result<(), JblError> {
    require_zero(response)?;
    let user_eq = response
        .get("feature_support")
        .and_then(|value| value.get("user_eq"))
        .and_then(Value::as_object)
        .ok_or(JblError::EqPresetInvalid)?;
    if user_eq.get("support").and_then(Value::as_str) != Some("true")
        || user_eq.get("band").and_then(Value::as_str) != Some("7")
        || user_eq.get("preset_support").and_then(Value::as_str) != Some("true")
    {
        return Err(JblError::EqPresetInvalid);
    }
    Ok(())
}

pub(crate) fn parse_eq_catalog(response: &Value) -> Result<EqCatalog, JblError> {
    require_zero(response)?;
    let active = response
        .get("active_eq_id")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty() && value.len() <= 64)
        .ok_or(JblError::EqPresetInvalid)?;
    let list = response
        .get("eq_list")
        .and_then(Value::as_array)
        .filter(|list| list.len() == 5)
        .ok_or(JblError::EqPresetInvalid)?;
    let mut entries = Vec::new();
    let mut seen_targets = Vec::new();
    let mut customize_count = 0_usize;
    for value in list {
        let object = value.as_object().ok_or(JblError::EqPresetInvalid)?;
        if object.get("band").and_then(Value::as_u64) != Some(7) {
            return Err(JblError::EqPresetInvalid);
        }
        let id = object
            .get("eq_id")
            .and_then(Value::as_str)
            .filter(|id| !id.is_empty() && id.len() <= 64 && id.is_ascii())
            .ok_or(JblError::EqPresetInvalid)?;
        let name = object
            .get("eq_name")
            .and_then(Value::as_str)
            .ok_or(JblError::EqPresetInvalid)?;
        let target = [
            EqPresetTarget::Signature,
            EqPresetTarget::Vocal,
            EqPresetTarget::Energetic,
            EqPresetTarget::Chill,
        ]
        .into_iter()
        .find(|target| target.name() == name);
        if name.len() > 64
            || (name == "CUSTOMIZE") != (id == "0")
            || (target.is_none() && name != "CUSTOMIZE")
            || entries.iter().any(|entry: &EqEntry| entry.id == id)
        {
            return Err(JblError::EqPresetInvalid);
        }
        if let Some(target) = target {
            if seen_targets.contains(&target) {
                return Err(JblError::EqPresetInvalid);
            }
            seen_targets.push(target);
        } else {
            customize_count += 1;
        }
        let payload = object
            .get("eq_payload")
            .and_then(Value::as_object)
            .ok_or(JblError::EqPresetInvalid)?;
        let numeric = |name: &str| -> Result<Vec<Number>, JblError> {
            payload
                .get(name)
                .and_then(Value::as_array)
                .filter(|values| values.len() == 7)
                .ok_or(JblError::EqPresetInvalid)?
                .iter()
                .map(|value| value.as_number().cloned().ok_or(JblError::EqPresetInvalid))
                .collect()
        };
        entries.push(EqEntry {
            target,
            id: id.to_string(),
            fs: numeric("fs")?,
            gain: numeric("gain")?,
        });
    }
    if seen_targets.len() != 4 || customize_count != 1 {
        return Err(JblError::EqPresetInvalid);
    }
    let active_entry = entries
        .iter()
        .find(|entry| entry.id == active)
        .ok_or(JblError::EqPresetInvalid)?;
    Ok(EqCatalog {
        active: active_entry.target,
        entries,
    })
}

fn require_zero(response: &Value) -> Result<(), JblError> {
    match response.get("error_code") {
        Some(Value::Number(code)) if code.as_i64() == Some(0) => Ok(()),
        Some(Value::String(code)) if code == "0" => Ok(()),
        _ => Err(JblError::DeviceReportedError),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn feature() -> Value {
        json!({"error_code":0,"feature_support":{"user_eq":{"support":"true","band":"7","preset_support":"true"}}})
    }

    fn catalog(active: &str) -> Value {
        let entry = |id: &str, name: &str, base: i64| {
            json!({
                "band":7,"eq_id":id,"eq_name":name,
                "eq_payload":{"fs":[1,2,3,4,5,6,7],"gain":[base,base,base,base,base,base,base]}
            })
        };
        json!({"error_code":0,"active_eq_id":active,"eq_list":[
            entry("1","JBL SIGNATURE",1), entry("2","VOCAL",2),
            entry("3","ENERGETIC",3), entry("4","CHILL",4),
            entry("0","CUSTOMIZE",0)
        ]})
    }

    #[test]
    fn exact_feature_and_catalog_produce_closed_ordered_body() {
        assert_eq!(parse_eq_feature(&feature()), Ok(()));
        let catalog = parse_eq_catalog(&catalog("1")).expect("catalog");
        assert_eq!(catalog.active(), Some(EqPresetTarget::Signature));
        assert_eq!(
            catalog.mutation_body(EqPresetTarget::Vocal).unwrap(),
            concat!(
                "command=setActiveEQ&payload={\"active_eq_id\":\"2\",\"band\":7,",
                "\"eq_payload\":{\"gain\":[2,2,2,2,2,2,2],\"fs\":[1,2,3,4,5,6,7]}}"
            )
        );
    }

    #[test]
    fn catalog_rejects_missing_duplicate_and_malformed_closed_entries() {
        let mut missing = catalog("1");
        missing["eq_list"].as_array_mut().unwrap().pop();
        assert_eq!(
            parse_eq_catalog(&missing).err(),
            Some(JblError::EqPresetInvalid)
        );
        let mut duplicate = catalog("1");
        duplicate["eq_list"][1]["eq_name"] = json!("JBL SIGNATURE");
        assert_eq!(
            parse_eq_catalog(&duplicate).err(),
            Some(JblError::EqPresetInvalid)
        );
        let mut malformed = catalog("1");
        malformed["eq_list"][0]["eq_payload"]["gain"] = json!([1, 2]);
        assert_eq!(
            parse_eq_catalog(&malformed).err(),
            Some(JblError::EqPresetInvalid)
        );
    }

    #[test]
    fn customize_active_is_explicitly_not_a_closed_target() {
        assert_eq!(parse_eq_catalog(&catalog("0")).unwrap().active(), None);
    }
}
