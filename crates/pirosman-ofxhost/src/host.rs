//! A minimal OpenFX 1.4 image-effect host.
//!
//! Every handle a plugin sees (effect, clip, parameter set, parameter, image,
//! argument set) is a pointer to a [`Node`]: a property set plus what kind of
//! object it is. Objects live until the process exits, except images, which
//! are freed on release or at the end of the render call that produced them.
//!
//! What is provided: the property, image effect, parameter (static values,
//! no keyframes), memory, multi-thread, message, time line and progress
//! suites; filter and general contexts with one `Source` clip; byte, short and
//! float RGBA images at render scale 1. Overlays and interacts, GPU render
//! paths, transitions and generators are not provided.
#![allow(non_upper_case_globals)]

use std::collections::HashMap;
use std::ffi::{c_char, c_double, c_int, c_uint, c_void, CStr, CString};
use std::sync::atomic::{AtomicPtr, AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex, OnceLock};

use serde_json::{json, Map, Value};

use crate::props::{PVal, PropError, Props};
use crate::protocol::{ParamInfo, PluginInfo};
use crate::sys::*;

const MAGIC: u64 = 0x5049_524F_4F46_5821;

pub struct Node {
    magic: u64,
    pub props: Props,
    pub kind: Kind,
}

pub enum Kind {
    Host,
    Args,
    Effect(Effect),
    ParamSet(ParamSet),
    Param(Param),
    Clip(Clip),
    Image(Image),
    Memory(Mutex<Vec<u8>>),
}

pub struct Effect {
    pub param_set: *mut Node,
    pub clips: Mutex<Vec<(String, *mut Node)>>,
    pub render: AtomicPtr<RenderCtx>,
}

pub struct ParamSet {
    pub params: Mutex<Vec<(String, *mut Node)>>,
}

pub struct Param {
    pub name: String,
    pub ptype: String,
    pub value: Mutex<PValue>,
}

#[derive(Clone)]
pub enum PValue {
    Num(Vec<f64>),
    Text(CString),
}

pub struct Clip {
    pub name: String,
    pub effect: *mut Node,
    pub connected: bool,
}

pub struct Image {
    pub _buf: Option<Vec<u8>>,
}

fn new_node(kind: Kind) -> *mut Node {
    Box::into_raw(Box::new(Node { magic: MAGIC, props: Props::new(), kind }))
}

unsafe fn node<'a>(h: *const c_void) -> Option<&'a Node> {
    if h.is_null() {
        return None;
    }
    let n = h as *const Node;
    if (*n).magic == MAGIC {
        Some(&*n)
    } else {
        None
    }
}

unsafe fn key<'a>(p: *const c_char) -> Option<&'a str> {
    if p.is_null() {
        None
    } else {
        CStr::from_ptr(p).to_str().ok()
    }
}

fn trace() -> bool {
    static T: OnceLock<bool> = OnceLock::new();
    *T.get_or_init(|| std::env::var_os("PIROSMAN_OFX_TRACE").is_some())
}

fn perr(e: PropError, n: &Node, k: &str) -> OfxStatus {
    if trace() {
        let what = match n.kind {
            Kind::Host => "host",
            Kind::Args => "args",
            Kind::Effect(_) => "effect",
            Kind::ParamSet(_) => "paramset",
            Kind::Param(_) => "param",
            Kind::Clip(_) => "clip",
            Kind::Image(_) => "image",
            Kind::Memory(_) => "memory",
        };
        eprintln!("ofx-trace: get {what}.{k}: {e:?}");
    }
    match e {
        PropError::Unknown => kOfxStatErrUnknown,
        PropError::BadIndex => kOfxStatErrBadIndex,
        PropError::Type => kOfxStatErrValue,
    }
}

// ================================================================ property suite

macro_rules! with_node {
    ($h:expr, $p:expr, |$n:ident, $k:ident| $body:expr) => {{
        let Some($n) = node($h) else { return kOfxStatErrBadHandle };
        let Some($k) = key($p) else { return kOfxStatErrValue };
        $body
    }};
}

unsafe extern "C" fn prop_set_pointer(h: Handle, p: *const c_char, i: c_int, v: *mut c_void) -> OfxStatus {
    with_node!(h, p, |n, k| {
        if i < 0 {
            return kOfxStatErrBadIndex;
        }
        n.props.set_ptr_at(k, i as usize, v as usize);
        kOfxStatOK
    })
}
unsafe extern "C" fn prop_set_string(h: Handle, p: *const c_char, i: c_int, v: *const c_char) -> OfxStatus {
    with_node!(h, p, |n, k| {
        if i < 0 || v.is_null() {
            return kOfxStatErrBadIndex;
        }
        n.props.set_str_at(k, i as usize, CStr::from_ptr(v));
        kOfxStatOK
    })
}
unsafe extern "C" fn prop_set_double(h: Handle, p: *const c_char, i: c_int, v: c_double) -> OfxStatus {
    with_node!(h, p, |n, k| {
        if i < 0 {
            return kOfxStatErrBadIndex;
        }
        n.props.set_double_at(k, i as usize, v);
        kOfxStatOK
    })
}
unsafe extern "C" fn prop_set_int(h: Handle, p: *const c_char, i: c_int, v: c_int) -> OfxStatus {
    with_node!(h, p, |n, k| {
        if i < 0 {
            return kOfxStatErrBadIndex;
        }
        n.props.set_int_at(k, i as usize, v);
        kOfxStatOK
    })
}
unsafe extern "C" fn prop_set_pointer_n(h: Handle, p: *const c_char, c: c_int, v: *const *mut c_void) -> OfxStatus {
    for i in 0..c.max(0) {
        let s = prop_set_pointer(h, p, i, *v.add(i as usize));
        if s != kOfxStatOK {
            return s;
        }
    }
    kOfxStatOK
}
unsafe extern "C" fn prop_set_string_n(h: Handle, p: *const c_char, c: c_int, v: *const *const c_char) -> OfxStatus {
    for i in 0..c.max(0) {
        let s = prop_set_string(h, p, i, *v.add(i as usize));
        if s != kOfxStatOK {
            return s;
        }
    }
    kOfxStatOK
}
unsafe extern "C" fn prop_set_double_n(h: Handle, p: *const c_char, c: c_int, v: *const c_double) -> OfxStatus {
    for i in 0..c.max(0) {
        let s = prop_set_double(h, p, i, *v.add(i as usize));
        if s != kOfxStatOK {
            return s;
        }
    }
    kOfxStatOK
}
unsafe extern "C" fn prop_set_int_n(h: Handle, p: *const c_char, c: c_int, v: *const c_int) -> OfxStatus {
    for i in 0..c.max(0) {
        let s = prop_set_int(h, p, i, *v.add(i as usize));
        if s != kOfxStatOK {
            return s;
        }
    }
    kOfxStatOK
}
unsafe extern "C" fn prop_get_pointer(h: Handle, p: *const c_char, i: c_int, v: *mut *mut c_void) -> OfxStatus {
    with_node!(h, p, |n, k| match n.props.get_ptr_at(k, i.max(0) as usize) {
        Ok(x) => {
            *v = x as *mut c_void;
            kOfxStatOK
        }
        Err(e) => perr(e, n, k),
    })
}
unsafe extern "C" fn prop_get_string(h: Handle, p: *const c_char, i: c_int, v: *mut *mut c_char) -> OfxStatus {
    with_node!(h, p, |n, k| match n.props.get_str_ptr_at(k, i.max(0) as usize) {
        Ok(x) => {
            *v = x as *mut c_char;
            kOfxStatOK
        }
        Err(e) => perr(e, n, k),
    })
}
unsafe extern "C" fn prop_get_double(h: Handle, p: *const c_char, i: c_int, v: *mut c_double) -> OfxStatus {
    with_node!(h, p, |n, k| match n.props.get_double_at(k, i.max(0) as usize) {
        Ok(x) => {
            *v = x;
            kOfxStatOK
        }
        Err(e) => perr(e, n, k),
    })
}
unsafe extern "C" fn prop_get_int(h: Handle, p: *const c_char, i: c_int, v: *mut c_int) -> OfxStatus {
    with_node!(h, p, |n, k| match n.props.get_int_at(k, i.max(0) as usize) {
        Ok(x) => {
            *v = x;
            kOfxStatOK
        }
        Err(e) => perr(e, n, k),
    })
}
unsafe extern "C" fn prop_get_pointer_n(h: Handle, p: *const c_char, c: c_int, v: *mut *mut c_void) -> OfxStatus {
    for i in 0..c.max(0) {
        let s = prop_get_pointer(h, p, i, v.add(i as usize));
        if s != kOfxStatOK {
            return s;
        }
    }
    kOfxStatOK
}
unsafe extern "C" fn prop_get_string_n(h: Handle, p: *const c_char, c: c_int, v: *mut *mut c_char) -> OfxStatus {
    for i in 0..c.max(0) {
        let s = prop_get_string(h, p, i, v.add(i as usize));
        if s != kOfxStatOK {
            return s;
        }
    }
    kOfxStatOK
}
unsafe extern "C" fn prop_get_double_n(h: Handle, p: *const c_char, c: c_int, v: *mut c_double) -> OfxStatus {
    for i in 0..c.max(0) {
        let s = prop_get_double(h, p, i, v.add(i as usize));
        if s != kOfxStatOK {
            return s;
        }
    }
    kOfxStatOK
}
unsafe extern "C" fn prop_get_int_n(h: Handle, p: *const c_char, c: c_int, v: *mut c_int) -> OfxStatus {
    for i in 0..c.max(0) {
        let s = prop_get_int(h, p, i, v.add(i as usize));
        if s != kOfxStatOK {
            return s;
        }
    }
    kOfxStatOK
}
unsafe extern "C" fn prop_reset(h: Handle, p: *const c_char) -> OfxStatus {
    with_node!(h, p, |n, k| if n.props.has(k) { kOfxStatOK } else { kOfxStatErrUnknown })
}
unsafe extern "C" fn prop_get_dimension(h: Handle, p: *const c_char, c: *mut c_int) -> OfxStatus {
    with_node!(h, p, |n, k| match n.props.dimension(k) {
        Some(d) => {
            *c = d as c_int;
            kOfxStatOK
        }
        None => perr(PropError::Unknown, n, k),
    })
}

