//! End-to-end checks against real FFmpeg/FFprobe binaries.
//! When the tools are missing these tests print `SKIPPED` and do not claim success
//! for the media path (the assertion count reported by cargo includes them, so
//! check the output for SKIPPED lines).

use std::collections::HashMap;
use std::path::Path;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;

use pirosman_core::bundles::{export_bundle, import_bundle};
use pirosman_core::diff::diff_entries;
use pirosman_core::media::{command, discover, import_media, make_proxy, make_thumbnail, set_bundled_dir, ToolSource, Tools};
use pirosman_core::merge::merge;
use pirosman_core::model::{ColorParams, Project, ProjectSettings};
use pirosman_core::project::{load_project, save_project};
use pirosman_core::render::{build_plan, execute, verify_output, AudioCodec, RenderSettings, VideoCodec};
use pirosman_core::revisions::RevisionStore;
use pirosman_core::time::Rational;
use pirosman_core::timeline::*;

/// `PIROSMAN_TEST_BUNDLED_DIR=<dir>` runs every test against that bundled FFmpeg
/// the way the app does (`set_bundled_dir`); adding `PIROSMAN_TEST_REQUIRE_BUNDLED=1`
/// makes a missing or rejected bundle a failure instead of a skip or a fallback.
fn tools_or_skip(test: &str) -> Option<Tools> {
    if let Some(d) = std::env::var_os("PIROSMAN_TEST_BUNDLED_DIR") {
        set_bundled_dir(d.into());
    }
    let require = std::env::var_os("PIROSMAN_TEST_REQUIRE_BUNDLED").is_some();
    match discover() {
        Ok(d) => {
            if require {
                assert_eq!(d.source, ToolSource::Bundled, "{test}: expected the bundled FFmpeg, got {:?} ({:?})", d.source, d.warning);
            }
            eprintln!("{test}: using {:?} FFmpeg {}", d.source, d.tools.ffmpeg.display());
            Some(d.tools)
        }
        Err(e) if require => panic!("{test}: bundled FFmpeg required: {e}"),
        Err(e) => {
            eprintln!("SKIPPED {test}: {e}");
            None
        }
    }
}

fn make_clip(t: &Tools, path: &Path, seconds: u32) {
    let st = command(&t.ffmpeg)
        .args(["-v", "error", "-y", "-f", "lavfi", "-i"])
        .arg(format!("testsrc2=size=640x360:rate=25:duration={seconds}"))
        .args(["-f", "lavfi", "-i"])
        .arg(format!("sine=frequency=440:sample_rate=48000:duration={seconds}"))
        .args(["-c:v", "libx264", "-pix_fmt", "yuv420p", "-c:a", "aac", "-shortest"])
        .arg(path)
        .status()
        .unwrap();
    assert!(st.success(), "could not synthesize test media");
}

fn count_frames(t: &Tools, path: &Path) -> u64 {
    let out = command(&t.ffprobe)
        .args(["-v", "error", "-select_streams", "v:0", "-count_frames", "-show_entries", "stream=nb_read_frames", "-of", "csv=p=0"])
        .arg(path)
        .output()
        .unwrap();
    String::from_utf8_lossy(&out.stdout).trim().parse().unwrap()
}

#[test]
fn import_edit_save_render_verify() {
    let Some(t) = tools_or_skip("import_edit_save_render_verify") else { return };
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("bars.mp4");
    make_clip(&t, &src, 4);

    let settings = ProjectSettings { width: 640, height: 360, frame_rate: Rational::new(25, 1), ..Default::default() };
    let mut p = Project::new("Integration", settings);
    let m = import_media(&t, &src, p.settings.frame_rate).unwrap();
    assert_eq!(m.duration_frames, Some(100));
    assert_eq!(m.sha256.as_ref().unwrap().len(), 64);
    let mid = m.id;
    p.media.push(m);

    let thumb = dir.path().join("thumb.jpg");
    make_thumbnail(&t, &src, 0.5, &thumb).unwrap();
    assert!(thumb.exists());

    let v1 = p.tracks[0].id;
    let v2 = p.tracks[1].id;
    let ids = place_clip(&mut p, PlaceClip { media_id: mid, track_id: v1, start: 0, source_in: Some(10), source_out: Some(60) }).unwrap();
    let right = split_clip(&mut p, ids[0], 20).unwrap();
    move_clip(&mut p, right[0], v2, 40).unwrap(); // gap 20..40 on V1/A1, clip on V2
    let mut col = ColorParams::default();
    col.saturation = 0.0;
    col.contrast = 1.2;
    update_clip(&mut p, right[0], ClipPatch { color: Some(col), ..Default::default() }).unwrap();
    assert_eq!(p.duration(), 70);

    let proj_path = dir.path().join("cut.pirosman");
    save_project(&p, &proj_path).unwrap();
    let reopened = load_project(&proj_path).unwrap();
    assert_eq!(reopened, p);

    let out = dir.path().join("render.mp4");
    let s = RenderSettings::from_project(&reopened, out.to_str().unwrap());
    let plan = build_plan(&reopened, &s, true).unwrap();
    let mut updates = 0;
    let res = execute(&t, &plan, false, Arc::new(AtomicBool::new(false)), |_| updates += 1).unwrap();
    assert!(updates > 0, "progress events were emitted");
    assert_eq!(res.video_codec.as_deref(), Some("h264"));
    assert_eq!(res.audio_codec.as_deref(), Some("aac"));
    assert_eq!((res.width, res.height), (Some(640), Some(360)));
    assert!((res.duration_seconds - 2.8).abs() < 0.08, "duration {}", res.duration_seconds);
    assert_eq!(count_frames(&t, &out), 70, "frame-exact output length");
    assert!(!Path::new(&plan.partial_output).exists());

    // Refuses to overwrite without consent.
    assert!(execute(&t, &plan, false, Arc::new(AtomicBool::new(false)), |_| {}).is_err());
}

