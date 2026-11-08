//! Keyboard & mouse → escape sequence encoding.

use alacritty_terminal::term::TermMode;
use winit::event::MouseButton;
use winit::keyboard::{Key, ModifiersState, NamedKey};

pub fn mod_code(mods: ModifiersState) -> u8 {
    // xterm modifier parameter: 1 + (shift|alt|ctrl|super)
    1 + u8::from(mods.shift_key())
        + 2 * u8::from(mods.alt_key())
        + 4 * u8::from(mods.control_key())
        + 8 * u8::from(mods.super_key())
}

fn push_csi(out: &mut Vec<u8>, s: &str) {
    out.extend_from_slice(s.as_bytes());
}

/// Encode a key press. Ctrl+Shift+C/V (copy/paste) are handled by the app before this.
pub fn encode_key(
    key: &Key,
    text: Option<&str>,
    mods: ModifiersState,
    mode: TermMode,
) -> Option<Vec<u8>> {
    let app_cursor = mode.contains(TermMode::APP_CURSOR);
    let ctrl = mods.control_key();
    let alt = mods.alt_key();
    let shift = mods.shift_key();
    let mc = mod_code(mods);

    let mut out = Vec::with_capacity(8);
    match key {
        Key::Named(NamedKey::Enter) => {
            if alt {
                out.push(0x1b);
            }
            out.push(if ctrl { 0x0a } else { 0x0d });
        }
        Key::Named(NamedKey::Backspace) => {
            if alt {
                out.push(0x1b);
            }
            out.push(if ctrl { 0x08 } else { 0x7f });
        }
        Key::Named(NamedKey::Tab) => {
            if shift {
                push_csi(&mut out, "\x1b[Z");
            } else if alt {
                out.extend_from_slice(b"\x1b\t");
            } else {
                out.push(b'\t');
            }
        }
        Key::Named(NamedKey::Escape) => out.push(0x1b),
        Key::Named(NamedKey::Space) => {
            return Some(encode_char(' ', mods));
        }
        Key::Named(NamedKey::ArrowUp) => arrow(&mut out, b'A', app_cursor, mc, alt),
        Key::Named(NamedKey::ArrowDown) => arrow(&mut out, b'B', app_cursor, mc, alt),
        Key::Named(NamedKey::ArrowRight) => arrow(&mut out, b'C', app_cursor, mc, alt),
        Key::Named(NamedKey::ArrowLeft) => arrow(&mut out, b'D', app_cursor, mc, alt),
        Key::Named(NamedKey::Home) => arrow(&mut out, b'H', app_cursor, mc, alt),
        Key::Named(NamedKey::End) => arrow(&mut out, b'F', app_cursor, mc, alt),
        Key::Named(NamedKey::Insert) => tilde(&mut out, 2, mc, alt),
        Key::Named(NamedKey::Delete) => tilde(&mut out, 3, mc, alt),
        Key::Named(NamedKey::PageUp) => tilde(&mut out, 5, mc, alt),
        Key::Named(NamedKey::PageDown) => tilde(&mut out, 6, mc, alt),
        Key::Named(NamedKey::F1) => fkey(&mut out, b'P', mc, alt),
        Key::Named(NamedKey::F2) => fkey(&mut out, b'Q', mc, alt),
        Key::Named(NamedKey::F3) => fkey(&mut out, b'R', mc, alt),
        Key::Named(NamedKey::F4) => fkey(&mut out, b'S', mc, alt),
        Key::Named(NamedKey::F5) => tilde(&mut out, 15, mc, alt),
        Key::Named(NamedKey::F6) => tilde(&mut out, 17, mc, alt),
        Key::Named(NamedKey::F7) => tilde(&mut out, 18, mc, alt),
        Key::Named(NamedKey::F8) => tilde(&mut out, 19, mc, alt),
        Key::Named(NamedKey::F9) => tilde(&mut out, 20, mc, alt),
        Key::Named(NamedKey::F10) => tilde(&mut out, 21, mc, alt),
        Key::Named(NamedKey::F11) => tilde(&mut out, 23, mc, alt),
        Key::Named(NamedKey::F12) => tilde(&mut out, 24, mc, alt),
        Key::Character(_) | _ => {
            let ch = text.and_then(|t| t.chars().next())?;
            // Ctrl+letter → control code.
            if ctrl && !alt && ch.is_ascii() {
                let code = ctrl_code(ch)?;
                out.push(code);
                return Some(out);
            }
            return Some(encode_char(ch, mods));
        }
    }
    Some(out)
}

