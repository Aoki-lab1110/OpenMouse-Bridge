//! Pure event planning and ownership. No OS calls or keyboard listener.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct KeyEvent {
    pub key: u16,
    pub down: bool,
    pub flags: u64,
    pub modifier: bool,
}
pub struct Stroke {
    pub press: Vec<KeyEvent>,
    pub release: Vec<KeyEvent>,
}
pub fn stroke(key: u16, requested: u64, physical: u64) -> Stroke {
    // Aggregate and device-specific left modifier bits must both be cleared
    // on release. Never inherit generated flags from a shared HID source.
    let modifiers = [
        (56, 1 << 17, 0x2),
        (59, 1 << 18, 0x1),
        (58, 1 << 19, 0x20),
        (55, 1 << 20, 0x8),
        (63, 1 << 23, 0),
    ];
    let mut press = Vec::new();
    let mut owned = Vec::new();
    let mut flags = physical;
    for (key, aggregate, device) in modifiers {
        if requested & aggregate != 0 && physical & aggregate == 0 {
            flags |= aggregate | device;
            press.push(KeyEvent {
                key,
                down: true,
                flags,
                modifier: true,
            });
            owned.push((key, aggregate, device));
        }
    }
    press.push(KeyEvent {
        key,
        down: true,
        flags: flags | requested,
        modifier: false,
    });
    press.push(KeyEvent {
        key,
        down: false,
        flags: flags | requested,
        modifier: false,
    });
    let release = owned
        .into_iter()
        .rev()
        .map(|(key, aggregate, device)| {
            flags &= !(aggregate | device);
            KeyEvent {
                key,
                down: false,
                flags,
                modifier: true,
            }
        })
        .collect();
    Stroke { press, release }
}

pub fn app_switcher(key: u16, flags: u64) -> bool {
    key == 48 && flags & (1 << 20) != 0 && flags & !((1 << 20) | (1 << 17)) == 0
}
pub fn release_flags(planned: u64, initial_physical: u64, current_physical: u64) -> u64 {
    (planned & !initial_physical) | current_physical
}

/// Store only already allocated releases. A cancellation always consumes them
/// exactly once, even after the saved mapping changes or a device disappears.
pub struct Held<E> {
    pub device: usize,
    pub button: u8,
    pub release: Vec<E>,
}
pub struct Ownership<E> {
    pub generation: u64,
    ticket: u64,
    held: Option<Held<E>>,
}
impl<E> Default for Ownership<E> {
    fn default() -> Self {
        Self {
            generation: 0,
            ticket: 0,
            held: None,
        }
    }
}
impl<E> Ownership<E> {
    pub fn hold(&mut self, held: Held<E>) -> u64 {
        assert!(self.held.is_none());
        self.ticket = self.ticket.wrapping_add(1);
        self.held = Some(held);
        self.ticket
    }
    pub fn expire(&mut self, ticket: u64) -> Vec<E> {
        if self.ticket == ticket {
            self.take()
        } else {
            Vec::new()
        }
    }
    pub fn release_button(&mut self, device: usize, released: u8) -> Vec<E> {
        if self
            .held
            .as_ref()
            .is_some_and(|h| h.device == device && released & (1 << (h.button - 1)) != 0)
        {
            self.take()
        } else {
            Vec::new()
        }
    }
    pub fn release_device(&mut self, device: usize) -> Vec<E> {
        if self.held.as_ref().is_some_and(|h| h.device == device) {
            self.take()
        } else {
            Vec::new()
        }
    }
    pub fn take(&mut self) -> Vec<E> {
        self.held.take().map_or_else(Vec::new, |h| h.release)
    }
    pub fn cancel(&mut self) -> Vec<E> {
        self.generation = self.generation.wrapping_add(1);
        self.take()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn every_modifier_combination_ends_at_the_physical_state() {
        for requested in 0..16u64 {
            let flags = requested << 17;
            for physical in [0, (1 << 20) | 0x10, (1 << 17) | 0x4, 1 << 16] {
                let plan = stroke(6, flags, physical);
                let events: Vec<_> = plan.press.iter().chain(&plan.release).collect();
                assert_eq!(events.last().unwrap().flags, physical);
                for key in [55, 56, 58, 59] {
                    let downs = events.iter().filter(|e| e.key == key && e.down).count();
                    let ups = events.iter().filter(|e| e.key == key && !e.down).count();
                    assert_eq!(downs, ups);
                }
                if physical & (1 << 20) != 0 {
                    assert!(!events.iter().any(|e| e.key == 55 || e.key == 54));
                }
            }
        }
    }
    #[test]
    fn command_tab_retains_command_until_mouse_release() {
        let plan = stroke(48, 1 << 20, 0);
        assert_eq!(
            plan.press
                .iter()
                .map(|e| (e.key, e.down))
                .collect::<Vec<_>>(),
            [(55, true), (48, true), (48, false)]
        );
        assert_eq!(
            plan.release,
            [KeyEvent {
                key: 55,
                down: false,
                flags: 0,
                modifier: true
            }]
        );
        assert!(app_switcher(48, 1 << 20));
        assert!(app_switcher(48, (1 << 20) | (1 << 17)));
        assert!(!app_switcher(48, (1 << 20) | (1 << 18)));
    }
    #[test]
    fn release_disconnect_and_cancellation_consume_owned_keys_once() {
        let mut owner = Ownership::default();
        owner.hold(Held {
            device: 10,
            button: 8,
            release: vec![55],
        });
        assert!(owner.release_button(11, 0x80).is_empty());
        assert!(owner.release_button(10, 0x40).is_empty());
        assert_eq!(owner.release_button(10, 0x80), [55]);
        assert!(owner.release_button(10, 0x80).is_empty());
        owner.hold(Held {
            device: 10,
            button: 8,
            release: vec![55],
        });
        assert!(owner.release_device(11).is_empty());
        assert_eq!(owner.release_device(10), [55]);
        owner.hold(Held {
            device: 10,
            button: 8,
            release: vec![55],
        });
        let old_generation = owner.generation;
        assert_eq!(owner.cancel(), [55]);
        assert_ne!(old_generation, owner.generation);
        assert!(owner.cancel().is_empty());
    }
    #[test]
    fn f11_has_no_injected_modifier_to_release() {
        let plan = stroke(103, 0, 0);
        assert_eq!(
            plan.press
                .iter()
                .map(|e| (e.key, e.down, e.flags))
                .collect::<Vec<_>>(),
            [(103, true, 0), (103, false, 0)]
        );
        assert!(plan.release.is_empty());
        let arrow = stroke(125, (1 << 18) | (1 << 21), 0);
        assert_eq!(arrow.press[1].flags & (1 << 21), 1 << 21);
        assert_eq!(arrow.release.last().unwrap().flags, 0);
    }
    #[test]
    fn old_deadline_cannot_release_a_new_press_and_release_tracks_physical_changes() {
        let mut owner = Ownership::default();
        let old = owner.hold(Held {
            device: 10,
            button: 8,
            release: vec![55],
        });
        assert_eq!(owner.take(), [55]);
        let new = owner.hold(Held {
            device: 10,
            button: 8,
            release: vec![55],
        });
        assert!(owner.expire(old).is_empty());
        assert_eq!(owner.expire(new), [55]);
        // Physical Command pressed during the mouse hold stays pressed.
        assert_eq!(release_flags(0, 0, (1 << 20) | 0x10), (1 << 20) | 0x10);
        // Physical Shift released during the hold does not remain stuck.
        assert_eq!(release_flags((1 << 17) | 0x4, (1 << 17) | 0x4, 0), 0);
    }
}