fn probe_stream_count(t: &Tools, path: &Path, kind: &str) -> usize {
    let out = command(&t.ffprobe)
        .args(["-v", "error", "-show_entries", "stream=codec_type", "-of", "csv=p=0"])
        .arg(path)
        .output()
        .unwrap();
    String::from_utf8_lossy(&out.stdout).lines().filter(|l| l.trim() == kind).count()
}

/// Every v2 feature through one real FFmpeg run: fades, a cross dissolve,
/// a dip to black, 2x reverse, 0.5x slow motion, a LUT, a title with
/// awkward characters, mixer pan/EQ/solo-free mix and an in/out range.
#[test]
fn v2_features_render_frame_exact() {
    use pirosman_core::model::{TitlePosition, Transition, TransitionKind};
    let Some(t) = tools_or_skip("v2_features_render_frame_exact") else { return };
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("bars.mp4");
    make_clip(&t, &src, 8);
    // A small valid .cube LUT (2x2x2 identity with a warm push).
    let lut = dir.path().join("warm look.cube");
    std::fs::write(&lut, "LUT_3D_SIZE 2\n0 0 0\n1.05 0 0\n0 0.95 0\n1 1 0\n0 0 0.9\n1 0 0.9\n0 1 0.9\n1 1 1\n").unwrap();
    let settings = ProjectSettings { width: 640, height: 360, frame_rate: Rational::new(25, 1), ..Default::default() };
    let mut p = Project::new("V2", settings);
    let m = import_media(&t, &src, p.settings.frame_rate).unwrap();
    let mid = m.id;
    p.media.push(m);
    let (v1, v2, a1) = (p.tracks[0].id, p.tracks[1].id, p.tracks[2].id);
    let a = place_clip(&mut p, PlaceClip { media_id: mid, track_id: v1, start: 0, source_in: Some(10), source_out: Some(60) }).unwrap()[0];
    let b = place_clip(&mut p, PlaceClip { media_id: mid, track_id: v1, start: 50, source_in: Some(80), source_out: Some(130) }).unwrap()[0];
    let c = place_clip(&mut p, PlaceClip { media_id: mid, track_id: v1, start: 100, source_in: Some(130), source_out: Some(170) }).unwrap()[0];
    update_clip(&mut p, a, ClipPatch { fade_in_frames: Some(8), lut: Some(Some(lut.to_string_lossy().into())), ..Default::default() }).unwrap();
    update_clip(&mut p, b, ClipPatch { transition_in: Some(Some(Transition { kind: TransitionKind::CrossDissolve, duration_frames: 10 })), ..Default::default() }).unwrap();
    update_clip(&mut p, c, ClipPatch { speed: Some(2.0), reversed: Some(true), ..Default::default() }).unwrap(); // 40 src → 20 frames: 100..120
    let d = place_clip(&mut p, PlaceClip { media_id: mid, track_id: v1, start: 120, source_in: Some(0), source_out: Some(20) }).unwrap()[0];
    update_clip(&mut p, d, ClipPatch { speed: Some(0.5), transition_in: Some(Some(Transition { kind: TransitionKind::DipToBlack, duration_frames: 6 })), ..Default::default() }).unwrap(); // 20 src → 40 frames: 120..160
    add_title(&mut p, NewTitle { track_id: v2, start: 20, duration: 50, text: "Harbour: 'night' 100% \\ test".into(), font_size: Some(40), color_hex: Some("#F4C430".into()), position: Some(TitlePosition::LowerThird) }).unwrap();
    update_track(&mut p, a1, TrackPatch { pan: Some(-0.4), eq_low_db: Some(3.0), eq_mid_db: Some(-2.0), eq_high_db: Some(1.5), volume_db: Some(-2.0), ..Default::default() }).unwrap();
    assert_eq!(p.duration(), 160);

    let out = dir.path().join("v2.mp4");
    let plan = build_plan(&p, &RenderSettings::from_project(&p, out.to_str().unwrap()), true).unwrap();
    execute(&t, &plan, false, Arc::new(AtomicBool::new(false)), |_| {}).unwrap();
    assert_eq!(count_frames(&t, &out), 160, "frame-exact with retimed, reversed and dissolved clips");
    assert!(plan.aux_files.iter().all(|a| !Path::new(&a.path).exists()), "title text files cleaned up");

    // In/out range: only frames 40..90.
    let mut s = RenderSettings::from_project(&p, dir.path().join("range.mp4").to_str().unwrap());
    s.range_in = Some(40);
    s.range_out = Some(90);
    let plan = build_plan(&p, &s, true).unwrap();
    let res = execute(&t, &plan, false, Arc::new(AtomicBool::new(false)), |_| {}).unwrap();
    assert_eq!(count_frames(&t, Path::new(&res.output)), 50);

    // HEVC, DNxHR and audio-only deliverables.
    for (name, vc, ac, expect_video) in [
        ("hevc.mp4", VideoCodec::Hevc, AudioCodec::Aac, true),
        ("dnx.mov", VideoCodec::Dnxhr, AudioCodec::Pcm, true),
        ("mix.wav", VideoCodec::None, AudioCodec::Pcm, false),
        ("mix.m4a", VideoCodec::None, AudioCodec::Aac, false),
    ] {
        let mut s = RenderSettings::from_project(&p, dir.path().join(name).to_str().unwrap());
        s.video_codec = vc;
        s.audio_codec = ac;
        s.range_out = Some(30);
        let plan = build_plan(&p, &s, true).unwrap();
        let r = execute(&t, &plan, false, Arc::new(AtomicBool::new(false)), |_| {}).unwrap();
        assert_eq!(probe_stream_count(&t, Path::new(&r.output), "video") > 0, expect_video, "{name}");
        assert_eq!(probe_stream_count(&t, Path::new(&r.output), "audio"), 1, "{name}");
        assert!((r.duration_seconds - 1.2).abs() < 0.08, "{name}: {}", r.duration_seconds);
    }
    let r = verify_output(&t, &dir.path().join("hevc.mp4"), true).unwrap();
    assert_eq!(r.video_codec.as_deref(), Some("hevc"));
}