fn ctrl_code(ch: char) -> Option<u8> {
    let c = ch.to_ascii_lowercase();
    Some(match c {
        'a'..='z' => c as u8 - b'a' + 1,
        ' ' | '@' | '2' => 0x00,
        '[' | '3' => 0x1b,
        '\\' | '4' => 0x1c,
        ']' | '5' => 0x1d,
        '^' | '6' => 0x1e,
        '_' | '/' | '7' | '-' => 0x1f,
        '?' | '8' => 0x7f,
        _ => return None,
    })
}

fn encode_char(ch: char, mods: ModifiersState) -> Vec<u8> {
    let mut out = Vec::with_capacity(4);
    if mods.alt_key() {
        out.push(0x1b);
    }
    if mods.control_key() {
        if let Some(code) = ctrl_code(ch) {
            out.push(code);
            return out;
        }
    }
    let mut buf = [0u8; 4];
    out.extend_from_slice(ch.encode_utf8(&mut buf).as_bytes());
    out
}

fn arrow(out: &mut Vec<u8>, final_byte: u8, app_cursor: bool, mc: u8, alt: bool) {
    if alt {
        out.push(0x1b);
    }
    if mc > 1 {
        push_csi(out, &format!("\x1b[1;{mc}{}", final_byte as char));
    } else if app_cursor {
        out.extend_from_slice(&[0x1b, b'O', final_byte]);
    } else {
        out.extend_from_slice(&[0x1b, b'[', final_byte]);
    }
}

fn tilde(out: &mut Vec<u8>, num: u8, mc: u8, alt: bool) {
    if alt {
        out.push(0x1b);
    }
    if mc > 1 {
        push_csi(out, &format!("\x1b[{num};{mc}~"));
    } else {
        push_csi(out, &format!("\x1b[{num}~"));
    }
}

fn fkey(out: &mut Vec<u8>, ss3: u8, mc: u8, alt: bool) {
    if alt {
        out.push(0x1b);
    }
    if mc > 1 {
        push_csi(out, &format!("\x1b[1;{mc}{}", ss3 as char));
    } else {
        out.extend_from_slice(&[0x1b, b'O', ss3]);
    }
}

/// Mouse press/release (click reporting enabled).
pub fn mouse_button(
    button: MouseButton,
    press: bool,
    x: usize,
    y: usize,
    mods: ModifiersState,
    mode: TermMode,
) -> Option<Vec<u8>> {
    // Mouse protocols are mutually exclusive: apps set exactly one of
    // MOUSE_REPORT_CLICK / MOUSE_DRAG / MOUSE_MOTION, so we must test for any.
    if !mode.intersects(TermMode::MOUSE_MODE) {
        return None;
    }
    let btn = match button {
        MouseButton::Left => 0,
        MouseButton::Middle => 1,
        MouseButton::Right => 2,
        _ => return None,
    };
    Some(encode_mouse(btn, press, x, y, mods, mode))
}

/// Drag motion while a button is held (1002).
pub fn mouse_drag(
    button: MouseButton,
    x: usize,
    y: usize,
    mods: ModifiersState,
    mode: TermMode,
) -> Option<Vec<u8>> {
    // 1002 reports drags directly; 1003 reports them too while a button is held.
    if !mode.intersects(TermMode::MOUSE_DRAG | TermMode::MOUSE_MOTION) {
        return None;
    }
    let btn = match button {
        MouseButton::Left => 0,
        MouseButton::Middle => 1,
        MouseButton::Right => 2,
        _ => return None,
    };
    Some(encode_mouse(btn | 32, true, x, y, mods, mode))
}

/// Free pointer motion (1003).
pub fn mouse_motion(x: usize, y: usize, mods: ModifiersState, mode: TermMode) -> Option<Vec<u8>> {
    if !mode.contains(TermMode::MOUSE_MOTION) {
        return None;
    }
    Some(encode_mouse(3 | 32, true, x, y, mods, mode))
}

