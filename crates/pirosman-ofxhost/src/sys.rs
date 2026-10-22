//! OpenFX 1.4 C ABI: the structs, suite tables and string constants a host
//! and a plugin share. Written from the published OpenFX headers (BSD-3).
//!
//! Variadic suite functions (`paramGetValue`, `message`, …) are declared by
//! the host as fixed functions taking 64-bit slots. On Windows x64 a variadic
//! call passes every argument in an 8-byte integer slot (doubles are copied
//! into the integer registers too), so the slots line up with what C plugins
//! push. Plugins written in Rust call them through the `...` pointer types.
#![allow(non_upper_case_globals, non_snake_case, dead_code)]

use std::ffi::{c_char, c_double, c_int, c_uint, c_void};

pub type OfxStatus = c_int;
pub type OfxTime = c_double;
pub type Handle = *mut c_void;

pub const kOfxStatOK: OfxStatus = 0;
pub const kOfxStatFailed: OfxStatus = 1;
pub const kOfxStatErrFatal: OfxStatus = 2;
pub const kOfxStatErrUnknown: OfxStatus = 3;
pub const kOfxStatErrMissingHostFeature: OfxStatus = 4;
pub const kOfxStatErrUnsupported: OfxStatus = 5;
pub const kOfxStatErrExists: OfxStatus = 6;
pub const kOfxStatErrFormat: OfxStatus = 7;
pub const kOfxStatErrMemory: OfxStatus = 8;
pub const kOfxStatErrBadHandle: OfxStatus = 9;
pub const kOfxStatErrBadIndex: OfxStatus = 10;
pub const kOfxStatErrValue: OfxStatus = 11;
pub const kOfxStatReplyYes: OfxStatus = 12;
pub const kOfxStatReplyNo: OfxStatus = 13;
pub const kOfxStatReplyDefault: OfxStatus = 14;

#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub struct OfxRectD {
    pub x1: f64,
    pub y1: f64,
    pub x2: f64,
    pub y2: f64,
}

#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub struct OfxRectI {
    pub x1: c_int,
    pub y1: c_int,
    pub x2: c_int,
    pub y2: c_int,
}

#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub struct OfxRangeD {
    pub min: f64,
    pub max: f64,
}

pub type FetchSuiteFn = unsafe extern "C" fn(host: Handle, name: *const c_char, version: c_int) -> *const c_void;

#[repr(C)]
pub struct OfxHost {
    pub host: Handle,
    pub fetchSuite: FetchSuiteFn,
}

pub type MainEntryFn = unsafe extern "C" fn(action: *const c_char, handle: *const c_void, in_args: Handle, out_args: Handle) -> OfxStatus;

#[repr(C)]
pub struct OfxPlugin {
    pub pluginApi: *const c_char,
    pub apiVersion: c_int,
    pub pluginIdentifier: *const c_char,
    pub pluginVersionMajor: c_uint,
    pub pluginVersionMinor: c_uint,
    pub setHost: unsafe extern "C" fn(host: *mut OfxHost),
    pub mainEntry: MainEntryFn,
}

pub type GetNumberOfPluginsFn = unsafe extern "C" fn() -> c_int;
pub type GetPluginFn = unsafe extern "C" fn(nth: c_int) -> *mut OfxPlugin;
pub type SetHostFn = unsafe extern "C" fn(host: *const OfxHost) -> OfxStatus;

