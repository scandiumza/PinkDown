use std::{ops::Range, path::PathBuf, time::Duration};

use eframe::egui::{self, Color32, FontFamily, FontId, RichText, TextFormat};
use egui_commonmark::CommonMarkCache;

use crate::{
    dialog::{self, Choice, FontSettingsAction, UpdateChoice, UpdatePromptMode},
    document::{pick_markdown_file, Document},
    export::{self, ExportFormat, ExportJob, ExportPoll},
    preview,
    settings::Settings,
    theme::{self, BASE, FOAM, GOLD, HIGHLIGHT_LOW, IRIS, MUTED, ROSE, SUBTLE, SURFACE, TEXT},
    update::{PollResult, UpdateChecker, UpdateOutcome, UpdateUi, AUTO_INSTALL},
};

#[cfg(target_os = "windows")]
use crate::theme::LOVE;

#[cfg(target_os = "macos")]
const SHORTCUT_MOD: &str = "Cmd";
#[cfg(not(target_os = "macos"))]
const SHORTCUT_MOD: &str = "Ctrl";

// Clearance for macOS traffic lights under full-size content / hidden titlebar.
#[cfg(target_os = "macos")]
const MACOS_TRAFFIC_LIGHT_INSET: f32 = 68.0;

/// App-drawn caption + command row.
const TOOLBAR_HEIGHT: f32 = 38.0;
/// Status line under the editor.
const STATUSBAR_HEIGHT: f32 = 28.0;

/// Gap between source and preview; also the drag hit strip.
const EDITOR_SPLIT_GAP: f32 = 12.0;
/// Neither pane shrinks below this until the window itself is too narrow.
const EDITOR_MIN_PANE: f32 = 180.0;
const FIND_BAR_HEIGHT: f32 = 40.0;

#[derive(Default)]
struct FindState {
    open: bool,
    query: String,
    matches: Vec<Range<usize>>,
    current: Option<usize>,
    jump_pending: bool,
    focus_pending: bool,
}

impl FindState {
    fn rebuild(&mut self, source: &str, reset_current: bool) {
        self.matches.clear();
        if !self.query.is_empty() {
            self.matches.extend(
                source
                    .match_indices(&self.query)
                    .map(|(start, found)| start..start + found.len()),
            );
        }

        self.current = if self.matches.is_empty() {
            None
        } else if reset_current {
            Some(0)
        } else {
            Some(self.current.unwrap_or(0).min(self.matches.len() - 1))
        };
        self.jump_pending = reset_current && self.current.is_some();
    }

    fn navigate(&mut self, reverse: bool) {
        let count = self.matches.len();
        if count == 0 {
            return;
        }
        let current = self.current.unwrap_or(0);
        self.current = Some(if reverse {
            (current + count - 1) % count
        } else {
            (current + 1) % count
        });
        self.jump_pending = true;
    }

    fn current_match(&self) -> Option<&Range<usize>> {
        self.current.and_then(|index| self.matches.get(index))
    }
}

pub struct PinkDown {
    document: Document,
    status: String,
    pending_action: Option<PendingAction>,
    allow_close: bool,
    markdown_cache: CommonMarkCache,
    update_checker: UpdateChecker,
    update_ui: UpdateUi,
    export_job: ExportJob,
    current_title: String,
    settings: Settings,
    /// Open font dialog; holds a draft typeface id until Apply / Cancel.
    font_settings_draft: Option<String>,
    /// Source pane share of the two-pane row (0.5 = equal).
    split_ratio: f32,
    find: FindState,
}

enum PendingAction {
    OpenDialog,
    OpenPath(PathBuf),
    Close,
    /// Quit so the staged updater can install (same close path, different copy).
    RestartForUpdate,
}

impl PendingAction {
    fn confirmation_text(&self) -> &'static str {
        match self {
            Self::OpenDialog | Self::OpenPath(_) => {
                "Save your changes before opening another document?"
            }
            Self::Close => "Save your changes before closing PinkDown?",
            Self::RestartForUpdate => "Save your changes before restarting to install the update?",
        }
    }
}

impl PinkDown {
    pub fn new(cc: &eframe::CreationContext<'_>, initial_path: Option<PathBuf>) -> Self {
        let settings = load_settings();
        theme::configure(&cc.egui_ctx, &settings.preferred_font);
        let mut app = Self {
            document: Document::default(),
            status: "Ready to write".into(),
            pending_action: None,
            allow_close: false,
            markdown_cache: CommonMarkCache::default(),
            update_checker: UpdateChecker::default(),
            update_ui: UpdateUi::Idle,
            export_job: ExportJob::default(),
            current_title: "PinkDown".into(),
            settings,
            font_settings_draft: None,
            split_ratio: 0.5,
            find: FindState::default(),
        };
        if let Some(path) = initial_path {
            app.open_path(path);
        }
        app
    }

    fn apply_font_settings(&mut self, draft: String, ctx: &egui::Context) {
        let preferred = theme::normalize_font_preference(&draft);
        let changed = preferred != self.settings.preferred_font;
        self.settings.preferred_font = preferred;
        self.settings.save();
        if changed {
            theme::configure_fonts(ctx, &self.settings.preferred_font);
            self.status = format!(
                "Font set to {}",
                theme::font_label(&self.settings.preferred_font)
            );
        }
        self.font_settings_draft = None;
    }

    fn show_font_settings(&mut self, ctx: &egui::Context) {
        let Some(mut draft) = self.font_settings_draft.take() else {
            return;
        };
        match dialog::font_settings(ctx, &mut draft) {
            FontSettingsAction::KeepOpen => self.font_settings_draft = Some(draft),
            FontSettingsAction::Apply => self.apply_font_settings(draft, ctx),
            FontSettingsAction::Cancel => self.font_settings_draft = None,
        }
    }

