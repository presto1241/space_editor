#![allow(clippy::too_many_arguments)]
use std::sync::Arc;

use bevy::{ecs::query::QueryFilter, prelude::*, utils::HashMap};
use bevy_egui::{
    egui::{Label, PointerButton, TextEdit, collapsing_header::CollapsingState}, *,
};
use space_editor_core::prelude::*;
use space_prefab::{component::SceneAutoChild, editor_registry::EditorRegistry};
use space_undo::{AddedEntity, NewChange, RemovedEntity, UndoSet};

use space_shared::*;

use space_editor_tabs::prelude::*;

use crate::{colors::WARN_COLOR, editor_tab_name::EditorTabName, ui_components::bundle_menus, ui_registration::BundleReg};

/// Event to clone entity with clone all registered components
#[derive(Event)]
pub struct CloneEvent {
    pub id: Entity,
}

/// Plugin to activate hierarchy UI in editor UI
#[derive(Default)]
pub struct SpaceHierarchyPlugin {}

impl Plugin for SpaceHierarchyPlugin {
    #[cfg(not(tarpaulin_include))]
    fn build(&self, app: &mut App) {
        app.init_resource::<HierarchyTabState>();
        app.editor_tab(EditorTabName::Hierarchy, show_hierarchy);

        // app.add_systems(Update, show_hierarchy.before(crate::editor::ui_camera_block).in_set(EditorSet::Editor));
        app.add_systems(Update, clone_enitites.in_set(EditorSet::Editor));
        app.add_systems(
            PostUpdate,
            detect_cloned_entities
                .in_set(EditorSet::Editor)
                .before(UndoSet::PerType),
        );
        app.add_event::<CloneEvent>();
    }
}

#[derive(Resource, Default)]
pub struct HierarchyTabState {
    pub show_editor_entities: bool,
    pub show_spawnable_bundles: bool,
    pub entity_filter: String,
    pub anchor: Option<Entity>
}

pub type HierarchyQueryIter<'a> = (
    Entity,
    Option<&'a Name>,
    Option<&'a Children>,
    Option<&'a Parent>,
);

/// Rebuilt every frame while the hierarchy draws, so it matches the screen.
#[derive(Default)]
struct VisibleRows {
    order: Vec<Entity>,
    click: Option<RowClick>,
}

impl VisibleRows {
    /// Rows from `anchor` to `clicked` inclusive, in either direction.
    fn range(&self, anchor: Entity, clicked: Entity) -> Option<&[Entity]> {
        let a = self.order.iter().position(|e| *e == anchor)?;
        let b = self.order.iter().position(|e| *e == clicked)?;
        Some(&self.order[a.min(b)..=a.max(b)])
    }
}

#[derive(Clone, Copy)]
enum RowClick { Select(Entity), Toggle(Entity), Range(Entity, bool) }

impl RowClick {
    fn from_modifiers(entity: Entity, m: egui::Modifiers) -> Self {
        if m.shift { return Self::Range(entity, m.command); }
        if m.command { return Self::Toggle(entity); }
        Self::Select(entity)
    }
}