#[repr(C)]
pub struct OfxPropertySuiteV1 {
    pub propSetPointer: unsafe extern "C" fn(Handle, *const c_char, c_int, *mut c_void) -> OfxStatus,
    pub propSetString: unsafe extern "C" fn(Handle, *const c_char, c_int, *const c_char) -> OfxStatus,
    pub propSetDouble: unsafe extern "C" fn(Handle, *const c_char, c_int, c_double) -> OfxStatus,
    pub propSetInt: unsafe extern "C" fn(Handle, *const c_char, c_int, c_int) -> OfxStatus,
    pub propSetPointerN: unsafe extern "C" fn(Handle, *const c_char, c_int, *const *mut c_void) -> OfxStatus,
    pub propSetStringN: unsafe extern "C" fn(Handle, *const c_char, c_int, *const *const c_char) -> OfxStatus,
    pub propSetDoubleN: unsafe extern "C" fn(Handle, *const c_char, c_int, *const c_double) -> OfxStatus,
    pub propSetIntN: unsafe extern "C" fn(Handle, *const c_char, c_int, *const c_int) -> OfxStatus,
    pub propGetPointer: unsafe extern "C" fn(Handle, *const c_char, c_int, *mut *mut c_void) -> OfxStatus,
    pub propGetString: unsafe extern "C" fn(Handle, *const c_char, c_int, *mut *mut c_char) -> OfxStatus,
    pub propGetDouble: unsafe extern "C" fn(Handle, *const c_char, c_int, *mut c_double) -> OfxStatus,
    pub propGetInt: unsafe extern "C" fn(Handle, *const c_char, c_int, *mut c_int) -> OfxStatus,
    pub propGetPointerN: unsafe extern "C" fn(Handle, *const c_char, c_int, *mut *mut c_void) -> OfxStatus,
    pub propGetStringN: unsafe extern "C" fn(Handle, *const c_char, c_int, *mut *mut c_char) -> OfxStatus,
    pub propGetDoubleN: unsafe extern "C" fn(Handle, *const c_char, c_int, *mut c_double) -> OfxStatus,
    pub propGetIntN: unsafe extern "C" fn(Handle, *const c_char, c_int, *mut c_int) -> OfxStatus,
    pub propReset: unsafe extern "C" fn(Handle, *const c_char) -> OfxStatus,
    pub propGetDimension: unsafe extern "C" fn(Handle, *const c_char, *mut c_int) -> OfxStatus,
}

#[repr(C)]
pub struct OfxImageEffectSuiteV1 {
    pub getPropertySet: unsafe extern "C" fn(Handle, *mut Handle) -> OfxStatus,
    pub getParamSet: unsafe extern "C" fn(Handle, *mut Handle) -> OfxStatus,
    pub clipDefine: unsafe extern "C" fn(Handle, *const c_char, *mut Handle) -> OfxStatus,
    pub clipGetHandle: unsafe extern "C" fn(Handle, *const c_char, *mut Handle, *mut Handle) -> OfxStatus,
    pub clipGetPropertySet: unsafe extern "C" fn(Handle, *mut Handle) -> OfxStatus,
    pub clipGetImage: unsafe extern "C" fn(Handle, OfxTime, *const OfxRectD, *mut Handle) -> OfxStatus,
    pub clipReleaseImage: unsafe extern "C" fn(Handle) -> OfxStatus,
    pub clipGetRegionOfDefinition: unsafe extern "C" fn(Handle, OfxTime, *mut OfxRectD) -> OfxStatus,
    pub abort: unsafe extern "C" fn(Handle) -> c_int,
    pub imageMemoryAlloc: unsafe extern "C" fn(Handle, usize, *mut Handle) -> OfxStatus,
    pub imageMemoryFree: unsafe extern "C" fn(Handle) -> OfxStatus,
    pub imageMemoryLock: unsafe extern "C" fn(Handle, *mut *mut c_void) -> OfxStatus,
    pub imageMemoryUnlock: unsafe extern "C" fn(Handle) -> OfxStatus,
}