    fn request_action(&mut self, action: PendingAction, ctx: &egui::Context) {
        if self.document.is_dirty() {
            self.pending_action = Some(action);
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
        } else {
            self.execute_action(action, ctx);
        }
    }

    fn execute_action(&mut self, action: PendingAction, ctx: &egui::Context) {
        match action {
            PendingAction::OpenDialog => {
                if let Some(path) = pick_markdown_file() {
                    self.open_path(path);
                }
            }
            PendingAction::OpenPath(path) => self.open_path(path),
            PendingAction::Close | PendingAction::RestartForUpdate => {
                self.allow_close = true;
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            }
        }
    }

    fn open_path(&mut self, path: PathBuf) {
        match Document::load(path) {
            Ok(document) => {
                let name = document.display_name();
                let encoding = document.encoding_label();
                self.document = document;
                self.markdown_cache = CommonMarkCache::default();
                self.find.rebuild(&self.document.text, true);
                if !self.find.open {
                    self.find.jump_pending = false;
                }
                self.status = format!("Opened {name} · {encoding}");
            }
            Err(error) => self.status = error,
        }
    }

    fn save(&mut self, force_dialog: bool) -> bool {
        match self.document.save(force_dialog) {
            Ok(true) => {
                self.status = format!("Saved {}", self.document.display_name());
                true
            }
            Ok(false) => false,
            Err(error) => {
                self.status = error;
                false
            }
        }
    }

    fn export_document(&mut self, format: ExportFormat) {
        if self.export_job.is_busy() {
            self.status = "An export is already in progress\u{2026}".into();
            return;
        }

        let title = self.document.export_stem();
        let base_dir = self.document.base_dir().map(PathBuf::from);

        match format {
            ExportFormat::Html => {
                match export::export_html(&self.document.text, &title, base_dir.as_deref()) {
                    Ok(Some(path)) => self.status = format!("Exported {}", file_name_label(&path)),
                    Ok(None) => {}
                    Err(error) => self.status = error,
                }
            }
            ExportFormat::Pdf => {
                let Some(path) = export::pick_destination(&title, ExportFormat::Pdf) else {
                    return;
                };
                if self
                    .export_job
                    .start_pdf(path, self.document.text.clone(), title, base_dir)
                {
                    self.status = "Exporting PDF\u{2026}".into();
                } else {
                    self.status = "An export is already in progress\u{2026}".into();
                }
            }
        }
    }

    fn poll_export(&mut self, ctx: &egui::Context) {
        match self.export_job.poll() {
            ExportPoll::Idle => {}
            ExportPoll::Pending => ctx.request_repaint_after(Duration::from_millis(100)),
            ExportPoll::Ready(Ok(path)) => {
                self.status = format!("Exported {}", file_name_label(&path));
            }
            ExportPoll::Ready(Err(error)) => self.status = error,
        }
    }

    fn check_close_request(&mut self, ctx: &egui::Context) {
        let close_requested = ctx.input(|input| input.viewport().close_requested());
        if close_requested && !self.allow_close {
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            if self.pending_action.is_none() {
                self.request_action(PendingAction::Close, ctx);
            }
        }
    }

    fn sync_window_title(&mut self, ctx: &egui::Context) {
        let title = self
            .document
            .window_title()
            .unwrap_or_else(|| "PinkDown".to_owned());
        if title != self.current_title {
            self.current_title = title;
            ctx.send_viewport_cmd(egui::ViewportCommand::Title(self.current_title.clone()));
        }
    }

    fn poll_update(&mut self, ctx: &egui::Context) {
        match self.update_checker.poll() {
            PollResult::Idle => {}
            PollResult::Pending => ctx.request_repaint_after(Duration::from_millis(100)),
            PollResult::Progress(progress) => {
                if let UpdateUi::Downloading(available) = &self.update_ui {
                    self.status = progress.status_line(&available.version);
                }
                // Keep the UI ticking so the status bar advances during download.
                ctx.request_repaint_after(Duration::from_millis(50));
            }
            PollResult::Ready(Err(error)) => {
                // Restore a deferred install offer; otherwise return to Idle.
                self.update_ui = match std::mem::take(&mut self.update_ui) {
                    UpdateUi::Available(available) | UpdateUi::Downloading(available) => {
                        UpdateUi::Available(available)
                    }
                    UpdateUi::Staged { version } => UpdateUi::Staged { version },
                    UpdateUi::Idle | UpdateUi::Checking => UpdateUi::Idle,
                };
                self.status = format!("Update failed: {error}");
            }
            PollResult::Ready(Ok(UpdateOutcome::UpToDate(version))) => {
                self.update_ui = UpdateUi::Idle;
                self.status = format!("PinkDown v{version} is up to date");
            }
            PollResult::Ready(Ok(UpdateOutcome::Available(available))) => {
                self.status = format!("PinkDown v{} is available", available.version);
                self.update_ui = UpdateUi::Available(available);
            }
            PollResult::Ready(Ok(UpdateOutcome::InstallReady(version))) => {
                self.update_ui = UpdateUi::Staged {
                    version: version.clone(),
                };
                self.status =
                    format!("PinkDown v{version} is staged and will install when PinkDown closes");
                self.request_action(PendingAction::RestartForUpdate, ctx);
            }
        }
    }

