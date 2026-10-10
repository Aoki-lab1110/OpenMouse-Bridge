use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

pub fn product(pid: u32) -> Option<(&'static str, u32)> {
    match pid {
        0x013d => Some(("mouse", 1)),
        0x013e => Some(("mouse", 2)),
        0x0128 | 0x0129 | 0x0130 => Some(("trackball", 1)),
        _ => None,
    }
}

// Native IOHID callbacks may include the ID in the buffer; the HTTP/UI never
// handles raw reports. Accept only the two explicit, fixed-size representations.
pub fn button_mask(pid: u32, report_id: u32, bytes: &[u8]) -> Option<u8> {
    let (_, expected) = product(pid)?;
    if report_id != expected {
        return None;
    }
    match bytes {
        [id, mask, _, _, _, _, _, _] if u32::from(*id) == expected => Some(*mask),
        [mask, _, _, _, _, _, _] => Some(*mask),
        _ => None,
    }
}

#[derive(Default)]
pub struct ButtonState {
    previous: Option<u8>,
}
impl ButtonState {
    pub fn transition(&mut self, mask: u8) -> Option<(u8, u8)> {
        let current = mask & 0xe0;
        let previous = self.previous.replace(current).unwrap_or(current);
        (previous != current).then_some((previous, current))
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Modifiers {
    pub command: bool,
    pub control: bool,
    pub option: bool,
    pub shift: bool,
}
impl Modifiers {
    pub fn flags(&self) -> u64 {
        ((self.shift as u64) << 17)
            | ((self.control as u64) << 18)
            | ((self.option as u64) << 19)
            | ((self.command as u64) << 20)
    }
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Shortcut {
    pub key_code: u16,
    pub modifiers: Modifiers,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub enum SystemAction {
    MissionControl,
    ApplicationWindows,
    ShowDesktop,
}
impl SystemAction {
    // Installed Apple's Mission Control binary dispatches 0/1/2 to Dock.
    // Keep arguments fixed; never accept command strings or executable paths.
    pub fn argument(&self) -> &'static str {
        match self {
            Self::MissionControl => "0",
            Self::ShowDesktop => "1",
            Self::ApplicationWindows => "2",
        }
    }
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SystemBinding {
    pub system_action: SystemAction,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(untagged)]
pub enum Action {
    Shortcut(Shortcut),
    System(SystemBinding),
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Config {
    pub enabled: bool,
    #[serde(default)]
    pub mappings: BTreeMap<String, BTreeMap<u8, Action>>,
}
impl Config {
    pub fn validate(&self) -> Result<(), &'static str> {
        for (family, bindings) in &self.mappings {
            if family != "mouse" && family != "trackball" {
                return Err("Unknown EX-G PRO family");
            }
            for (button, action) in bindings {
                if !(6..=8).contains(button) {
                    return Err("Only extra buttons 6, 7 and 8 can be remapped");
                }
                // Modifier-only keys and arbitrary/undefined virtual key codes are excluded.
                if let Action::Shortcut(shortcut) = action
                    && !matches!(shortcut.key_code, 0..=9 | 11..=49 | 51 | 53 | 96..=101 | 103 | 109 | 111 | 115..=126)
                {
                    return Err("Unsupported keyboard key code");
                }
            }
        }
        Ok(())
    }
    pub fn actions(&self, family: &str, previous: u8, current: u8) -> Vec<Action> {
        if !self.enabled {
            return Vec::new();
        }
        let rising = current & !previous;
        self.mappings
            .get(family)
            .into_iter()
            .flat_map(|bindings| bindings.iter())
            .filter(|(button, _)| rising & (1 << (**button - 1)) != 0)
            .map(|(_, action)| action.clone())
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn reads_extra_bits_and_rejects_unrelated_reports() {
        assert_eq!(
            button_mask(0x013e, 2, &[2, 0xe0, 0, 0, 0, 0, 0, 0]),
            Some(0xe0)
        );
        assert_eq!(
            button_mask(0x013e, 2, &[0xe0, 0, 0, 0, 0, 0, 0]),
            Some(0xe0)
        );
        assert_eq!(button_mask(0x013e, 1, &[1, 0xe0, 0, 0, 0, 0, 0, 0]), None);
        assert_eq!(button_mask(0x013e, 2, &[2, 0xe0]), None);
        assert_eq!(button_mask(0x9999, 2, &[2, 0xe0, 0, 0, 0, 0, 0, 0]), None);
    }
    fn configured() -> Config {
        let mut c = Config {
            enabled: true,
            ..Config::default()
        };
        c.mappings.insert(
            "mouse".into(),
            BTreeMap::from([(
                6,
                Action::Shortcut(Shortcut {
                    key_code: 8,
                    modifiers: Modifiers {
                        command: true,
                        ..Modifiers::default()
                    },
                }),
            )]),
        );
        c
    }
    #[test]
    fn held_buttons_do_not_repeat_and_primary_buttons_never_trigger() {
        let c = configured();
        assert!(c.validate().is_ok());
        assert_eq!(c.actions("mouse", 0, 0x20).len(), 1);
        assert!(c.actions("mouse", 0x20, 0x20).is_empty());
        assert!(c.actions("mouse", 0x20, 0).is_empty());
        assert!(c.actions("mouse", 0, 0x1f).is_empty());
        assert!(c.actions("trackball", 0, 0x20).is_empty());
        assert!(
            Config {
                enabled: false,
                ..c
            }
            .actions("mouse", 0, 0x20)
            .is_empty()
        );
    }
    #[test]
    fn unsafe_bindings_are_rejected() {
        let mut c = configured();
        c.mappings.get_mut("mouse").unwrap().insert(
            1,
            Action::Shortcut(Shortcut {
                key_code: 8,
                modifiers: Modifiers::default(),
            }),
        );
        assert!(c.validate().is_err());
        c.mappings.get_mut("mouse").unwrap().remove(&1);
        if let Action::Shortcut(shortcut) =
            c.mappings.get_mut("mouse").unwrap().get_mut(&6).unwrap()
        {
            shortcut.key_code = 65535;
        }
        assert!(c.validate().is_err());
        assert!(serde_json::from_str::<Config>(r#"{"enabled":true,"unexpected":true}"#).is_err());
    }
    #[test]
    fn startup_or_reconnection_never_fires_a_held_button() {
        let c = configured();
        let mut state = ButtonState::default();
        assert_eq!(state.transition(0x20), None);
        assert_eq!(state.transition(0x21), None); // primary click is ignored
        let (previous, current) = state.transition(0).unwrap();
        assert!(c.actions("mouse", previous, current).is_empty());
        let (previous, current) = state.transition(0x20).unwrap();
        assert_eq!(c.actions("mouse", previous, current).len(), 1);
        assert_eq!(state.transition(0x20), None);
        assert_eq!(ButtonState::default().transition(0x20), None);
    }
    #[test]
    fn legacy_shortcuts_and_system_actions_round_trip_and_trigger_once() {
        let legacy = r#"{"enabled":true,"mappings":{"mouse":{"6":{"keyCode":6,"modifiers":{"command":true,"control":false,"option":false,"shift":false}}}}}"#;
        let c: Config = serde_json::from_str(legacy).unwrap();
        assert!(c.validate().is_ok());
        assert_eq!(
            serde_json::to_value(&c).unwrap(),
            serde_json::from_str::<serde_json::Value>(legacy).unwrap()
        );
        for name in ["missionControl", "applicationWindows", "showDesktop"] {
            let json = format!(
                r#"{{"enabled":true,"mappings":{{"mouse":{{"6":{{"systemAction":"{name}"}}}}}}}}"#
            );
            let c: Config = serde_json::from_str(&json).unwrap();
            assert!(c.validate().is_ok());
            assert_eq!(c.actions("mouse", 0, 0x20).len(), 1);
            assert!(c.actions("mouse", 0x20, 0x20).is_empty());
            assert!(c.actions("mouse", 0x20, 0).is_empty());
            assert_eq!(
                serde_json::to_value(&c).unwrap(),
                serde_json::from_str::<serde_json::Value>(&json).unwrap()
            );
        }
    }
    #[test]
    fn arbitrary_commands_and_mixed_action_fields_are_rejected() {
        for action in [
            r#"{"systemAction":"runCommand"}"#,
            r#"{"systemAction":"missionControl","command":"curl example.com"}"#,
            r#"{"systemAction":"missionControl","keyCode":6,"modifiers":{"command":true,"control":false,"option":false,"shift":false}}"#,
        ] {
            assert!(serde_json::from_str::<Action>(action).is_err());
        }
    }
}
