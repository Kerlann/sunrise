//! What a pack reads with the device a renderer has (#6921): the Shader Options page, on the
//! title screen and in game, reads the pick with the renderer's macros where a renderer exists
//! (`shader_options_macros`), so its device's `IRIS_FEATURE_PER_BUFFER_BLENDING` answer reaches
//! the pack's `screen` entries; and a pack refused on one device is tried again once a renderer
//! with another device is installed (`ClientGame::set_renderer`, `native_shaders::follow_device`).
//! Original fixtures, written here.
use super::native_entity_refs::tests::{run_fresh_cases, test_window, with_client_game};
use super::native_shaders::{fingerprint, follow_device, refused, remember};
use super::shader_packs::{read_shader_options, shader_options_macros};
use sunrise_client_render::graphics::IrisCompat;
use sunrise_shaders::Macros;

/// A pack whose `sliders` screen entry only the per-buffer blending answer reaches.
fn guarded_pack(root: &std::path::Path) -> std::path::PathBuf {
    let pack = root.join("guarded");
    let shaders = pack.join("shaders");
    std::fs::create_dir_all(&shaders).unwrap();
    std::fs::write(
        shaders.join("shaders.properties"),
        "iris.features.optional = PER_BUFFER_BLENDING\n\
         #ifdef IRIS_FEATURE_PER_BUFFER_BLENDING\nsliders = LEVEL\n#endif\n",
    )
    .unwrap();
    std::fs::write(shaders.join("options.glsl"), "#define LEVEL 2 // [1 2]\n").unwrap();
    pack
}

/// The sliders the Shader Options page shows for the pack at `path`, read with `macros`.
fn sliders(path: &std::path::Path, macros: &Macros) -> Vec<String> {
    let options = read_shader_options(&path.to_string_lossy(), "en_us", macros, IrisCompat::Auto)
        .expect("the pack reads");
    options.screens.sliders.iter().cloned().collect()
}

/// Without a renderer the page reads with the standard macros alone, as before: no device
/// answer, so what only `IRIS_FEATURE_PER_BUFFER_BLENDING` reaches is not shown. The page shows
/// it from a macro set whose device draws per-buffer blends.
#[test]
fn the_options_page_reads_the_device_answer_it_is_given() {
    let root = tempfile::tempdir().unwrap();
    let pack = guarded_pack(root.path());
    assert_eq!(shader_options_macros(None), Macros::standard());
    assert!(sliders(&pack, &shader_options_macros(None)).is_empty());
    let mut able = Macros::standard();
    able.set_per_buffer_blending(true);
    assert_eq!(sliders(&pack, &able), ["LEVEL"]);
}

/// A real renderer's device: the page's macros (`shader_options_macros`) show the guarded
/// slider exactly when the device answers the flag, as the load reads it; and a refusal kept
/// from another device answer is tried again once this renderer is installed.
#[test]
#[ignore = "daily"]
fn the_renderers_device_reaches_the_options_page_and_the_refusals() {
    if run_fresh_cases(
        "game::client_side::shader_options_device_tests::the_renderers_device_reaches_the_options_page_and_the_refusals",
        "SUNRISE_SHADER_OPTIONS_DEVICE_CASE",
        &["device"],
    ) {
        return;
    }
    let _gpu = sunrise_client_render::gfx::lock_test_gpu();
    with_client_game(|game| {
        let root = tempfile::tempdir().unwrap();
        let pack = guarded_pack(root.path());
        game.common.options.offscreen = true;
        let (_event_loop, window) = test_window([320, 240]);
        let renderer = game.new_renderer(window).unwrap();
        // The device's answer, read as the load reads it.
        let mut load = Macros::standard();
        renderer.define_pack_macros(&mut load);
        let (_, notes) = sunrise_shaders::read_properties(
            &sunrise_shaders::Pack::open(&pack).unwrap(),
            &mut load,
        );
        let able = load.is_defined("IRIS_FEATURE_PER_BUFFER_BLENDING");
        eprintln!("SHADER_OPTIONS_DEVICE per-buffer blending {able}, notes {notes:?}");
        // A refusal given on a device with the other answer.
        let mut other = Macros::default();
        other.set_per_buffer_blending(!able);
        follow_device(&other);
        let stamp = fingerprint(&pack);
        remember(&pack, stamp, IrisCompat::Auto, false);
        assert!(refused(&pack, stamp, IrisCompat::Auto));
        game.set_renderer(renderer);
        assert!(!refused(&pack, stamp, IrisCompat::Auto));
        // The page reads with the installed renderer's macros.
        let macros = shader_options_macros(game.client.renderer.0.as_deref());
        let shown: &[&str] = if able { &["LEVEL"] } else { &[] };
        assert_eq!(sliders(&pack, &macros), shown, "per-buffer blending {able}");
    });
}
