// Typed bridge to the Rust command layer. Types mirror the serde output of
// crates/pirosman-core; the Rust model is the single source of truth.
import { invoke } from "@tauri-apps/api/core";

export type Uuid = string;
export interface Rational { num: number; den: number }
export type MediaKind = "video" | "audio" | "image";
export type TrackKind = "video" | "audio";

export interface ColorParams { lift: number; gamma: number; gain: number; contrast: number; saturation: number }

export interface MediaRef {
  id: Uuid; name: string; path: string; sha256: string | null; size: number; modified: number | null;
  kind: MediaKind; duration_frames: number | null; has_video: boolean; has_audio: boolean;
  width: number | null; height: number | null; frame_rate: Rational | null;
  video_codec: string | null; audio_codec: string | null; sample_rate: number | null; channels: number | null; container: string | null;
}

export type TransitionKind = "cross_dissolve" | "dip_to_black" | "dip_to_white" | "wipe" | "slide" | "push";
export interface Transition { kind: TransitionKind; duration_frames: number }

// v2 fields are omitted from JSON at their default values, hence optional.
export interface Clip {
  id: Uuid; media_id: Uuid; name: string; start: number; source_in: number; source_out: number;
  gain_db: number; color: ColorParams; link_id: Uuid | null;
  fade_in_frames?: number; fade_out_frames?: number; transition_in?: Transition | null;
  speed?: number; reversed?: boolean; lut?: string | null;
  transform?: Transform; crop?: Crop; opacity?: number; keyframes?: Keyframe[]; vfx?: Vfx[]; ofx?: OfxEffect[];
}

/** Transform and opacity at a *source* frame; when a clip has any, they replace its static values. */
export interface Keyframe { frame: number; transform: Transform; opacity: number }

/** A third-party OpenFX effect on a clip. Params left out use the plugin default. */
export type OfxValue = number | boolean | string | number[];
export interface OfxEffect {
  id: Uuid; plugin_id: string; label: string; grouping: string; binary: string; index: number;
  params: Record<string, OfxValue>; enabled: boolean;
}
export type OfxParamKind = "double" | "int" | "bool" | "choice" | "rgb" | "rgba" | "double2d" | "int2d" | "double3d" | "int3d" | "string" | "group" | "page" | "button" | "custom" | "parametric" | "bytes" | "unknown";
export interface OfxParamInfo {
  name: string; label: string; kind: OfxParamKind; default: OfxValue | null;
  min?: number[]; max?: number[]; display_min?: number[]; display_max?: number[]; options?: string[];
  hint?: string; parent?: string; secret: boolean; normalised: boolean;
}
export interface OfxPlugin {
  index: number; id: string; version: [number, number]; label: string; grouping: string; description: string;
  contexts: string[]; context: string | null; depths: string[]; params: OfxParamInfo[]; supported: boolean; reason?: string;
  binary: string; bundle: string; vendor: string; category: string;
}
export interface OfxCatalog { host: string | null; searched: string[]; plugins: OfxPlugin[]; failures: { bundle: string; binary: string; error: string }[] }

export interface Transform { scale_x: number; scale_y: number; position_x: number; position_y: number; rotation_deg: number }
export interface Crop { left: number; right: number; top: number; bottom: number }
export type VfxKind = "gaussian_blur" | "glow" | "vignette" | "film_grain" | "chromatic_aberration";
export interface Vfx { id: Uuid; kind: VfxKind; amount: number; enabled: boolean }

