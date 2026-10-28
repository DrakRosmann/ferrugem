//! Teclado PS/2: traduz o conjunto de scancodes 1 (layout US) para bytes.
//! As setas viram sequências ANSI (ESC [ A...), como num terminal.

use crate::cpu::inb;
use crate::input;
use crate::sync::SpinLock;

#[rustfmt::skip]
const NORMAL: &[u8; 58] =
    b"\0\x1b1234567890-=\x08\tqwertyuiop[]\n\0asdfghjkl;'`\0\\zxcvbnm,./\0*\0 ";
#[rustfmt::skip]
const SHIFTED: &[u8; 58] =
    b"\0\x1b!@#$%^&*()_+\x08\tQWERTYUIOP{}\n\0ASDFGHJKL:\"~\0|ZXCVBNM<>?\0*\0 ";

struct State {
    left_shift: bool,
    right_shift: bool,
    ctrl: bool,
    caps_lock: bool,
    extended: bool,
}

static STATE: SpinLock<State> =
    SpinLock::new(State { left_shift: false, right_shift: false, ctrl: false, caps_lock: false, extended: false });

pub fn handle_irq() {
    let scancode = unsafe { inb(0x60) };
    let mut state = STATE.lock();

    if scancode == 0xE0 {
        state.extended = true;
        return;
    }
    let released = scancode & 0x80 != 0;
    let code = scancode & 0x7F;

    if state.extended {
        state.extended = false;
        match code {
            0x1D => state.ctrl = !released,
            _ if released => {}
            0x48 => input::push_all(b"\x1b[A"),
            0x50 => input::push_all(b"\x1b[B"),
            0x4D => input::push_all(b"\x1b[C"),
            0x4B => input::push_all(b"\x1b[D"),
            0x1C => input::push(b'\n'),
            0x35 => input::push(b'/'),
            _ => {}
        }
        return;
    }

    match code {
        0x2A => state.left_shift = !released,
        0x36 => state.right_shift = !released,
        0x1D => state.ctrl = !released,
        0x3A if !released => state.caps_lock = !state.caps_lock,
        _ if released => {}
        _ => {
            let Some(&normal) = NORMAL.get(code as usize) else { return };
            if normal == 0 {
                return;
            }
            let shift = state.left_shift || state.right_shift;
            let byte = if normal.is_ascii_lowercase() {
                if shift != state.caps_lock { normal.to_ascii_uppercase() } else { normal }
            } else if shift {
                SHIFTED[code as usize]
            } else {
                normal
            };
            if state.ctrl && byte.is_ascii_alphabetic() {
                input::push(byte.to_ascii_lowercase() & 0x1F); // Ctrl+C = 3, Ctrl+L = 12...
            } else {
                input::push(byte);
            }
        }
    }
}