/// Host side of the parameter suite: variadic arguments arrive as 64-bit slots.
#[repr(C)]
pub struct OfxParameterSuiteV1Host {
    pub paramDefine: unsafe extern "C" fn(Handle, *const c_char, *const c_char, *mut Handle) -> OfxStatus,
    pub paramGetHandle: unsafe extern "C" fn(Handle, *const c_char, *mut Handle, *mut Handle) -> OfxStatus,
    pub paramSetGetPropertySet: unsafe extern "C" fn(Handle, *mut Handle) -> OfxStatus,
    pub paramGetPropertySet: unsafe extern "C" fn(Handle, *mut Handle) -> OfxStatus,
    pub paramGetValue: unsafe extern "C" fn(Handle, u64, u64, u64, u64) -> OfxStatus,
    pub paramGetValueAtTime: unsafe extern "C" fn(Handle, OfxTime, u64, u64, u64, u64) -> OfxStatus,
    pub paramGetDerivative: unsafe extern "C" fn(Handle, OfxTime, u64, u64, u64, u64) -> OfxStatus,
    pub paramGetIntegral: unsafe extern "C" fn(Handle, OfxTime, OfxTime, u64, u64, u64, u64) -> OfxStatus,
    pub paramSetValue: unsafe extern "C" fn(Handle, u64, u64, u64, u64) -> OfxStatus,
    pub paramSetValueAtTime: unsafe extern "C" fn(Handle, OfxTime, u64, u64, u64, u64) -> OfxStatus,
    pub paramGetNumKeys: unsafe extern "C" fn(Handle, *mut c_uint) -> OfxStatus,
    pub paramGetKeyTime: unsafe extern "C" fn(Handle, c_uint, *mut OfxTime) -> OfxStatus,
    pub paramGetKeyIndex: unsafe extern "C" fn(Handle, OfxTime, c_int, *mut c_int) -> OfxStatus,
    pub paramDeleteKey: unsafe extern "C" fn(Handle, OfxTime) -> OfxStatus,
    pub paramDeleteAllKeys: unsafe extern "C" fn(Handle) -> OfxStatus,
    pub paramCopy: unsafe extern "C" fn(Handle, Handle, OfxTime, *const OfxRangeD) -> OfxStatus,
    pub paramEditBegin: unsafe extern "C" fn(Handle, *const c_char) -> OfxStatus,
    pub paramEditEnd: unsafe extern "C" fn(Handle) -> OfxStatus,
}

/// Plugin side of the parameter suite, with real C variadics.
#[repr(C)]
pub struct OfxParameterSuiteV1 {
    pub paramDefine: unsafe extern "C" fn(Handle, *const c_char, *const c_char, *mut Handle) -> OfxStatus,
    pub paramGetHandle: unsafe extern "C" fn(Handle, *const c_char, *mut Handle, *mut Handle) -> OfxStatus,
    pub paramSetGetPropertySet: unsafe extern "C" fn(Handle, *mut Handle) -> OfxStatus,
    pub paramGetPropertySet: unsafe extern "C" fn(Handle, *mut Handle) -> OfxStatus,
    pub paramGetValue: unsafe extern "C" fn(Handle, ...) -> OfxStatus,
    pub paramGetValueAtTime: unsafe extern "C" fn(Handle, OfxTime, ...) -> OfxStatus,
    pub paramGetDerivative: unsafe extern "C" fn(Handle, OfxTime, ...) -> OfxStatus,
    pub paramGetIntegral: unsafe extern "C" fn(Handle, OfxTime, OfxTime, ...) -> OfxStatus,
    pub paramSetValue: unsafe extern "C" fn(Handle, ...) -> OfxStatus,
    pub paramSetValueAtTime: unsafe extern "C" fn(Handle, OfxTime, ...) -> OfxStatus,
    pub paramGetNumKeys: unsafe extern "C" fn(Handle, *mut c_uint) -> OfxStatus,
    pub paramGetKeyTime: unsafe extern "C" fn(Handle, c_uint, *mut OfxTime) -> OfxStatus,
    pub paramGetKeyIndex: unsafe extern "C" fn(Handle, OfxTime, c_int, *mut c_int) -> OfxStatus,
    pub paramDeleteKey: unsafe extern "C" fn(Handle, OfxTime) -> OfxStatus,
    pub paramDeleteAllKeys: unsafe extern "C" fn(Handle) -> OfxStatus,
    pub paramCopy: unsafe extern "C" fn(Handle, Handle, OfxTime, *const OfxRangeD) -> OfxStatus,
    pub paramEditBegin: unsafe extern "C" fn(Handle, *const c_char) -> OfxStatus,
    pub paramEditEnd: unsafe extern "C" fn(Handle) -> OfxStatus,
}

#[repr(C)]
pub struct OfxMemorySuiteV1 {
    pub memoryAlloc: unsafe extern "C" fn(Handle, usize, *mut *mut c_void) -> OfxStatus,
    pub memoryFree: unsafe extern "C" fn(*mut c_void) -> OfxStatus,
}

pub type ThreadFn = unsafe extern "C" fn(index: c_uint, max: c_uint, arg: *mut c_void);

