use core_foundation::{
    array::CFArrayRef,
    base::CFRelease,
};
use std::os::raw::c_void;
use std::ops::Deref;

#[derive(Debug)]
pub(super) struct BoxCFArrayRef {
    cf_array_ref: CFArrayRef,
}

impl Deref for BoxCFArrayRef {
    type Target = CFArrayRef;
    fn deref(&self) -> &Self::Target {
        &self.cf_array_ref
    }
}

impl Drop for BoxCFArrayRef {
    fn drop(&mut self) {
        unsafe {
            // Cast directly to avoid ToVoid<T> type-inference ambiguity (E0282) on Rust >=1.82.
            CFRelease(self.cf_array_ref as *const c_void);
        }
    }
}

impl BoxCFArrayRef {
    pub fn new(cf_array_ref: CFArrayRef) -> Self {
        BoxCFArrayRef { cf_array_ref }
    }
}
