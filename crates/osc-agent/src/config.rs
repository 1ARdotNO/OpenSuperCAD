//! Session settings an agent offers: its permission mode, model, effort and
//! the like (ACP session config options, plus the older session modes).
//!
//! Parsed from JSON rather than the schema types, so options this client
//! doesn't know yet (new kinds, grouped choices) are skipped or flattened
//! instead of failing the session.

use serde_json::Value;

/// Id of the option made from an agent's session *modes* when it has no
/// `mode` config option; set with `session/set_mode`.
pub const MODE_OPTION: &str = "__mode";

/// A setting with a fixed set of choices.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ConfigOption {
    pub id: String,
    pub name: String,
    pub description: Option<String>,
    /// ACP category: `mode`, `model`, `thought_level`, or another string.
    pub category: Option<String>,
    /// The value of the choice in use.
    pub current: String,
    pub choices: Vec<ConfigChoice>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ConfigChoice {
    pub value: String,
    pub name: String,
    pub description: Option<String>,
}

impl ConfigOption {
    /// The name of the choice in use (its value if unknown).
    pub fn current_name(&self) -> &str {
        self.choices
            .iter()
            .find(|c| c.value == self.current)
            .map_or(self.current.as_str(), |c| c.name.as_str())
    }

    /// Where the option sits in the composer: mode, model, effort, rest.
    pub fn order(&self) -> u8 {
        match self.category.as_deref() {
            Some("mode") => 0,
            Some("model") => 1,
            Some("thought_level") => 2,
            _ => 3,
        }
    }
}

/// The options in a `session/new`, `session/load` or
/// `session/set_config_option` result (or a `config_option_update`): its
/// `configOptions`, or, when there are none for modes, an option made from
/// its `modes`.
pub fn parse(result: &Value) -> Option<Vec<ConfigOption>> {
    let mut options: Vec<ConfigOption> = result
        .get("configOptions")
        .and_then(Value::as_array)
        .map(|list| list.iter().filter_map(parse_option).collect())
        .unwrap_or_default();
    let has_mode = options
        .iter()
        .any(|o| o.category.as_deref() == Some("mode"));
    if !has_mode && let Some(modes) = result.get("modes").and_then(parse_modes) {
        options.insert(0, modes);
    }
    let reported = result.get("configOptions").is_some() || result.get("modes").is_some();
    reported.then_some(options)
}

fn parse_option(v: &Value) -> Option<ConfigOption> {
    // Only selects; booleans and unknown kinds are left out.
    if v.get("type").and_then(Value::as_str) != Some("select") {
        return None;
    }
    let mut choices = Vec::new();
    for entry in v.get("options")?.as_array()? {
        // Grouped choices: `{group, name, options: [...]}`.
        match entry.get("options").and_then(Value::as_array) {
            Some(group) => choices.extend(group.iter().filter_map(parse_choice)),
            None => choices.extend(parse_choice(entry)),
        }
    }
    Some(ConfigOption {
        id: str_field(v, "id")?,
        name: str_field(v, "name")?,
        description: str_field(v, "description"),
        category: str_field(v, "category"),
        current: str_field(v, "currentValue")?,
        choices,
    })
}

fn parse_choice(v: &Value) -> Option<ConfigChoice> {
    Some(ConfigChoice {
        value: str_field(v, "value")?,
        name: str_field(v, "name")?,
        description: str_field(v, "description"),
    })
}

fn parse_modes(v: &Value) -> Option<ConfigOption> {
    let choices: Vec<ConfigChoice> = v
        .get("availableModes")?
        .as_array()?
        .iter()
        .filter_map(|m| {
            Some(ConfigChoice {
                value: str_field(m, "id")?,
                name: str_field(m, "name")?,
                description: str_field(m, "description"),
            })
        })
        .collect();
    (!choices.is_empty()).then(|| ConfigOption {
        id: MODE_OPTION.into(),
        name: "Mode".into(),
        description: None,
        category: Some("mode".into()),
        current: str_field(v, "currentModeId").unwrap_or_default(),
        choices,
    })
}

fn str_field(v: &Value, key: &str) -> Option<String> {
    v.get(key).and_then(Value::as_str).map(str::to_owned)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// What Claude Code's ACP adapter reports (trimmed).
    fn claude() -> Value {
        json!({
            "sessionId": "s",
            "modes": {"currentModeId": "default", "availableModes": [
                {"id": "default", "name": "Manual"}, {"id": "plan", "name": "Plan"}]},
            "configOptions": [
                {"id": "mode", "name": "Mode", "category": "mode", "type": "select",
                 "currentValue": "default", "options": [
                    {"value": "default", "name": "Manual", "description": "Always ask"},
                    {"value": "auto", "name": "Auto"}]},
                {"id": "model", "name": "Model", "category": "model", "type": "select",
                 "currentValue": "opus", "options": [
                    {"value": "opus", "name": "Opus 5.5"}, {"value": "haiku", "name": "Haiku 4.5"}]},
                {"id": "effort", "name": "Effort", "category": "thought_level", "type": "select",
                 "currentValue": "high", "options": [{"value": "high", "name": "High"}]},
                {"id": "fast", "name": "Fast", "type": "boolean", "currentValue": true}
            ]
        })
    }

    #[test]
    fn reads_config_options() {
        let options = parse(&claude()).unwrap();
        let ids: Vec<_> = options.iter().map(|o| o.id.as_str()).collect();
        // Booleans are skipped; the modes aren't duplicated.
        assert_eq!(ids, ["mode", "model", "effort"]);
        assert_eq!(options[1].current_name(), "Opus 5.5");
        assert_eq!(
            options[0].choices[0].description.as_deref(),
            Some("Always ask")
        );
        assert_eq!(options[2].order(), 2);
    }

    #[test]
    fn falls_back_to_session_modes() {
        let options = parse(
            &json!({"modes": {"currentModeId": "plan", "availableModes": [
            {"id": "default", "name": "Manual"}, {"id": "plan", "name": "Plan"}]}}),
        )
        .unwrap();
        assert_eq!(options.len(), 1);
        assert_eq!(options[0].id, MODE_OPTION);
        assert_eq!(options[0].current_name(), "Plan");
    }

    #[test]
    fn flattens_grouped_choices_and_ignores_silence() {
        let options = parse(&json!({"configOptions": [
            {"id": "model", "name": "Model", "type": "select", "currentValue": "b",
             "options": [{"group": "g", "name": "G", "options": [
                {"value": "a", "name": "A"}, {"value": "b", "name": "B"}]}]}]}))
        .unwrap();
        assert_eq!(options[0].choices.len(), 2);
        assert_eq!(options[0].current_name(), "B");
        assert_eq!(parse(&json!({"sessionId": "s"})), None);
    }
}
