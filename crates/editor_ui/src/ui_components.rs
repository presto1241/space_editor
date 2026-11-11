use bevy_egui::egui;

use crate::ui_registration::{BundleReg, EditorBundleUntyped};

/// Draws a vertical menu with bundles you can spawn.
pub fn bundle_menus<'a>(ui: &mut egui::Ui, bundles: &'a BundleReg) -> Option<&'a EditorBundleUntyped> {
    let mut picked = None;

    for (category_name, category) in bundles.bundles.iter() {
        ui.menu_button(category_name, |ui| {
            for (name, bundle) in category.iter() {

                if !ui.button(name).clicked() { continue; }

                picked = Some(bundle);
                ui.close_menu();
            }
        });
    }
    return picked;
}
