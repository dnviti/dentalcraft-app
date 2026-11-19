//! `dentalcraft`: the DentalCraft desktop application.
//!
//! `dentalcraft --control <port>` also listens on `127.0.0.1:<port>` for
//! the bridge protocol of `dentalcraft-cli mcp --connect <port>` (see
//! `dc_app::control`).

// Release builds on Windows are GUI-subsystem (no console window); logs go to the log file.
#![cfg_attr(
    all(target_os = "windows", not(debug_assertions)),
    windows_subsystem = "windows"
)]

use dc_app::config::{Dirs, Store};
use dc_app::control::{ControlServer, ControlledApp};
use dc_app::{AppDeps, DentalCraftApp, Updates};

/// The port of `--control <port>` / `--control=<port>`, if given.
fn control_port() -> Result<Option<u16>, String> {
    let mut args = std::env::args().skip(1);
    let mut port = None;
    while let Some(a) = args.next() {
        let value = if a == "--control" {
            args.next()
                .ok_or("--control needs a port (0 picks a free one)")?
        } else if let Some(v) = a.strip_prefix("--control=") {
            v.to_owned()
        } else {
            continue;
        };
        port = Some(
            value
                .parse::<u16>()
                .map_err(|_| format!("--control: invalid port {value:?}"))?,
        );
    }
    Ok(port)
}

fn main() -> eframe::Result {
    let control = match control_port().and_then(|p| {
        p.map(|port| ControlServer::bind(port).map_err(|e| format!("--control {port}: {e}")))
            .transpose()
    }) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("error: {e}");
            std::process::exit(2);
        }
    };
    let dirs = Dirs::standard();
    let store = Store::new(dirs);
    let settings = store.load_settings();
    let issuer = settings.issuer();
    // anonymous diagnostics: nothing is collected or sent without the
    // user's consent (asked at the first start)
    let diagnostics = dc_diagnostics::Diagnostics::new(
        dc_diagnostics::Config::standard(
            &dc_diagnostics::settings::Paths {
                config: store.dirs().config.clone(),
                data: store.dirs().data.clone(),
            },
            &issuer,
        ),
        &settings.consent(),
    );
    let _log_guard = dc_app::logging::init(&store.dirs().logs(), Some(diagnostics.layer()));
    dc_app::logging::install_panic_hook(store.dirs().logs(), Some(diagnostics.clone()));
    tracing::info!("DentalCraft {} starting", env!("CARGO_PKG_VERSION"));

    let updates = Updates::standard(store.dirs());
    // The refresh token goes to the OS keyring; a private file in the config folder
    // takes over where the keyring cannot be reached, so the session survives a restart.
    let tokens = dc_auth_client::FallbackTokenStore::keyring_or_file(store.dirs().auth_tokens())
        .on_primary_error(|e| tracing::warn!("OS keyring unavailable, using the token file: {e}"));
    let auth = match dc_auth_client::AuthClient::with_parts(
        &issuer,
        dc_auth_client::DEFAULT_CLIENT_ID,
        std::sync::Arc::new(tokens),
        std::sync::Arc::new(dc_auth_client::SystemBrowser),
    ) {
        Ok(a) => Some(a),
        Err(e) => {
            tracing::error!(
                code = "auth.invalid-issuer",
                source = "auth",
                "invalid sign-in server {issuer}: {e}"
            );
            None
        }
    };
    tracing::info!("sign-in server: {issuer}");
    if let Some(c) = &control {
        // scripts read the port from this line
        println!("control: listening on {}", c.local_addr());
        tracing::info!("control bridge listening on {}", c.local_addr());
    }

    let options = eframe::NativeOptions {
        renderer: eframe::Renderer::Wgpu,
        viewport: egui::ViewportBuilder::default()
            .with_title("DentalCraft")
            .with_app_id("com.dentalcraft.DentalCraft")
            .with_inner_size([1600.0, 960.0])
            .with_min_inner_size([1280.0, 800.0]),
        // vsync: `Fifo` waits for the display and is supported everywhere
        // (never tears, unlike `FifoRelaxed`, which `AutoVsync` may pick);
        // `dc_app::pacing` also holds every frame to 60 fps
        wgpu_options: egui_wgpu::WgpuConfiguration {
            surface: egui_wgpu::SurfaceConfig {
                present_mode: egui_wgpu::wgpu::PresentMode::Fifo,
                ..egui_wgpu::SurfaceConfig::LOW_LATENCY
            },
            ..Default::default()
        },
        ..Default::default()
    };
    let result = eframe::run_native(
        "DentalCraft",
        options,
        Box::new(move |cc| {
            let render_state = cc.wgpu_render_state.clone();
            if let Some(rs) = &render_state {
                let info = rs.adapter.get_info();
                tracing::info!(
                    "renderer: wgpu {:?}, {} ({})",
                    info.backend,
                    info.name,
                    info.driver
                );
            }
            let app = DentalCraftApp::new(AppDeps {
                store,
                auth,
                render_state,
                native_dialogs: true,
                updates: Some(updates),
                diagnostics: Some(diagnostics),
            });
            Ok(match control {
                Some(server) => Box::new(ControlledApp::new(app, server)),
                None => Box::new(app),
            })
        }),
    );
    match &result {
        Ok(()) => tracing::info!("DentalCraft ended: the window closed"),
        Err(e) => tracing::error!("DentalCraft ended with an error: {e}"),
    }
    result
}
