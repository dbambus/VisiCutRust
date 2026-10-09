//! Materials, processing steps, rules and engraving options for the egui
//! interface.
use eframe::egui;
use visicut_core::{
    device,
    mapping::{self, Attribute, Filter},
    materials::{Library, Material, MaterialProfile},
    project::{JobStep, Operation, ParameterSet, Project},
    raster::{Dithering, RasterSettings},
};

pub fn operation_title(operation: Operation) -> &'static str {
    match operation {
        Operation::Cut => "Schneiden",
        Operation::Engrave => "Gravieren",
        Operation::Engrave3d => "3D-Gravur",
        Operation::Mark => "Markieren",
    }
}

fn operation_selector(ui: &mut egui::Ui, operation: &mut Operation) -> bool {
    let mut changed = false;
    ui.horizontal_wrapped(|ui| {
        for candidate in Operation::ALL {
            changed |= ui
                .selectable_value(operation, candidate, operation_title(candidate))
                .changed();
        }
    });
    changed
}

fn raster_options(
    ui: &mut egui::Ui,
    id: impl std::hash::Hash,
    settings: &mut RasterSettings,
    operation: Operation,
) -> bool {
    let mut changed = false;
    if operation == Operation::Engrave {
        egui::ComboBox::from_id_salt(("dithering", id))
            .selected_text(settings.dithering.title())
            .show_ui(ui, |ui| {
                for d in Dithering::ALL {
                    changed |= ui
                        .selectable_value(&mut settings.dithering, d, d.title())
                        .changed();
                }
            });
    } else {
        ui.small("Graustufen: Leistung folgt der Helligkeit.");
    }
    ui.horizontal(|ui| {
        ui.label("Helligkeit");
        changed |= ui
            .add(egui::DragValue::new(&mut settings.color_shift).range(-255..=255))
            .changed();
    });
    changed |= ui
        .checkbox(&mut settings.invert, "Farben invertieren")
        .changed();
    changed |= ui
        .checkbox(&mut settings.bidirectional, "Bidirektional")
        .changed();
    changed |= ui
        .checkbox(&mut settings.bottom_up, "Von unten nach oben")
        .changed();
    changed
}

fn parameters(ui: &mut egui::Ui, power: &mut f32, speed: &mut f32, passes: &mut u32) -> bool {
    let mut changed = crate::number(ui, "Leistung", power, 0.0..=100.0, " %");
    changed |= crate::number(ui, "Geschwindigkeit", speed, 0.1..=100.0, " %");
    ui.horizontal(|ui| {
        ui.label("Durchgänge");
        changed |= ui
            .add(egui::DragValue::new(passes).range(1..=100))
            .changed();
    });
    changed
}

fn filters(ui: &mut egui::Ui, id: impl std::hash::Hash + Copy, filters: &mut Vec<Filter>) -> bool {
    let mut changed = false;
    let mut remove = None;
    if filters.is_empty() {
        ui.small("Ohne Bedingung: alle Objekte");
    }
    for (i, filter) in filters.iter_mut().enumerate() {
        ui.horizontal(|ui| {
            egui::ComboBox::from_id_salt(("attribute", id, i))
                .width(110.0)
                .selected_text(filter.attribute.title())
                .show_ui(ui, |ui| {
                    for a in Attribute::ALL {
                        changed |= ui
                            .selectable_value(&mut filter.attribute, a, a.title())
                            .changed();
                    }
                });
            changed |= ui.checkbox(&mut filter.inverted, "≠").changed();
            if filter.attribute == Attribute::StrokeWidth {
                changed |= ui.checkbox(&mut filter.compare, "≤").changed();
            }
            changed |= ui
                .add(egui::TextEdit::singleline(&mut filter.value).desired_width(70.0))
                .changed();
            if ui.small_button("✕").clicked() {
                remove = Some(i);
            }
        });
    }
    if let Some(i) = remove {
        filters.remove(i);
        changed = true;
    }
    if ui.small_button("Bedingung hinzufügen").clicked() {
        filters.push(Filter {
            attribute: Attribute::Color,
            value: "#ff0000".into(),
            compare: false,
            inverted: false,
        });
        changed = true;
    }
    changed
}

pub struct JobUi {
    pub library: Library,
    custom: bool,
    editor: Option<(Vec<Material>, usize)>,
}