export type TitlePosition = "top" | "center" | "bottom" | "lower_third";
export type TextAlign = "left" | "center" | "right";
/** Title typography; omitted from JSON when every field is at its default. */
export interface TextStyle {
  font: string; bold: boolean; align: TextAlign; line_spacing: number; outline_px: number; outline_hex: string;
  shadow: boolean; background: string | null; opacity: number;
}
export const DEFAULT_TEXT_STYLE: TextStyle = { font: "", bold: true, align: "left", line_spacing: 0.2, outline_px: 0, outline_hex: "#000000", shadow: true, background: null, opacity: 1 };
/** Mirrors `TITLE_FONTS` in pirosman-core (families whose files the renderer resolves). */
export const TITLE_FONTS = ["Segoe UI", "Arial", "Calibri", "Georgia", "Times New Roman", "Verdana", "Tahoma", "Trebuchet MS", "Consolas", "Courier New", "Impact"];
export interface TitleClip {
  id: Uuid; start: number; duration: number; text: string; font_size: number; color_hex: string; position: TitlePosition;
  fade_in_frames?: number; fade_out_frames?: number;
  /** Free placement (centre of the text block); absent = `position` preset layout. */
  transform?: Transform | null; style?: TextStyle;
}

export interface Track {
  id: Uuid; kind: TrackKind; name: string; muted: boolean; locked: boolean; clips: Clip[];
  titles?: TitleClip[]; solo?: boolean; volume_db?: number; pan?: number; eq_low_db?: number; eq_mid_db?: number; eq_high_db?: number;
}

export type MarkerColor = "yellow" | "red" | "blue" | "green" | "purple";
export interface Marker { id: Uuid; frame: number; name: string; color: MarkerColor; note?: string }

export interface Project {
  format: string; schema_version: number; id: Uuid; name: string;
  settings: { width: number; height: number; frame_rate: Rational; sample_rate: number };
  media: MediaRef[]; tracks: Track[]; markers?: Marker[]; mark_in?: number | null; mark_out?: number | null;
}

export interface ClipPatch {
  name?: string; gain_db?: number; color?: ColorParams; fade_in_frames?: number; fade_out_frames?: number;
  speed?: number; reversed?: boolean; transition_in?: Transition | null; lut?: string | null;
  transform?: Transform; crop?: Crop; opacity?: number; keyframes?: Keyframe[]; vfx?: Vfx[]; ofx?: OfxEffect[];
}
export interface TrackPatch {
  name?: string; muted?: boolean; locked?: boolean; solo?: boolean; volume_db?: number; pan?: number;
  eq_low_db?: number; eq_mid_db?: number; eq_high_db?: number;
}
export interface TitlePatch {
  text?: string; font_size?: number; color_hex?: string; position?: TitlePosition; duration?: number;
  fade_in_frames?: number; fade_out_frames?: number;
  style?: TextStyle; /** null returns the title to its `position` preset. */ transform?: Transform | null;
}
export interface Scopes { cols: number; levels: number; luma: number[]; red: number[]; green: number[]; blue: number[]; clipped_low: number; clipped_high: number }

export type MediaStatus = "online" | "missing" | "modified";
/** `thumb`, `wave` and `strip` are `pcache:` references: load them through `fileSrc`/`cssUrl` (lib/files). */
export interface MediaInfo {
  id: Uuid; status: MediaStatus; preview: string | null; thumb: string | null; wave: string | null;
  strip: string | null; strip_frames: number; needs_proxy: boolean;
}

export interface Change { op: string; [k: string]: unknown }
export interface DiffEntry extends Change { summary: string; clip_id: Uuid | null }

export interface VcsBrief { branch: string; head: string | null; head_message: string | null; changes: DiffEntry[]; merging: string | null }

export interface View {
  project: Project; path: string | null; unsaved: boolean; undo_label: string | null; redo_label: string | null;
  media: MediaInfo[]; vcs: VcsBrief | null; vcs_error: string | null;
}

export interface BranchInfo { name: string; head: string | null; current: boolean }
export interface RevisionSummary {
  id: string; parents: string[]; created_at: string; author: string; message: string; branches: string[];
  clip_count: number; duration_frames: number;
}
export type Side = "ours" | "theirs";
export interface Conflict {
  id: string; object_kind: string; object_id: string; label: string; property: string;
  base: unknown; ours: unknown; theirs: unknown; explanation: string; resolution: Side | null;
}
export interface MergeView {
  theirs_branch: string; base: string; ours: string; theirs: string;
  outcome: { conflicts: Conflict[]; auto_merged: string[]; merged: Project | null };
  incoming: DiffEntry[];
}
export interface VcsState { branches: BranchInfo[]; log: RevisionSummary[]; current: string; dirty: boolean; merge: MergeView | null }

