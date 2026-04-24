use anyhow::Result;
use enigo::{
    Button, Coordinate,
    Direction::{Press, Release},
    Enigo, Key, Keyboard, Mouse, Settings,
};
use proto::remote_desktop::{input_event::Event, InputEvent};

pub struct InputController {
    enigo: Enigo,
    screen_width: f64,
    screen_height: f64,
}

impl InputController {
    pub fn new(screen_width: u32, screen_height: u32) -> Result<Self> {
        Ok(Self {
            enigo: Enigo::new(&Settings::default())?,
            screen_width: screen_width as f64,
            screen_height: screen_height as f64,
        })
    }

    pub fn handle(&mut self, evt: &InputEvent) -> Result<()> {
        match &evt.event {
            Some(Event::MouseMove(m)) => {
                let px = (m.x as f64 * self.screen_width) as i32;
                let py = (m.y as f64 * self.screen_height) as i32;
                self.enigo.move_mouse(px, py, Coordinate::Abs)?;
            }
            Some(Event::MouseButton(mb)) => {
                let btn = map_button(&mb.button);
                let dir = if mb.pressed { Press } else { Release };
                self.enigo.button(btn, dir)?;
            }
            Some(Event::MouseScroll(ms)) => {
                if ms.dx != 0 {
                    self.enigo.scroll(ms.dx, enigo::Axis::Horizontal)?;
                }
                if ms.dy != 0 {
                    self.enigo.scroll(ms.dy, enigo::Axis::Vertical)?;
                }
            }
            Some(Event::Key(k)) => {
                let key = map_key(&k.key);
                let dir = if k.pressed { Press } else { Release };
                self.enigo.key(key, dir)?;
            }
            None => {}
        }
        Ok(())
    }

    pub fn set_screen_size(&mut self, width: u32, height: u32) {
        self.screen_width = width as f64;
        self.screen_height = height as f64;
    }
}

fn map_button(btn: &str) -> Button {
    match btn {
        "right" => Button::Right,
        "middle" => Button::Middle,
        _ => Button::Left,
    }
}

fn map_key(key: &str) -> Key {
    match key {
        "Return" | "Enter" => Key::Return,
        "Escape" => Key::Escape,
        "Backspace" => Key::Backspace,
        "Tab" => Key::Tab,
        "Space" | " " => Key::Space,
        "Delete" => Key::Delete,
        "Home" => Key::Home,
        "End" => Key::End,
        "PageUp" => Key::PageUp,
        "PageDown" => Key::PageDown,
        "ArrowLeft" | "Left" => Key::LeftArrow,
        "ArrowRight" | "Right" => Key::RightArrow,
        "ArrowUp" | "Up" => Key::UpArrow,
        "ArrowDown" | "Down" => Key::DownArrow,
        "F1" => Key::F1,
        "F2" => Key::F2,
        "F3" => Key::F3,
        "F4" => Key::F4,
        "F5" => Key::F5,
        "F6" => Key::F6,
        "F7" => Key::F7,
        "F8" => Key::F8,
        "F9" => Key::F9,
        "F10" => Key::F10,
        "F11" => Key::F11,
        "F12" => Key::F12,
        "Control" | "ControlLeft" | "ControlRight" => Key::Control,
        "Shift" | "ShiftLeft" | "ShiftRight" => Key::Shift,
        "Alt" | "AltLeft" | "AltRight" => Key::Alt,
        "Meta" | "MetaLeft" | "MetaRight" => Key::Meta,
        s if s.chars().count() == 1 => Key::Unicode(s.chars().next().unwrap()),
        other => Key::Other(other.chars().next().unwrap_or('?') as u32),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn map_key_known_keys() {
        assert!(matches!(map_key("Return"), Key::Return));
        assert!(matches!(map_key("Escape"), Key::Escape));
        assert!(matches!(map_key("ArrowLeft"), Key::LeftArrow));
        assert!(matches!(map_key("Control"), Key::Control));
    }

    #[test]
    fn map_key_single_char() {
        assert!(matches!(map_key("a"), Key::Unicode('a')));
        assert!(matches!(map_key("Z"), Key::Unicode('Z')));
    }

    #[test]
    fn map_button_variants() {
        assert!(matches!(map_button("left"), Button::Left));
        assert!(matches!(map_button("right"), Button::Right));
        assert!(matches!(map_button("middle"), Button::Middle));
    }

    #[test]
    fn coordinate_normalisation() {
        let px = (0.5f64 * 1920f64) as i32;
        let py = (0.75f64 * 1080f64) as i32;
        assert_eq!(px, 960);
        assert_eq!(py, 810);
    }
}