static PROPERTY_SUITE: OfxPropertySuiteV1 = OfxPropertySuiteV1 {
    propSetPointer: prop_set_pointer,
    propSetString: prop_set_string,
    propSetDouble: prop_set_double,
    propSetInt: prop_set_int,
    propSetPointerN: prop_set_pointer_n,
    propSetStringN: prop_set_string_n,
    propSetDoubleN: prop_set_double_n,
    propSetIntN: prop_set_int_n,
    propGetPointer: prop_get_pointer,
    propGetString: prop_get_string,
    propGetDouble: prop_get_double,
    propGetInt: prop_get_int,
    propGetPointerN: prop_get_pointer_n,
    propGetStringN: prop_get_string_n,
    propGetDoubleN: prop_get_double_n,
    propGetIntN: prop_get_int_n,
    propReset: prop_reset,
    propGetDimension: prop_get_dimension,
};

// ================================================================ images

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Depth {
    Byte,
    Short,
    Float,
}

impl Depth {
    pub fn name(self) -> &'static str {
        s(match self {
            Depth::Byte => kOfxBitDepthByte,
            Depth::Short => kOfxBitDepthShort,
            Depth::Float => kOfxBitDepthFloat,
        })
    }
    pub fn bytes(self) -> usize {
        match self {
            Depth::Byte => 1,
            Depth::Short => 2,
            Depth::Float => 4,
        }
    }
}

/// An RGBA float frame, top row first, values nominally 0–1.
pub type Frame = Arc<Vec<f32>>;

/// What a render call needs while the plugin is inside `kOfxImageEffectActionRender`.
pub struct RenderCtx {
    pub width: usize,
    pub height: usize,
    pub depth: Depth,
    pub time: f64,
    pub source: *const dyn Fn(f64) -> Frame,
    pub output: *mut u8,
    pub images: Mutex<Vec<usize>>,
}

/// Float frame (top-down) to plugin pixels (bottom-up, OpenFX orientation).
pub fn to_plugin(src: &[f32], w: usize, h: usize, d: Depth) -> Vec<u8> {
    let row = w * 4;
    let mut out = vec![0u8; w * h * 4 * d.bytes()];
    for y in 0..h {
        let s = &src[(h - 1 - y) * row..(h - y) * row];
        let o = y * row * d.bytes();
        match d {
            Depth::Byte => {
                for (i, v) in s.iter().enumerate() {
                    out[o + i] = (v.clamp(0.0, 1.0) * 255.0 + 0.5) as u8;
                }
            }
            Depth::Short => {
                for (i, v) in s.iter().enumerate() {
                    let x = (v.clamp(0.0, 1.0) * 65535.0 + 0.5) as u16;
                    out[o + i * 2..o + i * 2 + 2].copy_from_slice(&x.to_ne_bytes());
                }
            }
            Depth::Float => {
                for (i, v) in s.iter().enumerate() {
                    out[o + i * 4..o + i * 4 + 4].copy_from_slice(&v.to_ne_bytes());
                }
            }
        }
    }
    out
}

/// Plugin pixels (bottom-up) back to a float frame (top-down).
///
/// # Safety
/// `p` must point at `w * h * 4` components of depth `d`.
pub unsafe fn from_plugin(p: *const u8, w: usize, h: usize, d: Depth) -> Vec<f32> {
    let row = w * 4;
    let mut out = vec![0f32; w * h * 4];
    for y in 0..h {
        let dst = &mut out[(h - 1 - y) * row..(h - y) * row];
        let base = p.add(y * row * d.bytes());
        match d {
            Depth::Byte => {
                for (i, v) in dst.iter_mut().enumerate() {
                    *v = *base.add(i) as f32 / 255.0;
                }
            }
            Depth::Short => {
                let b = base as *const u16;
                for (i, v) in dst.iter_mut().enumerate() {
                    *v = b.add(i).read_unaligned() as f32 / 65535.0;
                }
            }
            Depth::Float => {
                let b = base as *const f32;
                for (i, v) in dst.iter_mut().enumerate() {
                    *v = b.add(i).read_unaligned();
                }
            }
        }
    }
    out
}

static IMAGE_ID: AtomicUsize = AtomicUsize::new(1);

fn make_image(ctx: &RenderCtx, data: *mut u8, buf: Option<Vec<u8>>, premult: &str) -> *mut Node {
    let n = new_node(Kind::Image(Image { _buf: buf }));
    let p = unsafe { &(*n).props };
    let (w, h) = (ctx.width as i32, ctx.height as i32);
    p.str(s(kOfxPropType), s(kOfxTypeImage));
    p.str(s(kOfxImageEffectPropPixelDepth), ctx.depth.name());
    p.str(s(kOfxImageEffectPropComponents), s(kOfxImageComponentRGBA));
    p.str(s(kOfxImageEffectPropPreMultiplication), premult);
    p.doubles(s(kOfxImageEffectPropRenderScale), &[1.0, 1.0]);
    p.double(s(kOfxImagePropPixelAspectRatio), 1.0);
    p.ptr(s(kOfxImagePropData), data as *mut c_void);
    p.ints(s(kOfxImagePropBounds), &[0, 0, w, h]);
    p.ints(s(kOfxImagePropRegionOfDefinition), &[0, 0, w, h]);
    p.int(s(kOfxImagePropRowBytes), w * 4 * ctx.depth.bytes() as i32);
    p.str(s(kOfxImagePropField), s(kOfxImageFieldNone));
    p.str(s(kOfxImagePropUniqueIdentifier), &format!("pirosman-{}", IMAGE_ID.fetch_add(1, Ordering::Relaxed)));
    ctx.images.lock().unwrap().push(n as usize);
    n
}

unsafe fn free_image(h: usize) {
    if let Some(n) = node(h as *const c_void) {
        if matches!(n.kind, Kind::Image(_)) {
            let b = Box::from_raw(h as *mut Node);
            let mut b = b;
            b.magic = 0;
            drop(b);
        }
    }
}

// ================================================================ image effect suite

unsafe fn effect_of<'a>(h: Handle) -> Option<(&'a Node, &'a Effect)> {
    let n = node(h)?;
    match &n.kind {
        Kind::Effect(e) => Some((n, e)),
        _ => None,
    }
}

unsafe extern "C" fn ie_get_property_set(h: Handle, out: *mut Handle) -> OfxStatus {
    if effect_of(h).is_none() {
        return kOfxStatErrBadHandle;
    }
    *out = h;
    kOfxStatOK
}
unsafe extern "C" fn ie_get_param_set(h: Handle, out: *mut Handle) -> OfxStatus {
    let Some((_, e)) = effect_of(h) else { return kOfxStatErrBadHandle };
    *out = e.param_set as Handle;
    kOfxStatOK
}

fn clip_descriptor_defaults(p: &Props, name: &str) {
    p.str(s(kOfxPropType), s(kOfxTypeClip));
    p.str(s(kOfxPropName), name);
    p.default_with(s(kOfxPropLabel), PVal::Str(vec![CString::new(name).unwrap_or_default()]));
    p.default_with(s(kOfxImageEffectPropSupportedComponents), PVal::Str(vec![]));
    p.default_with(s(kOfxImageEffectPropTemporalClipAccess), PVal::Int(vec![0]));
    p.default_with(s(kOfxImageClipPropOptional), PVal::Int(vec![0]));
    p.default_with(s(kOfxImageClipPropIsMask), PVal::Int(vec![0]));
    p.default_with(s(kOfxImageClipPropFieldExtraction), PVal::Str(vec![CString::new("OfxImageFieldDoubled").unwrap()]));
    p.default_with(s(kOfxImageEffectPropSupportsTiles), PVal::Int(vec![1]));
}

unsafe extern "C" fn ie_clip_define(h: Handle, name: *const c_char, out: *mut Handle) -> OfxStatus {
    let Some((_, e)) = effect_of(h) else { return kOfxStatErrBadHandle };
    let Some(name) = key(name) else { return kOfxStatErrValue };
    let mut clips = e.clips.lock().unwrap();
    if let Some((_, c)) = clips.iter().find(|(n, _)| n == name) {
        if !out.is_null() {
            *out = *c as Handle;
        }
        return kOfxStatOK;
    }
    let c = new_node(Kind::Clip(Clip { name: name.into(), effect: h as *mut Node, connected: false }));
    clip_descriptor_defaults(&(*c).props, name);
    clips.push((name.into(), c));
    if !out.is_null() {
        *out = c as Handle;
    }
    kOfxStatOK
}
unsafe extern "C" fn ie_clip_get_handle(h: Handle, name: *const c_char, clip: *mut Handle, props: *mut Handle) -> OfxStatus {
    let Some((_, e)) = effect_of(h) else { return kOfxStatErrBadHandle };
    let Some(name) = key(name) else { return kOfxStatErrValue };
    let clips = e.clips.lock().unwrap();
    match clips.iter().find(|(n, _)| n == name) {
        Some((_, c)) => {
            if !clip.is_null() {
                *clip = *c as Handle;
            }
            if !props.is_null() {
                *props = *c as Handle;
            }
            kOfxStatOK
        }
        None => kOfxStatErrUnknown,
    }
}
unsafe extern "C" fn ie_clip_get_property_set(c: Handle, out: *mut Handle) -> OfxStatus {
    match node(c) {
        Some(Node { kind: Kind::Clip(_), .. }) => {
            *out = c;
            kOfxStatOK
        }
        _ => kOfxStatErrBadHandle,
    }
}
unsafe extern "C" fn ie_clip_get_image(c: Handle, time: OfxTime, _region: *const OfxRectD, out: *mut Handle) -> OfxStatus {
    let Some(Node { kind: Kind::Clip(clip), props, .. }) = node(c) else { return kOfxStatErrBadHandle };
    let Some((_, e)) = effect_of(clip.effect as Handle) else { return kOfxStatErrBadHandle };
    let ctx = e.render.load(Ordering::SeqCst);
    if ctx.is_null() {
        return kOfxStatFailed;
    }
    let ctx = &*ctx;
    let premult = props.get_str(s(kOfxImageEffectPropPreMultiplication), 0).unwrap_or_else(|| s(kOfxImageOpaque).into());
    if clip.name == s(kOfxImageEffectOutputClipName) {
        *out = make_image(ctx, ctx.output, None, &premult) as Handle;
        return kOfxStatOK;
    }
    if !clip.connected {
        return kOfxStatFailed;
    }
    let frame = (*ctx.source)(time);
    let mut buf = to_plugin(&frame, ctx.width, ctx.height, ctx.depth);
    let data = buf.as_mut_ptr();
    *out = make_image(ctx, data, Some(buf), &premult) as Handle;
    kOfxStatOK
}
unsafe extern "C" fn ie_clip_release_image(img: Handle) -> OfxStatus {
    let Some(Node { kind: Kind::Image(_), .. }) = node(img) else { return kOfxStatErrBadHandle };
    // Images are owned by the render call that handed them out and freed when it returns.
    kOfxStatOK
}
unsafe extern "C" fn ie_clip_get_rod(c: Handle, _t: OfxTime, out: *mut OfxRectD) -> OfxStatus {
    let Some(Node { kind: Kind::Clip(clip), .. }) = node(c) else { return kOfxStatErrBadHandle };
    let Some((en, _)) = effect_of(clip.effect as Handle) else { return kOfxStatErrBadHandle };
    let size = en.props.get_f64s(s(kOfxImageEffectPropProjectSize));
    let (w, h) = (size.first().copied().unwrap_or(0.0), size.get(1).copied().unwrap_or(0.0));
    *out = OfxRectD { x1: 0.0, y1: 0.0, x2: w, y2: h };
    kOfxStatOK
}
unsafe extern "C" fn ie_abort(_h: Handle) -> c_int {
    0
}
unsafe extern "C" fn ie_mem_alloc(_h: Handle, n: usize, out: *mut Handle) -> OfxStatus {
    *out = new_node(Kind::Memory(Mutex::new(vec![0u8; n]))) as Handle;
    kOfxStatOK
}
unsafe extern "C" fn ie_mem_free(h: Handle) -> OfxStatus {
    match node(h) {
        Some(Node { kind: Kind::Memory(_), .. }) => {
            let mut b = Box::from_raw(h as *mut Node);
            b.magic = 0;
            drop(b);
            kOfxStatOK
        }
        _ => kOfxStatErrBadHandle,
    }
}
unsafe extern "C" fn ie_mem_lock(h: Handle, out: *mut *mut c_void) -> OfxStatus {
    match node(h) {
        Some(Node { kind: Kind::Memory(m), .. }) => {
            *out = m.lock().unwrap().as_mut_ptr() as *mut c_void;
            kOfxStatOK
        }
        _ => kOfxStatErrBadHandle,
    }
}
unsafe extern "C" fn ie_mem_unlock(h: Handle) -> OfxStatus {
    match node(h) {
        Some(Node { kind: Kind::Memory(_), .. }) => kOfxStatOK,
        _ => kOfxStatErrBadHandle,
    }
}