    fn show_update_prompt(&mut self, ctx: &egui::Context) {
        // Avoid stacking over unsaved-changes or font settings.
        if self.pending_action.is_some() || self.font_settings_draft.is_some() {
            return;
        }
        let UpdateUi::Available(available) = &self.update_ui else {
            return;
        };
        let current = env!("CARGO_PKG_VERSION");
        let latest = available.version.to_string();
        let mode = if AUTO_INSTALL {
            UpdatePromptMode::InstallAndRestart
        } else {
            UpdatePromptMode::OpenReleases
        };
        let choice = dialog::update_available(ctx, current, &latest, mode);

        match choice {
            Some(UpdateChoice::Update) => {
                let UpdateUi::Available(available) =
                    std::mem::replace(&mut self.update_ui, UpdateUi::Idle)
                else {
                    return;
                };
                if AUTO_INSTALL {
                    let version = available.version.clone();
                    if self.update_checker.start_install(available.clone()) {
                        self.update_ui = UpdateUi::Downloading(available);
                        self.status = format!("Downloading PinkDown v{version}\u{2026}");
                    } else {
                        // Restore the offer so Later / Update still work.
                        self.update_ui = UpdateUi::Available(available);
                        self.status = "An update operation is already in progress".into();
                    }
                } else {
                    self.status = format!(
                        "PinkDown v{} is available from GitHub Releases",
                        available.version
                    );
                    open_github_releases();
                }
            }
            Some(UpdateChoice::Later) => {
                if let UpdateUi::Available(available) =
                    std::mem::replace(&mut self.update_ui, UpdateUi::Idle)
                {
                    self.status = format!(
                        "Update to v{} deferred \u{2014} use Check updates when ready",
                        available.version
                    );
                }
            }
            None => {}
        }
    }

    fn start_update_check(&mut self) {
        match &self.update_ui {
            UpdateUi::Staged { version } => {
                self.status =
                    format!("PinkDown v{version} is staged and will install when PinkDown closes");
            }
            UpdateUi::Downloading(available) => {
                self.status = format!("Downloading PinkDown v{}\u{2026}", available.version);
            }
            UpdateUi::Available(available) => {
                self.status = format!("PinkDown v{} is available", available.version);
            }
            UpdateUi::Checking => {
                self.status = "Checking for updates\u{2026}".into();
            }
            UpdateUi::Idle => {
                if self.update_checker.start() {
                    self.update_ui = UpdateUi::Checking;
                    self.status = "Checking for updates\u{2026}".into();
                }
            }
        }
    }

    fn show_confirmation(&mut self, ctx: &egui::Context) {
        let Some(action) = self.pending_action.as_ref() else {
            return;
        };
        let message = action.confirmation_text();
        let document_name = self.document.display_name();
        let choice = dialog::unsaved_changes(ctx, &document_name, message);

        match choice {
            Some(Choice::Save) if self.save(false) => {
                if let Some(action) = self.pending_action.take() {
                    self.execute_action(action, ctx);
                }
            }
            Some(Choice::Discard) => {
                if let Some(action) = self.pending_action.take() {
                    self.execute_action(action, ctx);
                }
            }
            Some(Choice::Cancel) => self.pending_action = None,
            _ => {}
        }
    }
}

impl eframe::App for PinkDown {
    fn update(&mut self, ctx: &egui::Context, frame: &mut eframe::Frame) {
        // Before main UI so edge interact zones share the same hit-test pass as the toolbar.
        #[cfg(target_os = "windows")]
        crate::window::frame_chrome(ctx, frame);
        #[cfg(not(target_os = "windows"))]
        let _ = frame;

        self.check_close_request(ctx);
        self.poll_update(ctx);
        self.poll_export(ctx);
        self.handle_inputs(ctx);
        self.sync_window_title(ctx);
        paint_window_shell(ctx);
        self.show_toolbar(ctx);
        self.show_find_bar(ctx);
        self.show_statusbar(ctx);
        self.show_editor(ctx);

        self.show_confirmation(ctx);
        self.show_font_settings(ctx);
        self.show_update_prompt(ctx);
    }
}

fn open_github_releases() {
    let url = "https://github.com/3xian/PinkDown/releases";
    #[cfg(target_os = "macos")]
    let _ = std::process::Command::new("open").arg(url).spawn();
    #[cfg(target_os = "windows")]
    let _ = std::process::Command::new("cmd")
        .args(["/C", "start", "", url])
        .spawn();
    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    let _ = std::process::Command::new("xdg-open").arg(url).spawn();
}

impl PinkDown {
    fn handle_inputs(&mut self, ctx: &egui::Context) {
        if ctx.input(|input| input.modifiers.command && input.key_pressed(egui::Key::O)) {
            self.request_action(PendingAction::OpenDialog, ctx);
        }
        if ctx.input(|input| input.modifiers.command && input.key_pressed(egui::Key::S)) {
            let force_dialog = ctx.input(|input| input.modifiers.shift);
            self.save(force_dialog);
        }
        if ctx.input(|input| input.modifiers.command && input.key_pressed(egui::Key::F)) {
            self.activate_find(true);
        }

        #[cfg(target_os = "macos")]
        let find_navigation = ctx.input(|input| {
            (input.modifiers.command && input.key_pressed(egui::Key::G))
                .then_some(input.modifiers.shift)
        });
        #[cfg(not(target_os = "macos"))]
        let find_navigation = ctx.input(|input| {
            input
                .key_pressed(egui::Key::F3)
                .then_some(input.modifiers.shift)
        });
        if let Some(reverse) = find_navigation {
            self.find_navigate(reverse);
        }
        if self.find.open && ctx.input(|input| input.key_pressed(egui::Key::Escape)) {
            self.find.open = false;
        }

        let path = ctx
            .input(|input| input.raw.dropped_files.clone())
            .into_iter()
            .find_map(|file| file.path);
        #[cfg(target_os = "macos")]
        let path = crate::macos::take_open_path().or(path);
        if let Some(path) = path {
            self.request_action(PendingAction::OpenPath(path), ctx);
        }
    }

