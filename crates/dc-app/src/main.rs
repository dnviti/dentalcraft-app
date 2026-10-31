//! `dentalcraft`: the DentalCraft desktop application.

use dc_app::config::{Dirs, Store};
use dc_app::{AppDeps, DentalCraftApp};

fn main() -> eframe::Result {
    let dirs = Dirs::standard();
    let _log_guard = dc_app::logging::init(&dirs.logs());
    dc_app::logging::install_panic_hook(dirs.logs());
    tracing::info!("DentalCraft {} starting", env!("CARGO_PKG_VERSION"));

    let store = Store::new(dirs);
    let issuer = store.load_settings().issuer();
    let auth = match dc_auth_client::AuthClient::new(&issuer, dc_auth_client::DEFAULT_CLIENT_ID) {
        Ok(a) => Some(a),
        Err(e) => {
            tracing::error!("invalid sign-in server {issuer}: {e}");
            None
        }
    };
    tracing::info!("sign-in server: {issuer}");

    let options = eframe::NativeOptions {
        renderer: eframe::Renderer::Wgpu,
        viewport: egui::ViewportBuilder::default()
            .with_title("DentalCraft")
            .with_app_id("com.dentalcraft.DentalCraft")
            .with_inner_size([1600.0, 960.0])
            .with_min_inner_size([1280.0, 800.0]),
        ..Default::default()
    };
    eframe::run_native(
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
            Ok(Box::new(DentalCraftApp::new(AppDeps {
                store,
                auth,
                render_state,
                native_dialogs: true,
            })))
        }),
    )
}