static IMAGE_EFFECT_SUITE: OfxImageEffectSuiteV1 = OfxImageEffectSuiteV1 {
    getPropertySet: ie_get_property_set,
    getParamSet: ie_get_param_set,
    clipDefine: ie_clip_define,
    clipGetHandle: ie_clip_get_handle,
    clipGetPropertySet: ie_clip_get_property_set,
    clipGetImage: ie_clip_get_image,
    clipReleaseImage: ie_clip_release_image,
    clipGetRegionOfDefinition: ie_clip_get_rod,
    abort: ie_abort,
    imageMemoryAlloc: ie_mem_alloc,
    imageMemoryFree: ie_mem_free,
    imageMemoryLock: ie_mem_lock,
    imageMemoryUnlock: ie_mem_unlock,
};

// ================================================================ parameter suite

/// (components, integer?, text?) for a parameter type; components 0 = no value.
pub fn param_shape(t: &str) -> (usize, bool, bool) {
    match t {
        x if x == s(kOfxParamTypeInteger) || x == s(kOfxParamTypeBoolean) || x == s(kOfxParamTypeChoice) => (1, true, false),
        x if x == s(kOfxParamTypeDouble) => (1, false, false),
        x if x == s(kOfxParamTypeRGBA) => (4, false, false),
        x if x == s(kOfxParamTypeRGB) || x == s(kOfxParamTypeDouble3D) => (3, false, false),
        x if x == s(kOfxParamTypeDouble2D) => (2, false, false),
        x if x == s(kOfxParamTypeInteger2D) => (2, true, false),
        x if x == s(kOfxParamTypeInteger3D) => (3, true, false),
        x if x == s(kOfxParamTypeString) || x == s(kOfxParamTypeCustom) || x == s(kOfxParamTypeStrChoice) => (1, false, true),
        _ => (0, false, false),
    }
}

fn param_defaults(p: &Props, ptype: &str, name: &str) {
    let (n, int, text) = param_shape(ptype);
    p.str(s(kOfxPropType), s(kOfxTypeParameter));
    p.str(s(kOfxPropName), name);
    p.str(s(kOfxParamPropType), ptype);
    p.str(s(kOfxPropLabel), name);
    p.str(s(kOfxPropShortLabel), name);
    p.str(s(kOfxPropLongLabel), name);
    p.str(s(kOfxParamPropScriptName), name);
    p.str(s(kOfxParamPropHint), "");
    p.str(s(kOfxParamPropParent), "");
    p.int(s(kOfxParamPropSecret), 0);
    p.int(s(kOfxParamPropEnabled), 1);
    p.int(s(kOfxParamPropAnimates), if text || ptype == s(kOfxParamTypeBoolean) || ptype == s(kOfxParamTypeChoice) { 0 } else { 1 });
    p.int(s(kOfxParamPropIsAnimating), 0);
    p.int(s(kOfxParamPropIsAutoKeying), 0);
    p.int(s(kOfxParamPropPersistant), 1);
    p.int(s(kOfxParamPropEvaluateOnChange), 1);
    p.int(s(kOfxParamPropPluginMayWrite), 0);
    p.int(s(kOfxParamPropCanUndo), 1);
    p.str(s(kOfxParamPropCacheInvalidation), "OfxParamInvalidateValueChange");
    p.ptr(s(kOfxParamPropDataPtr), std::ptr::null_mut());
    p.ptr(s(kOfxParamPropInteractV1), std::ptr::null_mut());
    p.int(s(kOfxParamPropHasHostOverlayHandle), 0);
    p.int(s(kOfxParamPropUseHostOverlayHandle), 0);
    p.int(s(kOfxParamPropShowTimeMarker), 0);
    p.doubles(s(kOfxParamPropInteractSize), &[0.0, 0.0]);
    p.double(s(kOfxParamPropInteractSizeAspect), 1.0);
    p.ints(s(kOfxParamPropInteractMinimumSize), &[10, 10]);
    p.ints(s(kOfxParamPropInteractPreferedSize), &[10, 10]);
    if text {
        p.str(s(kOfxParamPropDefault), "");
        p.str(s(kOfxParamPropStringMode), s(kOfxParamStringIsSingleLine));
        p.int(s(kOfxParamPropStringFilePathExists), 1);
    } else if n > 0 {
        if int {
            p.ints(s(kOfxParamPropDefault), &vec![0; n]);
            p.ints(s(kOfxParamPropMin), &vec![i32::MIN; n]);
            p.ints(s(kOfxParamPropMax), &vec![i32::MAX; n]);
            p.ints(s(kOfxParamPropDisplayMin), &vec![i32::MIN; n]);
            p.ints(s(kOfxParamPropDisplayMax), &vec![i32::MAX; n]);
        } else {
            let (lo, hi) = if ptype == s(kOfxParamTypeRGB) || ptype == s(kOfxParamTypeRGBA) { (0.0, 1.0) } else { (-f64::MAX, f64::MAX) };
            p.doubles(s(kOfxParamPropDefault), &vec![0.0; n]);
            p.doubles(s(kOfxParamPropMin), &vec![lo; n]);
            p.doubles(s(kOfxParamPropMax), &vec![hi; n]);
            p.doubles(s(kOfxParamPropDisplayMin), &vec![lo; n]);
            p.doubles(s(kOfxParamPropDisplayMax), &vec![hi; n]);
            p.str(s(kOfxParamPropDoubleType), s(kOfxParamDoubleTypePlain));
            p.str(s(kOfxParamPropDefaultCoordinateSystem), s(kOfxParamCoordinatesCanonical));
            p.int(s(kOfxParamPropDigits), 2);
            p.double(s(kOfxParamPropIncrement), 1.0);
        }
        let labels: &[&str] = if n == 4 { &["r", "g", "b", "a"] } else if ptype == s(kOfxParamTypeRGB) { &["r", "g", "b"] } else { &["x", "y", "z"] };
        p.strs(s(kOfxParamPropDimensionLabel), &labels[..n.min(labels.len())]);
        if ptype == s(kOfxParamTypeChoice) {
            p.strs(s(kOfxParamPropChoiceOption), &[]);
        }
    }
    if ptype == s(kOfxParamTypePage) {
        p.strs(s(kOfxParamPropPageChild), &[]);
    }
    if ptype == s(kOfxParamTypeGroup) {
        p.int(s(kOfxParamPropGroupOpen), 1);
    }
}

unsafe fn param_of<'a>(h: Handle) -> Option<(&'a Node, &'a Param)> {
    let n = node(h)?;
    match &n.kind {
        Kind::Param(p) => Some((n, p)),
        _ => None,
    }
}
unsafe fn set_of<'a>(h: Handle) -> Option<&'a ParamSet> {
    match &node(h)?.kind {
        Kind::ParamSet(ps) => Some(ps),
        _ => None,
    }
}

unsafe extern "C" fn pm_define(set: Handle, ptype: *const c_char, name: *const c_char, out: *mut Handle) -> OfxStatus {
    let Some(ps) = set_of(set) else { return kOfxStatErrBadHandle };
    let (Some(ptype), Some(name)) = (key(ptype), key(name)) else { return kOfxStatErrValue };
    let mut params = ps.params.lock().unwrap();
    if params.iter().any(|(n, _)| n == name) {
        return kOfxStatErrExists;
    }
    let (n, _, text) = param_shape(ptype);
    let init = if text { PValue::Text(CString::default()) } else { PValue::Num(vec![0.0; n]) };
    let node = new_node(Kind::Param(Param { name: name.into(), ptype: ptype.into(), value: Mutex::new(init) }));
    param_defaults(&(*node).props, ptype, name);
    params.push((name.into(), node));
    if !out.is_null() {
        *out = node as Handle;
    }
    kOfxStatOK
}
unsafe extern "C" fn pm_get_handle(set: Handle, name: *const c_char, out: *mut Handle, props: *mut Handle) -> OfxStatus {
    let Some(ps) = set_of(set) else { return kOfxStatErrBadHandle };
    let Some(name) = key(name) else { return kOfxStatErrValue };
    let params = ps.params.lock().unwrap();
    match params.iter().find(|(n, _)| n == name) {
        Some((_, p)) => {
            if !out.is_null() {
                *out = *p as Handle;
            }
            if !props.is_null() {
                *props = *p as Handle;
            }
            kOfxStatOK
        }
        None => kOfxStatErrUnknown,
    }
}
unsafe extern "C" fn pm_set_get_props(set: Handle, out: *mut Handle) -> OfxStatus {
    if set_of(set).is_none() {
        return kOfxStatErrBadHandle;
    }
    *out = set;
    kOfxStatOK
}
unsafe extern "C" fn pm_get_props(p: Handle, out: *mut Handle) -> OfxStatus {
    if param_of(p).is_none() {
        return kOfxStatErrBadHandle;
    }
    *out = p;
    kOfxStatOK
}