    fn activate_find(&mut self, focus: bool) {
        let reset_current = !self.find.open;
        self.find.open = true;
        if focus {
            self.find.focus_pending = true;
        }
        self.find.rebuild(&self.document.text, reset_current);
    }

    fn find_navigate(&mut self, reverse: bool) {
        if self.find.query.is_empty() {
            return;
        }
        self.find.open = true;
        self.find.rebuild(&self.document.text, false);
        self.find.navigate(reverse);
    }

    fn show_toolbar(&mut self, ctx: &egui::Context) {
        egui::TopBottomPanel::top("window-toolbar")
            .exact_height(TOOLBAR_HEIGHT)
            .show_separator_line(false)
            .frame(egui::Frame::NONE)
            .show(ctx, |ui| {
                let panel = ui.max_rect();

                // Caption strip owns the top-right edge so Close is a corner hit
                // and title-drag never overlaps the buttons.
                #[cfg(target_os = "windows")]
                ui.scope_builder(
                    egui::UiBuilder::new()
                        .id_salt("window-controls")
                        .max_rect(caption_strip_rect(panel))
                        .layout(egui::Layout::right_to_left(egui::Align::Center)),
                    |ui| window_controls(ui, ctx, self),
                );

                ui.scope_builder(
                    egui::UiBuilder::new()
                        .id_salt("toolbar-content")
                        .max_rect(toolbar_content_rect(panel))
                        .layout(egui::Layout::left_to_right(egui::Align::Center)),
                    |ui| {
                        #[cfg(any(target_os = "windows", target_os = "macos"))]
                        configure_title_drag(ui, ctx);

                        #[cfg(target_os = "macos")]
                        ui.add_space(MACOS_TRAFFIC_LIGHT_INSET);

                        brand(ui);
                        ui.add_space(16.0);

                        if toolbar_button(ui, "Open", 52.0)
                            .on_hover_text(format!("Open a Markdown file  ({SHORTCUT_MOD}+O)"))
                            .clicked()
                        {
                            self.request_action(PendingAction::OpenDialog, ctx);
                        }
                        if toolbar_button(ui, "Save", 52.0)
                            .on_hover_text(format!("Save the current document  ({SHORTCUT_MOD}+S)"))
                            .clicked()
                        {
                            self.save(false);
                        }
                        if toolbar_button(ui, "Save as", 64.0)
                            .on_hover_text(format!(
                                "Save the document under a new name  ({SHORTCUT_MOD}+Shift+S)"
                            ))
                            .clicked()
                        {
                            self.save(true);
                        }
                        if toolbar_button(ui, "Find", 52.0)
                            .on_hover_text(format!("Find in the document  ({SHORTCUT_MOD}+F)"))
                            .clicked()
                        {
                            self.activate_find(true);
                        }
                        if toolbar_button(ui, "Font", 52.0)
                            .on_hover_text("Choose the UI and preview typeface")
                            .clicked()
                        {
                            self.font_settings_draft = Some(self.settings.preferred_font.clone());
                        }

                        let export_response = toolbar_button(ui, "Export", 60.0)
                            .on_hover_text("Export the document as HTML or PDF");
                        egui::Popup::menu(&export_response).show(|ui| {
                            ui.set_min_width(148.0);
                            if ui
                                .add(egui::Button::new(
                                    RichText::new("Export as HTML").size(12.0),
                                ))
                                .clicked()
                            {
                                self.export_document(ExportFormat::Html);
                            }
                            if ui
                                .add(egui::Button::new(RichText::new("Export as PDF").size(12.0)))
                                .clicked()
                            {
                                self.export_document(ExportFormat::Pdf);
                            }
                        });

                        if toolbar_button(ui, "Check updates", 96.0)
                            .on_hover_text("Check GitHub Releases for a newer version")
                            .clicked()
                        {
                            self.start_update_check();
                        }
                    },
                );
            });
    }
    fn show_find_bar(&mut self, ctx: &egui::Context) {
        if !self.find.open {
            return;
        }

        egui::TopBottomPanel::top("find-bar")
            .exact_height(FIND_BAR_HEIGHT)
            .show_separator_line(false)
            .frame(
                egui::Frame::NONE
                    .fill(BASE)
                    .inner_margin(egui::Margin::symmetric(20, 6)),
            )
            .show(ctx, |ui| {
                ui.horizontal_centered(|ui| {
                    ui.label(RichText::new("Find").size(12.0).strong().color(SUBTLE));
                    let query_id = egui::Id::new("find-query");
                    let enter_navigation =
                        ui.memory(|memory| memory.has_focus(query_id)).then(|| {
                            ui.input_mut(|input| {
                                if input.consume_key(egui::Modifiers::SHIFT, egui::Key::Enter) {
                                    Some(true)
                                } else if input.consume_key(egui::Modifiers::NONE, egui::Key::Enter)
                                {
                                    Some(false)
                                } else {
                                    None
                                }
                            })
                        });
                    let query = ui
                        .scope(|ui| {
                            let widgets = &mut ui.style_mut().visuals.widgets;
                            let capsule = egui::CornerRadius::same(10);
                            widgets.inactive.corner_radius = capsule;
                            widgets.hovered.corner_radius = capsule;
                            widgets.active.corner_radius = capsule;
                            widgets.open.corner_radius = capsule;
                            let input_font = FontId::proportional(12.0);
                            ui.add_sized(
                                [240.0, 20.0],
                                egui::TextEdit::singleline(&mut self.find.query)
                                    .id(query_id)
                                    .hint_text("Text in document")
                                    .hint_text_font(input_font.clone())
                                    .font(input_font)
                                    .text_color(FOAM)
                                    .margin(egui::Margin::symmetric(10, 0))
                                    .vertical_align(egui::Align::Center),
                            )
                        })
                        .inner;
                    if self.find.focus_pending {
                        query.request_focus();
                    }
                    if query.has_focus() {
                        self.find.focus_pending = false;
                    }
                    if query.changed() {
                        self.find.rebuild(&self.document.text, true);
                    }
                    if let Some(reverse) = enter_navigation.flatten() {
                        self.find.navigate(reverse);
                    }

                    let match_label = self.find.current.map_or_else(
                        || "No matches".to_owned(),
                        |current| format!("{} / {}", current + 1, self.find.matches.len()),
                    );
                    ui.label(RichText::new(match_label).size(11.0).color(MUTED));

                    if find_navigation_button(ui, "Previous", 80.0)
                        .on_hover_text(if cfg!(target_os = "macos") {
                            "Previous match  (Cmd+Shift+G)"
                        } else {
                            "Previous match  (Shift+F3)"
                        })
                        .clicked()
                    {
                        self.find.navigate(true);
                    }
                    if find_navigation_button(ui, "Next", 56.0)
                        .on_hover_text(if cfg!(target_os = "macos") {
                            "Next match  (Cmd+G)"
                        } else {
                            "Next match  (F3)"
                        })
                        .clicked()
                    {
                        self.find.navigate(false);
                    }
                    if find_close_button(ui)
                        .on_hover_text("Close  (Esc)")
                        .clicked()
                    {
                        self.find.open = false;
                    }
                });
            });
    }

