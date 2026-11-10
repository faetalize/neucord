//! What READY's `regional_feature_config` sets for the account's region.

use serde_json::Value;

/// Whether the media takedown regulation is TIDA, which the message menu's
/// order follows. The client (454409) takes any other value, or no
/// `regional_feature_config`, as not regulated.
pub fn media_takedown_regulated(regional_feature_config: &Value) -> bool {
    regional_feature_config["media_takedown_regulation"] == "tida"
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn only_tida_regulates_media_takedown() {
        assert!(media_takedown_regulated(&json!({ "media_takedown_regulation": "tida" })));
        for other in [json!("uk_stopncii"), json!("unheard_of"), json!(null)] {
            assert!(!media_takedown_regulated(&json!({ "media_takedown_regulation": other })));
        }
        assert!(!media_takedown_regulated(&json!({ "age_gated_features": 0 })));
        assert!(!media_takedown_regulated(&Value::Null));
    }
}