unsafe fn write_value(p: &Param, v: &PValue, slots: [u64; 4], scale: f64) -> OfxStatus {
    let (n, int, _) = param_shape(&p.ptype);
    match v {
        PValue::Text(t) => {
            if slots[0] == 0 {
                return kOfxStatErrValue;
            }
            *(slots[0] as *mut *const c_char) = t.as_ptr();
        }
        PValue::Num(x) => {
            for (i, slot) in slots.iter().enumerate().take(n) {
                if *slot == 0 {
                    return kOfxStatErrValue;
                }
                let val = x.get(i).copied().unwrap_or(0.0) * scale;
                if int {
                    *(*slot as *mut c_int) = val.round() as c_int;
                } else {
                    *(*slot as *mut c_double) = val;
                }
            }
        }
    }
    kOfxStatOK
}

unsafe fn read_value(p: &Param, slots: [u64; 4]) -> Option<PValue> {
    let (n, int, text) = param_shape(&p.ptype);
    if text {
        if slots[0] == 0 {
            return None;
        }
        return Some(PValue::Text(CStr::from_ptr(slots[0] as *const c_char).to_owned()));
    }
    Some(PValue::Num(
        slots.iter().take(n).map(|v| if int { (*v as u32) as i32 as f64 } else { f64::from_bits(*v) }).collect(),
    ))
}

unsafe extern "C" fn pm_get_value(h: Handle, a: u64, b: u64, c: u64, d: u64) -> OfxStatus {
    let Some((_, p)) = param_of(h) else { return kOfxStatErrBadHandle };
    let v = p.value.lock().unwrap().clone();
    write_value(p, &v, [a, b, c, d], 1.0)
}
unsafe extern "C" fn pm_get_value_at(h: Handle, _t: OfxTime, a: u64, b: u64, c: u64, d: u64) -> OfxStatus {
    pm_get_value(h, a, b, c, d)
}
unsafe extern "C" fn pm_get_derivative(h: Handle, _t: OfxTime, a: u64, b: u64, c: u64, d: u64) -> OfxStatus {
    let Some((_, p)) = param_of(h) else { return kOfxStatErrBadHandle };
    let v = p.value.lock().unwrap().clone();
    write_value(p, &v, [a, b, c, d], 0.0)
}
unsafe extern "C" fn pm_get_integral(h: Handle, t1: OfxTime, t2: OfxTime, a: u64, b: u64, c: u64, d: u64) -> OfxStatus {
    let Some((_, p)) = param_of(h) else { return kOfxStatErrBadHandle };
    let v = p.value.lock().unwrap().clone();
    write_value(p, &v, [a, b, c, d], t2 - t1)
}
unsafe extern "C" fn pm_set_value(h: Handle, a: u64, b: u64, c: u64, d: u64) -> OfxStatus {
    let Some((_, p)) = param_of(h) else { return kOfxStatErrBadHandle };
    match read_value(p, [a, b, c, d]) {
        Some(v) => {
            *p.value.lock().unwrap() = v;
            kOfxStatOK
        }
        None => kOfxStatErrValue,
    }
}
unsafe extern "C" fn pm_set_value_at(h: Handle, _t: OfxTime, a: u64, b: u64, c: u64, d: u64) -> OfxStatus {
    pm_set_value(h, a, b, c, d)
}
unsafe extern "C" fn pm_num_keys(h: Handle, n: *mut c_uint) -> OfxStatus {
    if param_of(h).is_none() {
        return kOfxStatErrBadHandle;
    }
    *n = 0;
    kOfxStatOK
}
unsafe extern "C" fn pm_key_time(_h: Handle, _n: c_uint, _t: *mut OfxTime) -> OfxStatus {
    kOfxStatErrBadIndex
}
unsafe extern "C" fn pm_key_index(_h: Handle, _t: OfxTime, _d: c_int, _i: *mut c_int) -> OfxStatus {
    kOfxStatFailed
}
unsafe extern "C" fn pm_delete_key(_h: Handle, _t: OfxTime) -> OfxStatus {
    kOfxStatErrBadIndex
}
unsafe extern "C" fn pm_delete_all_keys(_h: Handle) -> OfxStatus {
    kOfxStatOK
}
unsafe extern "C" fn pm_copy(to: Handle, from: Handle, _o: OfxTime, _r: *const OfxRangeD) -> OfxStatus {
    let (Some((_, a)), Some((_, b))) = (param_of(to), param_of(from)) else { return kOfxStatErrBadHandle };
    if a.ptype != b.ptype {
        return kOfxStatErrValue;
    }
    let v = b.value.lock().unwrap().clone();
    *a.value.lock().unwrap() = v;
    kOfxStatOK
}
unsafe extern "C" fn pm_edit_begin(_s: Handle, _n: *const c_char) -> OfxStatus {
    kOfxStatOK
}
unsafe extern "C" fn pm_edit_end(_s: Handle) -> OfxStatus {
    kOfxStatOK
}

static PARAMETER_SUITE: OfxParameterSuiteV1Host = OfxParameterSuiteV1Host {
    paramDefine: pm_define,
    paramGetHandle: pm_get_handle,
    paramSetGetPropertySet: pm_set_get_props,
    paramGetPropertySet: pm_get_props,
    paramGetValue: pm_get_value,
    paramGetValueAtTime: pm_get_value_at,
    paramGetDerivative: pm_get_derivative,
    paramGetIntegral: pm_get_integral,
    paramSetValue: pm_set_value,
    paramSetValueAtTime: pm_set_value_at,
    paramGetNumKeys: pm_num_keys,
    paramGetKeyTime: pm_key_time,
    paramGetKeyIndex: pm_key_index,
    paramDeleteKey: pm_delete_key,
    paramDeleteAllKeys: pm_delete_all_keys,
    paramCopy: pm_copy,
    paramEditBegin: pm_edit_begin,
    paramEditEnd: pm_edit_end,
};

// ================================================================ memory, threads, messages

const ALIGN: usize = 16;

unsafe extern "C" fn mem_alloc(_h: Handle, n: usize, out: *mut *mut c_void) -> OfxStatus {
    let Ok(layout) = std::alloc::Layout::from_size_align(n + ALIGN, ALIGN) else { return kOfxStatErrMemory };
    let p = std::alloc::alloc(layout);
    if p.is_null() {
        return kOfxStatErrMemory;
    }
    (p as *mut usize).write(n);
    *out = p.add(ALIGN) as *mut c_void;
    kOfxStatOK
}
unsafe extern "C" fn mem_free(p: *mut c_void) -> OfxStatus {
    if p.is_null() {
        return kOfxStatOK;
    }
    let base = (p as *mut u8).sub(ALIGN);
    let n = (base as *mut usize).read();
    std::alloc::dealloc(base, std::alloc::Layout::from_size_align_unchecked(n + ALIGN, ALIGN));
    kOfxStatOK
}

static MEMORY_SUITE: OfxMemorySuiteV1 = OfxMemorySuiteV1 { memoryAlloc: mem_alloc, memoryFree: mem_free };

thread_local! {
    static THREAD_INDEX: std::cell::Cell<Option<u32>> = const { std::cell::Cell::new(None) };
}

struct SendPtr(*mut c_void);
unsafe impl Send for SendPtr {}
unsafe impl Sync for SendPtr {}

unsafe extern "C" fn mt_multi_thread(f: ThreadFn, n: c_uint, arg: *mut c_void) -> OfxStatus {
    let n = n.max(1);
    let arg = SendPtr(arg);
    let arg = &arg;
    std::thread::scope(|sc| {
        for i in 0..n {
            sc.spawn(move || {
                THREAD_INDEX.with(|t| t.set(Some(i)));
                let a = arg;
                unsafe { f(i, n, a.0) };
            });
        }
    });
    kOfxStatOK
}
unsafe extern "C" fn mt_num_cpus(n: *mut c_uint) -> OfxStatus {
    *n = std::thread::available_parallelism().map(|x| x.get() as c_uint).unwrap_or(1);
    kOfxStatOK
}
unsafe extern "C" fn mt_index(i: *mut c_uint) -> OfxStatus {
    *i = THREAD_INDEX.with(|t| t.get()).unwrap_or(0);
    kOfxStatOK
}
unsafe extern "C" fn mt_is_spawned() -> c_int {
    THREAD_INDEX.with(|t| t.get()).is_some() as c_int
}

/// OpenFX mutexes are recursive.
struct RMutex {
    m: Mutex<(Option<std::thread::ThreadId>, u32)>,
    cv: Condvar,
}

unsafe extern "C" fn mt_mutex_create(out: *mut Handle, count: c_int) -> OfxStatus {
    let m = Box::new(RMutex { m: Mutex::new((None, 0)), cv: Condvar::new() });
    if count > 0 {
        *m.m.lock().unwrap() = (Some(std::thread::current().id()), count as u32);
    }
    *out = Box::into_raw(m) as Handle;
    kOfxStatOK
}
unsafe extern "C" fn mt_mutex_destroy(h: Handle) -> OfxStatus {
    if !h.is_null() {
        drop(Box::from_raw(h as *mut RMutex));
    }
    kOfxStatOK
}
unsafe extern "C" fn mt_mutex_lock(h: Handle) -> OfxStatus {
    if h.is_null() {
        return kOfxStatErrBadHandle;
    }
    let m = &*(h as *const RMutex);
    let me = std::thread::current().id();
    let mut g = m.m.lock().unwrap();
    loop {
        match g.0 {
            None => {
                *g = (Some(me), 1);
                return kOfxStatOK;
            }
            Some(o) if o == me => {
                g.1 += 1;
                return kOfxStatOK;
            }
            _ => g = m.cv.wait(g).unwrap(),
        }
    }
}
unsafe extern "C" fn mt_mutex_unlock(h: Handle) -> OfxStatus {
    if h.is_null() {
        return kOfxStatErrBadHandle;
    }
    let m = &*(h as *const RMutex);
    let mut g = m.m.lock().unwrap();
    if g.0 == Some(std::thread::current().id()) {
        g.1 -= 1;
        if g.1 == 0 {
            g.0 = None;
            m.cv.notify_one();
        }
    }
    kOfxStatOK
}
unsafe extern "C" fn mt_mutex_try_lock(h: Handle) -> OfxStatus {
    if h.is_null() {
        return kOfxStatErrBadHandle;
    }
    let m = &*(h as *const RMutex);
    let me = std::thread::current().id();
    let mut g = m.m.lock().unwrap();
    match g.0 {
        None => {
            *g = (Some(me), 1);
            kOfxStatOK
        }
        Some(o) if o == me => {
            g.1 += 1;
            kOfxStatOK
        }
        _ => kOfxStatFailed,
    }
}