/// Photo on the timeline with transform + crop, the full VFX stack, and a
/// Grab Still PNG through the export pipeline.
#[test]
fn v3_photo_transform_vfx_and_still() {
    use pirosman_core::model::{Crop, Transform, Vfx, VfxKind};
    let Some(t) = tools_or_skip("v3_photo_transform_vfx_and_still") else { return };
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("bars.mp4");
    make_clip(&t, &src, 3);
    let photo = dir.path().join("photo.png");
    assert!(command(&t.ffmpeg).args(["-v", "error", "-y", "-f", "lavfi", "-i", "smptehdbars=size=800x600", "-frames:v", "1"]).arg(&photo).status().unwrap().success());
    let settings = ProjectSettings { width: 640, height: 360, frame_rate: Rational::new(25, 1), ..Default::default() };
    let mut p = Project::new("V3", settings);
    let m = import_media(&t, &src, p.settings.frame_rate).unwrap();
    let ph = import_media(&t, &photo, p.settings.frame_rate).unwrap();
    let (mid, pid) = (m.id, ph.id);
    p.media.extend([m, ph]);
    let (v1, v2) = (p.tracks[0].id, p.tracks[1].id);
    let a = place_clip(&mut p, PlaceClip { media_id: mid, track_id: v1, start: 0, source_in: Some(0), source_out: Some(60) }).unwrap()[0];
    let s = place_clip(&mut p, PlaceClip { media_id: pid, track_id: v2, start: 10, source_in: Some(0), source_out: Some(40) }).unwrap()[0];
    let fx = [VfxKind::GaussianBlur, VfxKind::Glow, VfxKind::Vignette, VfxKind::FilmGrain, VfxKind::ChromaticAberration];
    update_clip(&mut p, a, ClipPatch { vfx: Some(fx.iter().map(|k| Vfx { id: uuid::Uuid::new_v4(), kind: *k, amount: 0.6, enabled: true }).collect()), ..Default::default() }).unwrap();
    update_clip(
        &mut p,
        s,
        ClipPatch {
            transform: Some(Transform { scale_x: 0.4, scale_y: 0.4, position_x: 0.25, position_y: -0.2, rotation_deg: 8.0 }),
            crop: Some(Crop { left: 0.1, right: 0.1, top: 0.0, bottom: 0.2 }),
            fade_in_frames: Some(5),
            ..Default::default()
        },
    )
    .unwrap();
    let out = dir.path().join("v3.mp4");
    let plan = build_plan(&p, &RenderSettings::from_project(&p, out.to_str().unwrap()), true).unwrap();
    execute(&t, &plan, false, Arc::new(AtomicBool::new(false)), |_| {}).unwrap();
    assert_eq!(count_frames(&t, &out), 60);
    let still = dir.path().join("still.png");
    let mut ss = RenderSettings::from_project(&p, still.to_str().unwrap());
    ss.video_codec = VideoCodec::Png;
    ss.audio_codec = AudioCodec::None;
    ss.range_in = Some(30);
    ss.range_out = Some(31);
    let r = execute(&t, &build_plan(&p, &ss, true).unwrap(), false, Arc::new(AtomicBool::new(false)), |_| {}).unwrap();
    assert!(r.size > 1000, "a real PNG was written");
    let probe = pirosman_core::media::ffprobe_json(&t, &still).unwrap();
    assert_eq!(probe["streams"][0]["width"], 640);
}