    fn show_statusbar(&self, ctx: &egui::Context) {
        egui::TopBottomPanel::bottom("statusbar")
            .exact_height(STATUSBAR_HEIGHT)
            .show_separator_line(false)
            .frame(egui::Frame::NONE.inner_margin(egui::Margin {
                left: 8,
                right: 8,
                top: 0,
                bottom: 8,
            }))
            .show(ctx, |ui| {
                ui.allocate_ui_with_layout(
                    ui.available_size(),
                    egui::Layout::left_to_right(egui::Align::Center),
                    |ui| {
                        ui.add_space(14.0);
                        let color = if self.document.is_dirty() {
                            GOLD
                        } else {
                            SUBTLE
                        };
                        let path = self.document.path().map_or_else(
                            || "Untitled".to_owned(),
                            |path| path.display().to_string(),
                        );
                        chrome_text(ui, &path, 12.0, color);
                        chrome_text(ui, &self.status, 11.0, MUTED);
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            ui.add_space(14.0);
                            chrome_text(
                                ui,
                                &format!("{} words", self.document.text.split_whitespace().count()),
                                11.0,
                                MUTED,
                            );
                            chrome_text(
                                ui,
                                &format!("{} lines", self.document.text.lines().count()),
                                11.0,
                                MUTED,
                            );
                        });
                    },
                );
            });
    }

    fn show_editor(&mut self, ctx: &egui::Context) {
        let current_match = self.find.current_match().cloned();
        let jump = self
            .find
            .jump_pending
            .then(|| current_match.clone())
            .flatten();
        self.find.jump_pending = false;
        let preview_source_offset = jump.as_ref().map(|range| range.start);
        let mut source_changed = false;

        egui::CentralPanel::default()
            .frame(egui::Frame::NONE.inner_margin(egui::Margin {
                left: 20,
                right: 20,
                top: 2,
                bottom: 8,
            }))
            .show(ctx, |ui| {
                let available = ui.available_size();
                let usable = (available.x - EDITOR_SPLIT_GAP).max(0.0);
                let min_ratio = if usable > 0.0 {
                    (EDITOR_MIN_PANE / usable).clamp(0.0, 0.5)
                } else {
                    0.5
                };
                let ratio = self.split_ratio.clamp(min_ratio, 1.0 - min_ratio);

                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 0.0;
                    ui.set_min_size(available);

                    ui.allocate_ui_with_layout(
                        egui::vec2(usable * ratio, available.y),
                        egui::Layout::top_down(egui::Align::Min),
                        |ui| {
                            source_changed = source_panel(
                                ui,
                                &mut self.document.text,
                                current_match.as_ref(),
                                jump.is_some(),
                            );
                        },
                    );

                    let response = editor_splitter(ui, available.y);
                    if response.dragged() && usable > 0.0 {
                        self.split_ratio = (ratio + response.drag_delta().x / usable)
                            .clamp(min_ratio, 1.0 - min_ratio);
                    }

                    ui.allocate_ui_with_layout(
                        egui::vec2(ui.available_width(), available.y),
                        egui::Layout::top_down(egui::Align::Min),
                        |ui| {
                            preview::panel(
                                ui,
                                &self.document.text,
                                &mut self.markdown_cache,
                                preview_source_offset,
                            );
                        },
                    );
                });
            });

        if source_changed {
            self.find.rebuild(&self.document.text, false);
        }
    }
}

fn file_name_label(path: &std::path::Path) -> &str {
    path.file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("file")
}

/// Load preferences and coerce any stale typeface id back to Auto.
fn load_settings() -> Settings {
    let mut settings = Settings::load();
    let preferred = theme::normalize_font_preference(&settings.preferred_font);
    if preferred != settings.preferred_font {
        settings.preferred_font = preferred;
        settings.save();
    }
    settings
}

fn paint_window_shell(ctx: &egui::Context) {
    let painter = ctx.layer_painter(egui::LayerId::background());
    painter.rect_filled(ctx.screen_rect(), 0.0, BASE);
}

fn paint_chrome_label(ui: &mut egui::Ui, rect: egui::Rect, text: &str, size: f32, color: Color32) {
    ui.painter().text(
        rect.center(),
        egui::Align2::CENTER_CENTER,
        text,
        FontId::new(size, FontFamily::Proportional),
        color,
    );
}

