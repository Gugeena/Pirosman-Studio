//! "Pirosman Samples": a small OpenFX plugin bundle shipped with Pirosman Studio.
//!
//! It gives every install a few OpenFX effects to start with and is the end-to-end
//! test of the host: four filter effects covering double, integer, boolean,
//! choice and RGB parameters, all three pixel depths, and temporal clip access.
//! It talks to the host only through the OpenFX C ABI, like any third-party plugin.
#![allow(non_snake_case, clippy::missing_safety_doc)]

use std::ffi::{c_char, c_double, c_int, c_void, CStr};
use std::sync::atomic::{AtomicPtr, Ordering};
use std::sync::OnceLock;

use pirosman_ofxhost::sys::*;

static HOST: AtomicPtr<OfxHost> = AtomicPtr::new(std::ptr::null_mut());
static PROP: AtomicPtr<OfxPropertySuiteV1> = AtomicPtr::new(std::ptr::null_mut());
static IE: AtomicPtr<OfxImageEffectSuiteV1> = AtomicPtr::new(std::ptr::null_mut());
static PARAM: AtomicPtr<OfxParameterSuiteV1> = AtomicPtr::new(std::ptr::null_mut());

fn prop() -> &'static OfxPropertySuiteV1 {
    unsafe { &*PROP.load(Ordering::Acquire) }
}
fn ie() -> &'static OfxImageEffectSuiteV1 {
    unsafe { &*IE.load(Ordering::Acquire) }
}
fn param() -> &'static OfxParameterSuiteV1 {
    unsafe { &*PARAM.load(Ordering::Acquire) }
}

#[derive(Clone, Copy, PartialEq)]
enum Fx {
    Invert,
    Tint,
    Posterize,
    Echo,
}

const ALL: [Fx; 4] = [Fx::Invert, Fx::Tint, Fx::Posterize, Fx::Echo];

impl Fx {
    fn id(self) -> &'static str {
        match self {
            Fx::Invert => "studio.pirosman.samples.Invert\0",
            Fx::Tint => "studio.pirosman.samples.Tint\0",
            Fx::Posterize => "studio.pirosman.samples.Posterize\0",
            Fx::Echo => "studio.pirosman.samples.Echo\0",
        }
    }
    fn label(self) -> &'static str {
        match self {
            Fx::Invert => "Invert\0",
            Fx::Tint => "Tint\0",
            Fx::Posterize => "Posterize\0",
            Fx::Echo => "Echo\0",
        }
    }
    fn grouping(self) -> &'static str {
        match self {
            Fx::Echo => "Pirosman Samples/Time\0",
            _ => "Pirosman Samples/Color\0",
        }
    }
}

struct Table(Vec<OfxPlugin>);
unsafe impl Send for Table {}
unsafe impl Sync for Table {}

unsafe extern "C" fn set_host(h: *mut OfxHost) {
    HOST.store(h, Ordering::Release);
}

fn plugins() -> &'static Table {
    static T: OnceLock<Table> = OnceLock::new();
    T.get_or_init(|| {
        Table(
            ALL.iter()
                .map(|fx| OfxPlugin {
                    pluginApi: c(kOfxImageEffectPluginApi),
                    apiVersion: 1,
                    pluginIdentifier: fx.id().as_ptr() as *const c_char,
                    pluginVersionMajor: 1,
                    pluginVersionMinor: 0,
                    setHost: set_host,
                    mainEntry: match fx {
                        Fx::Invert => entry_invert,
                        Fx::Tint => entry_tint,
                        Fx::Posterize => entry_posterize,
                        Fx::Echo => entry_echo,
                    },
                })
                .collect(),
        )
    })
}

#[no_mangle]
pub extern "C" fn OfxGetNumberOfPlugins() -> c_int {
    ALL.len() as c_int
}

#[no_mangle]
pub extern "C" fn OfxGetPlugin(nth: c_int) -> *mut OfxPlugin {
    match plugins().0.get(nth as usize) {
        Some(p) => p as *const OfxPlugin as *mut OfxPlugin,
        None => std::ptr::null_mut(),
    }
}