impl JobUi {
    pub fn load() -> (Self, Option<String>) {
        let (library, custom, error) = match Library::load(&device::config_dir()) {
            Ok((library, custom)) => (library, custom, None),
            Err(e) => (Library::bundled(), false, Some(e)),
        };
        let ui = Self {
            library,
            custom,
            editor: None,
        };
        (ui, error)
    }

    fn preset(&self, project: &Project, operation: Operation) -> Option<&MaterialProfile> {
        self.library
            .materials
            .iter()
            .find(|m| m.name == project.material)?
            .profiles
            .iter()
            .find(|p| {
                (p.thickness_mm - project.thickness_mm).abs() < 1e-4
                    && p.operation == operation
                    && p.power_percent > 0.0
            })
    }

    fn new_step(&self, project: &Project, operation: Operation) -> JobStep {
        let mut step = JobStep::new(operation);
        match self.preset(project, operation) {
            Some(p) => {
                (step.power_percent, step.speed_percent) = (p.power_percent, p.speed_percent)
            }
            None => {
                (step.power_percent, step.speed_percent) =
                    (project.power_percent, project.speed_percent)
            }
        }
        step
    }

    pub fn material(&mut self, ui: &mut egui::Ui, project: &mut Project, dirty: &mut bool) {
        egui::ComboBox::from_id_salt("material")
            .width(200.0)
            .selected_text(if project.material.is_empty() {
                "Material wählen"
            } else {
                &project.material
            })
            .show_ui(ui, |ui| {
                for m in &self.library.materials {
                    if ui
                        .selectable_label(project.material == m.name, &m.name)
                        .clicked()
                    {
                        project.material = m.name.clone();
                        if let Some(first) = m.profiles.first()
                            && !m
                                .profiles
                                .iter()
                                .any(|p| (p.thickness_mm - project.thickness_mm).abs() < 1e-4)
                        {
                            project.thickness_mm = first.thickness_mm;
                        }
                        *dirty = true;
                    }
                }
            });
        *dirty |= crate::number(ui, "Stärke", &mut project.thickness_mm, 0.0..=1000.0, " mm");
        ui.horizontal(|ui| {
            if let Some(p) = self.preset(project, project.operation).cloned() {
                if ui
                    .button(format!(
                        "Profil übernehmen ({} % / {} %)",
                        p.power_percent, p.speed_percent
                    ))
                    .clicked()
                {
                    (project.power_percent, project.speed_percent, project.passes) =
                        (p.power_percent, p.speed_percent, 1);
                    *dirty = true;
                }
            } else {
                ui.small("Kein Profil für diese Kombination");
            }
        });
        if ui.button("Materialbibliothek …").clicked() {
            self.editor = Some((self.library.materials.clone(), 0));
        }
    }

