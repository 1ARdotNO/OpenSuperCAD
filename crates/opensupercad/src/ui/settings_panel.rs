//! Project and OpenSCAD settings (`.opensupercad/settings.json`).

use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::scroll::ScrollableElement;
use gpui_kit::component::select::{Select, SelectEvent, SelectState};
use gpui_kit::component::switch::Switch;
use gpui_kit::component::{ActiveTheme, IndexPath, Sizable, h_flex, v_flex};
use gpui_kit::prelude::*;
use gpui_kit::*;
use osc_agent::Registry;
use osc_project::{Project, ProjectSettings};

pub enum SettingsEvent {
    Changed(ProjectSettings),
}

impl EventEmitter<SettingsEvent> for SettingsPanel {}

const BACKENDS: [(&str, Option<&str>); 3] = [
    ("OpenSCAD default", None),
    ("Manifold (fast)", Some("manifold")),
    ("CGAL", Some("cgal")),
];

pub struct SettingsPanel {
    settings: ProjectSettings,
    has_project: bool,
    main_file: Option<Entity<SelectState<Vec<SharedString>>>>,
    agent: Option<Entity<SelectState<Vec<SharedString>>>>,
    backend: Option<Entity<SelectState<Vec<SharedString>>>>,
    _subs: Vec<Subscription>,
}

impl SettingsPanel {
    pub fn new() -> Self {
        Self {
            settings: ProjectSettings::default(),
            has_project: false,
            main_file: None,
            agent: None,
            backend: None,
            _subs: Vec::new(),
        }
    }

    /// Rebuild the controls for a project.
    pub fn set_project(
        &mut self,
        project: Option<&Project>,
        registry: &Registry,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self._subs.clear();
        self.has_project = project.is_some();
        let Some(project) = project else {
            self.main_file = None;
            self.agent = None;
            self.backend = None;
            cx.notify();
            return;
        };
        self.settings = project.settings();

        // Main file.
        let files = project.scad_files();
        let current = project.main_file();
        let labels: Vec<SharedString> = files
            .iter()
            .map(|f| SharedString::from(f.clone()))
            .collect();
        let selected = current
            .and_then(|c| files.iter().position(|f| *f == c))
            .map(IndexPath::new);
        let main = cx.new(|cx| SelectState::new(labels.clone(), selected, window, cx));
        self._subs.push(cx.subscribe(
            &main,
            move |this, _, ev: &SelectEvent<Vec<SharedString>>, cx| {
                if let SelectEvent::Confirm(Some(v)) = ev {
                    this.settings.main_file = Some(v.to_string());
                    this.changed(cx);
                }
            },
        ));
        self.main_file = Some(main);

        // Default agent.
        let ids: Vec<String> = registry.agents().iter().map(|a| a.id.clone()).collect();
        let names: Vec<SharedString> = registry
            .agents()
            .iter()
            .map(|a| SharedString::from(a.name.clone()))
            .collect();
        let selected = self
            .settings
            .default_agent
            .as_ref()
            .and_then(|id| ids.iter().position(|i| i == id))
            .map(IndexPath::new);
        let agent = cx.new(|cx| SelectState::new(names.clone(), selected, window, cx));
        self._subs.push(cx.subscribe(
            &agent,
            move |this, _, ev: &SelectEvent<Vec<SharedString>>, cx| {
                if let SelectEvent::Confirm(Some(v)) = ev
                    && let Some(ix) = names.iter().position(|n| n == v)
                {
                    this.settings.default_agent = Some(ids[ix].clone());
                    this.changed(cx);
                }
            },
        ));
        self.agent = Some(agent);

        // OpenSCAD backend.
        let labels: Vec<SharedString> = BACKENDS
            .iter()
            .map(|(l, _)| SharedString::from(*l))
            .collect();
        let selected = BACKENDS
            .iter()
            .position(|(_, b)| *b == self.settings.openscad_backend.as_deref())
            .map(IndexPath::new);
        let backend = cx.new(|cx| SelectState::new(labels.clone(), selected, window, cx));
        self._subs.push(cx.subscribe(
            &backend,
            move |this, _, ev: &SelectEvent<Vec<SharedString>>, cx| {
                if let SelectEvent::Confirm(Some(v)) = ev
                    && let Some((_, b)) = BACKENDS.iter().find(|(l, _)| *l == v.as_ref())
                {
                    this.settings.openscad_backend = b.map(str::to_owned);
                    this.changed(cx);
                }
            },
        ));
        self.backend = Some(backend);
        cx.notify();
    }