#[repr(C)]
pub struct OfxMultiThreadSuiteV1 {
    pub multiThread: unsafe extern "C" fn(ThreadFn, c_uint, *mut c_void) -> OfxStatus,
    pub multiThreadNumCPUs: unsafe extern "C" fn(*mut c_uint) -> OfxStatus,
    pub multiThreadIndex: unsafe extern "C" fn(*mut c_uint) -> OfxStatus,
    pub multiThreadIsSpawnedThread: unsafe extern "C" fn() -> c_int,
    pub mutexCreate: unsafe extern "C" fn(*mut Handle, c_int) -> OfxStatus,
    pub mutexDestroy: unsafe extern "C" fn(Handle) -> OfxStatus,
    pub mutexLock: unsafe extern "C" fn(Handle) -> OfxStatus,
    pub mutexUnLock: unsafe extern "C" fn(Handle) -> OfxStatus,
    pub mutexTryLock: unsafe extern "C" fn(Handle) -> OfxStatus,
}

/// Host side of the message suites: the printf arguments are ignored.
#[repr(C)]
pub struct OfxMessageSuiteV2Host {
    pub message: unsafe extern "C" fn(Handle, *const c_char, *const c_char, *const c_char) -> OfxStatus,
    pub setPersistentMessage: unsafe extern "C" fn(Handle, *const c_char, *const c_char, *const c_char) -> OfxStatus,
    pub clearPersistentMessage: unsafe extern "C" fn(Handle) -> OfxStatus,
}

#[repr(C)]
pub struct OfxInteractSuiteV1 {
    pub interactSwapBuffers: unsafe extern "C" fn(Handle) -> OfxStatus,
    pub interactRedraw: unsafe extern "C" fn(Handle) -> OfxStatus,
    pub interactGetPropertySet: unsafe extern "C" fn(Handle, *mut Handle) -> OfxStatus,
}

#[repr(C)]
pub struct OfxTimeLineSuiteV1 {
    pub getTime: unsafe extern "C" fn(Handle, *mut c_double) -> OfxStatus,
    pub gotoTime: unsafe extern "C" fn(Handle, c_double) -> OfxStatus,
    pub getTimeBounds: unsafe extern "C" fn(Handle, *mut c_double, *mut c_double) -> OfxStatus,
}

#[repr(C)]
pub struct OfxProgressSuiteV2 {
    pub progressStart: unsafe extern "C" fn(Handle, *const c_char, *const c_char) -> OfxStatus,
    pub progressUpdate: unsafe extern "C" fn(Handle, c_double) -> OfxStatus,
    pub progressEnd: unsafe extern "C" fn(Handle) -> OfxStatus,
}

// ---------------------------------------------------------------- strings

macro_rules! cstrs {
    ($($name:ident = $val:literal;)*) => { $(pub const $name: &str = concat!($val, "\0");)* };
}

