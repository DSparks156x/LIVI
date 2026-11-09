// SPDX-FileCopyrightText: 2026 Drew Sparks
// SPDX-License-Identifier: GPL-3.0-or-later

// Copies a `View` into the Slint `CarpiState`. `include!`d where `carpi.slint` is compiled
// (livi-ui, and this crate's `shots` example), which each have their own generated types: the
// includer brings `CarpiState`, `CarpiPill`, `CarpiRow`, `CarpiCell`, `CarpiSpot`, `CarpiPoint`,
// `CarpiTick`, `CarpiLayout` and `CarpiKind` into scope.

/// The models `CarpiState` shows, kept so rows that didn't change keep their state.
#[derive(Default)]
pub struct Models {
    pills: std::rc::Rc<slint::VecModel<CarpiPill>>,
    rows: std::rc::Rc<slint::VecModel<CarpiRow>>,
    cells: std::rc::Rc<slint::VecModel<CarpiCell>>,
    spots: std::rc::Rc<slint::VecModel<CarpiSpot>>,
    learned: std::rc::Rc<slint::VecModel<CarpiPoint>>,
    manual: std::rc::Rc<slint::VecModel<CarpiPoint>>,
    ticks: std::rc::Rc<slint::VecModel<CarpiTick>>,
}

impl Models {
    /// Hands the models to `st`, once.
    pub fn wire(&self, st: &CarpiState<'_>) {
        use slint::ModelRc;
        st.set_pills(ModelRc::from(self.pills.clone()));
        st.set_rows(ModelRc::from(self.rows.clone()));
        st.set_cells(ModelRc::from(self.cells.clone()));
        st.set_spots(ModelRc::from(self.spots.clone()));
        st.set_plot_learned(ModelRc::from(self.learned.clone()));
        st.set_plot_manual(ModelRc::from(self.manual.clone()));
        st.set_plot_ticks(ModelRc::from(self.ticks.clone()));
    }

    pub fn fill(&self, st: &CarpiState<'_>, view: &livi_carpi_ui::view::View) {
        use livi_carpi_ui::view::{Act, Layout, RowKind, Spot};
        st.set_layout(match view.layout {
            Layout::List => CarpiLayout::List,
            Layout::Grid => CarpiLayout::Grid,
        });
        st.set_title(view.title.as_str().into());
        st.set_hint(view.hint.as_str().into());
        st.set_note(view.note.as_str().into());
        st.set_can_back(view.back);
        st.set_cols(view.cols as i32);
        st.set_outlined(view.outlined);
        sync(
            &self.pills,
            view.pills
                .iter()
                .map(|p| CarpiPill {
                    label: p.label.as_str().into(),
                    active: p.active,
                    tab: matches!(
                        p.act,
                        Act::Slots(livi_carpi_ui::slots::SlotAct::Page(_))
                    ),
                    focus_at: p.focus_at,
                })
                .collect(),
        );
        sync(
            &self.rows,
            view.rows
                .iter()
                .map(|r| CarpiRow {
                    kind: match r.kind {
                        RowKind::Section => CarpiKind::Section,
                        RowKind::Info => CarpiKind::Info,
                        RowKind::Route => CarpiKind::Route,
                        RowKind::Button => CarpiKind::Button,
                        RowKind::Toggle => CarpiKind::Toggle,
                        RowKind::Choice => CarpiKind::Choice,
                        RowKind::Step => CarpiKind::Step,
                    },
                    label: r.label.as_str().into(),
                    value: r.value.as_str().into(),
                    button: r.button.as_str().into(),
                    tag: r.tag.as_str().into(),
                    dimmed: r.dimmed,
                    alert: r.alert,
                    checked: r.checked,
                    mono: r.mono,
                    focus_at: r.focus_at,
                    first: r.first,
                    last: r.last,
                })
                .collect(),
        );
        sync(
            &self.cells,
            view.cells
                .iter()
                .map(|c| CarpiCell {
                    label: c.label.as_str().into(),
                    value: c.value.as_str().into(),
                    unit: c.unit.as_str().into(),
                    why: c.why.as_str().into(),
                    dim: c.dim,
                    empty: c.empty,
                    focus_at: c.focus_at,
                })
                .collect(),
        );
        sync(
            &self.spots,
            view.spots
                .iter()
                .map(|s| match *s {
                    Spot::Header => CarpiSpot {
                        line: -1,
                        rows: 0,
                        sections: 0,
                    },
                    Spot::Tile(line) => CarpiSpot {
                        line,
                        rows: 0,
                        sections: 0,
                    },
                    Spot::Row(rows, sections) => CarpiSpot {
                        line: -2,
                        rows,
                        sections,
                    },
                })
                .collect(),
        );
        st.set_plot_shown(view.plot.is_some());
        if let Some(p) = &view.plot {
            let point = |(x, y): (f32, f32)| CarpiPoint { x, y };
            st.set_plot_curve(p.curve.as_str().into());
            st.set_plot_caption(p.caption.as_str().into());
            st.set_plot_now_shown(p.now.is_some());
            st.set_plot_now(point(p.now.unwrap_or_default()));
            sync(&self.learned, p.learned.iter().copied().map(point).collect());
            sync(&self.manual, p.manual.iter().copied().map(point).collect());
            sync(
                &self.ticks,
                p.ticks
                    .iter()
                    .map(|(at, label)| CarpiTick {
                        at: *at,
                        label: label.as_str().into(),
                    })
                    .collect(),
            );
        }
    }
}

/// Updates `model` in place, so rows that didn't change keep their state.
fn sync<T: Clone + PartialEq + 'static>(model: &slint::VecModel<T>, items: Vec<T>) {
    use slint::Model;
    let n = items.len();
    for (i, item) in items.into_iter().enumerate() {
        if i >= model.row_count() {
            model.push(item);
        } else if model.row_data(i).as_ref() != Some(&item) {
            model.set_row_data(i, item);
        }
    }
    while model.row_count() > n {
        model.remove(model.row_count() - 1);
    }
}

/// Copies the on-screen keyboard (or its absence) into `CarpiKeyboard`.
pub fn fill_keyboard(kb: &CarpiKeyboard<'_>, k: Option<&livi_carpi_ui::keyboard::Keyboard>) {
    use slint::{ModelRc, VecModel};
    let Some(k) = k else {
        kb.set_shown(false);
        return;
    };
    let shown = k.shown();
    let at = shown
        .char_indices()
        .nth(k.caret())
        .map_or(shown.len(), |(i, _)| i);
    kb.set_shown(true);
    kb.set_touch(k.touch);
    kb.set_title(k.title.as_str().into());
    kb.set_before(shown[..at].into());
    kb.set_after(shown[at..].into());
    kb.set_count(k.text().chars().count() as i32);
    let (fr, fk) = k.focus().map_or((-1, -1), |(r, i)| (r as i32, i as i32));
    kb.set_focus_row(fr);
    kb.set_focus_key(fk);
    let rows: Vec<CarpiKeyRow> = k
        .rows()
        .iter()
        .map(|row| CarpiKeyRow {
            keys: ModelRc::new(VecModel::from(
                row.iter()
                    .map(|key| CarpiKey {
                        label: key.label.as_str().into(),
                        span: i32::from(key.span),
                        kind: match key.id {
                            livi_carpi_ui::keyboard::KeyId::Done => 2,
                            _ if key.is_control() => 1,
                            _ => 0,
                        },
                    })
                    .collect::<Vec<_>>(),
            )),
        })
        .collect();
    kb.set_rows(ModelRc::new(VecModel::from(rows)));
}