/// Layers: a transparent PNG shows the track below through its clear half,
/// opacity mixes layers, and a free-placed title box lands where placed.
#[test]
fn v4_transparent_overlay_opacity_and_styled_title() {
    use pirosman_core::model::{TextStyle, Transform};
    let Some(t) = tools_or_skip("v4_transparent_overlay_opacity_and_styled_title") else { return };
    let dir = tempfile::tempdir().unwrap();
    let ff = |args: &[&str], out: &Path| assert!(command(&t.ffmpeg).args(["-v", "error", "-y"]).args(args).arg(out).status().unwrap().success());
    let red = dir.path().join("red.mp4");
    ff(&["-f", "lavfi", "-i", "color=c=red:s=640x360:r=25:d=2", "-c:v", "libx264", "-pix_fmt", "yuv420p"], &red);
    let png = dir.path().join("half.png");
    ff(&["-f", "lavfi", "-i", "color=c=black@0.0:s=320x360,format=rgba", "-f", "lavfi", "-i", "color=c=blue:s=320x360,format=rgba", "-filter_complex", "[0][1]hstack,format=rgba", "-frames:v", "1"], &png);
    let settings = ProjectSettings { width: 640, height: 360, frame_rate: Rational::new(25, 1), ..Default::default() };
    let mut p = Project::new("Layers", settings);
    let (m, o) = (import_media(&t, &red, p.settings.frame_rate).unwrap(), import_media(&t, &png, p.settings.frame_rate).unwrap());
    let (mid, oid) = (m.id, o.id);
    p.media.extend([m, o]);
    let (v1, v2) = (p.tracks[0].id, p.tracks[1].id);
    place_clip(&mut p, PlaceClip { media_id: mid, track_id: v1, start: 0, source_in: Some(0), source_out: Some(50) }).unwrap();
    let ov = place_clip(&mut p, PlaceClip { media_id: oid, track_id: v2, start: 0, source_in: Some(0), source_out: Some(50) }).unwrap()[0];
    let pixel = |p: &Project, x: u32, y: u32| -> [u8; 3] {
        let still = dir.path().join("frame.png");
        let _ = std::fs::remove_file(&still);
        let mut s = RenderSettings::from_project(p, still.to_str().unwrap());
        (s.video_codec, s.audio_codec, s.range_in, s.range_out) = (VideoCodec::Png, AudioCodec::None, Some(10), Some(11));
        execute(&t, &build_plan(p, &s, true).unwrap(), false, Arc::new(AtomicBool::new(false)), |_| {}).unwrap();
        let out = command(&t.ffmpeg).args(["-v", "error", "-i"]).arg(&still).args(["-vf", &format!("crop=1:1:{x}:{y},format=rgb24"), "-f", "rawvideo", "-"]).output().unwrap();
        [out.stdout[0], out.stdout[1], out.stdout[2]]
    };
    let near = |c: [u8; 3], want: [i32; 3]| c.iter().zip(want).all(|(&a, b)| (a as i32 - b).abs() <= 40);
    let (clear, solid) = (pixel(&p, 80, 180), pixel(&p, 560, 180));
    assert!(near(clear, [255, 0, 0]), "the transparent half shows the red track below: {clear:?}");
    assert!(near(solid, [0, 0, 255]), "the opaque half is blue: {solid:?}");
    update_clip(&mut p, ov, ClipPatch { opacity: Some(0.5), ..Default::default() }).unwrap();
    let mixed = pixel(&p, 560, 180);
    assert!(near(mixed, [128, 0, 128]), "50% blue over red: {mixed:?}");
    // V2 is busy, so the title gets a new V3 above it.
    let x = add_title(&mut p, NewTitle { track_id: v2, start: 0, duration: 50, text: "MMMM".into(), font_size: Some(60), color_hex: Some("#00ff00".into()), position: None }).unwrap();
    assert_eq!(p.tracks.iter().filter(|t| t.titles.iter().any(|y| y.id == x)).map(|t| t.name.as_str()).collect::<Vec<_>>(), ["V3"]);
    let style = TextStyle { background: Some("#00ff00ff".into()), shadow: false, ..Default::default() };
    update_title(&mut p, x, TitlePatch { style: Some(style), transform: Some(Some(Transform { position_x: -0.25, ..Default::default() })), ..Default::default() }).unwrap();
    let boxed = pixel(&p, 160, 180);
    assert!(near(boxed, [0, 255, 0]), "the green title box sits a quarter frame left of centre: {boxed:?}");
}