/// Wheel scroll report (buttons 64/65).
pub fn mouse_wheel(
    up: bool,
    x: usize,
    y: usize,
    mods: ModifiersState,
    mode: TermMode,
) -> Option<Vec<u8>> {
    if !mode.intersects(TermMode::MOUSE_MODE) {
        return None;
    }
    let btn = if up { 64 } else { 65 };
    Some(encode_mouse(btn, true, x, y, mods, mode))
}

fn encode_mouse(
    btn: u8,
    press: bool,
    x: usize,
    y: usize,
    mods: ModifiersState,
    mode: TermMode,
) -> Vec<u8> {
    let mut b = btn;
    if mods.shift_key() {
        b |= 4;
    }
    if mods.alt_key() {
        b |= 8;
    }
    if mods.control_key() {
        b |= 16;
    }
    if mode.contains(TermMode::SGR_MOUSE) {
        format!(
            "\x1b[<{};{};{}{}",
            b,
            x + 1,
            y + 1,
            if press { 'M' } else { 'm' }
        )
        .into_bytes()
    } else {
        // Legacy X10 encoding; coordinates clamped to the representable range.
        let cx = (x + 1).min(222) as u8;
        let cy = (y + 1).min(222) as u8;
        vec![0x1b, b'[', b'M', 0x20 + b, 0x20 + cx, 0x20 + cy]
    }
}