/// System to show hierarchy
pub fn show_hierarchy(
    mut commands: Commands,
    query: Query<HierarchyQueryIter, With<PrefabMarker>>,
    all_entities: Query<HierarchyQueryIter>,
    mut selected: Query<Entity, With<Selected>>,
    mut clone_events: EventWriter<CloneEvent>,
    mut ui: NonSendMut<EditorUiRef>,
    mut changes: EventWriter<NewChange>,
    mut state: ResMut<HierarchyTabState>,
    auto_children: Query<(), With<SceneAutoChild>>,
    bundles: Res<BundleReg>
) {
    let mut all: Vec<_> = if state.show_editor_entities {
        all_entities.iter().collect()
    } else {
        query.iter().collect()
    };
    all.sort_by_key(|a| a.0);
    let ui = &mut ui.0;

    // Draw Search Bar
    ui.horizontal(|ui| {
        let button_size = ui
            .style()
            .text_styles
            .get(&egui::TextStyle::Button)
            .map(|f| f.size)
            .unwrap_or(14.);
        let button_padding = ui.style().spacing.button_padding.x * 2.;
        let space = ui.style().spacing.item_spacing.x;
        let width = 2.0f32.mul_add(-space, ui.available_width() - button_size - button_padding);
        ui.add(TextEdit::singleline(&mut state.entity_filter).desired_width(width));
        if ui.button("🗑").on_hover_text("Clear filter").clicked() {
            state.entity_filter.clear();
        }
    });
    ui.spacing();
    let lower_filter = state.entity_filter.to_lowercase();

    // Draw Hierarchy Empty Context Menu
    let area = ui.available_rect_before_wrap();
    let bg = ui.interact(area, ui.id().with("hierarchy_bg"), egui::Sense::click());
    bg.context_menu(|ui| {
        hierarchy_empty_context(ui, &query, &mut selected, &mut commands, &mut changes, &bundles);
    });

    if query.is_empty() {
        ui.vertical_centered(|ui| {
            ui.add(Label::new("No Entities in Hierarchy").selectable(false));
        });
    }

    let mut rows = VisibleRows::default();

    // Draw Scene Entities
    egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
        for (entity, _name, _children, parent) in all.iter().filter(|(_, name, _, _)| {
            name.map(|n| n.to_lowercase())
                .unwrap_or_else(|| "entity".to_string())
                .contains(&lower_filter)
        }) {
            // Draw entities without Parents
            if parent.is_none() {
                if state.show_editor_entities {
                    draw_entity::<()>(
                        &mut commands,
                        ui,
                        &all_entities,
                        *entity,
                        &mut selected,
                        &mut clone_events,
                        &mut changes,
                        &auto_children,
                        &bundles,
                        &mut rows
                    );
                // Draw entities with Parents
                } else {
                    draw_entity::<With<PrefabMarker>>(
                        &mut commands,
                        ui,
                        &query,
                        *entity,
                        &mut selected,
                        &mut clone_events,
                        &mut changes,
                        &auto_children,
                        &bundles,
                        &mut rows
                    );
                }
            }
        }
    });

    handle_selection(&mut rows, &mut state.anchor, &mut selected, &mut commands);
}

type DrawIter<'a> = (
    Entity,
    Option<&'a Name>,
    Option<&'a Children>,
    Option<&'a Parent>,
);

// Draws Scene Entities with context to the UI
fn draw_entity<F: QueryFilter>(
    commands: &mut Commands,
    ui: &mut egui::Ui,
    query: &Query<DrawIter, F>,
    entity: Entity,
    selected: &mut Query<Entity, With<Selected>>,
    clone_events: &mut EventWriter<CloneEvent>,
    changes: &mut EventWriter<NewChange>,
    auto_children: &Query<(), With<SceneAutoChild>>,
    bundles: &BundleReg,
    rows: &mut VisibleRows
) {
    let Ok((_, name, children, parent)) = query.get(entity) else {
        return;
    };
    rows.order.push(entity);

    let entity_name = name.map_or_else(
        || format!("Entity ({})", entity),
        |name| format!("{} ({})", name.as_str(), entity),
    );

    let is_selected = selected.contains(entity);

    if children.is_some_and(|children| children.iter().any(|child| query.get(*child).is_ok())) {
        CollapsingState::load_with_default_open(
            ui.ctx(),
            ui.make_persistent_id(entity_name.clone()),
            true,
        )
        .show_header(ui, |ui| {
            let mut entity_name = egui::RichText::new(entity_name.clone());
            let is_auto_child = auto_children.get(entity).is_ok();
            if is_auto_child {
                entity_name = entity_name.italics();
            }

            let response = ui.selectable_label(is_selected, entity_name);
            let is_clicked = response.clicked()
                || (response.clicked_by(PointerButton::Secondary) && !is_selected);

            if is_auto_child {
                response.context_menu(|ui| {
                    if ui.button("Delete").clicked() {
                        commands.entity(entity).despawn_recursive();
                    }
                    ui.label(crate::egui::RichText::new("⚠ Concrete Bevy entity cannot be reparented or cloned.\nTry \"Unpack gltf as prefab\" for that.").color(WARN_COLOR));
                });
            } else {
                response.context_menu(|ui| {
                    hierarchy_entity_context(
                        ui,
                        commands,
                        entity,
                        changes,
                        clone_events,
                        selected,
                        parent,
                        bundles
                    );
                });
            }

            if is_clicked {
                rows.click = Some(RowClick::from_modifiers(entity, ui.input(|i| i.modifiers)));
            }
        })
        .body(|ui| {
            // already checked that children is some
            for child in children.unwrap().iter() {
                draw_entity(
                    commands,
                    ui,
                    query,
                    *child,
                    selected,
                    clone_events,
                    changes,
                    auto_children,
                    bundles,
                    rows
                );
            }
        });
    } else {
        let mut entity_name = egui::RichText::new(format!("      {}", entity_name));
        let is_auto_child = auto_children.get(entity).is_ok();
        if is_auto_child {
            entity_name = entity_name.italics();
        }

        let selectable = ui.selectable_label(is_selected, entity_name);
        let is_clicked = selectable.clicked()
            || (selectable.clicked_by(PointerButton::Secondary) && !is_selected);

        if is_auto_child {
            selectable.context_menu(|ui| {
                if ui.button("Delete").clicked() {
                    commands.entity(entity).despawn_recursive();
                }
                ui.label(crate::egui::RichText::new("⚠ Concrete Bevy entity cannot be reparented or cloned.\nTry \"Unpack gltf as prefab\" for that.").color(WARN_COLOR));
            });
        } else {
            selectable.context_menu(|ui| {
                hierarchy_entity_context(
                    ui,
                    commands,
                    entity,
                    changes,
                    clone_events,
                    selected,
                    parent,
                    bundles
                );
            });
        }

        if is_clicked {
            rows.click = Some(RowClick::from_modifiers(entity, ui.input(|i| i.modifiers)));
        }
    };
}