#[test]
fn v5_wipe_slide_push_and_dip_to_white() {
    use pirosman_core::model::{Transition, TransitionKind};
    let Some(t) = tools_or_skip("v5_wipe_slide_push_and_dip_to_white") else { return };
    let dir = tempfile::tempdir().unwrap();
    let ff = |args: &[&str], out: &Path| assert!(command(&t.ffmpeg).args(["-v", "error", "-y"]).args(args).arg(out).status().unwrap().success());
    // Outgoing: red left half, green right half, so a push (which moves it) differs from a slide (which does not).
    let (rg, blue) = (dir.path().join("rg.mp4"), dir.path().join("blue.mp4"));
    ff(&["-f", "lavfi", "-i", "color=c=red:s=320x360:r=25:d=4", "-f", "lavfi", "-i", "color=c=lime:s=320x360:r=25:d=4", "-filter_complex", "[0][1]hstack", "-c:v", "libx264", "-pix_fmt", "yuv420p"], &rg);
    ff(&["-f", "lavfi", "-i", "color=c=blue:s=640x360:r=25:d=4", "-c:v", "libx264", "-pix_fmt", "yuv420p"], &blue);
    let settings = ProjectSettings { width: 640, height: 360, frame_rate: Rational::new(25, 1), ..Default::default() };
    let mut p = Project::new("Transitions", settings);
    let (a, b) = (import_media(&t, &rg, p.settings.frame_rate).unwrap(), import_media(&t, &blue, p.settings.frame_rate).unwrap());
    let (aid, bid) = (a.id, b.id);
    p.media.extend([a, b]);
    let v1 = p.tracks[0].id;
    place_clip(&mut p, PlaceClip { media_id: aid, track_id: v1, start: 0, source_in: Some(0), source_out: Some(50) }).unwrap();
    let inc = place_clip(&mut p, PlaceClip { media_id: bid, track_id: v1, start: 50, source_in: Some(25), source_out: Some(75) }).unwrap()[0];
    let pixel = |p: &Project, f: i64, x: u32| -> [u8; 3] {
        let still = dir.path().join("frame.png");
        let _ = std::fs::remove_file(&still);
        let mut s = RenderSettings::from_project(p, still.to_str().unwrap());
        (s.video_codec, s.audio_codec, s.range_in, s.range_out) = (VideoCodec::Png, AudioCodec::None, Some(f), Some(f + 1));
        execute(&t, &build_plan(p, &s, true).unwrap(), false, Arc::new(AtomicBool::new(false)), |_| {}).unwrap();
        let out = command(&t.ffmpeg).args(["-v", "error", "-i"]).arg(&still).args(["-vf", &format!("crop=1:1:{x}:180,format=rgb24"), "-f", "rawvideo", "-"]).output().unwrap();
        [out.stdout[0], out.stdout[1], out.stdout[2]]
    };
    let near = |c: [u8; 3], want: [i32; 3]| c.iter().zip(want).all(|(&a, b)| (a as i32 - b).abs() <= 40);
    let set = |p: &mut Project, kind| update_clip(p, inc, ClipPatch { transition_in: Some(Some(Transition { kind, duration_frames: 10 })), ..Default::default() }).unwrap();
    // A 10-frame transition spans frames 45..55; at frame 50 it is a little past halfway (6/11).
    set(&mut p, TransitionKind::Wipe);
    let (l, r) = (pixel(&p, 50, 100), pixel(&p, 50, 560));
    assert!(near(l, [0, 0, 255]) && near(r, [0, 255, 0]), "wipe: blue has covered the left half only: {l:?} {r:?}");
    set(&mut p, TransitionKind::Slide);
    let (l, r) = (pixel(&p, 50, 100), pixel(&p, 50, 560));
    assert!(near(l, [255, 0, 0]) && near(r, [0, 0, 255]), "slide: the outgoing clip stays put under the incoming one: {l:?} {r:?}");
    set(&mut p, TransitionKind::Push);
    let (l, r) = (pixel(&p, 50, 100), pixel(&p, 50, 560));
    assert!(near(l, [0, 255, 0]) && near(r, [0, 0, 255]), "push: the outgoing clip has moved half a frame left: {l:?} {r:?}");
    set(&mut p, TransitionKind::DipToWhite);
    let w = pixel(&p, 49, 320);
    assert!(w.iter().all(|&c| c > 200), "dip to white is white at the cut: {w:?}");
}

