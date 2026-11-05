// Skiff — a fast, modern FTP/FTPS/SFTP client written in pure Rust.
// Copyright (C) 2026 Skiff contributors
// Licensed under the GNU General Public License v3.0 or later. See LICENSE.

#![cfg_attr(all(not(debug_assertions), target_os = "windows"), windows_subsystem = "windows")]

mod app;
mod config;
mod events;
mod icon;
mod local;
mod remote;
mod session;
mod theme;
mod transfer;
mod trust;
mod ui;

fn main() -> eframe::Result {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("Skiff")
            .with_app_id("skiff")
            .with_inner_size([1320.0, 820.0])
            .with_min_inner_size([960.0, 600.0])
            .with_icon(icon::icon_data()),
        // OpenGL works everywhere (VMs, RDP, old GPUs).
        renderer: eframe::Renderer::Glow,
        ..Default::default()
    };
    eframe::run_native("Skiff", options, Box::new(|cc| Ok(Box::new(app::App::new(cc)))))
}