/// Bracketed paste wrapper.
pub fn bracketed_paste(text: &str, mode: TermMode) -> Vec<u8> {
    let mut out = Vec::with_capacity(text.len() + 16);
    if mode.contains(TermMode::BRACKETED_PASTE) {
        out.extend_from_slice(b"\x1b[200~");
    }
    out.extend_from_slice(text.as_bytes());
    if mode.contains(TermMode::BRACKETED_PASTE) {
        out.extend_from_slice(b"\x1b[201~");
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use winit::keyboard::SmolStr;

    fn key(k: &Key, text: Option<&str>, mods: ModifiersState, mode: TermMode) -> Vec<u8> {
        encode_key(k, text, mods, mode).expect("encoded")
    }

    fn mods(shift: bool, alt: bool, ctrl: bool, sup: bool) -> ModifiersState {
        let mut m = ModifiersState::empty();
        m.set(ModifiersState::SHIFT, shift);
        m.set(ModifiersState::ALT, alt);
        m.set(ModifiersState::CONTROL, ctrl);
        m.set(ModifiersState::SUPER, sup);
        m
    }

    #[test]
    fn arrows_respect_application_cursor_mode() {
        let normal = TermMode::SHOW_CURSOR | TermMode::LINE_WRAP;
        let app = normal | TermMode::APP_CURSOR;
        let k = Key::Named(NamedKey::ArrowUp);
        assert_eq!(
            key(&k, None, mods(false, false, false, false), normal),
            b"\x1b[A"
        );
        assert_eq!(
            key(&k, None, mods(false, false, false, false), app),
            b"\x1bOA"
        );
        assert_eq!(
            key(&k, None, mods(true, false, true, false), app),
            b"\x1b[1;6A"
        );
    }

    #[test]
    fn ctrl_and_alt_characters() {
        let m = TermMode::SHOW_CURSOR;
        assert_eq!(
            key(
                &Key::Character(SmolStr::new("a")),
                Some("a"),
                mods(false, false, true, false),
                m
            ),
            vec![0x01]
        );
        assert_eq!(
            key(
                &Key::Character(SmolStr::new("x")),
                Some("x"),
                mods(false, true, false, false),
                m
            ),
            vec![0x1b, b'x']
        );
        assert_eq!(
            key(
                &Key::Character(SmolStr::new("c")),
                Some("c"),
                mods(false, true, true, false),
                m
            ),
            vec![0x1b, 0x03]
        );
        assert_eq!(
            key(
                &Key::Character(SmolStr::new("Z")),
                Some("Z"),
                mods(false, false, false, false),
                m
            ),
            b"Z"
        );
    }

    #[test]
    fn specials() {
        let m = TermMode::SHOW_CURSOR;
        let none = mods(false, false, false, false);
        assert_eq!(key(&Key::Named(NamedKey::Enter), None, none, m), b"\r");
        assert_eq!(
            key(&Key::Named(NamedKey::Backspace), None, none, m),
            vec![0x7f]
        );
        assert_eq!(
            key(
                &Key::Named(NamedKey::Tab),
                None,
                mods(true, false, false, false),
                m
            ),
            b"\x1b[Z"
        );
        assert_eq!(
            key(&Key::Named(NamedKey::Delete), None, none, m),
            b"\x1b[3~"
        );
        assert_eq!(key(&Key::Named(NamedKey::F5), None, none, m), b"\x1b[15~");
        assert_eq!(
            key(
                &Key::Named(NamedKey::F5),
                None,
                mods(false, false, true, false),
                m
            ),
            b"\x1b[15;5~"
        );
        assert_eq!(key(&Key::Named(NamedKey::F1), None, none, m), b"\x1bOP");
    }

    #[test]
    fn mouse_sgr_encoding() {
        let mode = TermMode::MOUSE_MODE | TermMode::SGR_MOUSE | TermMode::MOUSE_REPORT_CLICK;
        let none = mods(false, false, false, false);
        let got = mouse_button(MouseButton::Left, true, 4, 9, none, mode).unwrap();
        assert_eq!(String::from_utf8(got).unwrap(), "\x1b[<0;5;10M");
        let got = mouse_button(MouseButton::Left, false, 4, 9, none, mode).unwrap();
        assert_eq!(String::from_utf8(got).unwrap(), "\x1b[<0;5;10m");
        let wheel = mouse_wheel(true, 1, 2, mods(true, false, false, false), mode).unwrap();
        assert_eq!(String::from_utf8(wheel).unwrap(), "\x1b[<68;2;3M");
    }

    /// nvim enables exactly one protocol bit (`?1002h`), never the union.
    #[test]
    fn mouse_works_with_single_protocol_bit() {
        let none = mods(false, false, false, false);
        // 1002 + 1006 (SGR), as set by nvim/helix/fzf.
        let mode = TermMode::MOUSE_DRAG | TermMode::SGR_MOUSE;
        let press = mouse_button(MouseButton::Left, true, 4, 9, none, mode).unwrap();
        assert_eq!(String::from_utf8(press).unwrap(), "\x1b[<0;5;10M");
        let release = mouse_button(MouseButton::Left, false, 4, 9, none, mode).unwrap();
        assert_eq!(String::from_utf8(release).unwrap(), "\x1b[<0;5;10m");
        let drag = mouse_drag(MouseButton::Left, 7, 3, none, mode).unwrap();
        assert_eq!(String::from_utf8(drag).unwrap(), "\x1b[<32;8;4M");
        let wheel = mouse_wheel(false, 0, 0, none, mode).unwrap();
        assert_eq!(String::from_utf8(wheel).unwrap(), "\x1b[<65;1;1M");
        // 1002 must not report free motion.
        assert!(mouse_motion(1, 1, none, mode).is_none());

        // 1000 (click-only): presses and wheel yes, drag no.
        let mode = TermMode::MOUSE_REPORT_CLICK | TermMode::SGR_MOUSE;
        assert!(mouse_button(MouseButton::Middle, true, 0, 0, none, mode).is_some());
        assert!(mouse_drag(MouseButton::Left, 1, 1, none, mode).is_none());
        assert!(mouse_motion(1, 1, none, mode).is_none());
        assert!(mouse_wheel(true, 0, 0, none, mode).is_some());

        // 1003 (all motion): everything reported, including held-button drag.
        let mode = TermMode::MOUSE_MOTION;
        assert!(mouse_button(MouseButton::Left, true, 0, 0, none, mode).is_some());
        assert!(mouse_drag(MouseButton::Left, 1, 1, none, mode).is_some());
        assert!(mouse_motion(1, 1, none, mode).is_some());
        // Legacy (non-SGR) encoding still works.
        let wheel = mouse_wheel(true, 0, 0, none, mode).unwrap();
        assert_eq!(wheel, b"\x1b[M\x60\x21\x21");
    }

    #[test]
    fn mouse_disabled_without_mode() {
        assert!(mouse_button(
            MouseButton::Left,
            true,
            0,
            0,
            mods(false, false, false, false),
            TermMode::SHOW_CURSOR
        )
        .is_none());
    }

    #[test]
    fn paste_wrapping() {
        assert_eq!(
            bracketed_paste("hi", TermMode::BRACKETED_PASTE),
            b"\x1b[200~hi\x1b[201~"
        );
        assert_eq!(bracketed_paste("hi", TermMode::SHOW_CURSOR), b"hi");
    }
}