static MULTI_THREAD_SUITE: OfxMultiThreadSuiteV1 = OfxMultiThreadSuiteV1 {
    multiThread: mt_multi_thread,
    multiThreadNumCPUs: mt_num_cpus,
    multiThreadIndex: mt_index,
    multiThreadIsSpawnedThread: mt_is_spawned,
    mutexCreate: mt_mutex_create,
    mutexDestroy: mt_mutex_destroy,
    mutexLock: mt_mutex_lock,
    mutexUnLock: mt_mutex_unlock,
    mutexTryLock: mt_mutex_try_lock,
};

/// Messages a plugin raised during the current action, for error reports.
pub static MESSAGES: Mutex<Vec<String>> = Mutex::new(Vec::new());

unsafe extern "C" fn msg_message(_h: Handle, kind: *const c_char, _id: *const c_char, fmt: *const c_char) -> OfxStatus {
    let kind = key(kind).unwrap_or("");
    let text = key(fmt).unwrap_or("").to_string();
    eprintln!("ofx-message [{kind}]: {text}");
    MESSAGES.lock().unwrap().push(text);
    if kind.contains("Question") {
        kOfxStatReplyDefault
    } else {
        kOfxStatOK
    }
}
unsafe extern "C" fn msg_clear(_h: Handle) -> OfxStatus {
    kOfxStatOK
}

static MESSAGE_SUITE: OfxMessageSuiteV2Host = OfxMessageSuiteV2Host { message: msg_message, setPersistentMessage: msg_message, clearPersistentMessage: msg_clear };

unsafe extern "C" fn ix_unsupported(_h: Handle) -> OfxStatus {
    kOfxStatErrUnsupported
}
unsafe extern "C" fn ix_props(_h: Handle, _o: *mut Handle) -> OfxStatus {
    kOfxStatErrUnsupported
}
static INTERACT_SUITE: OfxInteractSuiteV1 = OfxInteractSuiteV1 { interactSwapBuffers: ix_unsupported, interactRedraw: ix_unsupported, interactGetPropertySet: ix_props };

unsafe extern "C" fn tl_get_time(h: Handle, t: *mut c_double) -> OfxStatus {
    let Some((_, e)) = effect_of(h) else { return kOfxStatErrBadHandle };
    let ctx = e.render.load(Ordering::SeqCst);
    *t = if ctx.is_null() { 0.0 } else { (*ctx).time };
    kOfxStatOK
}
unsafe extern "C" fn tl_goto(_h: Handle, _t: c_double) -> OfxStatus {
    kOfxStatOK
}
unsafe extern "C" fn tl_bounds(h: Handle, a: *mut c_double, b: *mut c_double) -> OfxStatus {
    let Some((n, _)) = effect_of(h) else { return kOfxStatErrBadHandle };
    let r = n.props.get_f64s(s(kOfxImageEffectPropFrameRange));
    *a = r.first().copied().unwrap_or(0.0);
    *b = r.get(1).copied().unwrap_or(0.0);
    kOfxStatOK
}
static TIMELINE_SUITE: OfxTimeLineSuiteV1 = OfxTimeLineSuiteV1 { getTime: tl_get_time, gotoTime: tl_goto, getTimeBounds: tl_bounds };

unsafe extern "C" fn pg_start(_h: Handle, _a: *const c_char, _b: *const c_char) -> OfxStatus {
    kOfxStatOK
}
unsafe extern "C" fn pg_update(_h: Handle, _p: c_double) -> OfxStatus {
    kOfxStatOK
}
unsafe extern "C" fn pg_end(_h: Handle) -> OfxStatus {
    kOfxStatOK
}
static PROGRESS_SUITE: OfxProgressSuiteV2 = OfxProgressSuiteV2 { progressStart: pg_start, progressUpdate: pg_update, progressEnd: pg_end };

// ================================================================ host

unsafe extern "C" fn fetch_suite(_host: Handle, name: *const c_char, version: c_int) -> *const c_void {
    let Some(name) = key(name) else { return std::ptr::null() };
    let p: *const c_void = match (name, version) {
        (n, 1) if n == s(kOfxPropertySuite) => &PROPERTY_SUITE as *const _ as _,
        (n, 1) if n == s(kOfxImageEffectSuite) => &IMAGE_EFFECT_SUITE as *const _ as _,
        (n, 1) if n == s(kOfxParameterSuite) => &PARAMETER_SUITE as *const _ as _,
        (n, 1) if n == s(kOfxMemorySuite) => &MEMORY_SUITE as *const _ as _,
        (n, 1) if n == s(kOfxMultiThreadSuite) => &MULTI_THREAD_SUITE as *const _ as _,
        (n, 1 | 2) if n == s(kOfxMessageSuite) => &MESSAGE_SUITE as *const _ as _,
        (n, 1) if n == s(kOfxInteractSuite) => &INTERACT_SUITE as *const _ as _,
        (n, 1) if n == s(kOfxTimeLineSuite) => &TIMELINE_SUITE as *const _ as _,
        (n, 1 | 2) if n == s(kOfxProgressSuite) => &PROGRESS_SUITE as *const _ as _,
        _ => std::ptr::null(),
    };
    if trace() {
        eprintln!("ofx-trace: fetchSuite {name} v{version} -> {}", if p.is_null() { "none" } else { "ok" });
    }
    p
}

pub const HOST_NAME: &str = "studio.pirosman.ofxhost";

fn host_props(p: &Props) {
    p.str(s(kOfxPropType), s(kOfxTypeImageEffectHost));
    p.str(s(kOfxPropName), HOST_NAME);
    p.str(s(kOfxPropLabel), "Pirosman Studio");
    p.ints(s(kOfxPropAPIVersion), &[1, 4]);
    // Numeric major.minor.patch only; a pre-release suffix ("-alpha") stays in the label.
    let v: Vec<i32> = [env!("CARGO_PKG_VERSION_MAJOR"), env!("CARGO_PKG_VERSION_MINOR"), env!("CARGO_PKG_VERSION_PATCH")]
        .iter()
        .filter_map(|x| x.parse().ok())
        .collect();
    p.ints(s(kOfxPropVersion), &v);
    p.str(s(kOfxPropVersionLabel), env!("CARGO_PKG_VERSION"));
    p.int(s(kOfxImageEffectHostPropIsBackground), 1);
    p.int(s(kOfxImageEffectPropSupportsOverlays), 0);
    p.int(s(kOfxImageEffectPropSupportsMultiResolution), 0);
    p.int(s(kOfxImageEffectPropSupportsTiles), 0);
    p.int(s(kOfxImageEffectPropTemporalClipAccess), 1);
    p.strs(s(kOfxImageEffectPropSupportedComponents), &[s(kOfxImageComponentRGBA)]);
    p.strs(s(kOfxImageEffectPropSupportedContexts), &[s(kOfxImageEffectContextFilter), s(kOfxImageEffectContextGeneral)]);
    p.strs(s(kOfxImageEffectPropSupportedPixelDepths), &[s(kOfxBitDepthByte), s(kOfxBitDepthShort), s(kOfxBitDepthFloat)]);
    p.int(s(kOfxImageEffectPropSupportsMultipleClipDepths), 0);
    p.int(s(kOfxImageEffectPropSupportsMultipleClipPARs), 0);
    p.int(s(kOfxImageEffectPropSetableFrameRate), 0);
    p.int(s(kOfxImageEffectPropSetableFielding), 0);
    p.int(s(kOfxImageEffectInstancePropSequentialRender), 0);
    p.str(s(kOfxImageEffectPropOpenGLRenderSupported), "false");
    p.str(s(kOfxImageEffectPropCudaRenderSupported), "false");
    p.str(s(kOfxImageEffectPropOpenCLRenderSupported), "false");
    p.str(s(kOfxImageEffectPropMetalRenderSupported), "false");
    p.int(s(kOfxParamHostPropSupportsCustomInteract), 0);
    p.int(s(kOfxParamHostPropSupportsStringAnimation), 0);
    p.int(s(kOfxParamHostPropSupportsChoiceAnimation), 0);
    p.int(s(kOfxParamHostPropSupportsStrChoiceAnimation), 0);
    p.int(s(kOfxParamHostPropSupportsStrChoice), 0);
    p.int(s(kOfxParamHostPropSupportsBooleanAnimation), 0);
    p.int(s(kOfxParamHostPropSupportsCustomAnimation), 0);
    p.int(s(kOfxParamHostPropSupportsParametricAnimation), 0);
    p.int(s(kOfxParamHostPropMaxParameters), -1);
    p.int(s(kOfxParamHostPropMaxPages), 0);
    p.ints(s(kOfxParamHostPropPageRowColumnCount), &[0, 0]);
    p.ptr(s(kOfxPropHostOSHandleProp), std::ptr::null_mut());
}

struct HostPtr(*mut OfxHost);
unsafe impl Send for HostPtr {}
unsafe impl Sync for HostPtr {}

pub fn ofx_host() -> *mut OfxHost {
    static H: OnceLock<HostPtr> = OnceLock::new();
    H.get_or_init(|| {
        let n = new_node(Kind::Host);
        host_props(unsafe { &(*n).props });
        HostPtr(Box::into_raw(Box::new(OfxHost { host: n as Handle, fetchSuite: fetch_suite })))
    })
    .0
}

fn args() -> *mut Node {
    new_node(Kind::Args)
}

// ================================================================ binaries and plugins

pub struct Binary {
    pub path: String,
    pub plugins: Vec<*mut OfxPlugin>,
}

