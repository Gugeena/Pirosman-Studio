//! OpenFX property sets: named, typed, multi-dimensional values.
//!
//! Plugins read and write host-owned properties through the property suite.
//! Strings handed out stay valid until that property is next written (the
//! `CString` heap buffer does not move when the map rehashes).

use std::collections::HashMap;
use std::ffi::{c_void, CStr, CString};
use std::sync::Mutex;

#[derive(Debug, Clone)]
pub enum PVal {
    Int(Vec<i32>),
    Double(Vec<f64>),
    Str(Vec<CString>),
    Ptr(Vec<usize>),
}

impl PVal {
    pub fn len(&self) -> usize {
        match self {
            PVal::Int(v) => v.len(),
            PVal::Double(v) => v.len(),
            PVal::Str(v) => v.len(),
            PVal::Ptr(v) => v.len(),
        }
    }
}

#[derive(Debug)]
pub enum PropError {
    Unknown,
    BadIndex,
    Type,
}

#[derive(Debug, Default)]
pub struct Props {
    map: Mutex<HashMap<String, PVal>>,
}

fn cstring(s: &str) -> CString {
    CString::new(s.replace('\0', "")).unwrap_or_default()
}

impl Props {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn clone_values(&self) -> Self {
        Self { map: Mutex::new(self.map.lock().unwrap().clone()) }
    }

    pub fn has(&self, key: &str) -> bool {
        self.map.lock().unwrap().contains_key(key)
    }

    // ------------------------------------------------ whole-value helpers (host side)

    pub fn ints(&self, key: &str, v: &[i32]) {
        self.map.lock().unwrap().insert(key.into(), PVal::Int(v.to_vec()));
    }
    pub fn int(&self, key: &str, v: i32) {
        self.ints(key, &[v]);
    }
    pub fn doubles(&self, key: &str, v: &[f64]) {
        self.map.lock().unwrap().insert(key.into(), PVal::Double(v.to_vec()));
    }
    pub fn double(&self, key: &str, v: f64) {
        self.doubles(key, &[v]);
    }
    pub fn strs(&self, key: &str, v: &[&str]) {
        self.map.lock().unwrap().insert(key.into(), PVal::Str(v.iter().map(|s| cstring(s)).collect()));
    }
    pub fn str(&self, key: &str, v: &str) {
        self.strs(key, &[v]);
    }
    pub fn ptr(&self, key: &str, v: *mut c_void) {
        self.map.lock().unwrap().insert(key.into(), PVal::Ptr(vec![v as usize]));
    }
    /// Set only when the plugin has not set the property.
    pub fn default_with(&self, key: &str, v: PVal) {
        self.map.lock().unwrap().entry(key.into()).or_insert(v);
    }

    pub fn get(&self, key: &str) -> Option<PVal> {
        self.map.lock().unwrap().get(key).cloned()
    }
    pub fn get_str(&self, key: &str, i: usize) -> Option<String> {
        match self.map.lock().unwrap().get(key) {
            Some(PVal::Str(v)) => v.get(i).map(|s| s.to_string_lossy().into_owned()),
            _ => None,
        }
    }
    pub fn get_strs(&self, key: &str) -> Vec<String> {
        match self.map.lock().unwrap().get(key) {
            Some(PVal::Str(v)) => v.iter().map(|s| s.to_string_lossy().into_owned()).collect(),
            _ => vec![],
        }
    }
    pub fn get_f64s(&self, key: &str) -> Vec<f64> {
        match self.map.lock().unwrap().get(key) {
            Some(PVal::Double(v)) => v.clone(),
            Some(PVal::Int(v)) => v.iter().map(|x| *x as f64).collect(),
            _ => vec![],
        }
    }
    pub fn get_int(&self, key: &str, i: usize) -> Option<i32> {
        match self.map.lock().unwrap().get(key) {
            Some(PVal::Int(v)) => v.get(i).copied(),
            Some(PVal::Double(v)) => v.get(i).map(|x| *x as i32),
            _ => None,
        }
    }
    pub fn get_ptr(&self, key: &str) -> Option<usize> {
        match self.map.lock().unwrap().get(key) {
            Some(PVal::Ptr(v)) => v.first().copied(),
            _ => None,
        }
    }

    // ------------------------------------------------ suite primitives (plugin side)