#[test]
fn v6_keyframed_position_rotation_and_opacity() {
    use pirosman_core::model::{Keyframe, Transform};
    let Some(t) = tools_or_skip("v6_keyframed_position_rotation_and_opacity") else { return };
    let dir = tempfile::tempdir().unwrap();
    let ff = |args: &[&str], out: &Path| assert!(command(&t.ffmpeg).args(["-v", "error", "-y"]).args(args).arg(out).status().unwrap().success());
    let (red, sq) = (dir.path().join("red.mp4"), dir.path().join("square.png"));
    ff(&["-f", "lavfi", "-i", "color=c=red:s=640x360:r=25:d=2", "-c:v", "libx264", "-pix_fmt", "yuv420p"], &red);
    ff(&["-f", "lavfi", "-i", "color=c=blue:s=100x100", "-frames:v", "1"], &sq);
    let settings = ProjectSettings { width: 640, height: 360, frame_rate: Rational::new(25, 1), ..Default::default() };
    let mut p = Project::new("Keys", settings);
    let (a, b) = (import_media(&t, &red, p.settings.frame_rate).unwrap(), import_media(&t, &sq, p.settings.frame_rate).unwrap());
    let (aid, bid) = (a.id, b.id);
    p.media.extend([a, b]);
    let (v1, v2) = (p.tracks[0].id, p.tracks[1].id);
    place_clip(&mut p, PlaceClip { media_id: aid, track_id: v1, start: 0, source_in: Some(0), source_out: Some(50) }).unwrap();
    let ov = place_clip(&mut p, PlaceClip { media_id: bid, track_id: v2, start: 0, source_in: Some(0), source_out: Some(50) }).unwrap()[0];
    let pixel = |p: &Project, f: i64, x: u32, y: u32| -> [u8; 3] {
        let still = dir.path().join("frame.png");
        let _ = std::fs::remove_file(&still);
        let mut s = RenderSettings::from_project(p, still.to_str().unwrap());
        (s.video_codec, s.audio_codec, s.range_in, s.range_out) = (VideoCodec::Png, AudioCodec::None, Some(f), Some(f + 1));
        execute(&t, &build_plan(p, &s, true).unwrap(), false, Arc::new(AtomicBool::new(false)), |_| {}).unwrap();
        let out = command(&t.ffmpeg).args(["-v", "error", "-i"]).arg(&still).args(["-vf", &format!("crop=1:1:{x}:{y},format=rgb24"), "-f", "rawvideo", "-"]).output().unwrap();
        [out.stdout[0], out.stdout[1], out.stdout[2]]
    };
    let near = |c: [u8; 3], want: [i32; 3], tol: i32| c.iter().zip(want).all(|(&a, b)| (a as i32 - b).abs() <= tol);
    // The square fits to 360 px; at scale 0.25 it is 90 px. It travels from x=160 to x=480,
    // fading to 50% and turning 45° by frame 40, then holds.
    let small = |px: f64, rot: f64| Transform { scale_x: 0.25, scale_y: 0.25, position_x: px, rotation_deg: rot, ..Default::default() };
    let keys = vec![Keyframe { frame: 0, transform: small(-0.25, 0.0), opacity: 1.0 }, Keyframe { frame: 40, transform: small(0.25, 45.0), opacity: 0.5 }];
    update_clip(&mut p, ov, ClipPatch { keyframes: Some(keys), ..Default::default() }).unwrap();
    let (a, b) = (pixel(&p, 0, 160, 180), pixel(&p, 0, 480, 180));
    assert!(near(a, [0, 0, 255], 40) && near(b, [255, 0, 0], 40), "frame 0: square on the left: {a:?} {b:?}");
    let (m, l) = (pixel(&p, 20, 320, 180), pixel(&p, 20, 160, 180));
    assert!(near(m, [64, 0, 191], 40) && near(l, [255, 0, 0], 40), "frame 20: centred at 75%: {m:?} {l:?}");
    let (c, corner, tip) = (pixel(&p, 45, 480, 180), pixel(&p, 45, 520, 140), pixel(&p, 45, 480, 125));
    assert!(near(c, [128, 0, 128], 40), "held at 50% on the right: {c:?}");
    assert!(near(corner, [255, 0, 0], 40) && near(tip, [128, 0, 128], 40), "turned 45°: the square's corner is gone, the diamond's tip is there: {corner:?} {tip:?}");
    // Frame-exact: opacity 0 at frame 0 and 1 at frame 2 is half way at frame 1.
    let keys = vec![Keyframe { frame: 0, transform: small(0.0, 0.0), opacity: 0.0 }, Keyframe { frame: 2, transform: small(0.0, 0.0), opacity: 1.0 }];
    update_clip(&mut p, ov, ClipPatch { keyframes: Some(keys), ..Default::default() }).unwrap();
    let h = pixel(&p, 1, 320, 180);
    assert!(near(h, [128, 0, 128], 20), "frame 1 is exactly between the keys: {h:?}");
}