unsafe extern "C" fn entry_invert(a: *const c_char, h: *const c_void, i: Handle, o: Handle) -> OfxStatus {
    entry(Fx::Invert, a, h as Handle, i, o)
}
unsafe extern "C" fn entry_tint(a: *const c_char, h: *const c_void, i: Handle, o: Handle) -> OfxStatus {
    entry(Fx::Tint, a, h as Handle, i, o)
}
unsafe extern "C" fn entry_posterize(a: *const c_char, h: *const c_void, i: Handle, o: Handle) -> OfxStatus {
    entry(Fx::Posterize, a, h as Handle, i, o)
}
unsafe extern "C" fn entry_echo(a: *const c_char, h: *const c_void, i: Handle, o: Handle) -> OfxStatus {
    entry(Fx::Echo, a, h as Handle, i, o)
}

unsafe fn fetch<T>(name: &'static str, v: c_int) -> *mut T {
    let h = HOST.load(Ordering::Acquire);
    if h.is_null() {
        return std::ptr::null_mut();
    }
    ((*h).fetchSuite)((*h).host, c(name), v) as *mut T
}

unsafe fn entry(fx: Fx, action: *const c_char, h: Handle, in_args: Handle, _out: Handle) -> OfxStatus {
    let action = CStr::from_ptr(action).to_str().unwrap_or("");
    match action {
        a if a == s(kOfxActionLoad) => {
            PROP.store(fetch(kOfxPropertySuite, 1), Ordering::Release);
            IE.store(fetch(kOfxImageEffectSuite, 1), Ordering::Release);
            PARAM.store(fetch(kOfxParameterSuite, 1), Ordering::Release);
            if PROP.load(Ordering::Acquire).is_null() || IE.load(Ordering::Acquire).is_null() || PARAM.load(Ordering::Acquire).is_null() {
                kOfxStatErrMissingHostFeature
            } else {
                kOfxStatOK
            }
        }
        a if a == s(kOfxActionDescribe) => describe(fx, h),
        a if a == s(kOfxImageEffectActionDescribeInContext) => describe_in_context(fx, h),
        a if a == s(kOfxImageEffectActionRender) => render(fx, h, in_args),
        a if a == s(kOfxActionUnload) || a == s(kOfxActionCreateInstance) || a == s(kOfxActionDestroyInstance) => kOfxStatOK,
        _ => kOfxStatReplyDefault,
    }
}

unsafe fn describe(fx: Fx, h: Handle) -> OfxStatus {
    let mut props: Handle = std::ptr::null_mut();
    (ie().getPropertySet)(h, &mut props);
    let p = prop();
    (p.propSetString)(props, c(kOfxPropLabel), 0, fx.label().as_ptr() as *const c_char);
    (p.propSetString)(props, c(kOfxImageEffectPluginPropGrouping), 0, fx.grouping().as_ptr() as *const c_char);
    (p.propSetString)(props, c(kOfxImageEffectPropSupportedContexts), 0, c(kOfxImageEffectContextFilter));
    (p.propSetString)(props, c(kOfxImageEffectPropSupportedContexts), 1, c(kOfxImageEffectContextGeneral));
    for (i, d) in [kOfxBitDepthByte, kOfxBitDepthShort, kOfxBitDepthFloat].iter().enumerate() {
        (p.propSetString)(props, c(kOfxImageEffectPropSupportedPixelDepths), i as c_int, c(d));
    }
    (p.propSetInt)(props, c(kOfxImageEffectPropSupportsTiles), 0, 0);
    (p.propSetInt)(props, c(kOfxImageEffectPropTemporalClipAccess), 0, (fx == Fx::Echo) as c_int);
    (p.propSetString)(props, c(kOfxImageEffectPluginRenderThreadSafety), 0, c(kOfxImageEffectRenderFullySafe));
    kOfxStatOK
}

