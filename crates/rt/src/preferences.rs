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
pub fn ui(
    ctx: &egui::Context,
    settings: &mut Settings,
    close: &mut bool,
    families: &[String],
    mem_total_bytes: u64,
    cols: usize,
) {
    egui::Window::new("rt preferences")
        .collapsible(false)
        .resizable(false)
        .default_width(340.0)
        .show(ctx, |ui| {
            ui.heading("Font");
            // Size slider.
            ui.add(egui::Slider::new(&mut settings.font_size, 8.0..=48.0).text("Size (px)"));
            // Family combo, populated with the system's monospace families.
            egui::ComboBox::from_label("Family")
                .selected_text(settings.font_family.clone())
                .show_ui(ui, |ui| {
                    for fam in families {
                        ui.selectable_value(&mut settings.font_family, fam.clone(), fam);
                    }
                });

            ui.separator();
            ui.heading("Appearance");
            // Background opacity: 0.05 (see-through) .. 1.0 (opaque). Pair a
            // low-ish opacity with a dark background colour and the blur below for
            // a tasteful frosted look — legible text, only shapes/motion behind.
            ui.add(
                egui::Slider::new(
                    &mut settings.background_opacity,
                    Settings::MIN_OPACITY..=1.0,
                )
                .text("Background opacity"),
            );
            // Compositor blur where the protocol exists (KDE 6.7+, COSMIC, niri).
            // On/off only — the compositor picks the radius (the protocol exposes
            // no strength). Takes effect only while the background is translucent;
            // a silent no-op elsewhere.
            ui.checkbox(&mut settings.background_blur, "Background blur (compositor; if supported)");

            ui.separator();
            ui.heading("Colours");
            // Foreground / background swatches (egui colour pickers).
            ui.horizontal(|ui| {
                ui.label("Text");
                ui.color_edit_button_srgb(&mut settings.foreground);
                ui.label("Background");
                ui.color_edit_button_srgb(&mut settings.background);
            });
            // The 16 ANSI palette colours, in two rows of eight.
            ui.label("ANSI palette");
            ui.horizontal(|ui| {
                for c in settings.palette.iter_mut().take(8) {
                    ui.color_edit_button_srgb(c);
                }
            });
            ui.horizontal(|ui| {
                for c in settings.palette.iter_mut().skip(8) {
                    ui.color_edit_button_srgb(c);
                }
            });
            // Preset schemes (Terminator's `_Colors` menu): clicking one fills
            // fg/bg/palette, which the user can then tweak above.
            ui.horizontal_wrapped(|ui| {
                ui.label("Preset:");
                for scheme in rt_config::SCHEMES {
                    if ui.button(scheme.name).clicked() {
                        settings.foreground = scheme.foreground;
                        settings.background = scheme.background;
                        settings.palette = scheme.palette;
                    }
                }
            });

            ui.separator();
            ui.heading("Behaviour");
            // Focus mode: click-to-focus vs focus-follows-mouse (sloppy).
            ui.checkbox(&mut settings.focus_follows_mouse, "Focus follows mouse");
            // Per-pane titlebars (title + size + group) vs the borderless look.
            ui.checkbox(&mut settings.show_titlebar, "Show per-pane titlebars");
            // Scrollback buffer size (lines kept above the screen). Logarithmic
            // so the slider spans 1k…20M usefully. Applies to new terminals.
            ui.add(
                egui::Slider::new(&mut settings.scrollback, 1000..=Settings::MAX_SCROLLBACK)
                    .logarithmic(true)
                    .text("Scrollback (lines, new terminals)"),
            );
            // Live memory estimate for a FULL buffer at the current pane width —
            // PER PANE. This is the guardrail: sliding to the max can pick a size
            // no machine can hold, so show the cost (and its share of RAM) in a
            // colour that reddens as it approaches, before the user commits.
            {
                let per_line = cols.max(1) as u64 * rt_engine::CELL_BYTES as u64 + 32; // + row overhead
                let full = (settings.scrollback as u64).saturating_mul(per_line);
                let (val, unit) = if full >= 1_000_000_000 {
                    (full as f64 / 1e9, "GB")
                } else {
                    (full as f64 / 1e6, "MB")
                };
                let frac = if mem_total_bytes > 0 { full as f64 / mem_total_bytes as f64 } else { 0.0 };
                // Grey when comfortable, amber past a quarter of RAM, red past half.
                let colour = if frac > 0.5 {
                    egui::Color32::from_rgb(0xe0, 0x60, 0x50)
                } else if frac > 0.25 {
                    egui::Color32::from_rgb(0xe0, 0xc0, 0x50)
                } else {
                    ui.visuals().weak_text_color()
                };
                let text = if mem_total_bytes > 0 {
                    format!("≈ {val:.1} {unit} per pane if full — {:.0}% of RAM, at {cols} cols", frac * 100.0)
                } else {
                    format!("≈ {val:.1} {unit} per pane if full, at {cols} cols")
                };
                ui.colored_label(colour, text);
                ui.colored_label(colour, "each pane keeps its own buffer — N panes ⇒ N× this");
            }

            ui.add_space(6.0);
            ui.heading("Border instruments");
            ui.checkbox(&mut settings.inst_output, "Output activity (green flow)");
            ui.checkbox(&mut settings.inst_heat, "CPU heat (blackbody border)");
            ui.checkbox(&mut settings.inst_latency, "Latency (violet window frame)");
            ui.checkbox(&mut settings.show_jacks, "Patch-bay jacks");

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