    fn changed(&mut self, cx: &mut Context<Self>) {
        cx.emit(SettingsEvent::Changed(self.settings.clone()));
        cx.notify();
    }
}

fn open_agents_json(cx: &mut App) {
    let path = crate::agents::config_dir().join("agents.json");
    if !path.exists() {
        let _ = std::fs::create_dir_all(crate::agents::config_dir());
        let template = r#"[
  {
    "id": "my-agent",
    "name": "My agent",
    "command": "my-acp-agent",
    "args": [],
    "env": {}
  }
]
"#;
        let _ = std::fs::write(&path, template);
    }
    cx.open_with_system(&path);
}

impl Render for SettingsPanel {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let row = |label: &'static str, help: &'static str, control: AnyElement| {
            v_flex()
                .gap_1()
                .child(div().text_sm().child(label))
                .child(
                    div()
                        .text_xs()
                        .text_color(theme.muted_foreground)
                        .child(help),
                )
                .child(control)
        };
        let heading = |t: &'static str| {
            div()
                .text_xs()
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(theme.muted_foreground)
                .child(t)
        };

        let mut list = v_flex().p_3().gap_4();
        if self.has_project {
            list = list.child(heading("PROJECT"));
            if let Some(main) = &self.main_file {
                list = list.child(row(
                    "Main file",
                    "Rendered by default and used by the agent's tools.",
                    Select::new(main)
                        .small()
                        .placeholder("main.scad")
                        .into_any_element(),
                ));
            }
            if let Some(agent) = &self.agent {
                list = list.child(row(
                    "Default agent",
                    "Selected when this project is opened.",
                    Select::new(agent)
                        .small()
                        .placeholder("First available")
                        .into_any_element(),
                ));
            }
            let auto = self.settings.auto_checkpoint();
            list = list.child(row(
                "Checkpoint after each AI turn",
                "Snapshots all files to osc/checkpoints/<branch> so every iteration can be restored.",
                Switch::new("auto-checkpoint")
                    .checked(auto)
                    .small()
                    .on_click(cx.listener(|this, v: &bool, _, cx| {
                        this.settings.auto_checkpoint = Some(*v);
                        this.changed(cx);
                    }))
                    .into_any_element(),
            ));
            if let Some(backend) = &self.backend {
                list = list.child(row(
                    "OpenSCAD geometry backend",
                    "Manifold is much faster on OpenSCAD 2024+; older releases only have CGAL.",
                    Select::new(backend).small().into_any_element(),
                ));
            }
            list =
                list.child(div().text_xs().text_color(theme.muted_foreground).child(
                    "Saved to .opensupercad/settings.json; commit it to share with your team.",
                ));
        }
        list = list
            .child(heading("APPLICATION"))
            .child(
                h_flex().gap_2().child(
                    Button::new("theme")
                        .label("Toggle light/dark theme")
                        .small()
                        .on_click(|_, window, cx| {
                            window.dispatch_action(Box::new(super::ToggleTheme), cx)
                        }),
                ),
            )
            .child(row(
                "Agents",
                "Add agents or override the built-in commands in agents.json.",
                Button::new("agents-json")
                    .label("Open agents.json")
                    .small()
                    .ghost()
                    .on_click(|_, _, cx| open_agents_json(cx))
                    .into_any_element(),
            ));
        div().size_full().overflow_y_scrollbar().child(list)
    }
}