unsafe fn define(set: Handle, ptype: &'static str, name: &'static str, label: &'static str) -> Handle {
    let mut props: Handle = std::ptr::null_mut();
    (param().paramDefine)(set, c(ptype), c(name), &mut props);
    (prop().propSetString)(props, c(kOfxPropLabel), 0, c(label));
    props
}

unsafe fn describe_in_context(fx: Fx, h: Handle) -> OfxStatus {
    let p = prop();
    for clip in [kOfxImageEffectSimpleSourceClipName, kOfxImageEffectOutputClipName] {
        let mut cp: Handle = std::ptr::null_mut();
        (ie().clipDefine)(h, c(clip), &mut cp);
        (p.propSetString)(cp, c(kOfxImageEffectPropSupportedComponents), 0, c(kOfxImageComponentRGBA));
        if fx == Fx::Echo && clip == kOfxImageEffectSimpleSourceClipName {
            (p.propSetInt)(cp, c(kOfxImageEffectPropTemporalClipAccess), 0, 1);
        }
    }
    let mut set: Handle = std::ptr::null_mut();
    (ie().getParamSet)(h, &mut set);
    match fx {
        Fx::Invert => {
            let m = define(set, kOfxParamTypeDouble, "mix\0", "Mix\0");
            (p.propSetDouble)(m, c(kOfxParamPropDefault), 0, 1.0);
            (p.propSetDouble)(m, c(kOfxParamPropMin), 0, 0.0);
            (p.propSetDouble)(m, c(kOfxParamPropMax), 0, 1.0);
            (p.propSetDouble)(m, c(kOfxParamPropDisplayMin), 0, 0.0);
            (p.propSetDouble)(m, c(kOfxParamPropDisplayMax), 0, 1.0);
            let a = define(set, kOfxParamTypeBoolean, "alpha\0", "Invert alpha\0");
            (p.propSetInt)(a, c(kOfxParamPropDefault), 0, 0);
        }
        Fx::Tint => {
            let col = define(set, kOfxParamTypeRGB, "color\0", "Color\0");
            for (i, v) in [1.0, 0.55, 0.2].iter().enumerate() {
                (p.propSetDouble)(col, c(kOfxParamPropDefault), i as c_int, *v);
            }
            let a = define(set, kOfxParamTypeDouble, "amount\0", "Amount\0");
            (p.propSetDouble)(a, c(kOfxParamPropDefault), 0, 0.5);
            (p.propSetDouble)(a, c(kOfxParamPropMin), 0, 0.0);
            (p.propSetDouble)(a, c(kOfxParamPropMax), 0, 1.0);
            (p.propSetDouble)(a, c(kOfxParamPropDisplayMin), 0, 0.0);
            (p.propSetDouble)(a, c(kOfxParamPropDisplayMax), 0, 1.0);
            let m = define(set, kOfxParamTypeChoice, "mode\0", "Blend\0");
            (p.propSetString)(m, c(kOfxParamPropChoiceOption), 0, c("Multiply\0"));
            (p.propSetString)(m, c(kOfxParamPropChoiceOption), 1, c("Screen\0"));
            (p.propSetInt)(m, c(kOfxParamPropDefault), 0, 0);
        }
        Fx::Posterize => {
            let l = define(set, kOfxParamTypeInteger, "levels\0", "Levels\0");
            (p.propSetInt)(l, c(kOfxParamPropDefault), 0, 6);
            (p.propSetInt)(l, c(kOfxParamPropMin), 0, 2);
            (p.propSetInt)(l, c(kOfxParamPropMax), 0, 64);
            (p.propSetInt)(l, c(kOfxParamPropDisplayMin), 0, 2);
            (p.propSetInt)(l, c(kOfxParamPropDisplayMax), 0, 16);
        }
        Fx::Echo => {
            let d = define(set, kOfxParamTypeDouble, "decay\0", "Echo strength\0");
            (p.propSetDouble)(d, c(kOfxParamPropDefault), 0, 0.6);
            (p.propSetDouble)(d, c(kOfxParamPropMin), 0, 0.0);
            (p.propSetDouble)(d, c(kOfxParamPropMax), 0, 1.0);
            (p.propSetDouble)(d, c(kOfxParamPropDisplayMin), 0, 0.0);
            (p.propSetDouble)(d, c(kOfxParamPropDisplayMax), 0, 1.0);
            let o = define(set, kOfxParamTypeInteger, "offset\0", "Frames back\0");
            (p.propSetInt)(o, c(kOfxParamPropDefault), 0, 2);
            (p.propSetInt)(o, c(kOfxParamPropMin), 0, 1);
            (p.propSetInt)(o, c(kOfxParamPropMax), 0, 4);
        }
    }
    kOfxStatOK
}

