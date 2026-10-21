use crate::app::Player;
use crate::instance::{Instance, Request};
use eframe::egui::{self, ViewportCommand};
use std::sync::mpsc;

pub struct ResidentPlayer {
    player: Player,
    visible: bool,
    requests: mpsc::Receiver<Request>,
    _instance: Instance,
}

impl ResidentPlayer {
    pub fn new(player: Player, instance: Instance, requests: mpsc::Receiver<Request>) -> Self {
        Self {
            player,
            visible: true,
            requests,
            _instance: instance,
        }
    }
}

impl eframe::App for ResidentPlayer {
    fn update(&mut self, context: &egui::Context, frame: &mut eframe::Frame) {
        while let Ok(request) = self.requests.try_recv() {
            match request {
                Request::Quit => self.player.quit_requested = true,
                Request::Open(path, still, preserve_rotation) => {
                    if let Some(path) = path {
                        self.player.open_with_mode(path, still, preserve_rotation);
                    }
                    self.visible = true;
                    context.send_viewport_cmd(ViewportCommand::Visible(true));
                    context.send_viewport_cmd(ViewportCommand::Minimized(false));
                    context.send_viewport_cmd(ViewportCommand::Focus);
                }
            }
        }
        if self.player.quit_requested {
            context.send_viewport_cmd(ViewportCommand::Close);
            return;
        }
        if self.visible {
            self.player.update(context, frame);
        } else {
            self.player.poll(context);
        }
        if self.player.quit_requested {
            context.send_viewport_cmd(ViewportCommand::Close);
        } else if self.player.hide_requested
            || context.input(|input| input.viewport().close_requested())
        {
            context.send_viewport_cmd(ViewportCommand::CancelClose);
            context.send_viewport_cmd(ViewportCommand::Visible(false));
            context.send_viewport_cmd(ViewportCommand::Title("Player".into()));
            self.player.close(frame.gl().map(AsRef::as_ref));
            self.player.hide_requested = false;
            self.visible = false;
        }
    }

    fn on_exit(&mut self, gl: Option<&eframe::glow::Context>) {
        self.player.on_exit(gl);
    }
}
