//! The egui preferences dialog.
//!
//! Rendered as an egui overlay (ADR-0004) on top of the terminal when open. It
//! edits an `rt_config::Settings` in place; the caller diffs the result to
//! persist and apply changes. This is the first egui surface — colour pickers
//! and the palette editor join it as those settings land.

use rt_config::Settings;

/// Build the preferences window for this frame. Mutates `settings` directly
/// (sliders/checkboxes bind to its fields) and sets `close` true when the user
/// dismisses the dialog. Call once per frame from the egui run closure.
pub fn ui(ctx: &egui::Context, settings: &mut Settings, close: &mut bool) {
    egui::Window::new("rt preferences")
        .collapsible(false)
        .resizable(false)
        .default_width(320.0)
        .show(ctx, |ui| {
            ui.heading("Appearance");
            // Background opacity: 0.05 (see-through) .. 1.0 (opaque).
            ui.add(
                egui::Slider::new(
                    &mut settings.background_opacity,
                    Settings::MIN_OPACITY..=1.0,
                )
                .text("Background opacity"),
            );
            // Scrim: rt's portable blur stand-in (washes out what's behind).
            ui.add(
                egui::Slider::new(&mut settings.scrim_strength, 0.0..=Settings::MAX_SCRIM)
                    .text("Background scrim"),
            );

            ui.separator();
            ui.heading("Behaviour");
            // Focus mode: click-to-focus vs focus-follows-mouse (sloppy).
            ui.checkbox(&mut settings.focus_follows_mouse, "Focus follows mouse");

            ui.separator();
            // The dialog is dismissed by this button or the Escape key.
            ui.horizontal(|ui| {
                if ui.button("Close").clicked() {
                    *close = true;
                }
                ui.label("(Esc closes)");
            });
        });
}