struct Img {
    props: Handle,
    data: *mut u8,
    bounds: [c_int; 4],
    row_bytes: isize,
    bytes: usize,
}

impl Img {
    unsafe fn fetch(clip: Handle, t: f64) -> Option<Img> {
        let mut props: Handle = std::ptr::null_mut();
        if (ie().clipGetImage)(clip, t, std::ptr::null(), &mut props) != kOfxStatOK {
            return None;
        }
        let p = prop();
        let mut data: *mut c_void = std::ptr::null_mut();
        (p.propGetPointer)(props, c(kOfxImagePropData), 0, &mut data);
        let mut b = [0 as c_int; 4];
        (p.propGetIntN)(props, c(kOfxImagePropBounds), 4, b.as_mut_ptr());
        let mut rb: c_int = 0;
        (p.propGetInt)(props, c(kOfxImagePropRowBytes), 0, &mut rb);
        let mut depth: *mut c_char = std::ptr::null_mut();
        (p.propGetString)(props, c(kOfxImageEffectPropPixelDepth), 0, &mut depth);
        let depth = CStr::from_ptr(depth).to_str().unwrap_or("");
        let bytes = if depth == s(kOfxBitDepthFloat) {
            4
        } else if depth == s(kOfxBitDepthShort) {
            2
        } else {
            1
        };
        Some(Img { props, data: data as *mut u8, bounds: b, row_bytes: rb as isize, bytes })
    }
    unsafe fn px(&self, x: c_int, y: c_int) -> *mut u8 {
        let x = x.clamp(self.bounds[0], self.bounds[2] - 1);
        let y = y.clamp(self.bounds[1], self.bounds[3] - 1);
        self.data.offset((y - self.bounds[1]) as isize * self.row_bytes + ((x - self.bounds[0]) as usize * 4 * self.bytes) as isize)
    }
    unsafe fn get(&self, x: c_int, y: c_int) -> [f32; 4] {
        let p = self.px(x, y);
        let mut o = [0f32; 4];
        for (i, v) in o.iter_mut().enumerate() {
            *v = match self.bytes {
                4 => (p as *const f32).add(i).read_unaligned(),
                2 => (p as *const u16).add(i).read_unaligned() as f32 / 65535.0,
                _ => *p.add(i) as f32 / 255.0,
            };
        }
        o
    }
    unsafe fn set(&self, x: c_int, y: c_int, v: [f32; 4]) {
        let p = self.px(x, y);
        for (i, v) in v.iter().enumerate() {
            match self.bytes {
                4 => (p as *mut f32).add(i).write_unaligned(*v),
                2 => (p as *mut u16).add(i).write_unaligned((v.clamp(0.0, 1.0) * 65535.0 + 0.5) as u16),
                _ => *p.add(i) = (v.clamp(0.0, 1.0) * 255.0 + 0.5) as u8,
            }
        }
    }
    unsafe fn release(self) {
        (ie().clipReleaseImage)(self.props);
    }
}

unsafe fn param_handle(set: Handle, name: &'static str) -> Handle {
    let mut h: Handle = std::ptr::null_mut();
    (param().paramGetHandle)(set, c(name), &mut h, std::ptr::null_mut());
    h
}
unsafe fn double(set: Handle, name: &'static str, t: f64) -> f64 {
    let mut v: c_double = 0.0;
    (param().paramGetValueAtTime)(param_handle(set, name), t, &mut v as *mut c_double);
    v
}
unsafe fn int(set: Handle, name: &'static str, t: f64) -> i32 {
    let mut v: c_int = 0;
    (param().paramGetValueAtTime)(param_handle(set, name), t, &mut v as *mut c_int);
    v
}

