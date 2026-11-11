// SPDX-FileCopyrightText: 2026 Drew Sparks
// SPDX-License-Identifier: GPL-3.0-or-later

// Copies a `View` into the Slint `CarpiState`. `include!`d where `carpi.slint` is compiled
// (livi-ui, and this crate's `shots` example), which each have their own generated types: the
// includer brings `CarpiState`, `CarpiPill`, `CarpiRow`, `CarpiCell`, `CarpiSpot`, `CarpiPoint`,
// `CarpiTick`, `CarpiLayout`, `CarpiKind` and the panel types (`CarpiWidget`, `CarpiLine`,
// `CarpiItem`, `CarpiMark`, `CarpiEmpty`) into scope.

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
    widgets: std::rc::Rc<slint::VecModel<CarpiWidget>>,
    empties: std::rc::Rc<slint::VecModel<CarpiEmpty>>,
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
        st.set_panel_widgets(ModelRc::from(self.widgets.clone()));
        st.set_panel_empties(ModelRc::from(self.empties.clone()));
    }

    pub fn fill(&self, st: &CarpiState<'_>, view: &livi_carpi_ui::view::View) {
        use livi_carpi_ui::view::{Act, Layout, RowKind, Spot};
        st.set_layout(match view.layout {
            Layout::List => CarpiLayout::List,
            Layout::Grid => CarpiLayout::Grid,
            Layout::Panel => CarpiLayout::Panel,
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
                        Act::Slots(livi_carpi_ui::slots::SlotAct::Page(_)) | Act::Tab(_)
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
                    Spot::Panel => CarpiSpot {
                        line: 0,
                        rows: 0,
                        sections: 0,
                    },
                })
                .collect(),
        );
        st.set_panel_shown(view.panel.is_some());
        if let Some(p) = &view.panel {
            st.set_panel_cols(i32::from(p.cols));
            st.set_panel_rows(i32::from(p.rows));
            st.set_panel_dots(p.dots as i32);
            st.set_panel_at(p.at as i32);
            st.set_panel_edit(p.edit);
            st.set_panel_hint(p.hint.as_str().into());
            sync(&self.widgets, p.widgets.iter().map(widget).collect());
            sync(
                &self.empties,
                p.empties
                    .iter()
                    .map(|e| CarpiEmpty {
                        col: i32::from(e.col),
                        row: i32::from(e.row),
                        focus_at: e.focus_at,
                    })
                    .collect(),
            );
        } else {
            sync(&self.widgets, Vec::new());
            sync(&self.empties, Vec::new());
        }
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

/// A panel widget for Slint: labels upper case, the paths and lists as models.
fn widget(w: &livi_carpi_ui::panels::WidgetDraw) -> CarpiWidget {
    use slint::ModelRc;
    let marks = |m: &[livi_carpi_ui::panels::Tick]| {
        ModelRc::new(slint::VecModel::from(
            m.iter()
                .map(|t| CarpiMark {
                    x: t.x,
                    y: t.y,
                    label: t.label.as_str().into(),
                })
                .collect::<Vec<_>>(),
        ))
    };
    CarpiWidget {
        col: i32::from(w.col),
        row: i32::from(w.row),
        w: i32::from(w.w),
        h: i32::from(w.h),
        style: w.style,
        label: w.label.to_uppercase().into(),
        value: w.value.as_str().into(),
        unit: w.unit.as_str().into(),
        sub: w.sub.as_str().into(),
        sub_hi: w.sub_hi,
        value2: w.value2.as_str().into(),
        label2: w.label2.to_uppercase().into(),
        state: w.state,
        stale: w.stale,
        frac: w.frac,
        ticks_off: w.ticks_off.as_str().into(),
        ticks_on: w.ticks_on.as_str().into(),
        red_off: w.red_off.as_str().into(),
        red_on: w.red_on.as_str().into(),
        needle: w.needle.as_str().into(),
        peak: w.peak.as_str().into(),
        labels: marks(&w.labels),
        segs: w.segs,
        lit: w.lit,
        warn_from: w.warn_from,
        limit_from: w.limit_from,
        lines: ModelRc::new(slint::VecModel::from(
            w.lines
                .iter()
                .map(|l| CarpiLine {
                    path: l.path.as_str().into(),
                    color: l.color,
                    label: l.label.as_str().into(),
                })
                .collect::<Vec<_>>(),
        )),
        marks: marks(&w.marks),
        items: ModelRc::new(slint::VecModel::from(
            w.items
                .iter()
                .map(|i| CarpiItem {
                    label: i.label.as_str().into(),
                    value: i.value.as_str().into(),
                    frac: i.frac,
                    on: i.on,
                    state: i.state,
                })
                .collect::<Vec<_>>(),
        )),
        focus_at: w.focus_at,
        picked: w.picked,
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
