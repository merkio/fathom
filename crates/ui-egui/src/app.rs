//! eframe application: glue between the presenter, the backend bridge and the views.

use std::sync::Arc;
use std::sync::mpsc::Receiver;

use application::usecases::UseCases;
use eframe::egui;

use crate::backend::Backend;
use crate::bridge::Event;
use crate::presenter::Presenter;
use crate::views;

pub struct DbieverApp {
    presenter: Presenter,
    backend: Backend,
    events: Receiver<Event>,
}

impl DbieverApp {
    pub fn new(ctx: &egui::Context, use_cases: Arc<UseCases>) -> Self {
        let presenter = Presenter::new(use_cases.dialects());
        let repaint = ctx.clone();
        let (backend, events) = Backend::spawn(use_cases, move || repaint.request_repaint());
        Self {
            presenter,
            backend,
            events,
        }
    }

    fn pump_events(&mut self) {
        while let Ok(event) = self.events.try_recv() {
            self.presenter.handle(event);
        }
    }

    fn flush_commands(&mut self) {
        for command in self.presenter.take_commands() {
            self.backend.send(command);
        }
    }
}

impl eframe::App for DbieverApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        self.pump_events();
        views::show(ui, &mut self.presenter);
        self.flush_commands();
    }
}

/// Opens the main window and blocks until it is closed.
pub fn run(use_cases: Arc<UseCases>) -> eframe::Result {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("DBiever")
            .with_inner_size([1280.0, 800.0]),
        ..Default::default()
    };
    eframe::run_native(
        "DBiever",
        options,
        Box::new(move |cc| Ok(Box::new(DbieverApp::new(&cc.egui_ctx, use_cases)))),
    )
}