fn handle_selection(
    rows: &mut VisibleRows,
    anchor: &mut Option<Entity>,
    selected: &mut Query<Entity, With<Selected>>,
    commands: &mut Commands<'_, '_>,
) {
    let Some(click) = rows.click else { return; };

    let click = match click {
        RowClick::Range(e, _) if anchor.and_then(|a| rows.range(a, e)).is_none() => RowClick::Select(e),
        other => other
    };

    match click {
        RowClick::Toggle(entity) => {
            if selected.contains(entity) {
                commands.entity(entity).remove::<Selected>();
            }
            else {
                commands.entity(entity).insert(Selected);
            }

            *anchor = Some(entity);
        }


        RowClick::Select(entity) => {
            for s in selected.iter().filter(|s| *s != entity) {
                commands.entity(s).remove::<Selected>();
            }

            commands.entity(entity).insert(Selected);
            *anchor = Some(entity);
        }


        RowClick::Range(entity, additive) => {
            let Some(range) = anchor.and_then(|a| rows.range(a, entity)) else { return; };
            if !additive {
                for s in selected.iter().filter(|s| !range.contains(s)) {
                    commands.entity(s).remove::<Selected>();
                }
            }

            for r in range {
                commands.entity(*r).insert(Selected);
            }
        }
    }
}

fn hierarchy_empty_context(
    ui: &mut egui::Ui,
    query: &Query<HierarchyQueryIter, With<PrefabMarker>>,
    selected: &mut Query<Entity, With<Selected>>,
    commands: &mut Commands<'_, '_>,
    changes: &mut EventWriter<'_, NewChange>,
    bundles: &BundleReg
) {
    if ui.button("New Entity").clicked() {
        let id = commands.spawn_empty().insert((PrefabMarker, Transform::default())).id();
        changes.send(NewChange {
            change: Arc::new(AddedEntity { entity: id }),
        });
        ui.close_menu();
    }

    if ui.button("Select All").clicked() {
        for (entity, ..) in query.iter()
        {
            commands.entity(entity).insert(Selected);
        }
        ui.close_menu();
    }

    // Create a deselect all button if there is more than one entity selected
    if !selected.is_empty() {
        if ui.button("Deselect All").clicked() {
            selected.iter_mut().for_each(|e| {
                commands.entity(e).remove::<Selected>();
            });
        }
    }
    // Show a deselect all label if there are no entities selected
    else {
        ui.label("Deselect All");
    }

    // Show bundle menu
    let mut picked = None;
    ui.menu_button("Add Bundle", |ui| {
        picked = bundle_menus(ui, bundles);
    });

    if let Some(bundle) = picked {
       let entity = bundle.spawn(commands);
       changes.send(NewChange {
           change: Arc::new(AddedEntity { entity }),
       });
    }
}

