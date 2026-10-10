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
// Real Bluetooth M-XG*50MBSK declares only five standard mouse buttons.
// Fn buttons arrive in the vendor input report 0x10, not in report 2.
// Accept only this observed device/header/length; never decode arbitrary
// vendor packets or the mouse's keyboard/consumer collections as buttons.
pub fn button_update(pid: u32, report_id: u32, bytes: &[u8]) -> Option<(u8, u8)> {
    if pid == 0x013e && report_id == 0x10 {
        let payload = match bytes {
            [0x10, payload @ ..] if payload.len() == 16 => payload,
            payload if payload.len() == 16 => payload,
            _ => return None,
        };
        return (payload[0] == 0x12 && payload[1] == 0x12).then_some((payload[2] & 0xe0, 0xe0));
    }
    button_mask(pid, report_id, bytes).map(|mask| {
        let scope = if pid == 0x013e { 0x1f } else { 0xff };
        (mask & scope, scope)
    })
}

#[derive(Default)]
pub struct ButtonState {
    previous: Option<u8>,
    initialised: u8,
}
impl ButtonState {
    #[cfg(test)]
    pub fn transition(&mut self, mask: u8) -> Option<(u8, u8)> {
        self.transition_scoped(mask, 0xff)
    }
    pub fn transition_scoped(&mut self, mask: u8, scope: u8) -> Option<(u8, u8)> {
        let current = (self.previous.unwrap_or(0) & !scope) | (mask & scope);
        let previous = self.previous.replace(current).unwrap_or(current);
        let new_scope = scope & !self.initialised;
        self.initialised |= scope;
        if new_scope != 0 {
            // Standard and vendor reports each establish their own baseline.
            // An already held Fn button must not fire after an idle standard
            // report happened to arrive first during startup/reconnection.
            return (scope != 0xff).then_some((current, current));
        }
        (previous != current).then_some((previous, current))
    }
}

#[derive(Clone, Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ButtonObservation {
    pub pressed_mask: Option<u8>,
    pub press_counts: [u64; 8],
    pub last_button: Option<u8>,
}
impl ButtonObservation {
    pub fn update(&mut self, mask: u8) {
        // The first packet establishes state, without treating an already held
        // button as a fresh press. Keep quick clicks visible through counters.
        let previous = self.pressed_mask.replace(mask).unwrap_or(mask);
        for button in 1..=8 {
            if mask & !previous & (1 << (button - 1)) != 0 {
                self.press_counts[button - 1] = self.press_counts[button - 1].saturating_add(1);
                self.last_button = Some(button as u8);
            }
        }
    }
}

pub fn identify_until(active: bool, now: u64) -> u64 {
    if active {
        now.saturating_add(30_000)
    } else {
        0
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
    ApplicationSwitcher,
}
impl SystemAction {
    pub fn symbolic_id(&self) -> Option<&'static [u8]> {
        Some(match self {
            Self::MissionControl => b"32\0",
            Self::ShowDesktop => b"36\0",
            Self::ApplicationWindows => b"33\0",
            Self::ApplicationSwitcher => return None,
        })
    }
    pub fn default_key(&self) -> Option<(u16, u64)> {
        Some(match self {
            Self::MissionControl => (126, 1 << 18),
            Self::ApplicationWindows => (125, 1 << 18),
            Self::ShowDesktop => (103, 0),
            Self::ApplicationSwitcher => return None,
        })
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
    pub fn actions(&self, family: &str, previous: u8, current: u8) -> Vec<(u8, Action)> {
        if !self.enabled {
            return Vec::new();
        }
        let rising = current & !previous;
        self.mappings
            .get(family)
            .into_iter()
            .flat_map(|bindings| bindings.iter())
            .filter(|(button, _)| rising & (1 << (**button - 1)) != 0)
            .map(|(button, action)| (*button, action.clone()))
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
    #[test]
    fn bluetooth_vendor_fn_reports_merge_with_primary_buttons_without_false_release() {
        let mut state = ButtonState::default();
        assert_eq!(state.transition_scoped(0, 0x1f), Some((0, 0)));
        let frame = [
            0x10, 0x12, 0x12, 0x20, 0, 0, 0x10, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
        ];
        assert_eq!(button_update(0x013e, 16, &frame), Some((0x20, 0xe0)));
        assert_eq!(button_update(0x013e, 16, &frame[1..]), Some((0x20, 0xe0)));
        let (mask, scope) = button_update(0x013e, 16, &frame).unwrap();
        assert_eq!(state.transition_scoped(mask, scope), Some((0x20, 0x20)));
        assert_eq!(state.transition_scoped(1, 0x1f), Some((0x20, 0x21)));
        assert_eq!(state.transition_scoped(0, 0x1f), Some((0x21, 0x20)));
        assert_eq!(state.transition_scoped(mask, scope), None);
        assert_eq!(state.transition_scoped(0, 0xe0), Some((0x20, 0)));
        assert_eq!(state.transition_scoped(0x20, 0xe0), Some((0, 0x20)));
        assert_eq!(button_update(0x013d, 16, &frame), None);
        assert_eq!(button_update(0x013e, 16, &frame[..8]), None);
        let mut wrong_header = frame;
        wrong_header[2] = 0x13;
        assert_eq!(button_update(0x013e, 16, &wrong_header), None);
        let mut wrong_id = frame;
        wrong_id[0] = 0x20;
        assert_eq!(button_update(0x013e, 16, &wrong_id), None);
        for mask in [0x20, 0x40, 0x80, 0xe0, 0] {
            let mut packet = frame;
            packet[3] = mask;
            assert_eq!(button_update(0x013e, 16, &packet), Some((mask, 0xe0)));
        }
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
        let (previous, current) = state.transition(0x21).unwrap();
        assert!(c.actions("mouse", previous, current).is_empty());
        let (previous, current) = state.transition(0).unwrap();
        assert!(c.actions("mouse", previous, current).is_empty());
        let (previous, current) = state.transition(0x20).unwrap();
        assert_eq!(c.actions("mouse", previous, current).len(), 1);
        assert_eq!(state.transition(0x20), None);
        assert_eq!(ButtonState::default().transition(0x20), None);
    }
    #[test]
    fn observation_keeps_quick_clicks_and_startup_does_not_count_held_buttons() {
        let mut observation = ButtonObservation::default();
        observation.update(0x20);
        assert_eq!(observation.press_counts, [0; 8]);
        observation.update(0);
        observation.update(0x81);
        observation.update(0x81);
        observation.update(0);
        assert_eq!(observation.press_counts, [1, 0, 0, 0, 0, 0, 0, 1]);
        assert_eq!(observation.last_button, Some(8));
        assert_eq!(observation.pressed_mask, Some(0));
        observation.update(0x80);
        assert_eq!(observation.press_counts[7], 2);
        assert_eq!(identify_until(true, 100), 30_100);
        assert_eq!(identify_until(false, 100), 0);
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
        for name in [
            "missionControl",
            "applicationWindows",
            "showDesktop",
            "applicationSwitcher",
        ] {
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
    fn persistent_picker_has_no_synthetic_key_fallback() {
        assert!(SystemAction::ApplicationSwitcher.default_key().is_none());
        assert!(SystemAction::ApplicationSwitcher.symbolic_id().is_none());
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