#[cfg(windows)]
mod os {
    use std::ffi::{c_char, c_void};
    #[link(name = "kernel32")]
    extern "system" {
        fn LoadLibraryExW(name: *const u16, file: *mut c_void, flags: u32) -> *mut c_void;
        fn GetProcAddress(m: *mut c_void, name: *const c_char) -> *mut c_void;
        fn GetLastError() -> u32;
    }
    const LOAD_WITH_ALTERED_SEARCH_PATH: u32 = 0x8;

    pub fn load(path: &str) -> Result<*mut c_void, String> {
        let w: Vec<u16> = path.encode_utf16().chain(std::iter::once(0)).collect();
        let m = unsafe { LoadLibraryExW(w.as_ptr(), std::ptr::null_mut(), LOAD_WITH_ALTERED_SEARCH_PATH) };
        if m.is_null() {
            let code = unsafe { GetLastError() };
            let why = match code {
                126 => "a DLL it depends on is missing".to_string(),
                193 => "it is not a 64-bit Windows binary".to_string(),
                _ => format!("Windows error {code}"),
            };
            Err(format!("could not load {path}: {why}"))
        } else {
            Ok(m)
        }
    }
    pub fn sym(m: *mut c_void, name: &str) -> *mut c_void {
        let c = std::ffi::CString::new(name).unwrap();
        unsafe { GetProcAddress(m, c.as_ptr()) }
    }
}

#[cfg(not(windows))]
mod os {
    use std::ffi::c_void;
    pub fn load(_path: &str) -> Result<*mut c_void, String> {
        Err("OpenFX hosting is implemented for Windows only".into())
    }
    pub fn sym(_m: *mut c_void, _name: &str) -> *mut c_void {
        std::ptr::null_mut()
    }
}

pub fn load_binary(path: &str) -> Result<Binary, String> {
    let m = os::load(path)?;
    let get_num = os::sym(m, "OfxGetNumberOfPlugins");
    let get_plugin = os::sym(m, "OfxGetPlugin");
    if get_num.is_null() || get_plugin.is_null() {
        return Err(format!("{path} is not an OpenFX plugin (OfxGetNumberOfPlugins/OfxGetPlugin not exported)"));
    }
    let set_host = os::sym(m, "OfxSetHost");
    unsafe {
        if !set_host.is_null() {
            let f: SetHostFn = std::mem::transmute(set_host);
            f(ofx_host());
        }
        let get_num: GetNumberOfPluginsFn = std::mem::transmute(get_num);
        let get_plugin: GetPluginFn = std::mem::transmute(get_plugin);
        let n = get_num().max(0);
        let plugins = (0..n).map(|i| get_plugin(i)).collect();
        Ok(Binary { path: path.into(), plugins })
    }
}

unsafe fn cs(p: *const c_char) -> String {
    if p.is_null() {
        String::new()
    } else {
        CStr::from_ptr(p).to_string_lossy().into_owned()
    }
}

fn status_text(st: OfxStatus) -> &'static str {
    match st {
        kOfxStatFailed => "failed",
        kOfxStatErrFatal => "fatal error",
        kOfxStatErrUnknown => "unknown property or object",
        kOfxStatErrMissingHostFeature => "needs a host feature Pirosman does not provide",
        kOfxStatErrUnsupported => "unsupported",
        kOfxStatErrMemory => "out of memory",
        kOfxStatErrBadHandle => "bad handle",
        kOfxStatErrValue => "bad value",
        kOfxStatErrFormat => "unsupported pixel format",
        _ => "error",
    }
}

fn last_message() -> String {
    MESSAGES.lock().unwrap().last().cloned().map(|m| format!(": {m}")).unwrap_or_default()
}

/// One plugin from a binary, loaded and described.
#[derive(Clone)]
pub struct Plugin {
    pub raw: *mut OfxPlugin,
    pub index: u32,
    pub id: String,
    pub version: [u32; 2],
    pub descriptor: *mut Node,
}

impl Plugin {
    pub fn action(&self, name: &'static str, handle: *mut Node, in_args: *mut Node, out_args: *mut Node) -> OfxStatus {
        unsafe { ((*self.raw).mainEntry)(c(name), handle as *const c_void, in_args as Handle, out_args as Handle) }
    }

    /// `setHost`, load and describe. Fails for non image-effect plugins.
    pub fn open(raw: *mut OfxPlugin, index: u32) -> Result<Plugin, String> {
        if raw.is_null() {
            return Err(format!("plugin #{index} is null"));
        }
        let (api, ver, id, vmaj, vmin) = unsafe { (cs((*raw).pluginApi), (*raw).apiVersion, cs((*raw).pluginIdentifier), (*raw).pluginVersionMajor, (*raw).pluginVersionMinor) };
        if api != s(kOfxImageEffectPluginApi) || ver != 1 {
            return Err(format!("{id}: API {api} v{ver} is not an OpenFX image effect"));
        }
        unsafe { ((*raw).setHost)(ofx_host()) };
        let desc = new_effect();
        let mut p = Plugin { raw, index, id: id.clone(), version: [vmaj, vmin], descriptor: desc };
        let st = p.action(kOfxActionLoad, std::ptr::null_mut(), std::ptr::null_mut(), std::ptr::null_mut());
        if st != kOfxStatOK && st != kOfxStatReplyDefault {
            return Err(format!("{id}: load {}{}", status_text(st), last_message()));
        }
        effect_descriptor_defaults(unsafe { &(*desc).props });
        let st = p.action(kOfxActionDescribe, desc, std::ptr::null_mut(), std::ptr::null_mut());
        if st != kOfxStatOK && st != kOfxStatReplyDefault {
            return Err(format!("{id}: describe {}{}", status_text(st), last_message()));
        }
        p.descriptor = desc;
        Ok(p)
    }

    pub fn props(&self) -> &Props {
        unsafe { &(*self.descriptor).props }
    }

    pub fn label(&self) -> String {
        let p = self.props();
        p.get_str(s(kOfxPropLabel), 0).filter(|x| !x.is_empty()).unwrap_or_else(|| self.id.clone())
    }

    /// The context Pirosman instantiates: filter, else general.
    pub fn pick_context(&self) -> Option<String> {
        let ctxs = self.props().get_strs(s(kOfxImageEffectPropSupportedContexts));
        [s(kOfxImageEffectContextFilter), s(kOfxImageEffectContextGeneral)].into_iter().find(|c| ctxs.iter().any(|x| x == c)).map(String::from)
    }

    /// Byte, short or float, preferring float.
    pub fn pick_depth(&self) -> Option<Depth> {
        let d = self.props().get_strs(s(kOfxImageEffectPropSupportedPixelDepths));
        if d.iter().any(|x| x == s(kOfxBitDepthFloat)) {
            Some(Depth::Float)
        } else if d.iter().any(|x| x == s(kOfxBitDepthShort)) {
            Some(Depth::Short)
        } else if d.iter().any(|x| x == s(kOfxBitDepthByte)) {
            Some(Depth::Byte)
        } else {
            None
        }
    }

    /// Describe in a context: returns a context descriptor holding clips and params.
    pub fn describe_in(&self, ctx: &str) -> Result<*mut Node, String> {
        let d = new_effect();
        unsafe {
            let src = &(*self.descriptor).props;
            (*d).props = src.clone_values();
        }
        let a = args();
        unsafe { (*a).props.str(s(kOfxImageEffectPropContext), ctx) };
        let st = self.action(kOfxImageEffectActionDescribeInContext, d, a, std::ptr::null_mut());
        if st != kOfxStatOK && st != kOfxStatReplyDefault {
            return Err(format!("{}: describe in {ctx} {}{}", self.id, status_text(st), last_message()));
        }
        Ok(d)
    }
}

fn new_effect() -> *mut Node {
    let ps = new_node(Kind::ParamSet(ParamSet { params: Mutex::new(vec![]) }));
    new_node(Kind::Effect(Effect { param_set: ps, clips: Mutex::new(vec![]), render: AtomicPtr::new(std::ptr::null_mut()) }))
}

fn effect_descriptor_defaults(p: &Props) {
    p.str(s(kOfxPropType), s(kOfxTypeImageEffect));
    p.str(s(kOfxPropLabel), "");
    p.str(s(kOfxPropShortLabel), "");
    p.str(s(kOfxPropLongLabel), "");
    p.str(s(kOfxPropVersionLabel), "");
    p.str(s(kOfxPropPluginDescription), "");
    p.str(s(kOfxImageEffectPluginPropGrouping), "");
    p.strs(s(kOfxImageEffectPropSupportedContexts), &[]);
    p.strs(s(kOfxImageEffectPropSupportedPixelDepths), &[]);
    p.int(s(kOfxImageEffectPluginPropSingleInstance), 0);
    p.str(s(kOfxImageEffectPluginRenderThreadSafety), s(kOfxImageEffectRenderInstanceSafe));
    p.int(s(kOfxImageEffectPluginPropHostFrameThreading), 0);
    p.int(s(kOfxImageEffectPropSupportsMultiResolution), 1);
    p.int(s(kOfxImageEffectPropSupportsTiles), 1);
    p.int(s(kOfxImageEffectPropTemporalClipAccess), 0);
    p.int(s(kOfxImageEffectPluginPropFieldRenderTwiceAlways), 1);
    p.int(s(kOfxImageEffectPropSupportsMultipleClipDepths), 0);
    p.int(s(kOfxImageEffectPropSupportsMultipleClipPARs), 0);
    p.ptr(s(kOfxImageEffectPluginPropOverlayInteractV1), std::ptr::null_mut());
    p.strs(s(kOfxImageEffectPropClipPreferencesSlaveParam), &[]);
    p.str(s(kOfxImageEffectPropOpenGLRenderSupported), "false");
}

unsafe fn clips_of(d: *mut Node) -> Vec<(String, *mut Node)> {
    match &(*d).kind {
        Kind::Effect(e) => e.clips.lock().unwrap().clone(),
        _ => vec![],
    }
}
unsafe fn params_of(d: *mut Node) -> Vec<(String, *mut Node)> {
    match &(*d).kind {
        Kind::Effect(e) => match &(*e.param_set).kind {
            Kind::ParamSet(ps) => ps.params.lock().unwrap().clone(),
            _ => vec![],
        },
        _ => vec![],
    }
}