fn toolbar_button(ui: &mut egui::Ui, label: &str, width: f32) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(
        egui::vec2(width, ui.available_height()),
        egui::Sense::click(),
    );
    let color = if response.is_pointer_button_down_on() {
        TEXT
    } else if response.hovered() {
        SUBTLE
    } else {
        MUTED
    };
    paint_chrome_label(ui, rect, label, 12.0, color);
    response.on_hover_cursor(egui::CursorIcon::PointingHand)
}

fn find_navigation_button(ui: &mut egui::Ui, label: &str, width: f32) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(egui::vec2(width, 26.0), egui::Sense::click());
    let fill = if response.is_pointer_button_down_on() {
        HIGHLIGHT_LOW
    } else if response.hovered() {
        SURFACE
    } else {
        BASE
    };
    ui.painter().rect(
        rect,
        0.0,
        fill,
        egui::Stroke::new(1.0, HIGHLIGHT_LOW),
        egui::StrokeKind::Inside,
    );
    paint_chrome_label(ui, rect, label, 12.0, SUBTLE);
    response.on_hover_cursor(egui::CursorIcon::PointingHand)
}

fn find_close_button(ui: &mut egui::Ui) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(egui::Vec2::splat(26.0), egui::Sense::click());
    let fill = if response.is_pointer_button_down_on() {
        HIGHLIGHT_LOW
    } else if response.hovered() {
        SURFACE
    } else {
        BASE
    };
    let center = rect.center();
    ui.painter()
        .circle(center, 12.0, fill, egui::Stroke::new(1.0, HIGHLIGHT_LOW));
    let stroke = egui::Stroke::new(1.3, SUBTLE);
    ui.painter().line_segment(
        [
            center + egui::vec2(-3.5, -3.5),
            center + egui::vec2(3.5, 3.5),
        ],
        stroke,
    );
    ui.painter().line_segment(
        [
            center + egui::vec2(3.5, -3.5),
            center + egui::vec2(-3.5, 3.5),
        ],
        stroke,
    );
    response.on_hover_cursor(egui::CursorIcon::PointingHand)
}

fn chrome_text(ui: &mut egui::Ui, text: &str, size: f32, color: Color32) {
    let width = ui.fonts(|fonts| {
        fonts
            .layout_no_wrap(
                text.to_owned(),
                FontId::new(size, FontFamily::Proportional),
                color,
            )
            .size()
            .x
    });
    let (rect, _) = ui.allocate_exact_size(
        egui::vec2(width, ui.available_height()),
        egui::Sense::hover(),
    );
    paint_chrome_label(ui, rect, text, size, color);
}

fn brand(ui: &mut egui::Ui) {
    const WIDTH: f32 = 96.0;
    const SUBTITLE_DY: f32 = 14.0;
    const TOP_NUDGE: f32 = 1.0;

    let (rect, _) = ui.allocate_exact_size(
        egui::vec2(WIDTH, ui.available_height()),
        egui::Sense::hover(),
    );
    let text = "PinkDown";
    let denominator = text.chars().count().saturating_sub(1).max(1) as f32;
    let mut job = egui::text::LayoutJob::default();
    for (index, character) in text.chars().enumerate() {
        let progress = index as f32 / denominator;
        let color = if progress <= 0.5 {
            lerp_color(ROSE, IRIS, progress * 2.0)
        } else {
            lerp_color(IRIS, FOAM, (progress - 0.5) * 2.0)
        };
        job.append(
            &character.to_string(),
            0.0,
            TextFormat {
                font_id: FontId::new(13.0, FontFamily::Monospace),
                color,
                ..Default::default()
            },
        );
    }

    let title = ui.fonts(|fonts| fonts.layout_job(job));
    let subtitle = ui.fonts(|fonts| {
        fonts.layout_no_wrap(
            "MARKDOWN EDITOR".into(),
            FontId::new(8.0, FontFamily::Proportional),
            MUTED,
        )
    });
    let stack_h = SUBTITLE_DY + subtitle.size().y;
    let top = rect.center().y - stack_h * 0.5 + TOP_NUDGE;
    ui.painter()
        .galley(egui::pos2(rect.left(), top), title, Color32::WHITE);
    ui.painter()
        .galley(egui::pos2(rect.left(), top + SUBTITLE_DY), subtitle, MUTED);
}

fn lerp_color(from: Color32, to: Color32, amount: f32) -> Color32 {
    let mix = |start: u8, end: u8| {
        (start as f32 + (end as f32 - start as f32) * amount.clamp(0.0, 1.0)).round() as u8
    };
    Color32::from_rgb(
        mix(from.r(), to.r()),
        mix(from.g(), to.g()),
        mix(from.b(), to.b()),
    )
}

