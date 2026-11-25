//! Numeric phase timings only; never attach request data, URLs or credentials.
use std::time::Instant;

pub struct Phase {
    name: &'static str,
    started: Instant,
}

impl Phase {
    pub fn start(name: &'static str) -> Self {
        Self {
            name,
            started: Instant::now(),
        }
    }
}

impl Drop for Phase {
    fn drop(&mut self) {
        alog(&format!(
            "PHASE name={} us={}",
            self.name,
            self.started.elapsed().as_micros()
        ));
    }
}

#[cfg(target_os = "android")]
pub fn alog(message: &str) {
    use std::ffi::{CString, c_char, c_int};
    #[link(name = "log")]
    unsafe extern "C" {
        fn __android_log_write(priority: c_int, tag: *const c_char, text: *const c_char) -> c_int;
    }
    if let Ok(text) = CString::new(message) {
        unsafe {
            __android_log_write(4, c"palmcode".as_ptr(), text.as_ptr());
        }
    }
}

#[cfg(not(target_os = "android"))]
pub fn alog(_message: &str) {}