fn kind_name(t: &str) -> &'static str {
    match t {
        x if x == s(kOfxParamTypeInteger) => "int",
        x if x == s(kOfxParamTypeDouble) => "double",
        x if x == s(kOfxParamTypeBoolean) => "bool",
        x if x == s(kOfxParamTypeChoice) => "choice",
        x if x == s(kOfxParamTypeStrChoice) => "string",
        x if x == s(kOfxParamTypeRGBA) => "rgba",
        x if x == s(kOfxParamTypeRGB) => "rgb",
        x if x == s(kOfxParamTypeDouble2D) => "double2d",
        x if x == s(kOfxParamTypeInteger2D) => "int2d",
        x if x == s(kOfxParamTypeDouble3D) => "double3d",
        x if x == s(kOfxParamTypeInteger3D) => "int3d",
        x if x == s(kOfxParamTypeString) => "string",
        x if x == s(kOfxParamTypeCustom) => "custom",
        x if x == s(kOfxParamTypeBytes) => "bytes",
        x if x == s(kOfxParamTypeGroup) => "group",
        x if x == s(kOfxParamTypePage) => "page",
        x if x == s(kOfxParamTypePushButton) => "button",
        x if x == s(kOfxParamTypeParametric) => "parametric",
        _ => "unknown",
    }
}

/// Spatial double types whose values are fractions of the frame, and whether
/// the host must convert them to pixels (true for the non-deprecated types
/// declared with normalised default coordinates).
fn spatial(p: &Props) -> (bool, bool, [usize; 3]) {
    let t = p.get_str(s(kOfxParamPropDoubleType), 0).unwrap_or_default();
    let norm_coords = p.get_str(s(kOfxParamPropDefaultCoordinateSystem), 0).as_deref() == Some(s(kOfxParamCoordinatesNormalised));
    // Axis per component: 0 = x (width), 1 = y (height), 2 = none.
    let axes = match t.as_str() {
        "OfxParamDoubleTypeX" | "OfxParamDoubleTypeXAbsolute" | "OfxParamDoubleTypeNormalisedX" | "OfxParamDoubleTypeNormalisedXAbsolute" => [0, 2, 2],
        "OfxParamDoubleTypeY" | "OfxParamDoubleTypeYAbsolute" | "OfxParamDoubleTypeNormalisedY" | "OfxParamDoubleTypeNormalisedYAbsolute" => [1, 2, 2],
        "OfxParamDoubleTypeXY" | "OfxParamDoubleTypeXYAbsolute" | "OfxParamDoubleTypeNormalisedXY" | "OfxParamDoubleTypeNormalisedXYAbsolute" => [0, 1, 2],
        _ => return (false, false, [2, 2, 2]),
    };
    let deprecated = t.contains("Normalised");
    (deprecated || norm_coords, !deprecated && norm_coords, axes)
}

fn json_default(p: &Props, ptype: &str) -> Value {
    let (n, int, text) = param_shape(ptype);
    if text {
        return Value::String(p.get_str(s(kOfxParamPropDefault), 0).unwrap_or_default());
    }
    let v = p.get_f64s(s(kOfxParamPropDefault));
    if ptype == s(kOfxParamTypeBoolean) {
        return Value::Bool(v.first().copied().unwrap_or(0.0) != 0.0);
    }
    let num = |x: f64| if int { json!(x as i64) } else { json!(x) };
    if n == 1 {
        num(v.first().copied().unwrap_or(0.0))
    } else if n == 0 {
        Value::Null
    } else {
        Value::Array((0..n).map(|i| num(v.get(i).copied().unwrap_or(0.0))).collect())
    }
}

fn clamp_range(v: Vec<f64>) -> Vec<f64> {
    // Leave huge sentinel ranges out of the JSON (and out of the UI).
    if v.iter().all(|x| x.abs() >= 1e9) {
        vec![]
    } else {
        v
    }
}

/// Describe every plugin in a binary for the effects library.
pub fn scan_binary(path: &str) -> Result<Vec<PluginInfo>, String> {
    let b = load_binary(path)?;
    let mut out = vec![];
    for (i, raw) in b.plugins.iter().enumerate() {
        MESSAGES.lock().unwrap().clear();
        let i = i as u32;
        let id = unsafe { if raw.is_null() { String::new() } else { cs((**raw).pluginIdentifier) } };
        let p = match Plugin::open(*raw, i) {
            Ok(p) => p,
            Err(e) => {
                out.push(PluginInfo { index: i, id: id.clone(), version: [0, 0], label: id, grouping: String::new(), description: String::new(), contexts: vec![], context: None, depths: vec![], params: vec![], supported: false, reason: Some(e) });
                continue;
            }
        };
        let props = p.props();
        let contexts = props.get_strs(s(kOfxImageEffectPropSupportedContexts));
        let depths = props.get_strs(s(kOfxImageEffectPropSupportedPixelDepths));
        let mut info = PluginInfo {
            index: i,
            id: p.id.clone(),
            version: p.version,
            label: p.label(),
            grouping: props.get_str(s(kOfxImageEffectPluginPropGrouping), 0).unwrap_or_default(),
            description: props.get_str(s(kOfxPropPluginDescription), 0).unwrap_or_default(),
            contexts: contexts.clone(),
            context: p.pick_context(),
            depths,
            params: vec![],
            supported: false,
            reason: None,
        };
        let Some(ctx) = info.context.clone() else {
            info.reason = Some(format!("only offers {} (Pirosman hosts filter and general effects)", contexts.iter().map(|c| c.trim_start_matches("OfxImageEffectContext")).collect::<Vec<_>>().join(", ")));
            out.push(info);
            continue;
        };
        if p.pick_depth().is_none() {
            info.reason = Some("supports no 8-bit, 16-bit or float pixel depth".into());
            out.push(info);
            continue;
        }
        match p.describe_in(&ctx) {
            Err(e) => info.reason = Some(e),
            Ok(d) => {
                info.reason = check_clips(d);
                info.params = unsafe { params_of(d) }.into_iter().map(|(name, n)| param_info(&name, n)).collect();
                info.supported = info.reason.is_none();
            }
        }
        out.push(info);
    }
    Ok(out)
}

fn check_clips(d: *mut Node) -> Option<String> {
    let clips = unsafe { clips_of(d) };
    if !clips.iter().any(|(n, _)| n == s(kOfxImageEffectSimpleSourceClipName)) {
        return Some("defines no Source clip".into());
    }
    for (name, c) in &clips {
        let p = unsafe { &(**c).props };
        if name == s(kOfxImageEffectSimpleSourceClipName) || name == s(kOfxImageEffectOutputClipName) {
            let comps = p.get_strs(s(kOfxImageEffectPropSupportedComponents));
            if !comps.is_empty() && !comps.iter().any(|x| x == s(kOfxImageComponentRGBA)) {
                return Some(format!("its {name} clip does not accept RGBA images"));
            }
        } else if p.get_int(s(kOfxImageClipPropOptional), 0).unwrap_or(0) == 0 {
            return Some(format!("needs a second input ({name}), which Pirosman cannot connect"));
        }
    }
    None
}

fn param_info(name: &str, n: *mut Node) -> ParamInfo {
    let (p, ptype) = unsafe {
        let p = &(*n).props;
        let t = match &(*n).kind {
            Kind::Param(x) => x.ptype.clone(),
            _ => String::new(),
        };
        (p, t)
    };
    let (normalised, _, _) = spatial(p);
    ParamInfo {
        name: name.into(),
        label: p.get_str(s(kOfxPropLabel), 0).filter(|x| !x.is_empty()).unwrap_or_else(|| name.into()),
        kind: kind_name(&ptype).into(),
        default: json_default(p, &ptype),
        min: clamp_range(p.get_f64s(s(kOfxParamPropMin))),
        max: clamp_range(p.get_f64s(s(kOfxParamPropMax))),
        display_min: clamp_range(p.get_f64s(s(kOfxParamPropDisplayMin))),
        display_max: clamp_range(p.get_f64s(s(kOfxParamPropDisplayMax))),
        options: p.get_strs(s(kOfxParamPropChoiceOption)),
        hint: p.get_str(s(kOfxParamPropHint), 0).unwrap_or_default(),
        parent: p.get_str(s(kOfxParamPropParent), 0).unwrap_or_default(),
        secret: p.get_int(s(kOfxParamPropSecret), 0).unwrap_or(0) != 0,
        normalised,
    }
}

// ================================================================ instances and rendering

pub struct Instance {
    pub plugin: Plugin,
    pub node: *mut Node,
    pub depth: Depth,
    pub label: String,
    pub temporal: bool,
    out_buf: Vec<u8>,
    width: usize,
    height: usize,
}

fn value_from_json(v: &Value, ptype: &str) -> Option<PValue> {
    let (n, _, text) = param_shape(ptype);
    if text {
        return v.as_str().map(|x| PValue::Text(CString::new(x.replace('\0', "")).unwrap_or_default()));
    }
    let nums: Vec<f64> = match v {
        Value::Bool(b) => vec![if *b { 1.0 } else { 0.0 }],
        Value::Number(x) => vec![x.as_f64()?],
        Value::Array(a) => a.iter().map(|x| x.as_f64().or_else(|| x.as_bool().map(|b| b as i32 as f64))).collect::<Option<Vec<_>>>()?,
        _ => return None,
    };
    if nums.len() < n || nums.iter().any(|x| !x.is_finite()) {
        return None;
    }
    Some(PValue::Num(nums[..n].to_vec()))
}