fn source_panel(
    ui: &mut egui::Ui,
    source: &mut String,
    selection: Option<&Range<usize>>,
    scroll_to_selection: bool,
) -> bool {
    let mut changed = false;
    egui::Frame::new()
        .fill(SURFACE)
        .stroke(egui::Stroke::new(1.0_f32, HIGHLIGHT_LOW))
        .corner_radius(egui::CornerRadius::same(12))
        .inner_margin(egui::Margin::symmetric(18, 16))
        .show(ui, |ui| {
            ui.set_min_size(ui.available_size());
            ui.label(RichText::new("SOURCE").size(11.0).strong().color(MUTED));
            ui.add_space(8.0);
            egui::ScrollArea::vertical()
                .id_salt("source-scroll")
                .auto_shrink([false; 2])
                .show(ui, |ui| {
                    let selection = selection.filter(|range| {
                        range.end <= source.len()
                            && source.is_char_boundary(range.start)
                            && source.is_char_boundary(range.end)
                    });
                    let editor_id = ui.make_persistent_id("source-editor");
                    let selected_chars = selection.map(|range| {
                        let start = source[..range.start].chars().count();
                        let end = start + source[range.clone()].chars().count();
                        (start, end)
                    });
                    if scroll_to_selection {
                        if let Some((start, end)) = selected_chars {
                            let mut state =
                                egui::TextEdit::load_state(ui.ctx(), editor_id).unwrap_or_default();
                            state
                                .cursor
                                .set_char_range(Some(egui::text::CCursorRange::two(
                                    egui::text::CCursor::new(start),
                                    egui::text::CCursor::new(end),
                                )));
                            state.store(ui.ctx(), editor_id);
                        }
                    }

                    let editor_has_focus = ui.memory(|memory| memory.has_focus(editor_id));
                    let inactive_selection = (!editor_has_focus).then_some(selection).flatten();
                    let mut layouter =
                        |ui: &egui::Ui, text: &dyn egui::TextBuffer, wrap_width: f32| {
                            let text = text.as_str();
                            let mut job = egui::text::LayoutJob::default();
                            job.wrap.max_width = wrap_width;
                            let normal = TextFormat {
                                font_id: egui::TextStyle::Monospace.resolve(ui.style()),
                                color: TEXT,
                                ..Default::default()
                            };
                            if let Some(range) = inactive_selection.filter(|range| {
                                range.end <= text.len()
                                    && text.is_char_boundary(range.start)
                                    && text.is_char_boundary(range.end)
                            }) {
                                job.append(&text[..range.start], 0.0, normal.clone());
                                let selection_visuals = ui.visuals().selection;
                                let selected = TextFormat {
                                    color: selection_visuals.stroke.color,
                                    background: selection_visuals.bg_fill,
                                    ..normal.clone()
                                };
                                job.append(&text[range.clone()], 0.0, selected);
                                job.append(&text[range.end..], 0.0, normal);
                            } else {
                                job.append(text, 0.0, normal);
                            }
                            ui.fonts(|fonts| fonts.layout_job(job))
                        };

                    let mut editor = egui::TextEdit::multiline(source)
                        .id(editor_id)
                        .font(egui::TextStyle::Monospace)
                        .text_color(TEXT)
                        .frame(false)
                        .code_editor()
                        .desired_rows(30)
                        .lock_focus(true)
                        .desired_width(ui.available_width())
                        .min_size(egui::vec2(
                            ui.available_width(),
                            ui.available_height().max(200.0),
                        ));
                    if inactive_selection.is_some() {
                        editor = editor.layouter(&mut layouter);
                    }
                    let output = editor.show(ui);
                    changed = output.response.changed();

                    if scroll_to_selection {
                        if let Some((start, _)) = selected_chars {
                            let cursor_rect = output
                                .galley
                                .pos_from_cursor(egui::text::CCursor::new(start))
                                .translate(output.galley_pos.to_vec2())
                                .expand2(egui::vec2(8.0, 24.0));
                            ui.scroll_to_rect(cursor_rect, Some(egui::Align::Center));
                        }
                    }
                });
        });
    changed
}

fn editor_splitter(ui: &mut egui::Ui, height: f32) -> egui::Response {
    let (rect, response) =
        ui.allocate_exact_size(egui::vec2(EDITOR_SPLIT_GAP, height), egui::Sense::drag());
    if response.hovered() || response.dragged() {
        let color = if response.dragged() { IRIS } else { FOAM };
        let handle = egui::Rect::from_center_size(
            rect.center(),
            egui::vec2(2.0, (rect.height() - 32.0).max(24.0)),
        );
        ui.painter().rect_filled(handle, 1.0, color);
        ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeHorizontal);
    }
    response
}

fn toolbar_content_rect(panel: egui::Rect) -> egui::Rect {
    let mut rect = panel;
    rect.min.x += 20.0;
    #[cfg(target_os = "windows")]
    {
        rect.max.x -= CAPTION_STRIP_WIDTH;
    }
    #[cfg(not(target_os = "windows"))]
    {
        rect.max.x -= 20.0;
    }
    rect
}

#[cfg(any(target_os = "windows", target_os = "macos"))]
fn configure_title_drag(ui: &mut egui::Ui, ctx: &egui::Context) {
    // macOS keeps system traffic lights; exclude them so StartDrag does not fight AppKit.
    // Windows: this Ui is already the leftover content strip (caption buttons are outside).
    #[cfg(target_os = "macos")]
    let drag_rect = {
        let mut rect = ui.max_rect();
        rect.min.x += MACOS_TRAFFIC_LIGHT_INSET;
        rect
    };
    #[cfg(not(target_os = "macos"))]
    let drag_rect = ui.max_rect();

    // click_and_drag: Sense::drag() has no CLICK bit, so double_clicked never fires.
    let drag = ui.interact(
        drag_rect,
        ui.id().with("title-drag"),
        egui::Sense::click_and_drag(),
    );
    if drag.drag_started() {
        crate::window::begin_title_drag(ctx);
    }
    // Double-click maximize is Windows custom-chrome behavior; macOS uses system zoom.
    #[cfg(target_os = "windows")]
    if drag.double_clicked() {
        let maximized = ctx.input(|input| input.viewport().maximized.unwrap_or(false));
        ctx.send_viewport_cmd(egui::ViewportCommand::Maximized(!maximized));
    }
}

#[cfg(target_os = "windows")]
#[derive(Clone, Copy)]
enum WindowButton {
    Minimize,
    Maximize,
    Restore,
    Close,
}