    pub fn set_int_at(&self, key: &str, i: usize, v: i32) {
        let mut m = self.map.lock().unwrap();
        let e = m.entry(key.into()).or_insert(PVal::Int(vec![]));
        match e {
            PVal::Int(a) => put(a, i, v, 0),
            PVal::Double(a) => put(a, i, v as f64, 0.0),
            other => *other = PVal::Int(filled(i, v, 0)),
        }
    }
    pub fn set_double_at(&self, key: &str, i: usize, v: f64) {
        let mut m = self.map.lock().unwrap();
        let e = m.entry(key.into()).or_insert(PVal::Double(vec![]));
        match e {
            PVal::Double(a) => put(a, i, v, 0.0),
            PVal::Int(a) => {
                let mut d: Vec<f64> = a.iter().map(|x| *x as f64).collect();
                put(&mut d, i, v, 0.0);
                *e = PVal::Double(d);
            }
            other => *other = PVal::Double(filled(i, v, 0.0)),
        }
    }
    pub fn set_str_at(&self, key: &str, i: usize, v: &CStr) {
        let mut m = self.map.lock().unwrap();
        let e = m.entry(key.into()).or_insert(PVal::Str(vec![]));
        match e {
            PVal::Str(a) => put(a, i, v.to_owned(), CString::default()),
            other => *other = PVal::Str(filled(i, v.to_owned(), CString::default())),
        }
    }
    pub fn set_ptr_at(&self, key: &str, i: usize, v: usize) {
        let mut m = self.map.lock().unwrap();
        let e = m.entry(key.into()).or_insert(PVal::Ptr(vec![]));
        match e {
            PVal::Ptr(a) => put(a, i, v, 0),
            other => *other = PVal::Ptr(filled(i, v, 0)),
        }
    }

    pub fn get_int_at(&self, key: &str, i: usize) -> Result<i32, PropError> {
        match self.map.lock().unwrap().get(key) {
            None => Err(PropError::Unknown),
            Some(PVal::Int(v)) => v.get(i).copied().ok_or(PropError::BadIndex),
            Some(PVal::Double(v)) => v.get(i).map(|x| *x as i32).ok_or(PropError::BadIndex),
            Some(_) => Err(PropError::Type),
        }
    }
    pub fn get_double_at(&self, key: &str, i: usize) -> Result<f64, PropError> {
        match self.map.lock().unwrap().get(key) {
            None => Err(PropError::Unknown),
            Some(PVal::Double(v)) => v.get(i).copied().ok_or(PropError::BadIndex),
            Some(PVal::Int(v)) => v.get(i).map(|x| *x as f64).ok_or(PropError::BadIndex),
            Some(_) => Err(PropError::Type),
        }
    }
    /// Pointer to the stored C string (valid until the property is rewritten).
    pub fn get_str_ptr_at(&self, key: &str, i: usize) -> Result<*const std::ffi::c_char, PropError> {
        match self.map.lock().unwrap().get(key) {
            None => Err(PropError::Unknown),
            Some(PVal::Str(v)) => v.get(i).map(|s| s.as_ptr()).ok_or(PropError::BadIndex),
            Some(_) => Err(PropError::Type),
        }
    }
    pub fn get_ptr_at(&self, key: &str, i: usize) -> Result<usize, PropError> {
        match self.map.lock().unwrap().get(key) {
            None => Err(PropError::Unknown),
            Some(PVal::Ptr(v)) => v.get(i).copied().ok_or(PropError::BadIndex),
            Some(_) => Err(PropError::Type),
        }
    }
    pub fn dimension(&self, key: &str) -> Option<usize> {
        self.map.lock().unwrap().get(key).map(|v| v.len())
    }
}

fn put<T: Clone>(a: &mut Vec<T>, i: usize, v: T, fill: T) {
    if a.len() <= i {
        a.resize(i + 1, fill);
    }
    a[i] = v;
}

fn filled<T: Clone>(i: usize, v: T, fill: T) -> Vec<T> {
    let mut a = vec![fill; i + 1];
    a[i] = v;
    a
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn set_get_and_coerce() {
        let p = Props::new();
        p.set_int_at("a", 2, 7);
        assert_eq!(p.dimension("a"), Some(3));
        assert_eq!(p.get_int_at("a", 2).unwrap(), 7);
        p.set_double_at("a", 0, 1.5);
        assert_eq!(p.get_double_at("a", 0).unwrap(), 1.5);
        assert_eq!(p.get_double_at("a", 2).unwrap(), 7.0);
        assert!(matches!(p.get_int_at("missing", 0), Err(PropError::Unknown)));
        assert!(matches!(p.get_int_at("a", 9), Err(PropError::BadIndex)));
        p.set_str_at("s", 1, &CString::new("hi").unwrap());
        assert_eq!(p.get_strs("s"), vec!["".to_string(), "hi".to_string()]);
        let ptr = p.get_str_ptr_at("s", 1).unwrap();
        p.set_int_at("other", 0, 1);
        assert_eq!(unsafe { CStr::from_ptr(ptr) }.to_str().unwrap(), "hi");
    }
}