impl Instance {
    pub fn create(plugin: Plugin, ctx: &str, user: &Map<String, Value>, width: usize, height: usize, fps: f64, frames: u32) -> Result<Instance, String> {
        let label = plugin.label();
        let depth = plugin.pick_depth().ok_or_else(|| format!("{label}: no supported pixel depth"))?;
        let desc = plugin.describe_in(ctx)?;
        if let Some(why) = check_clips(desc) {
            return Err(format!("{label}: {why}"));
        }
        let temporal = plugin.props().get_int(s(kOfxImageEffectPropTemporalClipAccess), 0).unwrap_or(0) != 0;
        let inst = new_effect();
        let (w, h) = (width as f64, height as f64);
        unsafe {
            let ip = &mut (*inst).props;
            *ip = (*desc).props.clone_values();
            ip.str(s(kOfxPropType), s(kOfxTypeImageEffectInstance));
            ip.str(s(kOfxImageEffectPropContext), ctx);
            ip.default_with(s(kOfxPropInstanceData), PVal::Ptr(vec![0]));
            ip.doubles(s(kOfxImageEffectPropProjectSize), &[w, h]);
            ip.doubles(s(kOfxImageEffectPropProjectOffset), &[0.0, 0.0]);
            ip.doubles(s(kOfxImageEffectPropProjectExtent), &[w, h]);
            ip.double(s(kOfxImageEffectPropProjectPixelAspectRatio), 1.0);
            ip.double(s(kOfxImageEffectInstancePropEffectDuration), frames as f64);
            ip.int(s(kOfxImageEffectInstancePropSequentialRender), 0);
            ip.double(s(kOfxImageEffectPropFrameRate), fps);
            ip.doubles(s(kOfxImageEffectPropFrameRange), &[0.0, (frames.max(1) - 1) as f64]);
            ip.int(s(kOfxPropIsInteractive), 0);

            // Parameters: copy each definition, then set its value.
            let Kind::Effect(ie) = &(*inst).kind else { unreachable!() };
            let Kind::ParamSet(ips) = &(*ie.param_set).kind else { unreachable!() };
            for (name, dn) in params_of(desc) {
                let Kind::Param(dp) = &(*dn).kind else { continue };
                let dprops = &(*dn).props;
                let (_, convert, axes) = spatial(dprops);
                let mut value = user.get(&name).and_then(|v| value_from_json(v, &dp.ptype)).unwrap_or_else(|| {
                    let (_, _, text) = param_shape(&dp.ptype);
                    if text {
                        PValue::Text(CString::new(dprops.get_str(s(kOfxParamPropDefault), 0).unwrap_or_default()).unwrap_or_default())
                    } else {
                        PValue::Num(dprops.get_f64s(s(kOfxParamPropDefault)))
                    }
                });
                if convert {
                    if let PValue::Num(v) = &mut value {
                        for (i, x) in v.iter_mut().enumerate() {
                            match axes.get(i) {
                                Some(0) => *x *= w,
                                Some(1) => *x *= h,
                                _ => {}
                            }
                        }
                    }
                }
                let pn = new_node(Kind::Param(Param { name: name.clone(), ptype: dp.ptype.clone(), value: Mutex::new(value) }));
                (*pn).props = dprops.clone_values();
                (*pn).props.str(s(kOfxPropType), s(kOfxTypeParameterInstance));
                ips.params.lock().unwrap().push((name, pn));
            }
            // Clips: Source is connected; optional extra inputs stay unconnected.
            for (name, dc) in clips_of(desc) {
                let connected = name == s(kOfxImageEffectSimpleSourceClipName) || name == s(kOfxImageEffectOutputClipName);
                let cn = new_node(Kind::Clip(Clip { name: name.clone(), effect: inst, connected }));
                let cp = &mut (*cn).props;
                *cp = (*dc).props.clone_values();
                cp.str(s(kOfxImageEffectPropPixelDepth), depth.name());
                cp.str(s(kOfxImageEffectPropComponents), s(kOfxImageComponentRGBA));
                cp.str(s(kOfxImageClipPropUnmappedPixelDepth), depth.name());
                cp.str(s(kOfxImageClipPropUnmappedComponents), s(kOfxImageComponentRGBA));
                let premult = if name == s(kOfxImageEffectOutputClipName) { s(kOfxImagePreMultiplied) } else { s(kOfxImageOpaque) };
                cp.str(s(kOfxImageEffectPropPreMultiplication), premult);
                cp.double(s(kOfxImagePropPixelAspectRatio), 1.0);
                cp.double(s(kOfxImageEffectPropFrameRate), fps);
                cp.doubles(s(kOfxImageEffectPropFrameRange), &[0.0, (frames.max(1) - 1) as f64]);
                cp.doubles(s(kOfxImageEffectPropUnmappedFrameRange), &[0.0, (frames.max(1) - 1) as f64]);
                cp.double(s(kOfxImageEffectPropUnmappedFrameRate), fps);
                cp.str(s(kOfxImageClipPropFieldOrder), s(kOfxImageFieldNone));
                cp.int(s(kOfxImageClipPropConnected), connected as i32);
                cp.int(s(kOfxImageClipPropContinuousSamples), 0);
                ie.clips.lock().unwrap().push((name, cn));
            }
        }
        let st = plugin.action(kOfxActionCreateInstance, inst, std::ptr::null_mut(), std::ptr::null_mut());
        if st != kOfxStatOK && st != kOfxStatReplyDefault {
            return Err(format!("{label}: create instance {}{}", status_text(st), last_message()));
        }
        // Clip preferences: the host keeps RGBA at one depth, so the answers are informational.
        let out = args();
        unsafe {
            let op = &(*out).props;
            for (name, _) in clips_of(inst) {
                op.str(&format!("OfxImageClipPropComponents_{name}"), s(kOfxImageComponentRGBA));
                op.str(&format!("OfxImageClipPropDepth_{name}"), depth.name());
                op.double(&format!("OfxImageClipPropPAR_{name}"), 1.0);
            }
            op.double(s(kOfxImageEffectPropFrameRate), fps);
            op.str(s(kOfxImageClipPropFieldOrder), s(kOfxImageFieldNone));
            op.str(s(kOfxImageEffectPropPreMultiplication), s(kOfxImagePreMultiplied));
            op.int(s(kOfxImageClipPropContinuousSamples), 0);
            op.int(s(kOfxImageEffectFrameVarying), 0);
        }
        plugin.action(kOfxImageEffectActionGetClipPreferences, inst, args(), out);
        Ok(Instance { plugin, node: inst, depth, label, temporal, out_buf: vec![0u8; width * height * 4 * depth.bytes()], width, height })
    }

    fn seq_args(&self, frames: u32) -> *mut Node {
        let a = args();
        let p = unsafe { &(*a).props };
        p.doubles(s(kOfxImageEffectPropFrameRange), &[0.0, (frames.max(1) - 1) as f64]);
        p.double(s(kOfxImageEffectPropFrameStep), 1.0);
        p.int(s(kOfxPropIsInteractive), 0);
        p.doubles(s(kOfxImageEffectPropRenderScale), &[1.0, 1.0]);
        p.int(s(kOfxImageEffectPropSequentialRenderStatus), 1);
        p.int(s(kOfxImageEffectPropInteractiveRenderStatus), 0);
        p.int(s(kOfxImageEffectPropRenderQualityDraft), 0);
        for k in [kOfxImageEffectPropOpenGLEnabled, kOfxImageEffectPropCudaEnabled, kOfxImageEffectPropOpenCLEnabled, kOfxImageEffectPropMetalEnabled] {
            p.int(s(k), 0);
        }
        a
    }

    pub fn begin(&self, frames: u32) -> Result<(), String> {
        let st = self.plugin.action(kOfxImageEffectActionBeginSequenceRender, self.node, self.seq_args(frames), std::ptr::null_mut());
        if st != kOfxStatOK && st != kOfxStatReplyDefault {
            return Err(format!("{}: begin render {}{}", self.label, status_text(st), last_message()));
        }
        Ok(())
    }

    pub fn end(&self, frames: u32) {
        self.plugin.action(kOfxImageEffectActionEndSequenceRender, self.node, self.seq_args(frames), std::ptr::null_mut());
        self.plugin.action(kOfxActionDestroyInstance, self.node, std::ptr::null_mut(), std::ptr::null_mut());
    }

    /// Render one frame. `source` returns the input frame nearest a time.
    pub fn render(&mut self, time: f64, source: &dyn Fn(f64) -> Frame) -> Result<Vec<f32>, String> {
        MESSAGES.lock().unwrap().clear();
        // SAFETY: the context outlives the render action; the pointer is cleared before return.
        let source_ptr: *const dyn Fn(f64) -> Frame = unsafe { std::mem::transmute::<&dyn Fn(f64) -> Frame, &'static dyn Fn(f64) -> Frame>(source) };
        let mut ctx = RenderCtx { width: self.width, height: self.height, depth: self.depth, time, source: source_ptr, output: self.out_buf.as_mut_ptr(), images: Mutex::new(vec![]) };
        let Kind::Effect(e) = (unsafe { &(*self.node).kind }) else { unreachable!() };
        e.render.store(&mut ctx, Ordering::SeqCst);
        let a = args();
        unsafe {
            let p = &(*a).props;
            p.double(s(kOfxPropTime), time);
            p.str(s(kOfxImageEffectPropFieldToRender), s(kOfxImageFieldNone));
            p.ints(s(kOfxImageEffectPropRenderWindow), &[0, 0, self.width as i32, self.height as i32]);
            p.doubles(s(kOfxImageEffectPropRenderScale), &[1.0, 1.0]);
            p.int(s(kOfxImageEffectPropSequentialRenderStatus), 1);
            p.int(s(kOfxImageEffectPropInteractiveRenderStatus), 0);
            p.int(s(kOfxImageEffectPropRenderQualityDraft), 0);
            for k in [kOfxImageEffectPropOpenGLEnabled, kOfxImageEffectPropCudaEnabled, kOfxImageEffectPropOpenCLEnabled, kOfxImageEffectPropMetalEnabled] {
                p.int(s(k), 0);
            }
        }
        let st = self.plugin.action(kOfxImageEffectActionRender, self.node, a, std::ptr::null_mut());
        e.render.store(std::ptr::null_mut(), Ordering::SeqCst);
        for img in ctx.images.lock().unwrap().drain(..) {
            unsafe { free_image(img) };
        }
        if st != kOfxStatOK {
            return Err(format!("{} could not render frame {time} ({}{})", self.label, status_text(st), last_message()));
        }
        Ok(unsafe { from_plugin(self.out_buf.as_ptr(), self.width, self.height, self.depth) })
    }
}

/// Loaded binaries and opened plugins: each plugin is loaded and described once.
#[derive(Default)]
pub struct Library {
    binaries: HashMap<String, Binary>,
    opened: HashMap<(String, usize), Plugin>,
}

impl Library {
    /// Open the plugin `id` from `binary`, trying `index` first.
    pub fn open(&mut self, binary: &str, index: u32, id: &str) -> Result<Plugin, String> {
        if !self.binaries.contains_key(binary) {
            let b = load_binary(binary)?;
            self.binaries.insert(binary.into(), b);
        }
        let b = &self.binaries[binary];
        let id_of = |raw: *mut OfxPlugin| unsafe { if raw.is_null() { String::new() } else { cs((*raw).pluginIdentifier) } };
        let i = if b.plugins.get(index as usize).map(|r| id_of(*r)) == Some(id.to_string()) {
            index as usize
        } else {
            b.plugins.iter().position(|r| id_of(*r) == id).ok_or_else(|| format!("{binary} no longer contains the plugin {id}"))?
        };
        if let Some(p) = self.opened.get(&(binary.to_string(), i)) {
            return Ok(p.clone());
        }
        let p = Plugin::open(b.plugins[i], i as u32)?;
        self.opened.insert((binary.to_string(), i), p.clone());
        Ok(p)
    }
}