    pub fn processing(&mut self, ui: &mut egui::Ui, project: &mut Project, dirty: &mut bool) {
        let predefined = mapping::predefined();
        egui::ComboBox::from_id_salt("assignment")
            .width(240.0)
            .selected_text(if project.steps.is_empty() {
                "Gesamtes Motiv"
            } else {
                "Regeln"
            })
            .show_ui(ui, |ui| {
                if ui
                    .selectable_label(project.steps.is_empty(), "Gesamtes Motiv")
                    .clicked()
                {
                    project.steps.clear();
                    project.ignore_filters.clear();
                    *dirty = true;
                }
                for p in &predefined {
                    if ui.selectable_label(false, p.name).clicked() {
                        let steps = p
                            .rules
                            .iter()
                            .map(|(operation, filters, rest)| JobStep {
                                filters: filters.clone(),
                                rest: *rest,
                                ..self.new_step(project, *operation)
                            })
                            .collect();
                        project.steps = steps;
                        project.ignore_filters = p.ignore.clone();
                        *dirty = true;
                    }
                }
            });
        if project.steps.is_empty() {
            if operation_selector(ui, &mut project.operation) {
                *dirty = true;
            }
            *dirty |= parameters(
                ui,
                &mut project.power_percent,
                &mut project.speed_percent,
                &mut project.passes,
            );
            if project.operation.is_raster() {
                *dirty |= raster_options(ui, "whole", &mut project.raster, project.operation);
            }
            return;
        }
        let selections = mapping::resolve(project);
        if let Err(e) = &selections {
            ui.colored_label(egui::Color32::from_rgb(180, 75, 40), e);
        }
        let mut remove = None;
        let count = project.steps.len();
        for i in 0..count {
            let objects = match (&selections, &project.steps[i]) {
                (Ok(s), _) => s[i].len(),
                _ => 0,
            };
            let snapshot = project.steps[i].clone();
            let step = &mut project.steps[i];
            egui::CollapsingHeader::new(format!(
                "{}. {} · {} Objekte",
                i + 1,
                operation_title(step.operation),
                objects
            ))
            .id_salt(("step", i))
            .default_open(true)
            .show(ui, |ui| {
                if ui
                    .checkbox(&mut step.rest, "Rest: alle übrigen Objekte")
                    .changed()
                {
                    step.filters = if step.rest { None } else { Some(Vec::new()) };
                    step.objects.clear();
                    *dirty = true;
                }
                if let Some(list) = &mut step.filters {
                    *dirty |= filters(ui, ("step", i), list);
                }
                *dirty |= parameters(
                    ui,
                    &mut step.power_percent,
                    &mut step.speed_percent,
                    &mut step.passes,
                );
                let mut drop = None;
                for (j, set) in step.additional.iter_mut().enumerate() {
                    ui.label(format!("Parametersatz {}", j + 2));
                    *dirty |= parameters(
                        ui,
                        &mut set.power_percent,
                        &mut set.speed_percent,
                        &mut set.passes,
                    );
                    if ui.small_button("Parametersatz entfernen").clicked() {
                        drop = Some(j);
                    }
                }
                if let Some(j) = drop {
                    step.additional.remove(j);
                    *dirty = true;
                }
                if step.additional.len() < 15 && ui.small_button("Weiteren Parametersatz").clicked()
                {
                    step.additional.push(ParameterSet {
                        power_percent: snapshot.power_percent,
                        speed_percent: snapshot.speed_percent,
                        passes: snapshot.passes,
                    });
                    *dirty = true;
                }
                if step.operation.is_raster() {
                    *dirty |= raster_options(ui, ("step", i), &mut step.raster, step.operation);
                }
                if ui.small_button("Schritt entfernen").clicked() {
                    remove = Some(i);
                }
            });
        }
        if let Some(i) = remove {
            project.steps.remove(i);
            *dirty = true;
        }
        ui.horizontal_wrapped(|ui| {
            ui.label("Schritt:");
            for operation in Operation::ALL {
                if ui.small_button(operation_title(operation)).clicked() {
                    let step = JobStep {
                        filters: Some(Vec::new()),
                        ..self.new_step(project, operation)
                    };
                    project.steps.push(step);
                    *dirty = true;
                }
            }
        });
        ui.label("Ignorieren");
        let mut remove = None;
        for (i, list) in project.ignore_filters.iter_mut().enumerate() {
            *dirty |= filters(ui, ("ignore", i), list);
            if ui.small_button("Ignorierregel entfernen").clicked() {
                remove = Some(i);
            }
        }
        if let Some(i) = remove {
            project.ignore_filters.remove(i);
            *dirty = true;
        }
        if ui.small_button("Ignorierregel hinzufügen").clicked() {
            project.ignore_filters.push(Vec::new());
            *dirty = true;
        }
    }