#[cfg(target_os = "windows")]
const WINDOW_BUTTON_WIDTH: f32 = 36.0;

#[cfg(target_os = "windows")]
const CAPTION_STRIP_WIDTH: f32 = WINDOW_BUTTON_WIDTH * 3.0;

#[cfg(target_os = "windows")]
fn caption_strip_rect(panel: egui::Rect) -> egui::Rect {
    let mut rect = panel;
    rect.min.x = panel.max.x - CAPTION_STRIP_WIDTH;
    rect
}

#[cfg(target_os = "windows")]
fn window_controls(ui: &mut egui::Ui, ctx: &egui::Context, app: &mut PinkDown) {
    ui.spacing_mut().item_spacing.x = 0.0;
    if window_button(ui, WindowButton::Close, "Close").clicked() {
        app.request_action(PendingAction::Close, ctx);
    }
    let maximized = ctx.input(|input| input.viewport().maximized.unwrap_or(false));
    let kind = if maximized {
        WindowButton::Restore
    } else {
        WindowButton::Maximize
    };
    if window_button(ui, kind, if maximized { "Restore" } else { "Maximize" }).clicked() {
        ctx.send_viewport_cmd(egui::ViewportCommand::Maximized(!maximized));
    }
    if window_button(ui, WindowButton::Minimize, "Minimize").clicked() {
        ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(true));
    }
}

#[cfg(target_os = "windows")]
fn window_button(ui: &mut egui::Ui, kind: WindowButton, tooltip: &str) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(
        egui::vec2(WINDOW_BUTTON_WIDTH, ui.available_height()),
        egui::Sense::click(),
    );
    let close = matches!(kind, WindowButton::Close);
    if response.hovered() || response.is_pointer_button_down_on() {
        ui.painter()
            .rect_filled(rect, 0.0, if close { LOVE } else { HIGHLIGHT_LOW });
    }
    let color = if close && response.hovered() {
        TEXT
    } else {
        SUBTLE
    };
    let stroke = egui::Stroke::new(1.3_f32, color);
    let center = rect.center();
    match kind {
        WindowButton::Minimize => ui.painter().line_segment(
            [
                center + egui::vec2(-5.0, 3.0),
                center + egui::vec2(5.0, 3.0),
            ],
            stroke,
        ),
        WindowButton::Maximize => ui.painter().rect_stroke(
            egui::Rect::from_center_size(center, egui::vec2(9.0, 8.0)),
            1.0,
            stroke,
            egui::StrokeKind::Inside,
        ),
        WindowButton::Restore => {
            ui.painter().rect_stroke(
                egui::Rect::from_min_size(center + egui::vec2(-3.0, -5.0), egui::vec2(8.0, 7.0)),
                1.0,
                stroke,
                egui::StrokeKind::Inside,
            );
            ui.painter().rect_stroke(
                egui::Rect::from_min_size(center + egui::vec2(-5.0, -2.0), egui::vec2(8.0, 7.0)),
                1.0,
                stroke,
                egui::StrokeKind::Inside,
            )
        }
        WindowButton::Close => {
            ui.painter().line_segment(
                [
                    center + egui::vec2(-4.0, -4.0),
                    center + egui::vec2(4.0, 4.0),
                ],
                stroke,
            );
            ui.painter().line_segment(
                [
                    center + egui::vec2(4.0, -4.0),
                    center + egui::vec2(-4.0, 4.0),
                ],
                stroke,
            )
        }
    };
    response.on_hover_text(tooltip)
}

#[cfg(test)]
mod tests {
    use super::FindState;

    #[test]
    fn find_navigation_wraps_in_both_directions() {
        let mut find = FindState {
            query: "pink".into(),
            ..FindState::default()
        };
        find.rebuild("pink blue pink", true);

        assert_eq!(find.current_match(), Some(&(0..4)));
        find.navigate(false);
        assert_eq!(find.current_match(), Some(&(10..14)));
        find.navigate(false);
        assert_eq!(find.current_match(), Some(&(0..4)));
        find.navigate(true);
        assert_eq!(find.current_match(), Some(&(10..14)));
    }

    #[test]
    fn find_ranges_preserve_utf8_byte_offsets() {
        let mut find = FindState {
            query: "查找".into(),
            ..FindState::default()
        };
        find.rebuild("开头 查找 结尾", true);

        assert_eq!(find.current_match(), Some(&(7..13)));
    }

    #[test]
    fn empty_query_has_no_match_or_jump() {
        let mut find = FindState {
            query: String::new(),
            jump_pending: true,
            ..FindState::default()
        };
        find.rebuild("anything", true);

        assert!(find.matches.is_empty());
        assert_eq!(find.current, None);
        assert!(!find.jump_pending);
    }

    #[test]
    fn rebuild_on_new_document_drops_old_ranges() {
        let mut find = FindState {
            query: "ab".into(),
            ..FindState::default()
        };
        find.rebuild("ab cd ab", true);
        assert_eq!(find.current_match(), Some(&(0..2)));

        find.rebuild("zzzz ab", true);
        find.jump_pending = false;

        assert_eq!(find.matches, vec![5..7]);
        assert_eq!(find.current_match(), Some(&(5..7)));
        assert!(!find.jump_pending);

        find.navigate(false);
        assert_eq!(find.current_match(), Some(&(5..7)));
    }

    #[test]
    fn navigate_rebuilds_against_replaced_source() {
        let mut find = FindState {
            query: "foo".into(),
            ..FindState::default()
        };
        find.rebuild("foo old", true);
        assert_eq!(find.current_match(), Some(&(0..3)));

        find.rebuild("xx foo yy foo", false);
        find.navigate(false);

        assert_eq!(find.current_match(), Some(&(10..13)));
    }
}