#[test]
fn image_thumbnails_keep_shape_and_alpha() {
    let Some(t) = tools_or_skip("image_thumbnails_keep_shape_and_alpha") else { return };
    let dir = tempfile::tempdir().unwrap();
    let ff = |args: &[&str], out: &Path| assert!(command(&t.ffmpeg).args(["-v", "error", "-y"]).args(args).arg(out).status().unwrap().success());
    let probe = |p: &Path| -> String {
        let o = command(&t.ffprobe).args(["-v", "error", "-show_entries", "stream=width,height,pix_fmt", "-of", "csv=p=0"]).arg(p).output().unwrap();
        String::from_utf8_lossy(&o.stdout).trim().to_string()
    };
    // (file, source size, thumbnail size): 320 wide, height from the aspect ratio.
    let cases = [
        ("land.png", "800x600", "320,240"),
        ("portrait photo.jpg", "600x900", "320,480"),
        ("square.webp", "500x500", "320,320"),
        ("it's a ünïcode [copy].jpeg", "1000x250", "320,80"),
    ];
    for (name, size, want) in cases {
        let src = dir.path().join(name);
        ff(&["-f", "lavfi", "-i", &format!("testsrc2=s={size}"), "-frames:v", "1"], &src);
        let m = import_media(&t, &src, Rational::new(25, 1)).unwrap();
        assert_eq!(m.kind, pirosman_core::model::MediaKind::Image, "{name}");
        let thumb = dir.path().join(format!("{name}.thumb.png"));
        make_thumbnail(&t, &src, 0.0, &thumb).unwrap_or_else(|e| panic!("{name}: {e}"));
        assert!(probe(&thumb).starts_with(want), "{name}: {}", probe(&thumb));
    }
    // Transparency survives into a PNG thumbnail.
    let clear = dir.path().join("clear.png");
    ff(&["-f", "lavfi", "-i", "color=c=blue@0.5:s=400x300,format=rgba", "-frames:v", "1"], &clear);
    let thumb = dir.path().join("clear.thumb.png");
    make_thumbnail(&t, &clear, 0.0, &thumb).unwrap();
    assert!(probe(&thumb).contains("rgba"), "{}", probe(&thumb));
    // A file that is not a picture fails with an error instead of an empty thumbnail.
    let bad = dir.path().join("broken.png");
    std::fs::write(&bad, b"not an image").unwrap();
    assert!(make_thumbnail(&t, &bad, 0.0, &dir.path().join("broken.thumb.png")).is_err());
    assert!(import_media(&t, &bad, Rational::new(25, 1)).is_err());
}

/// MEDIA-02: a missing file is detected; relinking to different content is
/// refused; relinking to the moved original (same SHA-256) succeeds.
#[test]
fn hash_checked_relink() {
    use pirosman_core::media::{media_status, relink, MediaStatus};
    let Some(t) = tools_or_skip("hash_checked_relink") else { return };
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("orig.mp4");
    make_clip(&t, &src, 2);
    let other = dir.path().join("other.mp4");
    make_clip(&t, &other, 3);
    let mut p = Project::new("Relink", ProjectSettings::default());
    let m = import_media(&t, &src, p.settings.frame_rate).unwrap();
    let mid = m.id;
    p.media.push(m);
    let moved_dir = dir.path().join("moved");
    std::fs::create_dir_all(&moved_dir).unwrap();
    let moved = moved_dir.join("renamed.mp4");
    std::fs::rename(&src, &moved).unwrap();
    assert_eq!(media_status(&p.media[0]), MediaStatus::Missing);
    let before = p.clone();
    let err = relink(&mut p, mid, &other, &t, false).unwrap_err().to_string();
    assert!(err.contains("different content"), "{err}");
    assert_eq!(p, before, "a refused relink changes nothing");
    relink(&mut p, mid, &moved, &t, false).unwrap();
    assert_eq!(media_status(&p.media[0]), MediaStatus::Online);
    assert_eq!(p.media[0].path, moved.to_string_lossy());
    assert_eq!(p.media[0].id, mid, "identity survives relink");
    // Explicit consent allows different content.
    relink(&mut p, mid, &other, &t, true).unwrap();
    assert_eq!(p.media[0].duration_frames, Some(75));
}

