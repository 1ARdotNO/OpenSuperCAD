//! OpenSCAD's Customizer: groups, sliders, dropdowns, checkboxes and fields
//! generated from the parameter comments in the source.

use std::collections::BTreeMap;

use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::scroll::ScrollableElement;
use gpui_kit::component::select::{Select, SelectEvent, SelectState};
use gpui_kit::component::slider::{Slider, SliderEvent, SliderState};
use gpui_kit::component::switch::Switch;
use gpui_kit::component::{ActiveTheme, IndexPath, Sizable, h_flex, v_flex};
use gpui_kit::prelude::*;
use gpui_kit::*;
use osc_syntax::customizer::{Parameter, Value, Widget};

pub enum CustomizerEvent {
    Changed { name: String, value: Value },
}

impl EventEmitter<CustomizerEvent> for Customizer {}

enum Control {
    Slider(Entity<SliderState>),
    Select(Entity<SelectState<Vec<SharedString>>>, Vec<Value>),
    Toggle,
    Field(Entity<InputState>),
}

struct Row {
    param: Parameter,
    control: Control,
}

#[derive(Default)]
pub struct Customizer {
    rows: Vec<Row>,
    /// Names + widgets of the current rows; controls are rebuilt only when
    /// this changes, so editing values doesn't reset focus or drags.
    signature: String,
    _subs: Vec<Subscription>,
}

fn signature(params: &[Parameter]) -> String {
    params
        .iter()
        .map(|p| {
            format!(
                "{}:{:?}:{:?}",
                p.name,
                p.widget,
                std::mem::discriminant(&p.value)
            )
        })
        .collect::<Vec<_>>()
        .join("|")
}

fn value_text(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        other => other.to_scad(),
    }
}

impl Customizer {
    pub fn is_empty(&self) -> bool {
        self.rows.iter().all(|r| r.param.hidden)
    }

    /// Update from freshly parsed parameters.
    pub fn sync(&mut self, params: Vec<Parameter>, window: &mut Window, cx: &mut Context<Self>) {
        let sig = signature(&params);
        if sig != self.signature {
            self.signature = sig;
            self.rebuild(params, window, cx);
        } else {
            for (row, param) in self.rows.iter_mut().zip(params) {
                if row.param.value != param.value {
                    match (&row.control, &param.value) {
                        (Control::Slider(s), Value::Number(n)) => {
                            s.update(cx, |s, cx| s.set_value(*n as f32, window, cx))
                        }
                        (Control::Field(f), v) => {
                            let focused = f.read(cx).focus_handle(cx).is_focused(window);
                            if !focused {
                                let text = value_text(v);
                                f.update(cx, |f, cx| f.set_value(text, window, cx));
                            }
                        }
                        (Control::Select(s, values), v) => {
                            if let Some(ix) = values.iter().position(|c| c == v) {
                                s.update(cx, |s, cx| {
                                    s.set_selected_index(Some(IndexPath::new(ix)), window, cx)
                                });
                            }
                        }
                        _ => {}
                    }
                }
                row.param = param;
            }
        }
        cx.notify();
    }

    fn rebuild(&mut self, params: Vec<Parameter>, window: &mut Window, cx: &mut Context<Self>) {
        self._subs.clear();
        self.rows.clear();
        for param in params.into_iter().filter(|p| !p.hidden) {
            let name = param.name.clone();
            let control = match (&param.widget, &param.value) {
                (Widget::Slider { min, max, step }, Value::Number(n)) => {
                    let (min, max) = (*min as f32, *max as f32);
                    let step =
                        step.map(|s| s as f32)
                            .unwrap_or(if max - min > 20.0 { 1.0 } else { 0.1 });
                    let state = cx.new(|_| {
                        SliderState::new()
                            .min(min)
                            .max(max)
                            .step(step)
                            .default_value(*n as f32)
                    });
                    self._subs
                        .push(cx.subscribe(&state, move |_, _, ev: &SliderEvent, cx| {
                            let SliderEvent::Change(v) = ev else { return };
                            let value =
                                Value::Number(round_to(f64::from(v.start()), f64::from(step)));
                            cx.emit(CustomizerEvent::Changed {
                                name: name.clone(),
                                value,
                            });
                        }));
                    Control::Slider(state)
                }
                (Widget::Dropdown { choices }, current) => {
                    let labels: Vec<SharedString> = choices
                        .iter()
                        .map(|c| SharedString::from(c.label.clone()))
                        .collect();
                    let values: Vec<Value> = choices.iter().map(|c| c.value.clone()).collect();
                    let selected = values.iter().position(|v| v == current).map(IndexPath::new);
                    let state = cx.new(|cx| SelectState::new(labels.clone(), selected, window, cx));
                    let lookup = values.clone();
                    self._subs.push(cx.subscribe(
                        &state,
                        move |_, _, ev: &SelectEvent<Vec<SharedString>>, cx| {
                            let SelectEvent::Confirm(Some(label)) = ev else {
                                return;
                            };
                            if let Some(ix) = labels.iter().position(|l| l == label) {
                                cx.emit(CustomizerEvent::Changed {
                                    name: name.clone(),
                                    value: lookup[ix].clone(),
                                });
                            }
                        },
                    ));
                    Control::Select(state, values)
                }
                (_, Value::Bool(_)) => Control::Toggle,
                (_, value) => {
                    let text = value_text(value);
                    let kind = value.clone();
                    let state = cx.new(|cx| InputState::new(window, cx).default_value(text));
                    self._subs.push(cx.subscribe_in(
                        &state,
                        window,
                        move |_, state, ev: &InputEvent, _, cx| {
                            if !matches!(ev, InputEvent::PressEnter { .. } | InputEvent::Blur) {
                                return;
                            }
                            let raw = state.read(cx).value().to_string();
                            if let Some(value) = parse_like(&kind, &raw) {
                                cx.emit(CustomizerEvent::Changed {
                                    name: name.clone(),
                                    value,
                                });
                            }
                        },
                    ));
                    Control::Field(state)
                }
            };
            self.rows.push(Row { param, control });
        }
    }
}

