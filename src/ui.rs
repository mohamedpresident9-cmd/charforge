//! Studio UI: catalog → download → library → attach to the current character.

use std::sync::Arc;
use std::time::Duration;

use eframe::egui;

use crate::model::{AssetClass, AssetId, AssetKind, AssetMeta, Catalog, Quality};
use crate::pipeline::{Engine, Event, JobView};
use crate::store::{Store, StoredAsset};

enum Action {
    Download(AssetId, Quality),
    Cancel(u64),
    Delete(AssetId, Quality),
    Attach(AssetId),
    Verify(AssetId, Quality),
}

pub struct App {
    store: Arc<Store>,
    engine: Engine,
    catalog: Catalog,

    quality: Quality,
    class: AssetClass,
    kind: AssetKind,

    jobs: Vec<JobView>,
    library: Vec<StoredAsset>,
    log: Vec<String>,

    character: Option<AssetId>,
    animation: Option<AssetId>,
    clothing: Option<AssetId>,
}

impl App {
    pub fn new(store: Arc<Store>, engine: Engine) -> Self {
        let catalog = store.catalog().clone();
        let library = store.list();
        Self {
            store,
            engine,
            catalog,
            quality: Quality::High,
            class: AssetClass::Human,
            kind: AssetKind::Model,
            jobs: Vec::new(),
            library,
            log: Vec::new(),
            character: None,
            animation: None,
            clothing: None,
        }
    }

    /// Drain engine events and refresh the snapshots the UI renders.
    fn pump(&mut self) {
        for event in self.engine.drain_events() {
            match event {
                Event::Finished(id, stored) => {
                    self.log.push(format!(
                        "job #{id} → {} ({}) {}",
                        stored.id,
                        stored.quality.label(),
                        &stored.sha256[..8]
                    ));
                }
                Event::Failed(id, message) => {
                    self.log.push(format!("job #{id} failed: {message}"));
                }
                Event::Cancelled(id) => {
                    self.log.push(format!("job #{id} cancelled"));
                }
            }
        }
        self.jobs = self.engine.snapshot();
        self.library = self.store.list();
        if self.log.len() > 200 {
            let drop = self.log.len() - 200;
            self.log.drain(..drop);
        }
    }

    fn installed(&self, id: AssetId, quality: Quality) -> Option<&StoredAsset> {
        self.library
            .iter()
            .find(|row| row.id == id && row.quality == quality)
    }

    fn job_for(&self, id: AssetId, quality: Quality) -> Option<&JobView> {
        self.jobs
            .iter()
            .rev()
            .find(|job| job.asset == id && job.quality == quality && job.state.is_active())
    }

    fn apply(&mut self, action: Action) {
        match action {
            Action::Download(id, quality) => {
                let handle = self.engine.enqueue(id, quality);
                self.log
                    .push(format!("queued {} ({}) as #{handle}", id, quality.label()));
            }
            Action::Cancel(job) => self.engine.cancel(job),
            Action::Delete(id, quality) => {
                if let Err(error) = self.store.remove(id, quality) {
                    self.log.push(format!("delete {id} failed: {error}"));
                }
            }
            Action::Attach(id) => match self.catalog.get(id) {
                Some(meta) => match meta.kind {
                    AssetKind::Model => {
                        self.character = Some(id);
                        self.log.push(format!("character = {}", meta.name));
                    }
                    AssetKind::Animation => {
                        self.animation = Some(id);
                        self.log.push(format!("animation = {}", meta.name));
                    }
                    AssetKind::Clothing => {
                        self.clothing = Some(id);
                        self.log.push(format!("clothing = {}", meta.name));
                    }
                },
                None => {}
            },
            Action::Verify(id, quality) => {
                if let Some(row) = self.installed(id, quality).cloned() {
                    let ok = self.store.verify(&row);
                    self.log.push(format!(
                        "verify {} ({}): {}",
                        id,
                        quality.label(),
                        if ok { "intact" } else { "CORRUPT" }
                    ));
                }
            }
        }
    }
}

impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.pump();

        let mut action: Option<Action> = None;

        egui::TopBottomPanel::top("top").show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.heading("CharForge Studio");
                ui.separator();
                ui.label("Quality:");
                for quality in Quality::ALL {
                    if ui
                        .selectable_label(self.quality == quality, quality.label())
                        .clicked()
                    {
                        self.quality = quality;
                    }
                }
                ui.separator();
                ui.label(format!("Jobs in queue: {}", self.engine.queued_count()));
                ui.separator();
                let bytes: u64 = self.library.iter().map(|row| row.bytes).sum();
                ui.label(format!(
                    "Library: {} items · {:.1} MB",
                    self.library.len(),
                    bytes as f64 / (1024.0 * 1024.0)
                ));
            });
        });

        egui::TopBottomPanel::bottom("bottom").show(ctx, |ui| {
            ui.horizontal(|ui| {
                let character = self
                    .character
                    .and_then(|id| self.catalog.get(id))
                    .map(|m| m.name)
                    .unwrap_or("—");
                let animation = self
                    .animation
                    .and_then(|id| self.catalog.get(id))
                    .map(|m| m.name)
                    .unwrap_or("—");
                let clothing = self
                    .clothing
                    .and_then(|id| self.catalog.get(id))
                    .map(|m| m.name)
                    .unwrap_or("—");
                ui.label(format!("Character: {character}"));
                ui.separator();
                ui.label(format!("Animation: {animation}"));
                ui.separator();
                ui.label(format!("Clothing: {clothing}"));
            });
            if let Some(last) = self.log.last() {
                ui.label(egui::RichText::new(last).weak());
            }
        });

        egui::SidePanel::left("filters").show(ctx, |ui| {
            ui.heading("Catalog");
            for class in AssetClass::ALL {
                if ui
                    .selectable_label(self.class == class, class.label())
                    .clicked()
                {
                    self.class = class;
                }
            }
            ui.separator();
            for kind in AssetKind::ALL {
                if ui
                    .selectable_label(self.kind == kind, kind.label())
                    .clicked()
                {
                    self.kind = kind;
                }
            }
            ui.separator();
            ui.label("Quality tiers swap the payload: High is 4× the bytes and");
            ui.label("uses larger parts. The queue never runs two jobs for the");
            ui.label("same payload at once.");
        });

        egui::SidePanel::right("queue").show(ctx, |ui| {
            ui.heading(format!("Queue ({})", self.jobs.len()));
            egui::ScrollArea::vertical().show(ui, |ui| {
                for job in self.jobs.iter().rev() {
                    ui.group(|ui| {
                        let name = self
                            .catalog
                            .get(job.asset)
                            .map(|m| m.name)
                            .unwrap_or(job.asset.0);
                        ui.horizontal(|ui| {
                            ui.label(format!("#{} {name}", job.id));
                            ui.label(egui::RichText::new(job.state.label()).weak());
                        });
                        ui.add(egui::ProgressBar::new(job.progress).show_percentage());
                        ui.horizontal(|ui| {
                            ui.label(format!(
                                "{} · attempt {}/{}",
                                job.quality.label(),
                                job.attempts,
                                self.engine.max_attempts()
                            ));
                            if job.state.is_active() && ui.button("Cancel").clicked() {
                                action = Some(Action::Cancel(job.id));
                            }
                        });
                        if !job.message.is_empty() {
                            ui.label(egui::RichText::new(&job.message).weak());
                        }
                    });
                }
            });
        });

        egui::CentralPanel::default().show(ctx, |ui| {
            let rows = self.catalog.filter(self.class, self.kind);
            ui.heading(format!("{} · {}", self.class.label(), self.kind.label()));

            if rows.is_empty() {
                ui.label("Nothing in this shelf yet.");
            }

            egui::ScrollArea::vertical().show(ui, |ui| {
                for meta in rows {
                    if let Some(next) = asset_card(
                        ui,
                        &meta,
                        self.quality,
                        self.installed(meta.id, self.quality).is_some(),
                        self.job_for(meta.id, self.quality),
                    ) {
                        action = Some(next);
                    }
                }
            });
        });

        if let Some(action) = action {
            self.apply(action);
        }

        ctx.request_repaint_after(Duration::from_millis(120));
    }
}

/// One catalog row. Returns the action the user chose, if any.
fn asset_card(
    ui: &mut egui::Ui,
    meta: &AssetMeta,
    quality: Quality,
    installed: bool,
    job: Option<&JobView>,
) -> Option<Action> {
    let mut action = None;
    ui.group(|ui| {
        ui.horizontal(|ui| {
            ui.vertical(|ui| {
                ui.label(egui::RichText::new(meta.name).strong());
                ui.label(
                    egui::RichText::new(format!(
                        "{} · {} parts × {} KiB",
                        human_bytes(meta.bytes(quality)),
                        meta.chunk_count(quality),
                        meta.chunk_size(quality) / 1024
                    ))
                    .weak(),
                );
            });

            ui.with_layout(
                egui::Layout::right_to_left(egui::Align::Center),
                |ui| match job {
                    Some(job) => {
                        ui.add(egui::ProgressBar::new(job.progress).desired_width(140.0));
                        if ui.button("Cancel").clicked() {
                            action = Some(Action::Cancel(job.id));
                        }
                    }
                    None if installed => {
                        if ui.button("Use").clicked() {
                            action = Some(Action::Attach(meta.id));
                        }
                        if ui.button("Verify").clicked() {
                            action = Some(Action::Verify(meta.id, quality));
                        }
                        if ui.button("Delete").clicked() {
                            action = Some(Action::Delete(meta.id, quality));
                        }
                        ui.label(egui::RichText::new("installed").weak());
                    }
                    None => {
                        if ui.button("Download").clicked() {
                            action = Some(Action::Download(meta.id, quality));
                        }
                        ui.label(egui::RichText::new("remote").weak());
                    }
                },
            );
        });
    });
    action
}

fn human_bytes(bytes: u64) -> String {
    const UNITS: [&str; 4] = ["B", "KiB", "MiB", "GiB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    format!("{value:.1} {}", UNITS[unit])
}