#[test]
fn cancelled_render_leaves_no_file() {
    let Some(t) = tools_or_skip("cancelled_render_leaves_no_file") else { return };
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("long.mp4");
    make_clip(&t, &src, 3);
    let mut p = Project::new("Cancel", ProjectSettings::default());
    let m = import_media(&t, &src, p.settings.frame_rate).unwrap();
    let mid = m.id;
    p.media.push(m);
    let v1 = p.tracks[0].id;
    place_clip(&mut p, PlaceClip { media_id: mid, track_id: v1, start: 0, source_in: None, source_out: None }).unwrap();
    let out = dir.path().join("x.mp4");
    let plan = build_plan(&p, &RenderSettings::from_project(&p, out.to_str().unwrap()), true).unwrap();
    let cancel = Arc::new(AtomicBool::new(true));
    let r = execute(&t, &plan, false, cancel, |_| {});
    assert!(r.is_err());
    assert!(!out.exists() && !Path::new(&plan.partial_output).exists());
}

#[test]
fn proxy_for_preview() {
    let Some(t) = tools_or_skip("proxy_for_preview") else { return };
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("a.mp4");
    make_clip(&t, &src, 1);
    let px = dir.path().join("proxy").join("a.mp4");
    make_proxy(&t, &src, &px).unwrap();
    assert!(px.exists());
}

#[test]
fn revision_bundle_and_merge_workflow() {
    let Some(t) = tools_or_skip("revision_bundle_and_merge_workflow") else { return };
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("bars.mp4");
    make_clip(&t, &src, 2);
    let mut p = Project::new("Collab", ProjectSettings::default());
    let m = import_media(&t, &src, p.settings.frame_rate).unwrap();
    let mid = m.id;
    p.media.push(m);
    let v1 = p.tracks[0].id;
    let ids = place_clip(&mut p, PlaceClip { media_id: mid, track_id: v1, start: 0, source_in: None, source_out: Some(40) }).unwrap();

    // Editor A commits, branches, edits.
    let a = RevisionStore::open_or_init(&dir.path().join("a.history")).unwrap();
    let r1 = a.commit(&p, "assembly", "ana", None).unwrap();
    trim_clip(&mut p, ids[0], Edge::Out, 30).unwrap();
    a.commit(&p, "tighten intro", "ana", None).unwrap();
    let diff = diff_entries(&a.get(&r1.id).unwrap().body.snapshot, &p);
    assert!(diff.iter().any(|d| d.summary.contains("(V1): source-out changed from frame 40 to frame 30")));

    // Bundle to editor B (media included), who grades on top of r1.
    let bundle = dir.path().join("a.pirosman-bundle");
    export_bundle(&a, &p, &["main".into()], true, "ana", &bundle).unwrap();
    let b = RevisionStore::open_or_init(&dir.path().join("b.history")).unwrap();
    let mut b_view = a.get(&r1.id).unwrap().body.snapshot;
    b_view.media[0].path = dir.path().join("elsewhere.mp4").to_string_lossy().into();
    let rep = import_bundle(&b, &b_view, &bundle, &dir.path().join("b-media")).unwrap();
    assert_eq!(rep.new_revisions, 2);
    assert_eq!(rep.media_extracted.len(), 1);

    b.create_branch("grade", &r1.id).unwrap();
    b.switch_branch("grade").unwrap();
    let mut graded = b.get(&r1.id).unwrap().body.snapshot;
    let mut col = ColorParams::default();
    col.saturation = 1.3;
    update_clip(&mut graded, ids[0], ClipPatch { color: Some(col), ..Default::default() }).unwrap();
    let g = b.commit(&graded, "warm grade", "ben", None).unwrap();

    let incoming = b.branch_head("incoming/main").unwrap().unwrap();
    let base = b.merge_base(&g.id, &incoming).unwrap().unwrap();
    assert_eq!(base, r1.id);
    let out = merge(&b.get(&base).unwrap().body.snapshot, &graded, &b.get(&incoming).unwrap().body.snapshot, &HashMap::new()).unwrap();
    assert!(out.conflicts.is_empty());
    let merged = out.merged.unwrap();
    let c = merged.clip(ids[0]).unwrap().1;
    assert_eq!((c.source_out, c.color.saturation), (30, 1.3));
    let mr = b.commit(&merged, "merge incoming/main", "ben", Some(&incoming)).unwrap();
    assert_eq!(mr.body.parents.len(), 2);
}