export type VideoCodec = "h264" | "hevc" | "mpeg4" | "prores" | "dnxhr" | "png" | "none";
export type AudioCodec = "aac" | "pcm" | "none";
export interface RenderSettings {
  output: string; width: number; height: number; frame_rate: Rational; video_codec: VideoCodec; quality: number;
  audio_codec: AudioCodec; audio_bitrate_kbps: number; sample_rate: number; overwrite: boolean;
  range_in?: number | null; range_out?: number | null;
}
export interface RenderPlan {
  args: string[]; filtergraph: string; duration_frames: number; duration_us: number; output: string; partial_output: string;
  video_clips: number; audio_clips: number; titles: number; has_video: boolean; range: [number, number]; warnings: string[];
}
export interface RenderProgress { out_time_us: number; duration_us: number; fraction: number; speed: string | null; fps: number | null; stage?: string }
export interface RenderResult {
  output: string; size: number; duration_seconds: number; video_codec: string | null; audio_codec: string | null;
  width: number | null; height: number | null; frame_rate: string | null;
}

export interface ParityFeature {
  id: string; subsystem: string; reference: string; evidence_type: string; evidence: string; notes: string;
  approach: string; module: string; verification: string; status: string; limitations: string;
}
export interface Diagnostics {
  tools: {
    found: boolean; ffmpeg_path: string | null; ffprobe_path: string | null; ffmpeg_version: string | null; ffprobe_version: string | null; encoders: string[]; error: string | null;
    /** Where FFmpeg came from: the copy shipped with the app, PIROSMAN_FFMPEG_DIR, or a system install (fallback). */
    source: "bundled" | "override" | "system" | null; warning: string | null; bundled_dir: string | null;
  };
  ofx: { searched: string[]; plugins: { name: string; path: string; binaries: string[]; status: string }[]; note: string };
  parity: { registry_version: number; updated: string; reference: string; features: ParityFeature[] };
  app_version: string; cache_dir: string; history_dir: string | null;
}

export interface BundleImportReport {
  project_name: string; author: string; new_revisions: number; known_revisions: number;
  branches: Record<string, string>; media_extracted: string[]; media_missing: string[];
}