cstrs! {
    kOfxImageEffectPluginApi = "OfxImageEffectPluginAPI";

    kOfxPropertySuite = "OfxPropertySuite";
    kOfxImageEffectSuite = "OfxImageEffectSuite";
    kOfxParameterSuite = "OfxParameterSuite";
    kOfxMemorySuite = "OfxMemorySuite";
    kOfxMultiThreadSuite = "OfxMultiThreadSuite";
    kOfxMessageSuite = "OfxMessageSuite";
    kOfxInteractSuite = "OfxInteractSuite";
    kOfxTimeLineSuite = "OfxTimeLineSuite";
    kOfxProgressSuite = "OfxProgressSuite";

    kOfxActionLoad = "OfxActionLoad";
    kOfxActionDescribe = "OfxActionDescribe";
    kOfxActionUnload = "OfxActionUnload";
    kOfxActionCreateInstance = "OfxActionCreateInstance";
    kOfxActionDestroyInstance = "OfxActionDestroyInstance";
    kOfxActionInstanceChanged = "OfxActionInstanceChanged";
    kOfxActionBeginInstanceChanged = "OfxActionBeginInstanceChanged";
    kOfxActionEndInstanceChanged = "OfxActionEndInstanceChanged";
    kOfxImageEffectActionDescribeInContext = "OfxImageEffectActionDescribeInContext";
    kOfxImageEffectActionGetClipPreferences = "OfxImageEffectActionGetClipPreferences";
    kOfxImageEffectActionGetRegionOfDefinition = "OfxImageEffectActionGetRegionOfDefinition";
    kOfxImageEffectActionBeginSequenceRender = "OfxImageEffectActionBeginSequenceRender";
    kOfxImageEffectActionRender = "OfxImageEffectActionRender";
    kOfxImageEffectActionEndSequenceRender = "OfxImageEffectActionEndSequenceRender";

    kOfxPropType = "OfxPropType";
    kOfxPropName = "OfxPropName";
    kOfxPropLabel = "OfxPropLabel";
    kOfxPropShortLabel = "OfxPropShortLabel";
    kOfxPropLongLabel = "OfxPropLongLabel";
    kOfxPropVersion = "OfxPropVersion";
    kOfxPropVersionLabel = "OfxPropVersionLabel";
    kOfxPropAPIVersion = "OfxPropAPIVersion";
    kOfxPropTime = "OfxPropTime";
    kOfxPropIsInteractive = "OfxPropIsInteractive";
    kOfxPropInstanceData = "OfxPropInstanceData";
    kOfxPropPluginDescription = "OfxPropPluginDescription";
    kOfxPropChangeReason = "OfxPropChangeReason";
    kOfxPropHostOSHandle = "OfxPropHostOSHandle";

    kOfxTypeImageEffectHost = "OfxTypeImageEffectHost";
    kOfxTypeImageEffect = "OfxTypeImageEffect";
    kOfxTypeImageEffectInstance = "OfxTypeImageEffectInstance";
    kOfxTypeClip = "OfxTypeClip";
    kOfxTypeImage = "OfxTypeImage";
    kOfxTypeParameter = "OfxTypeParameter";
    kOfxTypeParameterInstance = "OfxTypeParameterInstance";

    kOfxImageEffectPropSupportedContexts = "OfxImageEffectPropSupportedContexts";
    kOfxImageEffectPropContext = "OfxImageEffectPropContext";
    kOfxImageEffectPluginPropGrouping = "OfxImageEffectPluginPropGrouping";
    kOfxImageEffectPropSupportedPixelDepths = "OfxImageEffectPropSupportedPixelDepths";
    kOfxImageEffectPropSupportedComponents = "OfxImageEffectPropSupportedComponents";
    kOfxImageEffectPropPixelDepth = "OfxImageEffectPropPixelDepth";
    kOfxImageEffectPropComponents = "OfxImageEffectPropComponents";
    kOfxImageClipPropUnmappedPixelDepth = "OfxImageClipPropUnmappedPixelDepth";
    kOfxImageClipPropUnmappedComponents = "OfxImageClipPropUnmappedComponents";
    kOfxImageEffectPropPreMultiplication = "OfxImageEffectPropPreMultiplication";
    kOfxImagePropPixelAspectRatio = "OfxImagePropPixelAspectRatio";
    kOfxImageEffectPropFrameRate = "OfxImageEffectPropFrameRate";
    kOfxImageEffectPropFrameRange = "OfxImageEffectPropFrameRange";
    kOfxImageEffectPropUnmappedFrameRange = "OfxImageEffectPropUnmappedFrameRange";
    kOfxImageEffectPropUnmappedFrameRate = "OfxImageEffectPropUnmappedFrameRate";
    kOfxImageEffectPropFrameStep = "OfxImageEffectPropFrameStep";
    kOfxImageClipPropFieldOrder = "OfxImageClipPropFieldOrder";
    kOfxImageClipPropConnected = "OfxImageClipPropConnected";
    kOfxImageClipPropContinuousSamples = "OfxImageClipPropContinuousSamples";
    kOfxImageClipPropOptional = "OfxImageClipPropOptional";
    kOfxImageClipPropIsMask = "OfxImageClipPropIsMask";
    kOfxImageClipPropFieldExtraction = "OfxImageClipPropFieldExtraction";
    kOfxImageEffectPropTemporalClipAccess = "OfxImageEffectPropTemporalClipAccess";
    kOfxImageEffectPropSupportsTiles = "OfxImageEffectPropSupportsTiles";
    kOfxImageEffectPropSupportsMultiResolution = "OfxImageEffectPropSupportsMultiResolution";
    kOfxImageEffectPropSupportsMultipleClipDepths = "OfxImageEffectPropSupportsMultipleClipDepths";
    kOfxImageEffectPropSupportsMultipleClipPARs = "OfxImageEffectPropSupportsMultipleClipPARs";
    kOfxImageEffectPropSetableFrameRate = "OfxImageEffectPropSetableFrameRate";
    kOfxImageEffectPropSetableFielding = "OfxImageEffectPropSetableFielding";
    kOfxImageEffectPropSupportsOverlays = "OfxImageEffectPropSupportsOverlays";
    kOfxImageEffectHostPropIsBackground = "OfxImageEffectHostPropIsBackground";
    kOfxImageEffectPluginPropSingleInstance = "OfxImageEffectPluginPropSingleInstance";
    kOfxImageEffectPluginRenderThreadSafety = "OfxImageEffectPluginRenderThreadSafety";
    kOfxImageEffectPluginPropHostFrameThreading = "OfxImageEffectPluginPropHostFrameThreading";
    kOfxImageEffectPluginPropFieldRenderTwiceAlways = "OfxImageEffectPluginPropFieldRenderTwiceAlways";
    kOfxImageEffectPluginPropOverlayInteractV1 = "OfxImageEffectPluginPropOverlayInteractV1";
    kOfxImageEffectPropClipPreferencesSlaveParam = "OfxImageEffectPropClipPreferencesSlaveParam";
    kOfxImageEffectPropProjectSize = "OfxImageEffectPropProjectSize";
    kOfxImageEffectPropProjectOffset = "OfxImageEffectPropProjectOffset";
    kOfxImageEffectPropProjectExtent = "OfxImageEffectPropProjectExtent";
    kOfxImageEffectPropProjectPixelAspectRatio = "OfxImageEffectPropProjectPixelAspectRatio";
    kOfxImageEffectInstancePropEffectDuration = "OfxImageEffectInstancePropEffectDuration";
    kOfxImageEffectInstancePropSequentialRender = "OfxImageEffectInstancePropSequentialRender";
    kOfxImageEffectPropRenderScale = "OfxImageEffectPropRenderScale";
    kOfxImageEffectPropRenderWindow = "OfxImageEffectPropRenderWindow";
    kOfxImageEffectPropFieldToRender = "OfxImageEffectPropFieldToRender";
    kOfxImageEffectPropSequentialRenderStatus = "OfxImageEffectPropSequentialRenderStatus";
    kOfxImageEffectPropInteractiveRenderStatus = "OfxImageEffectPropInteractiveRenderStatus";
    kOfxImageEffectPropRenderQualityDraft = "OfxImageEffectPropRenderQualityDraft";
    kOfxImageEffectPropOpenGLRenderSupported = "OfxImageEffectPropOpenGLRenderSupported";
    kOfxImageEffectPropOpenGLEnabled = "OfxImageEffectPropOpenGLEnabled";
    kOfxImageEffectPropCudaRenderSupported = "OfxImageEffectPropCudaRenderSupported";
    kOfxImageEffectPropCudaEnabled = "OfxImageEffectPropCudaEnabled";
    kOfxImageEffectPropOpenCLRenderSupported = "OfxImageEffectPropOpenCLRenderSupported";
    kOfxImageEffectPropOpenCLEnabled = "OfxImageEffectPropOpenCLEnabled";
    kOfxImageEffectPropMetalRenderSupported = "OfxImageEffectPropMetalRenderSupported";
    kOfxImageEffectPropMetalEnabled = "OfxImageEffectPropMetalEnabled";
    kOfxImageEffectPropRegionOfDefinition = "OfxImageEffectPropRegionOfDefinition";
    kOfxImageEffectFrameVarying = "OfxImageEffectFrameVarying";

    kOfxImagePropData = "OfxImagePropData";
    kOfxImagePropBounds = "OfxImagePropBounds";
    kOfxImagePropRegionOfDefinition = "OfxImagePropRegionOfDefinition";
    kOfxImagePropRowBytes = "OfxImagePropRowBytes";
    kOfxImagePropField = "OfxImagePropField";
    kOfxImagePropUniqueIdentifier = "OfxImagePropUniqueIdentifier";

    kOfxImageComponentRGBA = "OfxImageComponentRGBA";
    kOfxImageComponentRGB = "OfxImageComponentRGB";
    kOfxImageComponentAlpha = "OfxImageComponentAlpha";
    kOfxBitDepthByte = "OfxBitDepthByte";
    kOfxBitDepthShort = "OfxBitDepthShort";
    kOfxBitDepthHalf = "OfxBitDepthHalf";
    kOfxBitDepthFloat = "OfxBitDepthFloat";
    kOfxImageOpaque = "OfxImageOpaque";
    kOfxImagePreMultiplied = "OfxImageAlphaPremultiplied";
    kOfxImageUnPreMultiplied = "OfxImageAlphaUnPremultiplied";
    kOfxImageFieldNone = "OfxFieldNone";

    kOfxImageEffectContextGenerator = "OfxImageEffectContextGenerator";
    kOfxImageEffectContextFilter = "OfxImageEffectContextFilter";
    kOfxImageEffectContextTransition = "OfxImageEffectContextTransition";
    kOfxImageEffectContextGeneral = "OfxImageEffectContextGeneral";
    kOfxImageEffectOutputClipName = "Output";
    kOfxImageEffectSimpleSourceClipName = "Source";
    kOfxImageEffectRenderUnsafe = "OfxImageEffectRenderUnsafe";
    kOfxImageEffectRenderInstanceSafe = "OfxImageEffectRenderInstanceSafe";
    kOfxImageEffectRenderFullySafe = "OfxImageEffectRenderFullySafe";

    kOfxParamTypeInteger = "OfxParamTypeInteger";
    kOfxParamTypeDouble = "OfxParamTypeDouble";
    kOfxParamTypeBoolean = "OfxParamTypeBoolean";
    kOfxParamTypeChoice = "OfxParamTypeChoice";
    kOfxParamTypeStrChoice = "OfxParamTypeStrChoice";
    kOfxParamTypeRGBA = "OfxParamTypeRGBA";
    kOfxParamTypeRGB = "OfxParamTypeRGB";
    kOfxParamTypeDouble2D = "OfxParamTypeDouble2D";
    kOfxParamTypeInteger2D = "OfxParamTypeInteger2D";
    kOfxParamTypeDouble3D = "OfxParamTypeDouble3D";
    kOfxParamTypeInteger3D = "OfxParamTypeInteger3D";
    kOfxParamTypeString = "OfxParamTypeString";
    kOfxParamTypeCustom = "OfxParamTypeCustom";
    kOfxParamTypeBytes = "OfxParamTypeBytes";
    kOfxParamTypeGroup = "OfxParamTypeGroup";
    kOfxParamTypePage = "OfxParamTypePage";
    kOfxParamTypePushButton = "OfxParamTypePushButton";
    kOfxParamTypeParametric = "OfxParamTypeParametric";

    kOfxParamPropType = "OfxParamPropType";
    kOfxParamPropDefault = "OfxParamPropDefault";
    kOfxParamPropMin = "OfxParamPropMin";
    kOfxParamPropMax = "OfxParamPropMax";
    kOfxParamPropDisplayMin = "OfxParamPropDisplayMin";
    kOfxParamPropDisplayMax = "OfxParamPropDisplayMax";
    kOfxParamPropChoiceOption = "OfxParamPropChoiceOption";
    kOfxParamPropHint = "OfxParamPropHint";
    kOfxParamPropParent = "OfxParamPropParent";
    kOfxParamPropSecret = "OfxParamPropSecret";
    kOfxParamPropEnabled = "OfxParamPropEnabled";
    kOfxParamPropAnimates = "OfxParamPropAnimates";
    kOfxParamPropIsAnimating = "OfxParamPropIsAnimating";
    kOfxParamPropIsAutoKeying = "OfxParamPropIsAutoKeying";
    kOfxParamPropPersistant = "OfxParamPropPersistant";
    kOfxParamPropEvaluateOnChange = "OfxParamPropEvaluateOnChange";
    kOfxParamPropPluginMayWrite = "OfxParamPropPluginMayWrite";
    kOfxParamPropCanUndo = "OfxParamPropCanUndo";
    kOfxParamPropScriptName = "OfxParamPropScriptName";
    kOfxParamPropDoubleType = "OfxParamPropDoubleType";
    kOfxParamPropDefaultCoordinateSystem = "OfxParamPropDefaultCoordinateSystem";
    kOfxParamPropDigits = "OfxParamPropDigits";
    kOfxParamPropIncrement = "OfxParamPropIncrement";
    kOfxParamPropDimensionLabel = "OfxParamPropDimensionLabel";
    kOfxParamPropStringMode = "OfxParamPropStringMode";
    kOfxParamPropDataPtr = "OfxParamPropDataPtr";
    kOfxParamPropGroupOpen = "OfxParamPropGroupOpen";
    kOfxParamPropPageChild = "OfxParamPropPageChild";
    kOfxParamPropHasHostOverlayHandle = "OfxParamPropHasHostOverlayHandle";
    kOfxParamPropUseHostOverlayHandle = "OfxParamPropUseHostOverlayHandle";
    kOfxParamPropCacheInvalidation = "OfxParamPropCacheInvalidation";
    kOfxParamPropShowTimeMarker = "OfxParamPropShowTimeMarker";
    kOfxParamPropStringFilePathExists = "OfxParamPropStringFilePathExists";
    kOfxParamPropInteractV1 = "OfxParamPropInteractV1";
    kOfxParamPropInteractSize = "OfxParamPropInteractSize";
    kOfxParamPropInteractSizeAspect = "OfxParamPropInteractSizeAspect";
    kOfxParamPropInteractMinimumSize = "OfxParamPropInteractMinimumSize";
    kOfxParamPropInteractPreferedSize = "OfxParamPropInteractPreferedSize";
    kOfxParamPropCustomInterpCallbackV1 = "OfxParamPropCustomCallbackV1";
    kOfxParamPropChoiceOrder = "OfxParamPropChoiceOrder";
    kOfxParamPropChoiceEnum = "OfxParamPropChoiceEnum";

    kOfxParamCoordinatesCanonical = "OfxParamCoordinatesCanonical";
    kOfxParamCoordinatesNormalised = "OfxParamCoordinatesNormalised";
    kOfxParamDoubleTypePlain = "OfxParamDoubleTypePlain";
    kOfxParamStringIsSingleLine = "OfxParamStringIsSingleLine";
    kOfxParamInvalidateAll = "OfxParamInvalidateAll";

    kOfxParamHostPropSupportsCustomInteract = "OfxParamHostPropSupportsCustomInteract";
    kOfxParamHostPropSupportsStringAnimation = "OfxParamHostPropSupportsStringAnimation";
    kOfxParamHostPropSupportsChoiceAnimation = "OfxParamHostPropSupportsChoiceAnimation";
    kOfxParamHostPropSupportsStrChoiceAnimation = "OfxParamHostPropSupportsStrChoiceAnimation";
    kOfxParamHostPropSupportsStrChoice = "OfxParamHostPropSupportsStrChoice";
    kOfxParamHostPropSupportsBooleanAnimation = "OfxParamHostPropSupportsBooleanAnimation";
    kOfxParamHostPropSupportsCustomAnimation = "OfxParamHostPropSupportsCustomAnimation";
    kOfxParamHostPropSupportsParametricAnimation = "OfxParamHostPropSupportsParametricAnimation";
    kOfxParamHostPropMaxParameters = "OfxParamHostPropMaxParameters";
    kOfxParamHostPropMaxPages = "OfxParamHostPropMaxPages";
    kOfxParamHostPropPageRowColumnCount = "OfxParamHostPropPageRowColumnCount";
    kOfxPropHostOSHandleProp = "OfxPropHostOSHandle";
}

/// A `&'static str` constant from this module as a C pointer.
pub fn c(s: &'static str) -> *const c_char {
    debug_assert!(s.ends_with('\0'));
    s.as_ptr() as *const c_char
}

/// The constant without its trailing NUL.
pub fn s(k: &'static str) -> &'static str {
    k.trim_end_matches('\0')
}
