use std::time::UNIX_EPOCH;

use eframe::egui;

use crate::presenter::Presenter;

pub fn show(ui: &mut egui::Ui, p: &mut Presenter) {
    let mut chosen = None;
    egui::ScrollArea::vertical()
        .id_salt("history")
        .auto_shrink([false, false])
        .show(ui, |ui| {
            if p.history.is_empty() {
                ui.weak("No queries yet");
            }
            for (i, entry) in p.history.iter().enumerate() {
                let first_line = entry.text.lines().next().unwrap_or_default();
                let mut label = egui::RichText::new(first_line);
                if !entry.succeeded {
                    label = label.color(ui.visuals().error_fg_color);
                }
                let secs = entry
                    .executed_at
                    .duration_since(UNIX_EPOCH)
                    .map_or(0, |d| d.as_secs());
                let response = ui
                    .add(egui::Button::new(label).truncate().frame(false))
                    .on_hover_text(format!(
                        "{}\n\n{} ms, finished at unix time {secs}",
                        entry.text,
                        entry.elapsed.as_millis()
                    ));
                if response.clicked() {
                    chosen = Some(i);
                }
            }
        });
    if let Some(i) = chosen {
        p.use_history(i);
    }
}