export const api = {
  getView: () => invoke<View>("get_view"),
  newProject: (req: { name: string; width: number; height: number; fps_num: number; fps_den: number; sample_rate: number }) => invoke<View>("new_project", { req }),
  openProject: (path: string) => invoke<View>("open_project", { path }),
  saveProject: (path?: string) => invoke<View>("save_project", { path: path ?? null }),
  renameProject: (name: string) => invoke<View>("rename_project", { name }),
  undo: () => invoke<View>("undo"),
  redo: () => invoke<View>("redo"),
  importMedia: (paths: string[]) => invoke<{ view: View; imported: string[]; errors: string[] }>("import_media", { paths }),
  relinkMedia: (mediaId: Uuid, path: string, allowDifferent: boolean) => invoke<View>("relink_media", { mediaId, path, allowDifferent }),
  removeMedia: (mediaId: Uuid) => invoke<View>("remove_media", { mediaId }),
  placeClip: (req: { media_id: Uuid; track_id: Uuid; start: number; source_in: number | null; source_out: number | null }) => invoke<View>("place_clip", { req }),
  moveClip: (clipId: Uuid, trackId: Uuid, start: number) => invoke<View>("move_clip", { clipId, trackId, start }),
  trimClip: (clipId: Uuid, edge: "in" | "out", frame: number) => invoke<View>("trim_clip", { clipId, edge, frame }),
  splitClip: (clipId: Uuid, at: number) => invoke<View>("split_clip", { clipId, at }),
  splitAt: (at: number) => invoke<View>("split_at", { at }),
  deleteClips: (clipIds: Uuid[], ripple: boolean) => invoke<View>("delete_clips", { clipIds, ripple }),
  updateClip: (clipId: Uuid, patch: ClipPatch) => invoke<View>("update_clip", { clipId, patch }),
  insertEdit: (req: { media_id: Uuid; track_id: Uuid; start: number; source_in: number | null; source_out: number | null }) => invoke<View>("insert_edit", { req }),
  overwriteEdit: (req: { media_id: Uuid; track_id: Uuid; start: number; source_in: number | null; source_out: number | null }) => invoke<View>("overwrite_edit", { req }),
  slipClip: (clipId: Uuid, delta: number) => invoke<View>("slip_clip", { clipId, delta }),
  slideClip: (clipId: Uuid, delta: number) => invoke<View>("slide_clip", { clipId, delta }),
  rollEdit: (clipId: Uuid, delta: number) => invoke<View>("roll_edit", { clipId, delta }),
  addTitle: (req: { track_id: Uuid; start: number; duration: number; text: string; font_size?: number | null; color_hex?: string | null; position?: TitlePosition | null }) =>
    invoke<View>("add_title", { req: { font_size: null, color_hex: null, position: null, ...req } }),
  updateTitle: (titleId: Uuid, patch: TitlePatch) => invoke<View>("update_title", { titleId, patch }),
  addMarker: (frame: number, name?: string, color?: MarkerColor) => invoke<View>("add_marker", { frame, name: name ?? null, color: color ?? null }),
  updateMarker: (markerId: Uuid, patch: { frame?: number; name?: string; color?: MarkerColor; note?: string }) => invoke<View>("update_marker", { markerId, patch }),
  deleteMarker: (markerId: Uuid) => invoke<View>("delete_marker", { markerId }),
  setRange: (markIn: number | null, markOut: number | null) => invoke<View>("set_range", { markIn, markOut }),
  exportOtio: (path: string) => invoke<string>("export_otio", { path }),
  videoScopes: (frame: number) => invoke<Scopes | null>("video_scopes", { frame }),
  addTrack: (kind: TrackKind) => invoke<View>("add_track", { kind }),
  removeTrack: (trackId: Uuid) => invoke<View>("remove_track", { trackId }),
  updateTrack: (trackId: Uuid, patch: TrackPatch) => invoke<View>("update_track", { trackId, patch }),
  reorderTrack: (trackId: Uuid, newIndex: number) => invoke<View>("reorder_track", { trackId, newIndex }),
  renderPlan: (settings: RenderSettings) => invoke<RenderPlan>("render_plan", { settings }),
  defaultRenderSettings: (output: string) => invoke<RenderSettings>("default_render_settings", { output }),
  startRender: (settings: RenderSettings | null, preview: boolean) => invoke<RenderPlan>("start_render", { settings, preview }),
  cancelRender: () => invoke<boolean>("cancel_render"),
  vcsState: () => invoke<VcsState>("vcs_state"),
  commit: (message: string, author: string) => invoke<View>("vcs_commit", { message, author }),
  createBranch: (name: string, from: string | null, switchTo: boolean) => invoke<View>("vcs_create_branch", { name, from, switch: switchTo }),
  switchBranch: (name: string, mode: "require_clean" | "stash") => invoke<View>("vcs_switch_branch", { name, mode }),
  deleteBranch: (name: string, force: boolean) => invoke<View>("vcs_delete_branch", { name, force }),
  restore: (revision: string) => invoke<View>("vcs_restore", { revision }),
  diff: (from: string, to: string) => invoke<DiffEntry[]>("vcs_diff", { from, to }),
  mergeStart: (branch: string) => invoke<{ result: string; view: View }>("vcs_merge_start", { branch }),
  mergeResolve: (choices: Record<string, Side>) => invoke<View>("vcs_merge_resolve", { choices }),
  mergeCommit: (message: string, author: string) => invoke<View>("vcs_merge_commit", { message, author }),
  mergeAbort: () => invoke<View>("vcs_merge_abort"),
  bundleExport: (branches: string[], includeMedia: boolean, path: string, author: string) =>
    invoke<{ path: string; revisions: number; media_included: number; bytes: number }>("bundle_export", { branches, includeMedia, path, author }),
  bundleImport: (path: string) => invoke<{ report: BundleImportReport; view: View }>("bundle_import", { path }),
  copyItems: (ids: Uuid[]) => invoke<number>("copy_items", { ids }),
  pasteItems: (at: number) => invoke<View>("paste_items", { at }),
  duplicateItems: (ids: Uuid[]) => invoke<View>("duplicate_items", { ids }),
  recentProjects: () => invoke<{ path: string; name: string; exists: boolean }[]>("recent_projects"),
  hubProjects: () => invoke<HubProject[]>("hub_projects"),
  hubDuplicate: (path: string) => invoke<string>("hub_duplicate", { path }),
  hubRename: (path: string, name: string) => invoke<string>("hub_rename", { path, name }),
  hubTrash: (path: string) => invoke<void>("hub_trash", { path }),
  hubForget: (path: string) => invoke<void>("hub_forget", { path }),
  hubImportArchive: (bundle: string, dest: string) => invoke<string>("hub_import_archive", { bundle, dest }),
  storageReport: () => invoke<StorageReport>("storage_report"),
  storageClear: (key: string) => invoke<void>("storage_clear", { key }),
  recentRevisions: (limit?: number) => invoke<RecentRevision[]>("recent_revisions", { limit: limit ?? null }),
  importTimeline: (path: string) => invoke<{ view: View; format: string; warnings: string[] }>("import_timeline", { path }),
  grabStill: (frame: number, path: string) => invoke<string>("grab_still", { frame, path }),
  ofxCatalog: (rescan: boolean) => invoke<OfxCatalog>("ofx_catalog", { rescan }),
  ofxPreviewFrame: (frame: number) => invoke<string | null>("ofx_preview_frame", { frame }),
  resolveInspect: () => invoke<ResolveReport>("resolve_inspect"),
  collabHostStart: (port?: number) => invoke<{ port: number; code: string; local_url: string; lan_urls: string[] }>("collab_host_start", { port: port ?? null }),
  collabHostStop: () => invoke<boolean>("collab_host_stop"),
  collabSpawnPeer: (name: string) => invoke<void>("collab_spawn_peer", { name }),
  collabApplyRemote: (project: Project, author: string) => invoke<View>("collab_apply_remote", { project, author }),
  startupArgs: () => invoke<{ join: string | null; code: string | null; name: string | null }>("startup_args"),
  diagnostics: () => invoke<Diagnostics>("diagnostics"),
  revealPath: (path: string) => invoke<void>("reveal_path", { path }),
  ffmpegStatus: () => invoke<Diagnostics["tools"]>("ffmpeg_status"),
  revealFfmpegFolder: () => invoke<void>("reveal_ffmpeg_folder"),
};

export interface HubProject {
  path: string; name: string; exists: boolean; modified: number | null;
  width: number; height: number; fps_num: number; fps_den: number; duration_frames: number;
  clip_count: number; media_count: number; branch: string | null; revisions: number;
  thumb: string | null; error: string | null; open: boolean;
}
export interface StorageReport {
  cache_dir: string; config_dir: string; total: number;
  items: { key: string; label: string; detail: string; bytes: number; files: number; clearable: boolean }[];
}
export interface RecentRevision {
  project_path: string; project_name: string; id: string; message: string; author: string; created_at: string; branches: string[]; clip_count: number;
}

export interface PeInfo { machine: string; timestamp: number; file_version: string | null; export_count: number; exports_sample: string[] }
export interface ResolveReport {
  install_dir: string; found: boolean; product_version: string | null;
  binaries: { file: string; size: number; role: string; pe: PeInfo | null; error: string | null }[];
  bundled_libraries: string[]; fusion_plugins: string[]; lut_files: number; lut_dirs: string[]; ofx_bundles: string[];
  documented_export_types: string[]; dll_count: number; notes: string[];
}