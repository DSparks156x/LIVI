// SPDX-FileCopyrightText: 2026 Drew Sparks
// SPDX-License-Identifier: GPL-3.0-or-later

//! The on-screen keyboard (docs/PAGES.md 5b): one sheet, two layouts.
//!
//! Knob: letters in rows of 10 (alphabetical, or QWERTY), a last row of controls. The knob walks
//! every key in reading order and wraps; Up/Down jump a row keeping the column; push types;
//! the D-pad (Prev/Next held) moves the cursor; Return deletes; Return held closes. Keys are
//! tappable too.
//!
//! Touch: phone-style QWERTY with a 123 page. The faceplate still works on it.

use serde::{Deserialize, Serialize};

/// Which keyboard opens for a text field.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Style {
    #[default]
    Knob,
    Touch,
    /// None: a USB or Bluetooth keyboard types.
    Off,
}

/// The knob keyboard's letters.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Order {
    #[default]
    Alphabetical,
    Qwerty,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Prefs {
    pub style: Style,
    pub order: Order,
    /// Passwords show while typing.
    pub show_secret: bool,
}

impl Default for Prefs {
    fn default() -> Self {
        Prefs {
            style: Style::Knob,
            order: Order::Alphabetical,
            show_secret: true,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyId {
    Char(char),
    Shift,
    Symbols,
    Space,
    Delete,
    /// Show or hide a password.
    Show,
    Done,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Key {
    pub id: KeyId,
    pub label: String,
    /// Width in key units.
    pub span: u8,
}

impl Key {
    fn ch(c: char) -> Key {
        Key {
            id: KeyId::Char(c),
            label: c.to_string(),
            span: 1,
        }
    }
    fn f(id: KeyId, label: &str, span: u8) -> Key {
        Key {
            id,
            label: label.into(),
            span,
        }
    }
    /// A control key, drawn darker; Done is drawn as the action.
    pub fn is_control(&self) -> bool {
        !matches!(self.id, KeyId::Char(_))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Layer {
    Lower,
    /// One capital, then back to lower.
    Upper,
    Symbols,
}

/// What the faceplate (or a tap) does to the keyboard.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Input {
    /// Knob detents.
    Turn(i32),
    Up,
    Down,
    /// Knob push: types the lit key.
    Push,
    /// D-pad: moves the cursor.
    Left,
    Right,
    /// Return: deletes before the cursor.
    Delete,
    /// Return held: closes without saving.
    Close,
    /// A tap on key `i` of row `r`.
    Tap(usize, usize),
    /// A real keyboard's character.
    Char(char),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    /// Still open.
    Open,
    Done(String),
    Cancelled,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Keyboard {
    pub title: String,
    /// The action key's word: Join, Save, Done.
    pub action: String,
    pub secret: bool,
    pub show: bool,
    pub touch: bool,
    order: Order,
    layer: Layer,
    text: Vec<char>,
    caret: usize,
    /// Characters at most.
    pub max: usize,
    /// Knob keyboard: the lit key, (row, index).
    focus: (usize, usize),
}

const ABC: [&str; 3] = ["abcdefghij", "klmnopqrst", "uvwxyz-_.@"];
const QWERTY: [&str; 3] = ["qwertyuiop", "asdfghjkl'", "zxcvbnm-_."];
const SYMBOLS: [&str; 3] = ["1234567890", "!#$%&*()=+", "/\\:;?,\"<>~"];
const TOUCH: [&str; 3] = ["qwertyuiop", "asdfghjkl", "zxcvbnm"];
const TOUCH_SYMBOLS: [&str; 3] = ["1234567890", "-/:;()$&@\"", ".,?!'#%+="];

impl Keyboard {
    pub fn open(title: &str, initial: &str, secret: bool, action: &str, prefs: Prefs) -> Keyboard {
        let text: Vec<char> = initial.chars().collect();
        Keyboard {
            title: title.into(),
            action: action.into(),
            secret,
            show: !secret || prefs.show_secret,
            touch: prefs.style == Style::Touch,
            order: prefs.order,
            layer: Layer::Lower,
            caret: text.len(),
            text,
            max: 63,
            focus: (0, 0),
        }
    }

    pub fn text(&self) -> String {
        self.text.iter().collect()
    }

    pub fn caret(&self) -> usize {
        self.caret
    }

    /// The field as shown: dots for a hidden password.
    pub fn shown(&self) -> String {
        if self.show {
            self.text()
        } else {
            "•".repeat(self.text.len())
        }
    }

    /// The lit key on the knob keyboard; `None` on the touch one.
    pub fn focus(&self) -> Option<(usize, usize)> {
        (!self.touch).then_some(self.focus)
    }

    pub fn rows(&self) -> Vec<Vec<Key>> {
        let up = self.layer == Layer::Upper;
        let case = |c: char| if up { c.to_ascii_uppercase() } else { c };
        let letters = |set: [&str; 3]| -> Vec<Vec<Key>> {
            set.iter()
                .map(|r| r.chars().map(|c| Key::ch(case(c))).collect())
                .collect()
        };
        let sym = self.layer == Layer::Symbols;
        let show = if self.show { "Hide" } else { "Show" };
        if self.touch {
            let mut rows = letters(if sym { TOUCH_SYMBOLS } else { TOUCH });
            if !sym {
                rows[2].insert(0, Key::f(KeyId::Shift, "⇧", 2));
            }
            rows[2].push(Key::f(KeyId::Delete, "⌫", 2));
            let mut last = vec![Key::f(KeyId::Symbols, if sym { "abc" } else { "123" }, 2)];
            if self.secret {
                last.push(Key::f(KeyId::Show, show, 2));
            }
            last.push(Key::f(KeyId::Space, "space", 8));
            last.push(Key::ch('.'));
            last.push(Key::f(KeyId::Done, &self.action, 3));
            rows.push(last);
            return rows;
        }
        let set = match (sym, self.order) {
            (true, _) => SYMBOLS,
            (false, Order::Alphabetical) => ABC,
            (false, Order::Qwerty) => QWERTY,
        };
        let mut rows = letters(set);
        // Ten columns, like the letters: Show takes one from space.
        let mut last = vec![
            Key::f(KeyId::Shift, if up { "⇧ abc" } else { "⇧ ABC" }, 2),
            Key::f(KeyId::Symbols, if sym { "abc" } else { "123 #+" }, 2),
            Key::f(KeyId::Space, "space", if self.secret { 2 } else { 3 }),
            Key::f(KeyId::Delete, "⌫", 1),
        ];
        if self.secret {
            last.push(Key::f(KeyId::Show, show, 1));
        }
        last.push(Key::f(KeyId::Done, &self.action, 2));
        rows.push(last);
        rows
    }

    fn type_char(&mut self, c: char) {
        if self.text.len() >= self.max {
            return;
        }
        self.text.insert(self.caret, c);
        self.caret += 1;
        if self.layer == Layer::Upper {
            self.layer = Layer::Lower;
        }
    }

    fn delete(&mut self) {
        if self.caret > 0 {
            self.caret -= 1;
            self.text.remove(self.caret);
        }
    }

    fn press(&mut self, id: KeyId) -> Outcome {
        match id {
            KeyId::Char(c) => self.type_char(c),
            KeyId::Shift => {
                self.layer = match self.layer {
                    Layer::Upper => Layer::Lower,
                    _ => Layer::Upper,
                }
            }
            KeyId::Symbols => {
                self.layer = match self.layer {
                    Layer::Symbols => Layer::Lower,
                    _ => Layer::Symbols,
                }
            }
            KeyId::Space => self.type_char(' '),
            KeyId::Delete => self.delete(),
            KeyId::Show => self.show = !self.show,
            KeyId::Done => return Outcome::Done(self.text()),
        }
        Outcome::Open
    }

    /// Where a column lands in a row: the key covering that column.
    fn at_col(row: &[Key], col: usize) -> usize {
        let mut c = 0;
        for (i, k) in row.iter().enumerate() {
            c += usize::from(k.span);
            if col < c {
                return i;
            }
        }
        row.len().saturating_sub(1)
    }

    fn col_of(row: &[Key], i: usize) -> usize {
        row[..i].iter().map(|k| usize::from(k.span)).sum()
    }

    fn clamp_focus(&mut self) {
        let rows = self.rows();
        let r = self.focus.0.min(rows.len() - 1);
        let i = self.focus.1.min(rows[r].len() - 1);
        self.focus = (r, i);
    }

    pub fn input(&mut self, input: Input) -> Outcome {
        let rows = self.rows();
        self.clamp_focus();
        let (r, i) = self.focus;
        let out = match input {
            Input::Turn(n) => {
                let flat: Vec<(usize, usize)> = rows
                    .iter()
                    .enumerate()
                    .flat_map(|(ri, row)| (0..row.len()).map(move |ii| (ri, ii)))
                    .collect();
                let at = flat.iter().position(|p| *p == (r, i)).unwrap_or(0) as i64;
                let len = flat.len() as i64;
                self.focus = flat[(at + i64::from(n)).rem_euclid(len) as usize];
                Outcome::Open
            }
            Input::Up | Input::Down => {
                let n = rows.len();
                let to = if input == Input::Down {
                    (r + 1) % n
                } else {
                    (r + n - 1) % n
                };
                let col = Self::col_of(&rows[r], i);
                self.focus = (to, Self::at_col(&rows[to], col));
                Outcome::Open
            }
            Input::Push => match rows.get(r).and_then(|row| row.get(i)) {
                Some(k) if !self.touch => self.press(k.id),
                _ => Outcome::Open,
            },
            Input::Tap(tr, ti) => match rows.get(tr).and_then(|row| row.get(ti)) {
                Some(k) => {
                    if !self.touch {
                        self.focus = (tr, ti);
                    }
                    self.press(k.id)
                }
                None => Outcome::Open,
            },
            Input::Left => {
                self.caret = self.caret.saturating_sub(1);
                Outcome::Open
            }
            Input::Right => {
                self.caret = (self.caret + 1).min(self.text.len());
                Outcome::Open
            }
            Input::Delete => {
                self.delete();
                Outcome::Open
            }
            Input::Char(c) => {
                self.type_char(c);
                Outcome::Open
            }
            Input::Close => Outcome::Cancelled,
        };
        self.clamp_focus();
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kb(secret: bool) -> Keyboard {
        Keyboard::open("Password", "", secret, "Join", Prefs::default())
    }

    fn type_by_knob(k: &mut Keyboard, word: &str) {
        for c in word.chars() {
            // Walk to the key with this character, then push.
            for _ in 0..200 {
                let rows = k.rows();
                let (r, i) = k.focus().unwrap();
                if rows[r][i].id == KeyId::Char(c) {
                    break;
                }
                k.input(Input::Turn(1));
            }
            assert_eq!(k.input(Input::Push), Outcome::Open);
        }
    }

    #[test]
    fn the_knob_reaches_every_key_and_types() {
        let mut k = kb(true);
        type_by_knob(&mut k, "ttn");
        assert_eq!(k.text(), "ttn");
        // Symbols for the digit.
        k.input(Input::Left);
        while k.rows()[k.focus().unwrap().0][k.focus().unwrap().1].id != KeyId::Symbols {
            k.input(Input::Turn(-1));
        }
        k.input(Input::Push);
        type_by_knob(&mut k, "8");
        assert_eq!(k.text(), "tt8n");
        assert_eq!(k.caret(), 3);
        // Turning back past the start wraps to the last key (Done).
        k.focus = (0, 0);
        k.input(Input::Turn(-1));
        let (r, i) = k.focus().unwrap();
        assert_eq!(k.rows()[r][i].id, KeyId::Done);
        assert_eq!(k.input(Input::Push), Outcome::Done("tt8n".into()));
    }

    #[test]
    fn up_down_keep_the_column_and_shift_is_one_capital() {
        let mut k = kb(false);
        k.focus = (0, 4); // e
        k.input(Input::Down);
        assert_eq!(k.focus(), Some((1, 4))); // o
        k.input(Input::Down);
        k.input(Input::Down);
        // Bottom row: column 4 is inside "123 #+" (columns 2-3) or space (4-5).
        let (r, i) = k.focus().unwrap();
        assert_eq!(r, 3);
        assert_eq!(k.rows()[r][i].id, KeyId::Space);
        k.input(Input::Tap(3, 0)); // shift
        k.input(Input::Tap(0, 0));
        k.input(Input::Tap(0, 1));
        assert_eq!(k.text(), "Ab");
    }

    #[test]
    fn faceplate_edits_and_closes() {
        let mut k = Keyboard::open("Name", "Daily", false, "Save", Prefs::default());
        k.input(Input::Left);
        k.input(Input::Delete);
        assert_eq!(k.text(), "Daiy");
        k.input(Input::Char('!'));
        assert_eq!(k.text(), "Dai!y");
        assert_eq!(k.input(Input::Close), Outcome::Cancelled);
    }

    #[test]
    fn secrets_mask_unless_shown() {
        let prefs = Prefs {
            show_secret: false,
            ..Prefs::default()
        };
        let mut k = Keyboard::open("Password", "abc", true, "Join", prefs);
        assert_eq!(k.shown(), "•••");
        let show = k.rows()[3]
            .iter()
            .position(|k| k.id == KeyId::Show)
            .unwrap();
        k.input(Input::Tap(3, show));
        assert_eq!(k.shown(), "abc");
    }

    #[test]
    fn touch_layout_is_qwerty_with_a_symbols_page() {
        let prefs = Prefs {
            style: Style::Touch,
            ..Prefs::default()
        };
        let mut k = Keyboard::open("Password", "", true, "Join", prefs);
        assert_eq!(k.focus(), None);
        assert_eq!(k.rows()[0][0].id, KeyId::Char('q'));
        k.input(Input::Tap(3, 0));
        assert_eq!(k.rows()[0][0].id, KeyId::Char('1'));
        // Push does nothing without a lit key.
        assert_eq!(k.input(Input::Push), Outcome::Open);
        assert_eq!(k.text(), "");
    }

    #[test]
    fn qwerty_order_for_the_knob() {
        let prefs = Prefs {
            order: Order::Qwerty,
            ..Prefs::default()
        };
        let k = Keyboard::open("x", "", false, "Save", prefs);
        assert_eq!(k.rows()[0][0].id, KeyId::Char('q'));
    }
}