unsafe fn render(fx: Fx, h: Handle, in_args: Handle) -> OfxStatus {
    let p = prop();
    let mut t: c_double = 0.0;
    (p.propGetDouble)(in_args, c(kOfxPropTime), 0, &mut t);
    let mut win = [0 as c_int; 4];
    (p.propGetIntN)(in_args, c(kOfxImageEffectPropRenderWindow), 4, win.as_mut_ptr());
    let (mut src_clip, mut out_clip): (Handle, Handle) = (std::ptr::null_mut(), std::ptr::null_mut());
    (ie().clipGetHandle)(h, c(kOfxImageEffectSimpleSourceClipName), &mut src_clip, std::ptr::null_mut());
    (ie().clipGetHandle)(h, c(kOfxImageEffectOutputClipName), &mut out_clip, std::ptr::null_mut());
    let mut set: Handle = std::ptr::null_mut();
    (ie().getParamSet)(h, &mut set);
    let (Some(src), Some(dst)) = (Img::fetch(src_clip, t), Img::fetch(out_clip, t)) else { return kOfxStatFailed };
    let earlier = if fx == Fx::Echo { Img::fetch(src_clip, t - int(set, "offset\0", t) as f64) } else { None };
    match fx {
        Fx::Invert => {
            let mix = double(set, "mix\0", t) as f32;
            let alpha = int(set, "alpha\0", t) != 0;
            for y in win[1]..win[3] {
                for x in win[0]..win[2] {
                    let v = src.get(x, y);
                    let inv = |c: f32| mix * (1.0 - c) + (1.0 - mix) * c;
                    dst.set(x, y, [inv(v[0]), inv(v[1]), inv(v[2]), if alpha { 1.0 - v[3] } else { v[3] }]);
                }
            }
        }
        Fx::Tint => {
            let mut col = [0f64; 3];
            (param().paramGetValueAtTime)(param_handle(set, "color\0"), t, &mut col[0] as *mut f64, &mut col[1] as *mut f64, &mut col[2] as *mut f64);
            let amount = double(set, "amount\0", t) as f32;
            let screen = int(set, "mode\0", t) == 1;
            for y in win[1]..win[3] {
                for x in win[0]..win[2] {
                    let v = src.get(x, y);
                    let mut o = v;
                    for i in 0..3 {
                        let k = col[i] as f32;
                        let tinted = if screen { 1.0 - (1.0 - v[i]) * (1.0 - k) } else { v[i] * k };
                        o[i] = v[i] + (tinted - v[i]) * amount;
                    }
                    dst.set(x, y, o);
                }
            }
        }
        Fx::Posterize => {
            let n = int(set, "levels\0", t).clamp(2, 64) as f32;
            for y in win[1]..win[3] {
                for x in win[0]..win[2] {
                    let v = src.get(x, y);
                    let q = |c: f32| ((c.clamp(0.0, 1.0) * n).floor().min(n - 1.0)) / (n - 1.0);
                    dst.set(x, y, [q(v[0]), q(v[1]), q(v[2]), v[3]]);
                }
            }
        }
        Fx::Echo => {
            let decay = double(set, "decay\0", t) as f32;
            for y in win[1]..win[3] {
                for x in win[0]..win[2] {
                    let v = src.get(x, y);
                    let e = earlier.as_ref().map(|i| i.get(x, y)).unwrap_or(v);
                    let mix = |a: f32, b: f32| (a + decay * b) / (1.0 + decay);
                    dst.set(x, y, [mix(v[0], e[0]), mix(v[1], e[1]), mix(v[2], e[2]), v[3]]);
                }
            }
        }
    }
    if let Some(e) = earlier {
        e.release();
    }
    src.release();
    dst.release();
    kOfxStatOK
}