    pub fn windows(&mut self, ctx: &egui::Context, status: &mut String) {
        let Some((mut draft, mut selected)) = self.editor.take() else {
            return;
        };
        let mut open = true;
        let mut keep = true;
        egui::Window::new("Materialbibliothek")
            .open(&mut open)
            .default_size([680.0, 460.0])
            .show(ctx, |ui| {
                ui.horizontal_top(|ui| {
                    ui.vertical(|ui| {
                        ui.set_width(180.0);
                        egui::ScrollArea::vertical()
                            .id_salt("materials")
                            .max_height(360.0)
                            .show(ui, |ui| {
                                for (i, m) in draft.iter().enumerate() {
                                    ui.selectable_value(&mut selected, i, &m.name);
                                }
                            });
                        ui.horizontal(|ui| {
                            if ui.button("Neu").clicked() {
                                draft.push(Material {
                                    id: format!("eigen-{}", draft.len()),
                                    name: format!("Neues Material {}", draft.len() + 1),
                                    profiles: Vec::new(),
                                });
                                selected = draft.len() - 1;
                            }
                            if ui
                                .add_enabled(!draft.is_empty(), egui::Button::new("Entfernen"))
                                .clicked()
                            {
                                draft.remove(selected);
                                selected = selected.saturating_sub(1);
                            }
                        });
                    });
                    ui.separator();
                    ui.vertical(|ui| {
                        let Some(material) = draft.get_mut(selected) else {
                            return;
                        };
                        ui.text_edit_singleline(&mut material.name);
                        let mut drop = None;
                        egui::ScrollArea::vertical()
                            .id_salt("profiles")
                            .max_height(320.0)
                            .show(ui, |ui| {
                                egui::Grid::new("profiles").num_columns(5).show(ui, |ui| {
                                    ui.label("Stärke");
                                    ui.label("Verfahren");
                                    ui.label("Leistung");
                                    ui.label("Geschwindigkeit");
                                    ui.end_row();
                                    for (j, p) in material.profiles.iter_mut().enumerate() {
                                        ui.add(
                                            egui::DragValue::new(&mut p.thickness_mm)
                                                .range(0.0..=1000.0)
                                                .suffix(" mm"),
                                        );
                                        egui::ComboBox::from_id_salt(("profile", j))
                                            .selected_text(operation_title(p.operation))
                                            .show_ui(ui, |ui| {
                                                for o in Operation::ALL {
                                                    ui.selectable_value(
                                                        &mut p.operation,
                                                        o,
                                                        operation_title(o),
                                                    );
                                                }
                                            });
                                        ui.add(
                                            egui::DragValue::new(&mut p.power_percent)
                                                .range(0.0..=100.0)
                                                .suffix(" %"),
                                        );
                                        ui.add(
                                            egui::DragValue::new(&mut p.speed_percent)
                                                .range(0.0..=100.0)
                                                .suffix(" %"),
                                        );
                                        if ui.small_button("✕").clicked() {
                                            drop = Some(j);
                                        }
                                        ui.end_row();
                                    }
                                });
                            });
                        if let Some(j) = drop {
                            material.profiles.remove(j);
                        }
                        if ui.button("Profil hinzufügen").clicked() {
                            material.profiles.push(MaterialProfile {
                                thickness_mm: 3.0,
                                operation: Operation::Cut,
                                power_percent: 100.0,
                                speed_percent: 10.0,
                                source: None,
                            });
                        }
                    });
                });
                ui.separator();
                ui.horizontal(|ui| {
                    if ui.button("Importieren …").clicked()
                        && let Some(path) = rfd::FileDialog::new()
                            .add_filter("JSON", &["json"])
                            .pick_file()
                    {
                        let mut library = Library {
                            materials: draft.clone(),
                            ..self.library.clone()
                        };
                        match std::fs::read(&path)
                            .map_err(|e| e.to_string())
                            .and_then(|b| Library::import(&b))
                        {
                            Ok(other) => {
                                *status =
                                    format!("{} Materialien importiert", library.merge(other));
                                draft = library.materials;
                            }
                            Err(e) => *status = e,
                        }
                    }
                    if ui.button("Exportieren …").clicked()
                        && let Some(path) = rfd::FileDialog::new()
                            .add_filter("JSON", &["json"])
                            .set_file_name("Materialien.json")
                            .save_file()
                    {
                        let library = Library {
                            materials: draft.clone(),
                            ..self.library.clone()
                        };
                        *status = match library.export(&path) {
                            Ok(()) => "Materialbibliothek exportiert".into(),
                            Err(e) => e,
                        };
                    }
                    if ui
                        .add_enabled(self.custom, egui::Button::new("FAU wiederherstellen"))
                        .clicked()
                    {
                        match Library::reset(&device::config_dir()) {
                            Ok(library) => {
                                draft = library.materials.clone();
                                self.library = library;
                                self.custom = false;
                                *status = "FAU-Materialbibliothek wiederhergestellt".into();
                            }
                            Err(e) => *status = e,
                        }
                    }
                    if ui
                        .add_enabled(
                            draft != self.library.materials,
                            egui::Button::new("Sichern"),
                        )
                        .clicked()
                    {
                        let library = Library {
                            materials: draft.clone(),
                            ..self.library.clone()
                        };
                        match library.save(&device::config_dir()) {
                            Ok(()) => {
                                self.library = library;
                                self.custom = true;
                                *status = "Materialbibliothek gesichert".into();
                            }
                            Err(e) => *status = e,
                        }
                    }
                    if ui.button("Schließen").clicked() {
                        keep = false;
                    }
                });
            });
        if open && keep {
            self.editor = Some((draft, selected));
        }
    }
}