fn hierarchy_entity_context(
    ui: &mut egui::Ui,
    commands: &mut Commands<'_, '_>,
    entity: Entity,
    changes: &mut EventWriter<'_, NewChange>,
    clone_events: &mut EventWriter<'_, CloneEvent>,
    selected: &mut Query<'_, '_, Entity, With<Selected>>,
    parent: Option<&Parent>,
    bundles: &BundleReg
) {
    // Spawn and select new entity
    if ui.button("Add child").clicked() {
        for target in selected.iter() {
            // Create and child the new entity
            let new_id = commands.spawn_empty().insert((PrefabMarker, Transform::default())).id();
            commands.entity(target).add_child(new_id);

            changes.send(NewChange {
                change: Arc::new(AddedEntity { entity: new_id }),
            });

            // Select new entity and de-select current
            commands.entity(target).remove::<Selected>();
            commands.entity(new_id).insert(Selected);
        }
        ui.close_menu();
    }

    if ui.button("Delete").clicked() {
        for target in selected.iter() {
            commands.entity(target).despawn_recursive();
            changes.send(NewChange {
                change: Arc::new(RemovedEntity { entity: target }),
            });
        }
        ui.close_menu();
    }

    if ui.button("Clone").clicked() {
        for target in selected.iter() {
            clone_events.send(CloneEvent { id: target });
        }
        ui.close_menu();
    }

    if !selected.is_empty() && !selected.contains(entity) && ui.button("Attach to").clicked() {
        for target in selected.iter() {
            commands.entity(entity).add_child(target);
        }
    }

    if parent.is_some() && ui.button("Detach").clicked() {
        for target in selected.iter() {
            commands.entity(target).remove_parent();
        }
    }

    // Show bundle menu
    //let picked = bundle_menus(ui, bundles);

    let mut picked = None;
    ui.menu_button("Add Bundle", |ui| {
        picked = bundle_menus(ui, bundles);
    });

    if let Some(bundle) = picked {
        for target in selected.iter() {
            let bundle = bundle.spawn(commands);
            commands.entity(target).add_child(bundle);

            changes.send(NewChange {
                change: Arc::new(AddedEntity { entity: bundle }),
            });

            commands.entity(target).remove::<Selected>();
            commands.entity(bundle).insert(Selected);
        }
        ui.close_menu();
    }
}

#[derive(Component)]
pub struct ClonedEntity;

fn clone_enitites(
    mut commands: Commands,
    query: Query<EntityRef>,
    mut events: EventReader<CloneEvent>,
    editor_registry: Res<EditorRegistry>,
) {
    for event in events.read() {
        let mut queue = vec![(event.id, commands.spawn_empty().id())];
        let mut map = HashMap::new();

        while let Some((src_id, dst_id)) = queue.pop() {
            map.insert(src_id, dst_id);
            if let Ok(entity) = query.get(src_id) {
                if entity.contains::<PrefabMarker>() {
                    let mut cmds = commands.entity(dst_id);
                    cmds.insert(ClonedEntity);

                    editor_registry.clone_entity_flat(&mut cmds, &entity);

                    if let Some(parent) = entity.get::<Parent>() {
                        if let Some(new_parent) = map.get(&parent.get()) {
                            commands.entity(*new_parent).add_child(dst_id);
                        } else {
                            commands.entity(parent.get()).add_child(dst_id);
                        }
                    }

                    if let Some(children) = entity.get::<Children>() {
                        for id in children {
                            queue.push((*id, commands.spawn_empty().id()));
                        }
                    }
                }
            }
        }
    }
    events.clear();
}

fn detect_cloned_entities(
    mut commands: Commands,
    query: Query<Entity, Added<ClonedEntity>>,
    mut changes: EventWriter<NewChange>,
) {
    for entity in query.iter() {
        commands.entity(entity).remove::<ClonedEntity>();
        changes.send(NewChange {
            change: Arc::new(AddedEntity { entity }),
        });
    }
}