fn round_to(v: f64, step: f64) -> f64 {
    if step <= 0.0 {
        return v;
    }
    let r = (v / step).round() * step;
    // Trim float noise like 0.30000000000000004.
    (r * 1e6).round() / 1e6
}

/// Parse a field's text as a value of the same type as `kind`.
fn parse_like(kind: &Value, raw: &str) -> Option<Value> {
    let raw = raw.trim();
    match kind {
        Value::Number(_) => raw.parse().ok().map(Value::Number),
        Value::String(_) => Some(Value::String(raw.to_owned())),
        Value::Vector(_) => raw
            .trim_start_matches('[')
            .trim_end_matches(']')
            .split(',')
            .filter(|s| !s.trim().is_empty())
            .map(|s| s.trim().parse::<f64>().ok())
            .collect::<Option<Vec<_>>>()
            .map(Value::Vector),
        Value::Bool(_) => match raw {
            "true" => Some(Value::Bool(true)),
            "false" => Some(Value::Bool(false)),
            _ => None,
        },
    }
}

impl Render for Customizer {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        if self.is_empty() {
            return v_flex()
                .size_full()
                .p_3()
                .gap_1()
                .text_sm()
                .text_color(theme.muted_foreground)
                .child("No customizer parameters.")
                .child(
                    div()
                        .text_xs()
                        .child("Top-level assignments like `width = 40; // [10:100]` before the first module appear here."),
                )
                .into_any_element();
        }

        let mut groups: BTreeMap<(usize, String), Vec<usize>> = BTreeMap::new();
        let mut order: Vec<String> = Vec::new();
        for (ix, row) in self.rows.iter().enumerate() {
            let g = row
                .param
                .group
                .clone()
                .unwrap_or_else(|| "Parameters".into());
            if !order.contains(&g) {
                order.push(g.clone());
            }
            let pos = order.iter().position(|o| *o == g).unwrap_or(0);
            groups.entry((pos, g)).or_default().push(ix);
        }

        let mut list = v_flex().gap_3().p_3();
        for ((_, group), rows) in groups {
            let mut section = v_flex().gap_2().child(
                div()
                    .text_xs()
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(theme.muted_foreground)
                    .child(group.to_uppercase()),
            );
            for ix in rows {
                section = section.child(self.render_row(ix, &theme, cx));
            }
            list = list.child(section);
        }
        div()
            .size_full()
            .overflow_y_scrollbar()
            .child(list)
            .into_any_element()
    }
}

impl Customizer {
    fn render_row(
        &self,
        ix: usize,
        theme: &gpui_kit::component::Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let row = &self.rows[ix];
        let p = &row.param;
        let label = h_flex()
            .justify_between()
            .child(div().text_sm().child(p.name.clone()))
            .child(
                div()
                    .text_xs()
                    .text_color(theme.muted_foreground)
                    .child(value_text(&p.value)),
            );
        let control: AnyElement = match &row.control {
            Control::Slider(s) => Slider::new(s).w_full().into_any_element(),
            Control::Select(s, _) => Select::new(s).small().into_any_element(),
            Control::Field(f) => Input::new(f).small().into_any_element(),
            Control::Toggle => {
                let checked = matches!(p.value, Value::Bool(true));
                let name = p.name.clone();
                Switch::new(SharedString::from(format!("sw-{}", p.name)))
                    .checked(checked)
                    .small()
                    .on_click(cx.listener(move |_, v: &bool, _, cx| {
                        cx.emit(CustomizerEvent::Changed {
                            name: name.clone(),
                            value: Value::Bool(*v),
                        })
                    }))
                    .into_any_element()
            }
        };
        v_flex()
            .gap_1()
            .child(label)
            .when_some(p.description.clone(), |el, d| {
                el.child(div().text_xs().text_color(theme.muted_foreground).child(d))
            })
            .child(control)
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::{Value, parse_like, round_to};

    #[test]
    fn parses_field_values() {
        assert_eq!(
            parse_like(&Value::Number(0.0), " 2.5 "),
            Some(Value::Number(2.5))
        );
        assert_eq!(parse_like(&Value::Number(0.0), "x"), None);
        assert_eq!(
            parse_like(&Value::Vector(vec![]), "[1, 2,3]"),
            Some(Value::Vector(vec![1.0, 2.0, 3.0]))
        );
        assert_eq!(round_to(0.30000000000000004, 0.1), 0.3);
        assert_eq!(round_to(17.4, 1.0), 17.0);
    }
}
