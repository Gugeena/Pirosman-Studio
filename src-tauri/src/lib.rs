mod commands;
mod hub;
mod preflight;
mod state;

use tauri::Manager;

/// Drop Windows' `\\?\` verbatim prefix (from canonicalized resource paths) so
/// paths shown to users and passed to tools look normal. UNC paths are kept.
fn plain_path(p: std::path::PathBuf) -> std::path::PathBuf {
    match p.to_str().and_then(|s| s.strip_prefix(r"\\?\")) {
        Some(rest) if !rest.starts_with(r"UNC\") => rest.into(),
        _ => p,
    }
}

fn percent_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' && i + 2 < b.len() {
            if let Some(v) = std::str::from_utf8(&b[i + 1..i + 3]).ok().and_then(|h| u8::from_str_radix(h, 16).ok()) {
                out.push(v);
                i += 3;
                continue;
            }
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Serve one image from the app cache. Only plain relative paths to .jpg/.png/.webp
/// files inside the cache folder are answered.
fn cache_protocol<R: tauri::Runtime>(app: &tauri::AppHandle<R>, uri_path: &str) -> tauri::http::Response<Vec<u8>> {
    use std::path::{Component, Path};
    let deny = |code: u16| tauri::http::Response::builder().status(code).body(Vec::new()).unwrap();
    let Some(st) = app.try_state::<state::AppState>() else { return deny(503) };
    let rel = percent_decode(uri_path.trim_start_matches('/'));
    let rel = Path::new(&rel);
    if rel.as_os_str().is_empty() || rel.components().any(|c| !matches!(c, Component::Normal(_))) {
        return deny(403);
    }
    let mime = match rel.extension().and_then(|e| e.to_str()).map(|e| e.to_ascii_lowercase()).as_deref() {
        Some("jpg") | Some("jpeg") => "image/jpeg",
        Some("png") => "image/png",
        Some("webp") => "image/webp",
        _ => return deny(403),
    };
    match std::fs::read(st.cache_dir.join(rel)) {
        Ok(bytes) => tauri::http::Response::builder().header("Content-Type", mime).header("Cache-Control", "no-cache").body(bytes).unwrap(),
        Err(_) => deny(404),
    }
}

pub fn run() {
    preflight::install_panic_hook();
    if !preflight::check() {
        std::process::exit(1);
    }
    let result = tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        // Pictures generated into the cache (media thumbnails, filmstrips, waveforms,
        // project cards) are served by our own scheme, pcache://localhost/<relative path>,
        // so they never depend on the asset-protocol scope.
        .register_uri_scheme_protocol("pcache", |ctx, req| cache_protocol(ctx.app_handle(), req.uri().path()))
        .setup(|app| {
            // FFmpeg ships next to the app: <install dir or portable folder>\ffmpeg.
            // Tauri's resource dir is the exe's folder on Windows for both builds.
            let res = app.path().resource_dir().ok().or_else(|| std::env::current_exe().ok().and_then(|p| p.parent().map(|d| d.to_path_buf())));
            if let Some(dir) = res {
                pirosman_core::media::set_bundled_dir(plain_path(dir.join("ffmpeg")));
                // The OpenFX host and the bundled sample plugins live in <app>\ofx.
                pirosman_core::ofx::set_app_dir(plain_path(dir));
            }
            let cache = app.path().app_cache_dir().unwrap_or_else(|_| std::env::temp_dir().join("pirosman-studio"));
            std::fs::create_dir_all(&cache)?;
            // Generated previews, thumbnails and waveforms may be shown in the UI.
            app.asset_protocol_scope().allow_directory(&cache, true)?;
            let config = app.path().app_config_dir().unwrap_or_else(|_| cache.join("config"));
            std::fs::create_dir_all(&config)?;
            app.manage(state::AppState::new(cache, config));
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::get_view,
            commands::new_project,
            commands::open_project,
            commands::save_project,
            commands::rename_project,
            commands::undo,
            commands::redo,
            commands::import_media,
            commands::relink_media,
            commands::remove_media,
            commands::place_clip,
            commands::move_clip,
            commands::trim_clip,
            commands::split_clip,
            commands::split_at,
            commands::delete_clips,
            commands::update_clip,
            commands::insert_edit,
            commands::overwrite_edit,
            commands::slip_clip,
            commands::slide_clip,
            commands::roll_edit,
            commands::add_title,
            commands::update_title,
            commands::add_marker,
            commands::update_marker,
            commands::delete_marker,
            commands::set_range,
            commands::export_otio,
            commands::video_scopes,
            commands::copy_items,
            commands::paste_items,
            commands::duplicate_items,
            commands::recent_projects,
            commands::import_timeline,
            commands::grab_still,
            commands::ofx_catalog,
            commands::ofx_preview_frame,
            commands::resolve_inspect,
            commands::collab_host_start,
            commands::collab_host_stop,
            commands::collab_spawn_peer,
            commands::collab_apply_remote,
            commands::startup_args,
            commands::add_track,
            commands::remove_track,
            commands::update_track,
            commands::reorder_track,
            commands::render_plan,
            commands::default_render_settings,
            commands::start_render,
            commands::cancel_render,
            commands::vcs_state,
            commands::vcs_commit,
            commands::vcs_create_branch,
            commands::vcs_switch_branch,
            commands::vcs_delete_branch,
            commands::vcs_restore,
            commands::vcs_diff,
            commands::vcs_merge_start,
            commands::vcs_merge_resolve,
            commands::vcs_merge_commit,
            commands::vcs_merge_abort,
            commands::bundle_export,
            commands::bundle_import,
            commands::diagnostics,
            commands::reveal_path,
            commands::ffmpeg_status,
            hub::hub_projects,
            hub::hub_duplicate,
            hub::hub_rename,
            hub::hub_trash,
            hub::hub_forget,
            hub::hub_import_archive,
            hub::storage_report,
            hub::storage_clear,
            hub::recent_revisions,
            commands::reveal_ffmpeg_folder,
        ])
        .run(tauri::generate_context!());
    if let Err(e) = result {
        preflight::fatal(&e);
        std::process::exit(1);
    }
}
